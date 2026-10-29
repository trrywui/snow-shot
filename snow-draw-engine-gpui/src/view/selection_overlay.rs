//! Selection decoration in logical view pixels, matching the Qt overlay painter.
use super::*;
use usvg::tiny_skia_path as skia;

#[cfg(test)]
#[path = "selection_overlay_tests.rs"]
mod tests;

pub(super) fn paint_rectangle(
    item: &draw::UiRectangleDisplayItem,
    frame: draw::FrameView,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    for (path, color) in rectangle_paths(item, frame, bounds) {
        window.paint_path(path, element::gpui_color(color));
    }
}

// Keep geometry and painting together: offscreen tests inspect the same tessellated
// paths sent to GPUI, without depending on a platform GPU or font rasterizer.
fn rectangle_paths(
    item: &draw::UiRectangleDisplayItem,
    frame: draw::FrameView,
    bounds: Bounds<Pixels>,
) -> Vec<(gpui::Path<Pixels>, draw::ColorRgba8)> {
    let zoom = frame.camera.zoom;
    let width = item.width * zoom;
    let height = item.height * zoom;
    let stroke_width = item.stroke_width * zoom;
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Vec::new();
    }
    let center = draw::canvas_to_view(
        draw::Point::new(item.center_x, item.center_y),
        &frame.camera,
        frame.surface,
    );
    let center = bounds.origin + point(px(center.x as f32), px(center.y as f32));
    let mut paths = Vec::with_capacity(3);
    let map_local = |x: f64, y: f64| {
        let (sin, cos) = item.rotation.sin_cos();
        let world = draw::Point::new(
            item.center_x + (x * cos - y * sin) / zoom,
            item.center_y + (x * sin + y * cos) / zoom,
        );
        let p = draw::canvas_to_view(world, &frame.camera, frame.surface);
        bounds.origin + local(p.x, p.y)
    };
    if item.kind == draw::UiShapeKind::TextHoverUnderline {
        if item.stroke.a != 0 && stroke_width > 0.0 {
            let stroke = skia::Stroke {
                width: stroke_width as f32,
                line_cap: skia::LineCap::Round,
                ..Default::default()
            };
            if let Some(path) = overlay_geometry::stroke_line(
                map_local(-width / 2.0, height / 2.0),
                map_local(width / 2.0, height / 2.0),
                bounds,
                &stroke,
                None,
            ) {
                paths.push((path, item.stroke));
            }
        }
        return paths;
    }

    if item.fill_style.is_visible(item.fill) {
        let mut path = PathBuilder::fill();
        rectangle_contour(&mut path, item, width, height, zoom);
        push(&mut paths, path, item.rotation, center, item.fill);
    }
    if item.stroke.a != 0 && stroke_width > 0.0 {
        if matches!(
            item.kind,
            draw::UiShapeKind::SelectionMultiFrame
                | draw::UiShapeKind::TextActualFrame
                | draw::UiShapeKind::MagnifierSelectionFrame
        ) {
            // These editor frames have square corners. Clip each edge before
            // dashing: a partially visible selection can span billions of pixels.
            let w = width / 2.0;
            let h = height / 2.0;
            let corners = [
                map_local(-w, -h),
                map_local(w, -h),
                map_local(w, h),
                map_local(-w, h),
            ];
            let stroke = skia::Stroke {
                width: stroke_width as f32,
                ..Default::default()
            };
            let pattern = [(5.0 * stroke_width) as f32, (3.0 * stroke_width) as f32];
            let mut phase = 0.0;
            for index in 0..4 {
                let a = corners[index];
                let b = corners[(index + 1) % 4];
                if let Some(path) =
                    overlay_geometry::stroke_line(a, b, bounds, &stroke, Some((&pattern, phase)))
                {
                    paths.push((path, item.stroke));
                }
                phase += (f64::from(f32::from(b.x)) - f64::from(f32::from(a.x)))
                    .hypot(f64::from(f32::from(b.y)) - f64::from(f32::from(a.y)));
            }
            return paths;
        }
        let mut path = PathBuilder::stroke(px(stroke_width as f32));
        rectangle_contour(&mut path, item, width, height, zoom);
        push(&mut paths, path, item.rotation, center, item.stroke);

        match item.kind {
            draw::UiShapeKind::ShapeTextHandle | draw::UiShapeKind::SpeechBubbleAddTailHandle => {
                let half = width.min(height) * 0.25;
                let mut path = PathBuilder::stroke(px(stroke_width as f32));
                path.move_to(local(-half, 0.0));
                path.line_to(local(half, 0.0));
                path.move_to(local(0.0, -half));
                path.line_to(local(0.0, half));
                push(&mut paths, path, item.rotation, center, item.stroke);
            }
            draw::UiShapeKind::MagnifierMoveHandle => {
                let unit = width.min(height) / 14.0;
                let mut path = PathBuilder::fill();
                for x in [-1.5, 1.5] {
                    for y in [-2.5, 0.0, 2.5] {
                        ellipse(&mut path, x * unit, y * unit, 0.65 * unit, 0.65 * unit);
                    }
                }
                push(&mut paths, path, item.rotation, center, item.stroke);
            }
            _ => {}
        }
    }
    paths
}

fn push(
    paths: &mut Vec<(gpui::Path<Pixels>, draw::ColorRgba8)>,
    mut builder: PathBuilder,
    rotation: f64,
    center: Point<Pixels>,
    color: draw::ColorRgba8,
) {
    builder.rotate(rotation.to_degrees() as f32);
    builder.translate(center);
    if let Ok(path) = builder.build() {
        paths.push((path, color));
    }
}

fn local(x: f64, y: f64) -> Point<Pixels> {
    point(px(x as f32), px(y as f32))
}

fn rectangle_contour(
    path: &mut PathBuilder,
    item: &draw::UiRectangleDisplayItem,
    width: f64,
    height: f64,
    zoom: f64,
) {
    match item.kind {
        draw::UiShapeKind::MagnifierMoveHandle => {
            let radius = width.min(height) / 2.0;
            ellipse(path, 0.0, 0.0, radius, radius);
        }
        draw::UiShapeKind::ArrowEndpointHandle
        | draw::UiShapeKind::ArrowFocusHandle
        | draw::UiShapeKind::ArrowSegmentHandle
        | draw::UiShapeKind::EraserCursor
        | draw::UiShapeKind::SpeechBubbleAddTailHandle
        | draw::UiShapeKind::SpeechBubbleTailBaseHandle
        | draw::UiShapeKind::SpeechBubbleTailTipHandle
        | draw::UiShapeKind::ShapeTextHandle => ellipse(path, 0.0, 0.0, width / 2.0, height / 2.0),
        _ => {
            // Qt normalizes adjacent radii proportionally, preserving independent
            // corners and allowing a full semicircle on a narrow control.
            let r = item.corner_radii.scaled(zoom);
            let mut radii =
                [r.top_left, r.top_right, r.bottom_right, r.bottom_left].map(|r| r.max(0.0));
            let mut factor: f64 = 1.0;
            for (length, sum) in [
                (width, radii[0] + radii[1]),
                (width, radii[3] + radii[2]),
                (height, radii[0] + radii[3]),
                (height, radii[1] + radii[2]),
            ] {
                if sum > length {
                    factor = factor.min(length / sum);
                }
            }
            radii.iter_mut().for_each(|r| *r *= factor);
            let [tl, tr, br, bl] = radii;
            let w = width / 2.0;
            let h = height / 2.0;
            path.move_to(local(-w + tl, -h));
            path.line_to(local(w - tr, -h));
            corner(path, tr, local(w, -h + tr));
            path.line_to(local(w, h - br));
            corner(path, br, local(w - br, h));
            path.line_to(local(-w + bl, h));
            corner(path, bl, local(-w, h - bl));
            path.line_to(local(-w, -h + tl));
            corner(path, tl, local(-w + tl, -h));
            path.close();
        }
    }
}

fn corner(path: &mut PathBuilder, radius: f64, end: Point<Pixels>) {
    if radius > 0.0 {
        path.arc_to(local(radius, radius), px(0.0), false, true, end);
    } else {
        path.line_to(end);
    }
}

fn ellipse(path: &mut PathBuilder, x: f64, y: f64, rx: f64, ry: f64) {
    path.move_to(local(x + rx, y));
    path.arc_to(local(rx, ry), px(0.0), false, true, local(x - rx, y));
    path.arc_to(local(rx, ry), px(0.0), false, true, local(x + rx, y));
    path.close();
}
