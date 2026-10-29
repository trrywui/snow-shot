use super::*;
use usvg::tiny_skia_path as skia;

/// Stroke only the visible part of a straight overlay segment. `dash` supplies
/// logical pixel intervals and their phase at the original `a`; `None` is solid.
pub(super) fn stroke_line(
    a: Point<Pixels>,
    b: Point<Pixels>,
    clip: Bounds<Pixels>,
    stroke: &skia::Stroke,
    dash: Option<(&[f32], f64)>,
) -> Option<gpui::Path<Pixels>> {
    if !stroke.width.is_finite()
        || stroke.width <= 0.0
        || clip.size.width < px(0.0)
        || clip.size.height < px(0.0)
    {
        return None;
    }
    let margin = px(stroke.width * 2.0 + 1.0);
    let expanded = Bounds::new(
        clip.origin - point(margin, margin),
        gpui::size(
            clip.size.width + margin * 2.0,
            clip.size.height + margin * 2.0,
        ),
    );
    let (start, end, trimmed) = clipped_line(a, b, expanded)?;
    let mut stroke = stroke.clone();
    stroke.dash = if let Some((pattern, phase)) = dash {
        let period: f64 = pattern.iter().copied().map(f64::from).sum();
        if !period.is_finite() || period <= 0.0 || !phase.is_finite() {
            return None;
        }
        Some(skia::StrokeDash::new(
            pattern.to_vec(),
            (phase + trimmed).rem_euclid(period) as f32,
        )?)
    } else {
        None
    };
    let mut path = skia::PathBuilder::new();
    path.move_to(f32::from(start.x), f32::from(start.y));
    path.line_to(f32::from(end.x), f32::from(end.y));
    connection_overlay::stroke_path(&path.finish()?, &stroke)
}

/// Clip before tessellating so overlay work follows the visible viewport size.
/// The returned distance from `a` to the clipped start preserves dash phase.
pub(super) fn clipped_line(
    a: Point<Pixels>,
    b: Point<Pixels>,
    clip: Bounds<Pixels>,
) -> Option<(Point<Pixels>, Point<Pixels>, f64)> {
    let [ax, ay, bx, by, left, top, width, height] = [
        a.x,
        a.y,
        b.x,
        b.y,
        clip.origin.x,
        clip.origin.y,
        clip.size.width,
        clip.size.height,
    ]
    .map(|v| f64::from(f32::from(v)));
    if ![ax, ay, bx, by, left, top, width, height]
        .iter()
        .all(|v| v.is_finite())
        || width < 0.0
        || height < 0.0
    {
        return None;
    }
    let dx = bx - ax;
    let dy = by - ay;
    let mut start: f64 = 0.0;
    let mut end: f64 = 1.0;
    for (direction, distance) in [
        (-dx, ax - left),
        (dx, left + width - ax),
        (-dy, ay - top),
        (dy, top + height - ay),
    ] {
        if direction == 0.0 {
            if distance < 0.0 {
                return None;
            }
            continue;
        }
        let t = distance / direction;
        if direction < 0.0 {
            start = start.max(t);
        } else {
            end = end.min(t);
        }
        if start > end {
            return None;
        }
    }
    let at = |t| point(px((ax + dx * t) as f32), px((ay + dy * t) as f32));
    Some((at(start), at(end), dx.hypot(dy) * start))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }

    fn clip(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(p(x, y), gpui::size(px(width), px(height)))
    }

    fn covers(path: &gpui::Path<Pixels>, x: f32, y: f32) -> bool {
        let query = p(x, y);
        path.vertices.as_chunks::<3>().0.iter().any(|triangle| {
            let [a, b, c] = [
                triangle[0].xy_position,
                triangle[1].xy_position,
                triangle[2].xy_position,
            ];
            let cross = |a: Point<Pixels>, b: Point<Pixels>, c: Point<Pixels>| {
                f32::from(b.x - a.x) * f32::from(c.y - a.y)
                    - f32::from(b.y - a.y) * f32::from(c.x - a.x)
            };
            let edges = [cross(a, b, query), cross(b, c, query), cross(c, a, query)];
            cross(a, b, c).abs() > 1.0e-6
                && (edges.iter().all(|v| *v >= -1.0e-5) || edges.iter().all(|v| *v <= 1.0e-5))
        })
    }

    #[test]
    fn selection_overlay_large_dashed_strokes_keep_visible_phase_and_bounded_geometry() {
        let viewport = clip(10.0, 20.0, 100.0, 80.0);
        let stroke = skia::Stroke {
            width: 1.0,
            ..Default::default()
        };
        let forward = stroke_line(
            p(-1.0e9, 30.0),
            p(1.0e9, 30.0),
            viewport,
            &stroke,
            Some((&[5.0, 3.0], 0.0)),
        )
        .unwrap();
        assert!(forward.vertices.len() < 1000);
        assert!(covers(&forward, 10.3, 30.2));
        assert!(!covers(&forward, 14.3, 30.2));
        let reverse = stroke_line(
            p(1.0e9, 30.0),
            p(-1.0e9, 30.0),
            viewport,
            &stroke,
            Some((&[5.0, 3.0], 0.0)),
        )
        .unwrap();
        assert!(reverse.vertices.len() < 1000);
        assert!(!covers(&reverse, 10.3, 30.2));
        assert!(covers(&reverse, 14.3, 30.2));
        let shifted = stroke_line(
            p(-1.0e9, 30.0),
            p(1.0e9, 30.0),
            viewport,
            &stroke,
            Some((&[5.0, 3.0], 2.0)),
        )
        .unwrap();
        assert!(covers(&shifted, 10.3, 30.2));
        assert!(!covers(&shifted, 12.3, 30.2));
    }

    #[test]
    fn selection_overlay_line_clipping_retains_strokes_just_outside_viewport() {
        let path = stroke_line(
            p(-1.0e9, 19.5),
            p(1.0e9, 19.5),
            clip(10.0, 20.0, 100.0, 80.0),
            &skia::Stroke {
                width: 2.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert!(covers(&path, 50.0, 20.1));
    }

    #[test]
    fn selection_overlay_clips_huge_rotated_lines_before_tessellation() {
        let (start, end, trimmed) = clipped_line(
            p(-1.0e9, -1.0e9),
            p(1.0e9, 1.0e9),
            clip(10.0, 20.0, 100.0, 80.0),
        )
        .unwrap();
        assert_eq!(start, p(20.0, 20.0));
        assert_eq!(end, p(100.0, 100.0));
        assert!((trimmed - (1.0e9_f64 + 20.0) * 2.0_f64.sqrt()).abs() < 1.0e-6);
    }

    #[test]
    fn selection_overlay_clipping_preserves_direction_and_dash_phase() {
        let viewport = clip(3.0, 4.0, 60.0, 80.0);
        let (start, end, trimmed) =
            clipped_line(p(-3.0e6, -4.0e6), p(3.0e6, 4.0e6), viewport).unwrap();
        assert_eq!(start, p(3.0, 4.0));
        assert_eq!(end, p(63.0, 84.0));
        assert!((trimmed.rem_euclid(8.0) - 5.0).abs() < 1.0e-8);

        let (reverse_start, reverse_end, reverse_trimmed) =
            clipped_line(p(3.0e6, 4.0e6), p(-3.0e6, -4.0e6), viewport).unwrap();
        assert_eq!(reverse_start, end);
        assert_eq!(reverse_end, start);
        assert!((reverse_trimmed - (5.0e6 - 105.0)).abs() < 1.0e-8);
    }

    #[test]
    fn selection_overlay_clipping_culls_misses_and_preserves_parallel_visible_lines() {
        let viewport = clip(10.0, 20.0, 100.0, 80.0);
        assert!(clipped_line(p(-100.0, 19.0), p(200.0, 19.0), viewport).is_none());
        assert!(clipped_line(p(9.0, -100.0), p(9.0, 200.0), viewport).is_none());
        assert!(clipped_line(p(0.0, 20.0), p(10.0, 0.0), viewport).is_none());
        let (start, end, trimmed) =
            clipped_line(p(-100.0, 20.0), p(200.0, 20.0), viewport).unwrap();
        assert_eq!(start, p(10.0, 20.0));
        assert_eq!(end, p(110.0, 20.0));
        assert!((trimmed - 110.0).abs() < 1.0e-10);
    }

    #[test]
    fn selection_overlay_clipping_handles_points_and_already_visible_lines() {
        let viewport = clip(10.0, 20.0, 100.0, 80.0);
        assert_eq!(
            clipped_line(p(30.0, 40.0), p(70.0, 80.0), viewport),
            Some((p(30.0, 40.0), p(70.0, 80.0), 0.0))
        );
        assert_eq!(
            clipped_line(p(10.0, 20.0), p(10.0, 20.0), viewport),
            Some((p(10.0, 20.0), p(10.0, 20.0), 0.0))
        );
        assert!(clipped_line(p(0.0, 0.0), p(0.0, 0.0), viewport).is_none());
    }

    #[test]
    fn selection_overlay_clipping_rejects_nonfinite_geometry_and_negative_bounds() {
        let viewport = clip(10.0, 20.0, 100.0, 80.0);
        assert!(clipped_line(p(f32::NAN, 0.0), p(0.0, 0.0), viewport).is_none());
        assert!(clipped_line(p(0.0, 0.0), p(f32::INFINITY, 0.0), viewport).is_none());
        assert!(clipped_line(p(30.0, 40.0), p(70.0, 80.0), clip(10.0, 20.0, -1.0, 80.0)).is_none());
    }
}
