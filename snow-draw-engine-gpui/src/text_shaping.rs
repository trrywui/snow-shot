//! One shaping result supplies GPU glyph placement, caret geometry and exports.
//! Outlines are loaded lazily; measuring a paragraph never parses SVG or flattens
//! its glyphs into raster paths.
use crate::vector::{ShapedGlyph, TextLine};
use snow_draw_engine::TextStyle;
use std::{ops::Range, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;
use usvg::fontdb;

#[derive(Clone)]
struct Glyph {
    id: u16,
    font: fontdb::ID,
    range: Range<usize>,
    rtl: bool,
    advance: i32,
    offset: i32,
    offset_y: i32,
    scale: f64,
    ink: Option<(i16, i16)>,
}
fn shape_font(
    database: &fontdb::Database,
    font: fontdb::ID,
    text: &str,
    runs: &[(Range<usize>, bool)],
    size: f64,
) -> Option<Vec<Glyph>> {
    database.with_face_data(font, |data, index| {
        let face = rustybuzz::Face::from_slice(data, index)?;
        let mut glyphs = Vec::new();
        for (run, rtl) in runs {
            let mut buffer = rustybuzz::UnicodeBuffer::new();
            buffer.push_str(&text[run.clone()]);
            buffer.set_direction(if *rtl {
                rustybuzz::Direction::RightToLeft
            } else {
                rustybuzz::Direction::LeftToRight
            });
            let shaped = rustybuzz::shape(&face, &[], buffer);
            let mut boundaries: Vec<_> = shaped
                .glyph_infos()
                .iter()
                .map(|info| info.cluster as usize)
                .chain([run.len()])
                .collect();
            boundaries.sort_unstable();
            boundaries.dedup();
            for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
                let cluster = info.cluster as usize;
                let end = boundaries[boundaries.partition_point(|b| *b <= cluster)];
                glyphs.push(Glyph {
                    id: info.glyph_id as u16,
                    font,
                    range: run.start + cluster..run.start + end,
                    rtl: *rtl,
                    advance: pos.x_advance,
                    offset: pos.x_offset,
                    offset_y: pos.y_offset,
                    scale: size / f64::from(face.units_per_em()),
                    ink: face
                        .glyph_bounding_box(rustybuzz::ttf_parser::GlyphId(info.glyph_id as u16))
                        .map(|bbox| (bbox.x_min, bbox.x_max)),
                });
            }
        }
        Some(glyphs)
    })?
}

pub(crate) fn shape(
    mut database: Arc<fontdb::Database>,
    text: &str,
    style: &TextStyle,
    id: fontdb::ID,
) -> Option<TextLine> {
    let original = text;
    let normalized = text.replace(['\r', '\n', '\t'], " ");
    let text = normalized.as_str();
    let bidi = unicode_bidi::BidiInfo::new(text, Some(unicode_bidi::Level::ltr()));
    let paragraph = bidi.paragraphs.first()?;
    let (levels, runs) = bidi.visual_runs(paragraph, paragraph.range.clone());
    let runs: Vec<_> = runs
        .into_iter()
        .map(|run| {
            let rtl = levels[run.start].is_rtl();
            (run, rtl)
        })
        .collect();
    let mut glyphs = shape_font(&database, id, text, &runs, style.font_size)?;
    let mut used = vec![id];
    // Prefer one fallback face when it covers the complete run. Otherwise
    // replace missing glyphs without changing resolved cluster positions.
    while let Some(missing) = glyphs.iter().find(|g| g.id == 0) {
        let character = text[missing.range.clone()].chars().next()?;
        let Some(font) = crate::vector::resolve_fallback(character, &used, &mut database) else {
            break;
        };
        if used.contains(&font) {
            break;
        }
        let fallback = shape_font(&database, font, text, &runs, style.font_size)?;
        if fallback.iter().all(|g| g.id != 0) {
            glyphs = fallback;
            break;
        }
        if fallback.len() != glyphs.len() {
            break;
        }
        for (glyph, fallback) in glyphs.iter_mut().zip(fallback) {
            if glyph.id == 0 && fallback.id != 0 {
                *glyph = fallback;
            }
        }
        used.push(font);
    }
    let mut line = TextLine {
        text: original.into(),
        width: 0.0,
        start: 0,
        carets: Default::default(),
        clusters: Default::default(),
        ink_reserve: 0.0,
        glyphs: Default::default(),
    };
    let mut placements = Vec::with_capacity(glyphs.len());
    let mut carets = Vec::new();
    let mut clusters = Vec::new();
    let mut ink_left = 0.0_f64;
    let mut ink_right = 0.0_f64;
    let mut index = 0;
    while index < glyphs.len() {
        let first = &glyphs[index];
        let range = first.range.clone();
        let rtl = first.rtl;
        let mut advance = 0.0_f64;
        let mut raw_offset = 0_i64;
        while index < glyphs.len() && glyphs[index].range.start == range.start {
            let glyph = &glyphs[index];
            let origin = line.width + (raw_offset as f64 + f64::from(glyph.offset)) * glyph.scale;
            placements.push(ShapedGlyph {
                font: glyph.font,
                id: glyph.id,
                x: origin,
                y: -f64::from(glyph.offset_y) * glyph.scale,
                scale: glyph.scale,
            });
            if let Some((left, right)) = glyph.ink {
                ink_left = ink_left.min(origin + f64::from(left) * glyph.scale);
                ink_right = ink_right.max(origin + f64::from(right) * glyph.scale);
            }
            advance = advance.max(f64::from(glyph.advance as f32 * glyph.scale as f32));
            raw_offset += i64::from(glyph.advance);
            index += 1;
        }
        let boundaries: Vec<_> = text[range.clone()]
            .grapheme_indices(true)
            .map(|(i, _)| range.start + i)
            .chain([range.end])
            .collect();
        let count = boundaries.len().saturating_sub(1).max(1) as f64;
        for (part, byte) in boundaries.iter().copied().enumerate() {
            let fraction = part as f64 / count;
            carets.push((
                byte,
                line.width + advance * if rtl { 1.0 - fraction } else { fraction },
            ));
        }
        for (part, pair) in boundaries.windows(2).enumerate() {
            let a = advance * part as f64 / count;
            let b = advance * (part + 1) as f64 / count;
            clusters.push((
                pair[0]..pair[1],
                line.width + if rtl { advance - b } else { a },
                line.width + if rtl { advance - a } else { b },
            ));
        }
        line.width = f64::from(line.width as f32 + advance as f32);
    }
    line.ink_reserve = (-ink_left).max(ink_right - line.width).max(0.0);
    line.glyphs = placements.into();
    if carets.is_empty() {
        carets.push((0, 0.0));
    }
    line.carets = carets.into();
    line.clusters = clusters.into();
    Some(line)
}
