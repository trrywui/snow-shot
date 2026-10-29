use crate::{CanvasDocument, CanvasError, vector::VectorContext};
use snow_draw_engine as draw;
use unicode_segmentation::UnicodeSegmentation;

pub(crate) fn synchronize(
    metrics: &mut VectorContext,
    document: &mut CanvasDocument,
    viewport: draw::ViewportId,
) -> Result<(), CanvasError> {
    document.command(|engine| engine.begin_presentation_update())?;
    let result = synchronize_inner(metrics, document, viewport);
    let end = document.command(|engine| engine.end_presentation_update());
    result?;
    end?;
    Ok(())
}
fn style_from_text(text: &draw::TextData) -> draw::TextStyle {
    draw::TextStyle {
        color: text.color,
        font_size: text.font_size,
        bold: text.bold,
        italic: text.italic,
        font_family: text.font_family.clone(),
        fill: text.fill,
        fill_style: text.fill_style,
        stroke: text.stroke,
        stroke_width: text.stroke_width,
        corner_radii: text.corner_radii,
        horizontal_align: text.horizontal_align,
        vertical_align: text.vertical_align,
        opacity: text.opacity,
    }
}
fn synchronize_inner(
    metrics: &mut VectorContext,
    document: &mut CanvasDocument,
    viewport: draw::ViewportId,
) -> Result<(), CanvasError> {
    let requests = document.engine().arrow_text_layout_requests(viewport)?;
    let mut layouts = Vec::with_capacity(requests.len());
    for request in requests {
        let style = style_from_text(&request.text);
        let natural = metrics.layout(&request.text.text, &style, 1.0e8);
        let layout = metrics.layout(&request.text.text, &style, request.max_width);
        layouts.push((
            request.text_id,
            request.key,
            draw::TextLayoutSize::with_content(
                layout.width,
                layout.height,
                layout.width,
                layout.height,
            ),
            natural.width,
        ));
    }
    if !layouts.is_empty() {
        document.command(|engine| engine.apply_arrow_text_measurements(viewport, &layouts))?;
    }
    let requests = document
        .engine()
        .speech_bubble_text_layout_requests(viewport)?;
    let mut layouts = Vec::with_capacity(requests.len());
    for request in requests {
        let style = style_from_text(&request.text);
        let minimum = request
            .text
            .text
            .graphemes(true)
            .map(|g| metrics.text_width(g, &style))
            .fold(1.0, f64::max);
        let constraints = draw::speech_bubble_text_constraints(
            request.body,
            minimum,
            request.text.text_ink_reserve,
            style.font_size * 1.3,
        )?;
        let layout = metrics.layout(&request.text.text, &style, constraints.content_width);
        layouts.push((
            request.text_id,
            draw::SpeechBubbleTextMeasurement {
                key: request.key,
                size: draw::TextLayoutSize::with_content(
                    constraints.content_width,
                    layout.height,
                    layout.width,
                    layout.height,
                ),
                minimum_grapheme_width: minimum,
                text_ink_reserve: request.text.text_ink_reserve,
            },
        ));
    }
    if !layouts.is_empty() {
        document.command(|engine| engine.apply_speech_bubble_text_layouts(viewport, &layouts))?;
    }
    if let Some(request) = document.engine().bound_label_layout_request(viewport)? {
        let style = draw::TextStyle {
            font_size: request.font_size,
            font_family: request.font_family,
            ..document
                .engine()
                .viewport_style_toolbar_state(viewport)?
                .text_style
        };
        let layout = metrics.layout("", &style, 1.0e8);
        document.command(|engine| {
            engine.apply_bound_label_layout(
                viewport,
                request.text_id,
                draw::TextLayoutSize::with_content(
                    layout.width.max(1.0),
                    layout.height,
                    layout.width.max(1.0),
                    layout.height,
                ),
            )
        })?;
    }
    if let Some(request) = document
        .engine()
        .active_text_resize_measurement_request(viewport)?
    {
        let style = document
            .engine()
            .viewport_style_toolbar_state(viewport)?
            .text_style;
        let layout = metrics.layout(
            &request.text,
            &draw::TextStyle {
                font_size: request.font_size,
                bold: request.bold,
                italic: request.italic,
                font_family: request.font_family.clone(),
                ..style
            },
            if request.measure_natural_width {
                1.0e8
            } else {
                request.width
            },
        );
        document.command(|engine| {
            engine.apply_active_text_resize_measurement_with_viewport_changes(
                viewport,
                draw::TextLayoutSize::with_content(
                    if request.auto_resize {
                        layout.width
                    } else {
                        request.width
                    },
                    layout.height,
                    layout.width,
                    layout.height,
                ),
            )
        })?;
    }
    Ok(())
}
