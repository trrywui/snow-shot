use super::*;
use usvg::tiny_skia_path as skia;

pub(super) fn guide_paths(
    item: &draw::SnapGuideDisplayItem,
    frame: draw::FrameView,
    bounds: Bounds<Pixels>,
) -> Vec<gpui::Path<Pixels>> {
    if item.line_width <= 0.0 || item.color.a == 0 {
        return Vec::new();
    }
    let map = |p: draw::Point<f64>| {
        let p = draw::canvas_to_view(p, &frame.camera, frame.surface);
        bounds.origin + point(px(p.x as f32), px(p.y as f32))
    };
    let stroke = skia::Stroke {
        width: item.line_width as f32,
        line_cap: skia::LineCap::Round,
        line_join: skia::LineJoin::Round,
        ..Default::default()
    };
    let pattern = [
        (item.gap_dash_length * item.line_width) as f32,
        (item.gap_dash_gap * item.line_width) as f32,
    ];
    let dash = (item.kind == draw::SnapGuideKind::Gap
        && item.gap_dash_length > 0.0
        && item.gap_dash_gap > 0.0)
        .then_some((&pattern[..], 0.0));
    let mut paths = Vec::with_capacity(5);
    let mut line = |a, b| {
        if let Some(path) = overlay_geometry::stroke_line(a, b, bounds, &stroke, dash) {
            paths.push(path);
        }
    };
    line(map(item.start), map(item.end));
    if item.marker_size <= 0.0 {
        return paths;
    }
    // Qt uses endpoints when the editor supplies no explicit snap markers.
    let endpoints = [item.start, item.end];
    let markers = if item.marker_count == 0 {
        &endpoints[..]
    } else {
        &item.markers[..usize::from(item.marker_count).min(item.markers.len())]
    };
    for marker in markers {
        let p = map(*marker);
        if item.kind == draw::SnapGuideKind::Gap {
            let half = px((item.marker_size * 0.75) as f32);
            let offset = if item.axis == draw::SnapGuideAxis::Horizontal {
                point(px(0.0), half)
            } else {
                point(half, px(0.0))
            };
            line(p - offset, p + offset);
        } else {
            let half = px((item.marker_size * 0.35) as f32);
            line(p - point(half, half), p + point(half, half));
            line(p + point(-half, half), p + point(half, -half));
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covers(paths: &[gpui::Path<Pixels>], x: f32, y: f32) -> bool {
        paths.iter().any(|path| {
            path.vertices.as_chunks::<3>().0.iter().any(|t| {
                let p = point(px(x), px(y));
                let cross = |a: Point<Pixels>, b: Point<Pixels>, c: Point<Pixels>| {
                    f32::from(b.x - a.x) * f32::from(c.y - a.y)
                        - f32::from(b.y - a.y) * f32::from(c.x - a.x)
                };
                let [a, b, c] = [t[0].xy_position, t[1].xy_position, t[2].xy_position];
                let edges = [cross(a, b, p), cross(b, c, p), cross(c, a, p)];
                cross(a, b, c).abs() > 1.0e-6
                    && (edges.iter().all(|e| *e >= -1.0e-5) || edges.iter().all(|e| *e <= 1.0e-5))
            })
        })
    }

    fn guide(kind: draw::SnapGuideKind) -> draw::SnapGuideDisplayItem {
        draw::SnapGuideDisplayItem {
            kind,
            start: draw::Point::new(20.0, 30.0),
            end: draw::Point::new(80.0, 30.0),
            color: draw::ColorRgba8 {
                r: 255,
                a: 255,
                ..Default::default()
            },
            line_width: 1.0,
            marker_size: 8.0,
            gap_dash_length: 4.0,
            gap_dash_gap: 4.0,
            ..Default::default()
        }
    }

    fn paths(item: &draw::SnapGuideDisplayItem, zoom: f64) -> Vec<gpui::Path<Pixels>> {
        guide_paths(
            item,
            draw::FrameView {
                camera: draw::Camera {
                    zoom,
                    ..Default::default()
                },
                ..Default::default()
            },
            Bounds::new(Point::default(), gpui::size(px(1024.0), px(768.0))),
        )
    }

    #[test]
    fn selection_gap_guides_have_dashes_and_perpendicular_ticks() {
        for zoom in [0.5, 1.0, 2.5] {
            let mut item = guide(draw::SnapGuideKind::Gap);
            // Hold endpoints fixed in view pixels while testing zoom-independent styles.
            item.start.x /= zoom;
            item.start.y /= zoom;
            item.end.x /= zoom;
            item.end.y /= zoom;
            let paths = paths(&item, zoom);
            assert!(covers(&paths, 22.0, 30.0));
            assert!(!covers(&paths, 26.0, 30.0));
            assert!(covers(&paths, 20.0, 25.0));
            assert!(!covers(&paths, 18.0, 28.0));
        }
    }

    #[test]
    fn selection_point_guides_use_endpoint_crosses_and_explicit_markers() {
        let mut item = guide(draw::SnapGuideKind::Point);
        let automatic = paths(&item, 1.0);
        assert!(covers(&automatic, 20.0, 30.0));
        assert!(covers(&automatic, 21.5, 31.5));
        // Qt's cross has a 2.8px half span; the previous 4px span reached here.
        assert!(!covers(&automatic, 23.5, 33.5));
        item.marker_count = 1;
        item.markers[0] = draw::Point::new(50.0, 30.0);
        let explicit = paths(&item, 1.0);
        assert!(!covers(&explicit, 21.5, 31.5));
        assert!(covers(&explicit, 51.5, 31.5));
    }

    #[test]
    fn selection_vertical_gap_guides_use_horizontal_ticks() {
        let mut item = guide(draw::SnapGuideKind::Gap);
        item.axis = draw::SnapGuideAxis::Vertical;
        item.end = draw::Point::new(20.0, 90.0);
        let paths = paths(&item, 1.0);
        assert!(covers(&paths, 15.0, 30.0));
        assert!(!covers(&paths, 20.0, 25.0));
    }
}
