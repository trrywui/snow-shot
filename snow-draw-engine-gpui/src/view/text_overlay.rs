//! Selected glyphs reuse document shaping. Only clipped triangle geometry is
//! prepared on the CPU; coverage and isolated group opacity run on GPUI's device.
use super::element::gpui_color;
use super::*;
use crate::gpu_geometry::{ItemGeometry, Mesh, Paint, color_glyph_mesh, compose, glyph_mesh};
#[cfg(target_os = "windows")]
use crate::gpu_renderer::{GpuCanvas, GpuCanvasFrame};
use crate::vector::ShapedGlyph;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct TextSelectionOverlay {
    #[cfg(target_os = "windows")]
    cached: Option<(SelectionKey, GpuCanvasFrame)>,
    #[cfg(target_os = "windows")]
    canvas: Option<GpuCanvas>,
    published: TextOverlayPublication,
}

#[derive(Default)]
pub(super) struct TextOverlayPublication {
    origin: Point<Pixels>,
    selection: Vec<gpui::Path<Pixels>>,
    underlines: Vec<(gpui::Path<Pixels>, draw::ColorRgba8)>,
    caret: Option<(gpui::Path<Pixels>, draw::ColorRgba8)>,
    #[cfg(target_os = "windows")]
    glyphs: Option<GpuCanvasFrame>,
}

#[cfg(test)]
#[derive(Debug, PartialEq)]
pub(super) struct TextPublicationSnapshot {
    paths: Vec<gpui::Path<Pixels>>,
    colors: Vec<draw::ColorRgba8>,
    textures: Vec<(u64, u64)>,
}

struct SelectionLine {
    glyphs: Arc<[ShapedGlyph]>,
    origin: [f64; 2],
}
impl PartialEq for SelectionLine {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.glyphs, &other.glyphs) && self.origin == other.origin
    }
}

#[derive(PartialEq)]
struct SelectionKey {
    lines: Vec<SelectionLine>,
    rectangles: Vec<[f64; 4]>,
    transform: [f64; 6],
    frame: draw::FrameView,
    scale: f32,
    opacity: f64,
    fill_alpha: u8,
    stroke_alpha: u8,
    stroke_width: f64,
}

impl TextSelectionOverlay {
    pub(super) fn clear(&mut self, _window: &mut Window) {
        #[cfg(target_os = "windows")]
        {
            self.cached = None;
            self.canvas = None;
        }
    }

    pub(super) fn clear_published(&mut self, window: &mut Window) {
        self.clear(window);
        self.published = Default::default();
    }

    pub(super) fn publish(&mut self, publication: TextOverlayPublication) {
        self.published = publication;
    }

    pub(super) fn is_ready(&mut self) -> Result<bool, CanvasError> {
        #[cfg(target_os = "windows")]
        if let Some(canvas) = &mut self.canvas {
            return canvas
                .is_ready()
                .map_err(|error| CanvasError::Gpu(error.to_string()));
        }
        Ok(true)
    }

    pub(super) fn has_published_caret(&self) -> bool {
        self.published.caret.is_some()
    }

    #[cfg(test)]
    pub(super) fn publication_snapshot(&self) -> TextPublicationSnapshot {
        let mut paths = self.published.selection.clone();
        let mut colors = Vec::new();
        for (path, color) in self
            .published
            .underlines
            .iter()
            .chain(self.published.caret.iter())
        {
            paths.push(path.clone());
            colors.push(*color);
        }
        let mut textures = Vec::new();
        #[cfg(target_os = "windows")]
        if let Some(frame) = &self.published.glyphs {
            textures.extend(
                frame
                    .tiles
                    .iter()
                    .map(|tile| (tile.texture.resource_id(), tile.revision)),
            );
        }
        TextPublicationSnapshot {
            paths,
            colors,
            textures,
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn prepare(
        &mut self,
        key: Option<SelectionKey>,
        _metrics: &VectorContext,
        _window: &mut Window,
    ) -> Result<(), CanvasError> {
        if key.is_some() {
            Err(CanvasError::Unsupported(
                "GPU text selection requires the Windows backend",
            ))
        } else {
            Ok(())
        }
    }

    #[cfg(target_os = "windows")]
    fn prepare(
        &mut self,
        key: Option<SelectionKey>,
        metrics: &VectorContext,
        window: &mut Window,
    ) -> Result<Option<GpuCanvasFrame>, CanvasError> {
        let Some(key) = key else {
            self.cached = None;
            return Ok(None);
        };
        let error = |error: anyhow::Error| CanvasError::Gpu(error.to_string());
        let service = window.gpu_service().map_err(error)?;
        if self
            .canvas
            .as_ref()
            .is_none_or(|canvas| canvas.service().service_id() != service.service_id())
            || self.cached.as_ref().is_some_and(|(_, frame)| {
                frame
                    .tiles
                    .iter()
                    .any(|tile| tile.texture.epoch() != service.epoch())
            })
        {
            self.cached = None;
            self.canvas = Some(GpuCanvas::new(service).map_err(error)?);
        }
        if !self.cached.as_ref().is_some_and(|cached| cached.0 == key) {
            let geometry = selection_geometry(&key, metrics)?;
            let frame = self
                .canvas
                .as_mut()
                .unwrap()
                .prepare_overlay(&geometry, key.frame, key.scale)
                .map_err(error)?;
            self.cached = Some((key, frame));
        }
        Ok(self.cached.as_ref().map(|(_, frame)| frame.clone()))
    }

    pub(super) fn paint_published(
        &self,
        bounds: Bounds<Pixels>,
        caret_visible: bool,
        window: &mut Window,
    ) -> Result<(), CanvasError> {
        let offset = bounds.origin - self.published.origin;
        for path in &self.published.selection {
            window.paint_path(translated_path(path, offset), rgba(0x3399ff78));
        }
        #[cfg(target_os = "windows")]
        if let Some(frame) = &self.published.glyphs {
            for tile in frame.tiles.iter() {
                let region = tile.region;
                window
                    .paint_gpu_texture(
                        Bounds::new(
                            bounds.origin
                                + point(
                                    px(region.x as f32 / frame.scale),
                                    px(region.y as f32 / frame.scale),
                                ),
                            gpui::size(
                                px(region.width as f32 / frame.scale),
                                px(region.height as f32 / frame.scale),
                            ),
                        ),
                        tile.texture.clone(),
                        gpui::GpuTextureOptions {
                            revision: tile.revision,
                            ..Default::default()
                        },
                    )
                    .map_err(|error| CanvasError::Gpu(error.to_string()))?;
            }
        }
        #[cfg(not(target_os = "windows"))]
        let _ = bounds;
        if caret_visible && let Some((path, color)) = &self.published.caret {
            window.paint_path(translated_path(path, offset), gpui_color(*color));
        }
        for (path, color) in &self.published.underlines {
            window.paint_path(translated_path(path, offset), gpui_color(*color));
        }
        Ok(())
    }
}

// Published paths retain the geometry of the completed document, while their
// screen origin follows the canvas even if preparing its next revision fails.
fn translated_path(path: &gpui::Path<Pixels>, offset: Point<Pixels>) -> gpui::Path<Pixels> {
    let mut path = path.clone();
    path.bounds.origin += offset;
    for vertex in &mut path.vertices {
        vertex.xy_position += offset;
    }
    path
}

pub(super) fn prepare_publication(
    this: &mut CanvasView,
    window: &mut Window,
) -> Result<TextOverlayPublication, CanvasError> {
    let Some(draft) = this.draft.as_ref() else {
        return Ok(Default::default());
    };
    let selection = draft.buffer.selection();
    let empty_selection = selection.is_empty();
    let marked = draft.buffer.marked.clone();
    let color = editing_color(draft.style.color);
    let rectangles = if marked.is_none() && !empty_selection {
        this.text_segments(selection)
    } else {
        Vec::new()
    };
    let mut publication = TextOverlayPublication {
        origin: this.bounds.origin,
        ..Default::default()
    };
    for &(left, top, right, bottom) in &rectangles {
        let points = [(left, top), (right, top), (right, bottom), (left, bottom)]
            .map(|(x, y)| this.text_world_position(x, y).unwrap());
        let mut path = PathBuilder::fill();
        path.add_polygon(&points, true);
        if let Ok(path) = path.build() {
            publication.selection.push(path);
        }
    }
    let key = this.draft.as_ref().and_then(|draft| {
        selection_key(
            draft,
            &rectangles,
            this.session.scene().frame(),
            window.scale_factor(),
        )
    });
    #[cfg(target_os = "windows")]
    {
        publication.glyphs = this
            .text_selection_overlay
            .prepare(key, &this.metrics, window)?;
    }
    #[cfg(not(target_os = "windows"))]
    this.text_selection_overlay
        .prepare(key, &this.metrics, window)?;
    if empty_selection {
        publication.caret = caret_path(this);
    }
    if let Some(marked) = marked {
        for (left, _, right, bottom) in this.text_segments(marked) {
            let mut path = PathBuilder::stroke(px(1.0));
            path.move_to(this.text_world_position(left, bottom).unwrap());
            path.line_to(this.text_world_position(right, bottom).unwrap());
            if let Ok(path) = path.build() {
                publication.underlines.push((path, color));
            }
        }
    }
    Ok(publication)
}

pub(super) fn caret_path(this: &mut CanvasView) -> Option<(gpui::Path<Pixels>, draw::ColorRgba8)> {
    let draft = this.draft.as_ref()?;
    let caret = draft.buffer.caret;
    let height = draft.layout.line_height;
    let color = editing_color(draft.style.color);
    let (x, y) = this.text_local_position(caret)?;
    let path = caret_stroke(
        this.text_world_position(x, y)?,
        this.text_world_position(x, y + height)?,
        this.session.scene().frame().camera.zoom,
        this.bounds,
    )?;
    Some((path, color))
}

fn caret_stroke(
    start: Point<Pixels>,
    end: Point<Pixels>,
    zoom: f64,
    bounds: Bounds<Pixels>,
) -> Option<gpui::Path<Pixels>> {
    let stroke = usvg::tiny_skia_path::Stroke {
        width: (1.2 * zoom).max(1.0) as f32,
        line_cap: usvg::tiny_skia_path::LineCap::Butt,
        ..Default::default()
    };
    overlay_geometry::stroke_line(start, end, bounds, &stroke, None)
}

pub(super) fn editing_color(color: draw::ColorRgba8) -> draw::ColorRgba8 {
    if color.a == 0 {
        draw::ColorRgba8 {
            r: 30,
            g: 30,
            b: 30,
            a: 255,
        }
    } else {
        color
    }
}

fn selection_key(
    draft: &TextDraft,
    rectangles: &[(f64, f64, f64, f64)],
    frame: draw::FrameView,
    scale: f32,
) -> Option<SelectionKey> {
    let selection = draft.buffer.selection();
    if selection.is_empty() || draft.buffer.marked.is_some() || rectangles.is_empty() {
        return None;
    }
    let center = draw::canvas_to_view(draft.center, &frame.camera, frame.surface);
    let (sin, cos) = draft.rotation.sin_cos();
    let mut extent = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    let rectangles: Vec<_> = rectangles
        .iter()
        .filter_map(|&(left, top, right, bottom)| {
            if ![left, top, right, bottom]
                .iter()
                .all(|value| value.is_finite())
                || right <= left
                || bottom <= top
            {
                return None;
            }
            for (x, y) in [(left, top), (right, top), (right, bottom), (left, bottom)] {
                let x_view = center.x + (x * cos - y * sin) * frame.camera.zoom;
                let y_view = center.y + (x * sin + y * cos) * frame.camera.zoom;
                extent[0] = extent[0].min(x_view);
                extent[1] = extent[1].min(y_view);
                extent[2] = extent[2].max(x_view);
                extent[3] = extent[3].max(y_view);
            }
            Some([left, top, right, bottom])
        })
        .collect();
    if !extent.iter().all(|value| value.is_finite())
        || extent[2] <= 0.0
        || extent[3] <= 0.0
        || extent[0] >= f64::from(frame.surface.width)
        || extent[1] >= f64::from(frame.surface.height)
    {
        return None;
    }
    let width = if draft.auto_resize {
        draft.layout.width.max(1.0)
    } else {
        draft.width
    };
    let lines = draft
        .layout
        .lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            if selection.start >= line.start + line.text.len() || selection.end <= line.start {
                return None;
            }
            let x = match draft.style.horizontal_align {
                draw::TextHorizontalAlign::Left => -width / 2.0,
                draw::TextHorizontalAlign::Center => -line.width / 2.0,
                draw::TextHorizontalAlign::Right => width / 2.0 - line.width,
            };
            let y = -draft.layout.height / 2.0
                + index as f64 * draft.layout.line_height
                + draft.style.font_size;
            Some(SelectionLine {
                glyphs: line.glyphs.clone(),
                origin: [x, y],
            })
        })
        .collect();
    Some(SelectionKey {
        lines,
        rectangles,
        transform: [cos, sin, -sin, cos, draft.center.x, draft.center.y],
        frame,
        scale,
        opacity: draft.style.opacity,
        fill_alpha: draft.style.color.a,
        stroke_alpha: draft.style.stroke.a,
        stroke_width: draft.style.stroke_width,
    })
}

fn selection_geometry(
    key: &SelectionKey,
    metrics: &VectorContext,
) -> Result<ItemGeometry, CanvasError> {
    let mut output = ItemGeometry {
        opacity: key.opacity,
        ..Default::default()
    };
    let mut glyphs = HashMap::new();
    // Reuse the entire shaped line, including ligatures and bidi context. Clip
    // coverage geometry, never reshape a selected substring.
    for (line_index, line) in key.lines.iter().enumerate() {
        for (stroke, alpha, width) in [
            (true, key.stroke_alpha, key.stroke_width),
            (false, key.fill_alpha, 0.0),
        ] {
            if alpha == 0 || (stroke && width <= 0.0) {
                continue;
            }
            for glyph in line.glyphs.iter() {
                let color_glyph = (!stroke)
                    .then(|| metrics.color_glyph(glyph.font, glyph.id))
                    .flatten();
                let mesh = if let Some(mesh) =
                    glyphs.get(&(glyph.font, glyph.id, glyph.scale.to_bits(), stroke))
                {
                    Arc::clone(mesh)
                } else {
                    let mesh = if let Some(color) = &color_glyph {
                        color_glyph_mesh(color)?
                    } else {
                        let Some(mesh) = glyph_mesh(
                            metrics,
                            glyph,
                            width,
                            key.frame.camera.zoom * f64::from(key.scale),
                        )?
                        else {
                            continue;
                        };
                        mesh
                    };
                    let mesh = Arc::new(mesh);
                    glyphs.insert(
                        (glyph.font, glyph.id, glyph.scale.to_bits(), stroke),
                        mesh.clone(),
                    );
                    mesh
                };
                let mut mesh = clipped_glyph(&mesh, glyph, line.origin, &key.rectangles)?;
                if mesh.indices.is_empty() {
                    continue;
                }
                let transform = if color_glyph.is_some() {
                    // Keep native color UVs in font coordinates after clipping
                    // against selection rectangles in the text's local space.
                    let vertices = mesh
                        .vertices
                        .iter()
                        .map(|point| {
                            [
                                ((f64::from(point[0]) - line.origin[0] - glyph.x) / glyph.scale)
                                    as f32,
                                (-(f64::from(point[1]) - line.origin[1] - glyph.y) / glyph.scale)
                                    as f32,
                            ]
                        })
                        .collect();
                    mesh = Mesh::from_vertices(vertices, mesh.indices)?;
                    compose(
                        key.transform,
                        [
                            glyph.scale,
                            0.0,
                            0.0,
                            -glyph.scale,
                            line.origin[0] + glyph.x,
                            line.origin[1] + glyph.y,
                        ],
                    )
                } else {
                    key.transform
                };
                output.parts.push(Paint {
                    mesh: Arc::new(mesh),
                    color: draw::ColorRgba8 {
                        r: 255,
                        g: 255,
                        b: 255,
                        a: alpha,
                    },
                    fill_style: draw::DisplayFillStyle::Solid,
                    transform,
                    rough_style: draw::StrokeRenderingStyle::Architect,
                    rough_seed: 0,
                    rough_scale: 0.0,
                    is_text: true,
                    coverage_group: if color_glyph.is_some() {
                        0
                    } else {
                        line_index as u32 * 2 + if stroke { 1 } else { 2 }
                    },
                    color_glyph,
                    glyph_tint: true,
                });
            }
        }
    }
    Ok(output)
}

fn clipped_glyph(
    mesh: &Mesh,
    glyph: &ShapedGlyph,
    origin: [f64; 2],
    rectangles: &[[f64; 4]],
) -> Result<Mesh, CanvasError> {
    let map = |p: [f32; 2]| {
        [
            origin[0] + glyph.x + f64::from(p[0]) * glyph.scale,
            origin[1] + glyph.y - f64::from(p[1]) * glyph.scale,
        ]
    };
    let first = map([mesh.bounds[0], mesh.bounds[1]]);
    let last = map([mesh.bounds[2], mesh.bounds[3]]);
    let bounds = [
        first[0].min(last[0]),
        first[1].min(last[1]),
        first[0].max(last[0]),
        first[1].max(last[1]),
    ];
    let rectangles: Vec<_> = rectangles
        .iter()
        .filter(|rect| {
            rect[0] < bounds[2] && rect[2] > bounds[0] && rect[1] < bounds[3] && rect[3] > bounds[1]
        })
        .collect();
    if rectangles.is_empty() {
        return Mesh::from_vertices(Vec::new(), Vec::new());
    }
    if rectangles.iter().any(|rect| {
        rect[0] <= bounds[0] && rect[1] <= bounds[1] && rect[2] >= bounds[2] && rect[3] >= bounds[3]
    }) {
        return Mesh::from_vertices(
            mesh.vertices
                .iter()
                .map(|point| map(*point).map(|value| value as f32))
                .collect(),
            mesh.indices.clone(),
        );
    }
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let triangle = [
            map(mesh.vertices[triangle[0] as usize]),
            map(mesh.vertices[triangle[1] as usize]),
            map(mesh.vertices[triangle[2] as usize]),
        ];
        for rect in &rectangles {
            let mut polygon = triangle.to_vec();
            for (axis, edge, greater) in [
                (0, rect[0], true),
                (0, rect[2], false),
                (1, rect[1], true),
                (1, rect[3], false),
            ] {
                polygon = clip_edge(&polygon, axis, edge, greater);
                if polygon.len() < 3 {
                    break;
                }
            }
            if vertices.len() + polygon.len() >= 1_048_576 {
                return Err(CanvasError::ResourceLimit);
            }
            let start = vertices.len() as u32;
            for index in 2..polygon.len() {
                indices.extend([start, start + index as u32 - 1, start + index as u32]);
            }
            vertices.extend(polygon.into_iter().map(|[x, y]| [x as f32, y as f32]));
        }
    }
    Mesh::from_vertices(vertices, indices)
}

fn clip_edge(polygon: &[[f64; 2]], axis: usize, edge: f64, greater: bool) -> Vec<[f64; 2]> {
    let Some(&last) = polygon.last() else {
        return Vec::new();
    };
    let inside = |point: [f64; 2]| {
        if greater {
            point[axis] >= edge
        } else {
            point[axis] <= edge
        }
    };
    let mut previous = last;
    let mut output = Vec::with_capacity(polygon.len() + 1);
    for &point in polygon {
        if inside(previous) != inside(point) {
            let t = (edge - previous[axis]) / (point[axis] - previous[axis]);
            let mut hit = [
                previous[0] + (point[0] - previous[0]) * t,
                previous[1] + (point[1] - previous[1]) * t,
            ];
            hit[axis] = edge;
            output.push(hit);
        }
        if inside(point) {
            output.push(point);
        }
        previous = point;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(metrics: &mut VectorContext, text: &str) -> TextDraft {
        let style = draw::TextStyle {
            font_size: 28.0,
            color: draw::ColorRgba8 {
                r: 200,
                g: 20,
                b: 40,
                a: 180,
            },
            opacity: 0.6,
            bold: false,
            italic: false,
            font_family: None,
            fill: draw::ColorRgba8 {
                r: 0,
                g: 0,
                b: 0,
                a: 0,
            },
            fill_style: draw::FillStyle::Solid,
            stroke: draw::ColorRgba8 {
                r: 0,
                g: 0,
                b: 0,
                a: 0,
            },
            stroke_width: 0.0,
            corner_radii: Default::default(),
            horizontal_align: draw::TextHorizontalAlign::Left,
            vertical_align: draw::TextVerticalAlign::Center,
        };
        let layout = metrics.layout(text, &style, 1.0e8);
        let mut buffer = TextBuffer::new(text.into());
        buffer.anchor = 0;
        TextDraft {
            buffer,
            target: draw::TextCommitTarget::New,
            center: draw::Point::new(0.0, 0.0),
            rotation: 0.0,
            style,
            width: layout.width,
            auto_resize: true,
            layout,
            changed: false,
            minimum_grapheme_width: 1.0,
            ink_reserve: 0.0,
            preferred_x: None,
            caret_affinity: None,
        }
    }
    fn frame(zoom: f64) -> draw::FrameView {
        draw::FrameView {
            camera: draw::Camera {
                center: draw::Point::new(0.0, 0.0),
                zoom,
            },
            surface: draw::SurfaceSize {
                width: 640,
                height: 480,
            },
            ..Default::default()
        }
    }
    fn bounds() -> Bounds<Pixels> {
        Bounds::new(point(px(10.0), px(20.0)), gpui::size(px(640.0), px(480.0)))
    }
    fn rectangles(draft: &TextDraft) -> Vec<(f64, f64, f64, f64)> {
        draft
            .layout
            .lines
            .iter()
            .enumerate()
            .flat_map(|(index, line)| {
                let left = -draft.layout.width / 2.0;
                let top = -draft.layout.height / 2.0 + index as f64 * draft.layout.line_height;
                line.selection_segments(0..line.text.len())
                    .into_iter()
                    .map(move |(a, b)| (left + a, top, left + b, top + draft.layout.line_height))
            })
            .collect()
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn selected_color_glyph_uses_native_alpha_with_white_tint_and_partial_clipping()
    -> anyhow::Result<()> {
        let host = gpui_windows::gpu_extensions::OffscreenGpuHost::new()?;
        let service = host.service();
        let mut metrics = VectorContext::default();
        let mut draft = draft(&mut metrics, "😀");
        draft.style.font_family = Some("Segoe UI Emoji".into());
        draft.style.font_size = 96.0;
        draft.layout = metrics.layout(&draft.buffer.text, &draft.style, 1000.0);
        draft.width = draft.layout.width;
        let width = draft.layout.width;
        let height = draft.layout.height;
        let key = selection_key(
            &draft,
            &[(-width / 2.0, -height / 2.0, 0.0, height / 2.0)],
            frame(1.0),
            1.0,
        )
        .unwrap();
        let geometry = selection_geometry(&key, &metrics)?;
        assert!(
            geometry
                .parts
                .iter()
                .any(|part| part.color_glyph.is_some() && part.glyph_tint)
        );
        let mut canvas = GpuCanvas::new(service.clone())?;
        let prepared = canvas.prepare_overlay(&geometry, key.frame, key.scale)?;
        let image = crate::gpu_tests::read_frame(&service, &prepared)?;
        assert!(image.pixels().any(|pixel| pixel[3] > 80));
        for (x, _, pixel) in image
            .enumerate_pixels()
            .filter(|(_, _, pixel)| pixel[3] > 0)
        {
            assert!(x <= 320, "selection leaked past its local rectangle");
            assert!(
                pixel[0].abs_diff(pixel[3]) <= 1
                    && pixel[1].abs_diff(pixel[3]) <= 1
                    && pixel[2].abs_diff(pixel[3]) <= 1,
                "palette was not tinted: {pixel:?}"
            );
        }
        Ok(())
    }
    #[test]
    fn text_selection_reuses_shaped_glyphs_and_isolates_group_opacity() {
        let mut metrics = VectorContext::default();
        let mut draft = draft(&mut metrics, "office text");
        draft.buffer.caret = 2;
        draft.style.stroke = draw::ColorRgba8 {
            r: 255,
            g: 0,
            b: 0,
            a: 120,
        };
        draft.style.stroke_width = 2.0;
        let line = &draft.layout.lines[0];
        let left = -draft.layout.width / 2.0;
        let top = -draft.layout.height / 2.0;
        let rectangles: Vec<_> = line
            .selection_segments(0..2)
            .into_iter()
            .map(|(a, b)| (left + a, top, left + b, top + draft.layout.line_height))
            .collect();
        let key = selection_key(&draft, &rectangles, frame(1.0), 1.5).unwrap();
        assert!(Arc::ptr_eq(&key.lines[0].glyphs, &line.glyphs));
        let geometry = selection_geometry(&key, &metrics).unwrap();
        assert!(!geometry.parts.is_empty());
        assert_eq!(geometry.opacity, 0.6);
        assert!(
            geometry
                .parts
                .iter()
                .all(|part| part.color.r == 255 && part.color.g == 255 && part.color.b == 255)
        );
        assert!(geometry.parts.iter().any(|part| part.color.a == 180));
        assert!(geometry.parts.iter().any(|part| part.color.a == 120));
        for part in &geometry.parts {
            for vertex in &part.mesh.vertices {
                assert!(
                    rectangles
                        .iter()
                        .any(
                            |&(left, top, right, bottom)| f64::from(vertex[0]) >= left - 0.001
                                && f64::from(vertex[0]) <= right + 0.001
                                && f64::from(vertex[1]) >= top - 0.001
                                && f64::from(vertex[1]) <= bottom + 0.001
                        )
                );
            }
        }
        assert!(key == selection_key(&draft, &rectangles, frame(1.0), 1.5).unwrap());
        draft.buffer.marked = Some(0..2);
        assert!(selection_key(&draft, &rectangles, frame(1.0), 1.5).is_none());
        draft.buffer.marked = None;
        draft.buffer.anchor = draft.buffer.caret;
        assert!(selection_key(&draft, &rectangles, frame(1.0), 1.5).is_none());
    }
    #[test]
    fn text_selection_preserves_bidi_multiline_rotation_and_dpi() {
        let mut metrics = VectorContext::default();
        let mut draft = draft(&mut metrics, "Latin אבג 😀\nsecond line");
        let rectangles = rectangles(&draft);
        draft.rotation = 0.4;
        let first = selection_key(&draft, &rectangles, frame(1.0), 1.0).unwrap();
        assert_eq!(first.lines.len(), 2);
        assert!(
            !selection_geometry(&first, &metrics)
                .unwrap()
                .parts
                .is_empty()
        );
        assert!(first != selection_key(&draft, &rectangles, frame(1.0), 2.0).unwrap());
        assert!(first != selection_key(&draft, &rectangles, frame(0.5), 1.0).unwrap());
        draft.center = draw::Point::new(-320.0, -240.0);
        assert!(selection_key(&draft, &rectangles, frame(1.0), 2.0).is_some());
        draft.center = draw::Point::new(-10_000.0, -10_000.0);
        assert!(selection_key(&draft, &rectangles, frame(1.0), 2.0).is_none());
    }
    #[test]
    fn text_selection_clips_partial_ligature_coverage_without_reshaping() {
        let mut metrics = VectorContext::default();
        let draft = draft(&mut metrics, "office");
        let mut glyph = draft.layout.lines[0].glyphs[0].clone();
        glyph.x = 0.0;
        glyph.y = 0.0;
        glyph.scale = 1.0;
        let mesh = Mesh::from_vertices(vec![[-2.0, -2.0], [2.0, -2.0], [0.0, 2.0]], vec![0, 1, 2])
            .unwrap();
        let clipped = clipped_glyph(&mesh, &glyph, [0.0, 0.0], &[[-1.0, -1.0, 1.0, 1.0]]).unwrap();
        assert!(!clipped.indices.is_empty());
        assert!(
            clipped.vertices.iter().all(
                |vertex| (-1.0..=1.0).contains(&vertex[0]) && (-1.0..=1.0).contains(&vertex[1])
            )
        );
    }
    #[test]
    fn text_caret_geometry_has_centered_flat_ends_and_zoom_minimum() {
        for (zoom, width) in [(0.25, 1.0_f32), (1.0, 1.2), (2.0, 2.4)] {
            for angle in [0.0_f32, 0.4, 1.5] {
                let start = point(px(100.0), px(100.0));
                let (sin, cos) = angle.sin_cos();
                let end = start + point(px(-sin * 30.0), px(cos * 30.0));
                let path = caret_stroke(start, end, zoom, bounds()).unwrap();
                let mut across = (f32::INFINITY, f32::NEG_INFINITY);
                let mut along = (f32::INFINITY, f32::NEG_INFINITY);
                for vertex in &path.vertices {
                    let delta = vertex.xy_position - start;
                    let x = f32::from(delta.x);
                    let y = f32::from(delta.y);
                    let a = x * cos + y * sin;
                    let b = -x * sin + y * cos;
                    across.0 = across.0.min(a);
                    across.1 = across.1.max(a);
                    along.0 = along.0.min(b);
                    along.1 = along.1.max(b);
                }
                assert!((across.0 + width / 2.0).abs() < 0.001);
                assert!((across.1 - width / 2.0).abs() < 0.001);
                assert!(along.0.abs() < 0.001);
                assert!((along.1 - 30.0).abs() < 0.001);
            }
        }
        assert_eq!(
            editing_color(draw::ColorRgba8 {
                r: 200,
                g: 0,
                b: 0,
                a: 0
            }),
            draw::ColorRgba8 {
                r: 30,
                g: 30,
                b: 30,
                a: 255
            }
        );
    }
}
