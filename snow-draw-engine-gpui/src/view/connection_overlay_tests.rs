use super::*;

const BLUE: draw::ColorRgba8 = draw::ColorRgba8 {
    r: 64,
    g: 150,
    b: 255,
    a: 255,
};

fn paths(
    item: &draw::UiFocusConnectionDisplayItem,
    filter: bool,
    zoom: f64,
) -> Vec<(gpui::Path<Pixels>, draw::ColorRgba8)> {
    connection_paths(
        item,
        filter,
        draw::FrameView {
            camera: draw::Camera {
                center: draw::Point::default(),
                zoom,
            },
            surface: draw::SurfaceSize {
                width: 400,
                height: 300,
            },
            ..Default::default()
        },
        Bounds::new(point(px(11.0), px(17.0)), gpui::size(px(400.0), px(300.0))),
    )
}

fn ink(
    paths: &[(gpui::Path<Pixels>, draw::ColorRgba8)],
    x: f32,
    y: f32,
) -> Option<draw::ColorRgba8> {
    let p = point(px(211.0 + x), px(167.0 + y));
    paths.iter().rev().find_map(|(path, color)| {
        path.vertices
            .as_chunks::<3>()
            .0
            .iter()
            .any(|t| {
                let cross = |a: Point<Pixels>, b: Point<Pixels>, c: Point<Pixels>| {
                    f32::from(b.x - a.x) * f32::from(c.y - a.y)
                        - f32::from(b.y - a.y) * f32::from(c.x - a.x)
                };
                let a = t[0].xy_position;
                let b = t[1].xy_position;
                let c = t[2].xy_position;
                let edges = [cross(a, b, p), cross(b, c, p), cross(c, a, p)];
                cross(a, b, c).abs() > 1.0e-8
                    && (edges.iter().all(|n| *n >= -1.0e-5) || edges.iter().all(|n| *n <= 1.0e-5))
            })
            .then_some(*color)
    })
}

fn line(zoom: f64, width: f64) -> draw::UiFocusConnectionDisplayItem {
    draw::UiFocusConnectionDisplayItem {
        points: vec![[-40.0 / zoom, 0.0], [40.0 / zoom, 0.0]],
        stroke: BLUE,
        stroke_width: width / zoom,
        ..Default::default()
    }
}

#[test]
fn magnifier_focus_connections_keep_standard_qt_dashes_and_round_caps() {
    for zoom in [0.5, 1.0, 2.75] {
        let mut item = line(zoom, 1.0);
        item.stroke_style = draw::StrokeStyle::Dashed;
        let painted = paths(&item, false, zoom);
        assert_eq!(ink(&painted, -38.0, 0.1), Some(BLUE));
        assert_eq!(ink(&painted, -35.0, 0.1), None);
        assert_eq!(ink(&painted, -32.0, 0.1), Some(BLUE));
        assert_eq!(
            ink(&painted, -40.3, 0.1),
            Some(BLUE),
            "Qt round cap is missing"
        );
        item.stroke_style = draw::StrokeStyle::Dotted;
        let painted = paths(&item, false, zoom);
        assert_eq!(ink(&painted, -39.5, 0.1), Some(BLUE));
        assert_eq!(ink(&painted, -38.0, 0.1), None);
        assert_eq!(ink(&painted, -36.5, 0.1), Some(BLUE));
    }
}

#[test]
fn pen_filter_shows_one_pixel_boundary_without_covering_the_filter() {
    for zoom in [0.5, 1.0, 2.75] {
        let painted = paths(&line(zoom, 20.0), true, zoom);
        assert_eq!(
            ink(&painted, 0.1, 0.0),
            None,
            "filter interior must stay visible"
        );
        assert_eq!(ink(&painted, 0.1, 10.0), Some(BLUE));
        assert_eq!(
            ink(&painted, 0.1, 11.0),
            None,
            "boundary must stay one logical pixel wide"
        );
        assert_eq!(
            ink(&painted, 50.0, 0.1),
            Some(BLUE),
            "brush cap must be rounded"
        );
        assert_eq!(ink(&painted, 50.8, 0.1), None);
    }
}

#[test]
fn pen_filter_curve_commands_generate_hollow_contours() {
    let mut item = line(1.0, 20.0);
    item.path_commands = vec![
        draw::PathCommand::MoveTo {
            point: [-40.0, 0.0],
        },
        draw::PathCommand::QuadTo {
            control: [0.0, 40.0],
            end: [40.0, 0.0],
        },
    ];
    let painted = paths(&item, true, 1.0);
    assert_eq!(ink(&painted, 0.0, 20.0), None);
    assert_eq!(ink(&painted, 0.0, 30.0), Some(BLUE));
    assert_eq!(ink(&painted, 0.0, 10.0), Some(BLUE));
    assert_eq!(ink(&painted, 0.0, 32.0), None);
}

#[test]
fn pen_filter_self_intersections_and_retracing_have_no_interior_seams() {
    let mut item = line(1.0, 16.0);
    item.points = vec![[-35.0, -35.0], [35.0, 35.0], [-35.0, 35.0], [35.0, -35.0]];
    let painted = paths(&item, true, 1.0);
    assert_eq!(ink(&painted, 0.0, 0.0), None);
    assert_eq!(
        ink(&painted, 5.0, -6.3137),
        None,
        "overlap must be simplified before outlining"
    );
    assert_eq!(ink(&painted, 0.0, -11.3137), Some(BLUE));
    item.points = vec![[-40.0, 0.0], [40.0, 0.0], [-40.0, 0.0]];
    let painted = paths(&item, true, 1.0);
    assert_eq!(ink(&painted, 0.0, 0.0), None);
    assert_eq!(ink(&painted, 0.0, 8.0), Some(BLUE));
}

#[test]
fn pen_filter_closed_loop_retains_both_inner_and_outer_boundaries() {
    let mut item = line(1.0, 12.0);
    item.points = vec![
        [-30.0, -30.0],
        [30.0, -30.0],
        [30.0, 30.0],
        [-30.0, 30.0],
        [-30.0, -30.0],
    ];
    let painted = paths(&item, true, 1.0);
    assert_eq!(ink(&painted, 0.0, -36.0), Some(BLUE));
    assert_eq!(ink(&painted, 0.0, -24.0), Some(BLUE));
    assert_eq!(ink(&painted, 0.0, -30.0), None);
    assert_eq!(ink(&painted, 0.0, 0.0), None);
}

#[test]
fn focus_arrowheads_keep_fill_background_and_dash_modes() {
    let mut item = line(1.0, 1.0);
    item.stroke_style = draw::StrokeStyle::Dashed;
    item.arrowhead_primitives = vec![
        draw::ArrowheadDisplayPrimitive {
            kind: draw::ArrowheadDisplayPrimitiveKind::Circle,
            points: vec![],
            center: [20.0, 12.0],
            diameter: 8.0,
            roughness_cap: None,
            fill_mode: draw::ArrowheadDisplayFillMode::Stroke,
            dash_mode: draw::ArrowheadDisplayDashMode::Solid,
        },
        draw::ArrowheadDisplayPrimitive {
            kind: draw::ArrowheadDisplayPrimitiveKind::Polygon,
            points: vec![[-4.0, 16.0], [4.0, 16.0], [0.0, 24.0]],
            center: [0.0, 20.0],
            diameter: 0.0,
            roughness_cap: None,
            fill_mode: draw::ArrowheadDisplayFillMode::Background,
            dash_mode: draw::ArrowheadDisplayDashMode::Solid,
        },
    ];
    let painted = paths(&item, false, 1.0);
    assert_eq!(ink(&painted, 20.0, 12.0), Some(BLUE));
    assert_eq!(
        ink(&painted, 0.0, 20.0),
        Some(draw::ColorRgba8 {
            r: 255,
            g: 255,
            b: 255,
            a: 255
        })
    );
    assert_eq!(ink(&painted, 0.0, 16.0), Some(BLUE));
}

#[test]
fn invalid_and_degenerate_connections_do_not_emit_geometry() {
    for width in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(paths(&line(1.0, width), true, 1.0).is_empty());
    }
    let mut item = line(1.0, 12.0);
    item.points = vec![[0.0, 0.0], [0.0, 0.0]];
    assert!(paths(&item, true, 1.0).is_empty());
}

#[test]
fn huge_magnifier_sources_tessellate_only_visible_dashes() {
    let mut item = line(1.0, 1.0);
    item.stroke_style = draw::StrokeStyle::Dashed;
    item.points = vec![
        [-1.0e8, -20.0],
        [1.0e8, -20.0],
        [1.0e8, 20.0],
        [-1.0e8, 20.0],
    ];
    let painted = paths(&item, false, 1.0);
    assert!(!painted.is_empty());
    assert!(
        painted
            .iter()
            .map(|(path, _)| path.vertices.len())
            .sum::<usize>()
            < 20_000
    );
    assert_eq!(ink(&painted, 0.0, 0.0), None);

    // The visible top of a giant elliptical source must remain bounded even
    // though its curved perimeter extends millions of logical pixels away.
    let r = 1.0e7;
    let k = 0.5522847498307936;
    item.path_commands = vec![
        draw::PathCommand::MoveTo { point: [0.0, 0.0] },
        draw::PathCommand::CubicTo {
            control_1: [k * r, 0.0],
            control_2: [r, (1.0 - k) * r],
            end: [r, r],
        },
    ];
    let painted = paths(&item, false, 1.0);
    assert!(!painted.is_empty());
    assert!(
        painted
            .iter()
            .map(|(path, _)| path.vertices.len())
            .sum::<usize>()
            < 20_000
    );
    assert_eq!(ink(&painted, 2.0, 0.1), Some(BLUE));
    assert_eq!(ink(&painted, 5.0, 0.1), None);
}

#[test]
fn curve_clipping_preserves_dash_phase_of_the_full_visible_path() {
    let mut builder = skia::PathBuilder::new();
    builder.move_to(131.0, 167.0);
    builder.cubic_to(151.0, 247.0, 251.0, 87.0, 291.0, 167.0);
    let centerline = builder.finish().unwrap();
    let render = |clip| {
        vec![(
            clipped_focus_stroke(&centerline, 1.0, draw::StrokeStyle::Dashed, clip).unwrap(),
            BLUE,
        )]
    };
    let full = render(Bounds::new(
        point(px(11.0), px(17.0)),
        gpui::size(px(400.0), px(300.0)),
    ));
    let clipped = render(Bounds::new(
        point(px(211.0), px(117.0)),
        gpui::size(px(45.0), px(100.0)),
    ));
    for x in 3..42 {
        for y in -45..45 {
            assert_eq!(
                ink(&full, x as f32 + 0.17, y as f32 + 0.13),
                ink(&clipped, x as f32 + 0.17, y as f32 + 0.13),
                "clipping changed curve dash phase at {x},{y}"
            );
        }
    }
}
