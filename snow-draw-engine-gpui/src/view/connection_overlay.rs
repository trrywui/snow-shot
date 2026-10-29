//! Connections use their stroke style; pen filters expose only the brush boundary.
use super::*;
use std::collections::BTreeMap;
use usvg::tiny_skia_path as skia;

#[cfg(test)]
#[path = "connection_overlay_tests.rs"]
mod tests;

pub(super) fn paint(
    item: &draw::UiFocusConnectionDisplayItem,
    pen_filter: bool,
    frame: draw::FrameView,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    for (path, color) in connection_paths(item, pen_filter, frame, bounds) {
        window.paint_path(path, element::gpui_color(color));
    }
}

fn connection_paths(
    item: &draw::UiFocusConnectionDisplayItem,
    pen_filter: bool,
    frame: draw::FrameView,
    bounds: Bounds<Pixels>,
) -> Vec<(gpui::Path<Pixels>, draw::ColorRgba8)> {
    let width = (item.stroke_width * frame.camera.zoom) as f32;
    if !width.is_finite() || width <= 0.0 || item.stroke.a == 0 {
        return Vec::new();
    }
    let map = |[x, y]: [f64; 2]| {
        let view = draw::canvas_to_view(draw::Point::new(x, y), &frame.camera, frame.surface);
        skia::Point::from_xy(
            f32::from(bounds.origin.x) + view.x as f32,
            f32::from(bounds.origin.y) + view.y as f32,
        )
    };
    let mut paths = Vec::new();
    if let Some(centerline) = centerline(item, map) {
        let stroke = if pen_filter {
            // Qt simplifies the filled brush footprint before drawing its 1px outline.
            // The nonzero fill tessellation resolves overlaps. Only mesh boundary edges
            // survive, so a crossing or retraced segment cannot leave an interior seam.
            styled_stroke(&centerline, width, draw::StrokeStyle::Solid)
                .and_then(|footprint| union_boundary(&footprint))
                .and_then(|boundary| styled_stroke(&boundary, 1.0, draw::StrokeStyle::Solid))
                .and_then(|path| tessellate(&path))
        } else {
            clipped_focus_stroke(&centerline, width, item.stroke_style, bounds)
        };
        if let Some(path) = stroke {
            paths.push((path, item.stroke));
        }
    }
    if pen_filter {
        return paths;
    }
    for head in &item.arrowhead_primitives {
        let mut builder = skia::PathBuilder::new();
        let closed = head.kind != draw::ArrowheadDisplayPrimitiveKind::Line;
        if head.kind == draw::ArrowheadDisplayPrimitiveKind::Circle {
            let center = map(head.center);
            builder.push_circle(
                center.x,
                center.y,
                (head.diameter * frame.camera.zoom / 2.0) as f32,
            );
        } else {
            for (index, point) in head.points.iter().copied().map(map).enumerate() {
                if index == 0 {
                    builder.move_to(point.x, point.y);
                } else {
                    builder.line_to(point.x, point.y);
                }
            }
            if closed {
                builder.close();
            }
        }
        let Some(shape) = builder.finish() else {
            continue;
        };
        if closed && let Some(path) = tessellate(&shape) {
            let color = match head.fill_mode {
                draw::ArrowheadDisplayFillMode::Stroke => item.stroke,
                draw::ArrowheadDisplayFillMode::Background => draw::ColorRgba8 {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                },
            };
            paths.push((path, color));
        }
        let style = match head.dash_mode {
            draw::ArrowheadDisplayDashMode::Inherit => item.stroke_style,
            draw::ArrowheadDisplayDashMode::Solid => draw::StrokeStyle::Solid,
            draw::ArrowheadDisplayDashMode::DottedCap => draw::StrokeStyle::Dotted,
        };
        if let Some(path) = stroke_path(&shape, &qt_stroke(width, style)) {
            paths.push((path, item.stroke));
        }
    }
    paths
}

fn centerline(
    item: &draw::UiFocusConnectionDisplayItem,
    map: impl Fn([f64; 2]) -> skia::Point,
) -> Option<skia::Path> {
    let mut path = skia::PathBuilder::new();
    for command in &item.path_commands {
        match *command {
            draw::PathCommand::MoveTo { point } => {
                let p = map(point);
                path.move_to(p.x, p.y);
            }
            draw::PathCommand::LineTo { point } => {
                let p = map(point);
                path.line_to(p.x, p.y);
            }
            draw::PathCommand::QuadTo { control, end } => {
                let c = map(control);
                let p = map(end);
                path.quad_to(c.x, c.y, p.x, p.y);
            }
            draw::PathCommand::CubicTo {
                control_1,
                control_2,
                end,
            } => {
                let a = map(control_1);
                let b = map(control_2);
                let p = map(end);
                path.cubic_to(a.x, a.y, b.x, b.y, p.x, p.y);
            }
        }
    }
    if item.path_commands.is_empty() {
        for (index, point) in item.points.iter().copied().map(map).enumerate() {
            if index == 0 {
                path.move_to(point.x, point.y)
            } else {
                path.line_to(point.x, point.y)
            }
        }
    }
    let path = path.finish()?;
    (path.bounds().width() > 0.0 || path.bounds().height() > 0.0).then_some(path)
}

type Curve = [[f64; 2]; 4];

fn clipped_focus_stroke(
    path: &skia::Path,
    width: f32,
    style: draw::StrokeStyle,
    clip: Bounds<Pixels>,
) -> Option<gpui::Path<Pixels>> {
    let stroke = qt_stroke(width, draw::StrokeStyle::Solid);
    let pattern = match style {
        draw::StrokeStyle::Solid => None,
        draw::StrokeStyle::Dashed => Some([4.0 * width, 2.0 * width]),
        draw::StrokeStyle::Dotted => Some([width, 2.0 * width]),
    };
    let margin = f64::from(width) * 2.0 + 1.0;
    let region = [
        f64::from(clip.origin.x) - margin,
        f64::from(clip.origin.y) - margin,
        f64::from(clip.origin.x + clip.size.width) + margin,
        f64::from(clip.origin.y + clip.size.height) + margin,
    ];
    let mut mesh = None;
    let mut phase = 0.0;
    let mut current = [0.0, 0.0];
    let mut start = current;
    let p = |p: skia::Point| [f64::from(p.x), f64::from(p.y)];
    let mut segment = |a: [f64; 2], b: [f64; 2], phase: f64| {
        let p = |[x, y]: [f64; 2]| point(px(x as f32), px(y as f32));
        if let Some(path) = overlay_geometry::stroke_line(
            p(a),
            p(b),
            clip,
            &stroke,
            pattern.as_ref().map(|p| (&p[..], phase)),
        ) {
            append_mesh(&mut mesh, path);
        }
    };
    for command in path.segments() {
        match command {
            skia::PathSegment::MoveTo(point) => {
                current = p(point);
                start = current;
                phase = 0.0;
            }
            skia::PathSegment::LineTo(point) => {
                let end = p(point);
                segment(current, end, phase);
                phase += distance(current, end);
                current = end;
            }
            skia::PathSegment::QuadTo(control, end) => {
                let end = p(end);
                let control = p(control);
                let curve = [
                    current,
                    mix(current, control, 2.0 / 3.0),
                    mix(end, control, 2.0 / 3.0),
                    end,
                ];
                clipped_curve(curve, region, &mut phase, &mut segment);
                current = end;
            }
            skia::PathSegment::CubicTo(a, b, end) => {
                let end = p(end);
                clipped_curve([current, p(a), p(b), end], region, &mut phase, &mut segment);
                current = end;
            }
            skia::PathSegment::Close => {
                segment(current, start, phase);
                phase += distance(current, start);
                current = start;
            }
        }
    }
    mesh
}

fn append_mesh(mesh: &mut Option<gpui::Path<Pixels>>, path: gpui::Path<Pixels>) {
    let Some(mesh) = mesh else {
        *mesh = Some(path);
        return;
    };
    for t in path.vertices.as_chunks::<3>().0.iter() {
        mesh.push_triangle(
            (t[0].xy_position, t[1].xy_position, t[2].xy_position),
            (t[0].st_position, t[1].st_position, t[2].st_position),
        );
    }
}

fn mix(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}
fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}
fn split(c: Curve) -> (Curve, Curve) {
    let a = mix(c[0], c[1], 0.5);
    let b = mix(c[1], c[2], 0.5);
    let d = mix(c[2], c[3], 0.5);
    let e = mix(a, b, 0.5);
    let f = mix(b, d, 0.5);
    let mid = mix(e, f, 0.5);
    ([c[0], a, e, mid], [mid, f, d, c[3]])
}

fn clipped_curve(
    c: Curve,
    clip: [f64; 4],
    phase: &mut f64,
    segment: &mut impl FnMut([f64; 2], [f64; 2], f64),
) {
    let outside = c.iter().all(|p| p[0] < clip[0])
        || c.iter().all(|p| p[0] > clip[2])
        || c.iter().all(|p| p[1] < clip[1])
        || c.iter().all(|p| p[1] > clip[3]);
    if outside {
        *phase += curve_length(c, 0.001);
        return;
    }
    let distance_to_chord = |p: [f64; 2]| {
        let dx = c[3][0] - c[0][0];
        let dy = c[3][1] - c[0][1];
        let squared = dx * dx + dy * dy;
        let t = if squared == 0.0 {
            0.0
        } else {
            (((p[0] - c[0][0]) * dx + (p[1] - c[0][1]) * dy) / squared).clamp(0.0, 1.0)
        };
        distance(p, mix(c[0], c[3], t))
    };
    // A tenth of a logical pixel bounds visible curve flattening error at every zoom.
    if distance_to_chord(c[1]).max(distance_to_chord(c[2])) <= 0.1 {
        segment(c[0], c[3], *phase);
        *phase += curve_length(c, 0.001);
    } else {
        let (a, b) = split(c);
        clipped_curve(a, clip, phase, segment);
        clipped_curve(b, clip, phase, segment);
    }
}

fn curve_length(c: Curve, tolerance: f64) -> f64 {
    // Eight-point Gauss-Legendre quadrature retains dash phase through invisible
    // curve portions without generating offscreen dash geometry.
    let estimate = |c: Curve| {
        [
            (0.1834346424956498, 0.362683783378362),
            (0.525532409916329, 0.3137066458778873),
            (0.7966664774136267, 0.2223810344533745),
            (0.9602898564975363, 0.1012285362903763),
        ]
        .into_iter()
        .map(|(x, w)| {
            [-x, x]
                .into_iter()
                .map(|v| {
                    let t = (v + 1.0) / 2.0;
                    let s = 1.0 - t;
                    let d = |i| {
                        3.0 * (s * s * (c[1][i] - c[0][i])
                            + 2.0 * s * t * (c[2][i] - c[1][i])
                            + t * t * (c[3][i] - c[2][i]))
                    };
                    w * d(0).hypot(d(1)) / 2.0
                })
                .sum::<f64>()
        })
        .sum::<f64>()
    };
    let whole = estimate(c);
    let (a, b) = split(c);
    let parts = estimate(a) + estimate(b);
    if (parts - whole).abs() <= tolerance.max(f64::EPSILON * parts * 32.0) {
        parts
    } else {
        curve_length(a, tolerance / 2.0) + curve_length(b, tolerance / 2.0)
    }
}

fn styled_stroke(path: &skia::Path, width: f32, style: draw::StrokeStyle) -> Option<skia::Path> {
    stroke_geometry(path, &qt_stroke(width, style))
}

fn qt_stroke(width: f32, style: draw::StrokeStyle) -> skia::Stroke {
    let dash = match style {
        draw::StrokeStyle::Solid => None,
        // Qt::DashLine and Qt::DotLine use pen-width-relative patterns.
        draw::StrokeStyle::Dashed => skia::StrokeDash::new(vec![4.0 * width, 2.0 * width], 0.0),
        draw::StrokeStyle::Dotted => skia::StrokeDash::new(vec![width, 2.0 * width], 0.0),
    };
    skia::Stroke {
        width,
        line_cap: skia::LineCap::Round,
        line_join: skia::LineJoin::Round,
        dash,
        ..Default::default()
    }
}

pub(super) fn stroke_path(path: &skia::Path, stroke: &skia::Stroke) -> Option<gpui::Path<Pixels>> {
    tessellate(&stroke_geometry(path, stroke)?)
}

fn stroke_geometry(path: &skia::Path, stroke: &skia::Stroke) -> Option<skia::Path> {
    let dashed;
    let path = if let Some(dash) = &stroke.dash {
        dashed = path.dash(dash, 1.0)?;
        &dashed
    } else {
        path
    };
    path.stroke(stroke, 1.0)
}

fn tessellate(path: &skia::Path) -> Option<gpui::Path<Pixels>> {
    let mut builder = PathBuilder::fill().with_style(gpui::PathStyle::Fill(
        gpui::FillOptions::default().with_fill_rule(gpui::FillRule::NonZero),
    ));
    let p = |p: skia::Point| point(px(p.x), px(p.y));
    for segment in path.segments() {
        match segment {
            skia::PathSegment::MoveTo(point) => builder.move_to(p(point)),
            skia::PathSegment::LineTo(point) => builder.line_to(p(point)),
            skia::PathSegment::QuadTo(control, end) => builder.curve_to(p(end), p(control)),
            skia::PathSegment::CubicTo(a, b, end) => builder.cubic_bezier_to(p(end), p(a), p(b)),
            skia::PathSegment::Close => builder.close(),
        }
    }
    builder.build().ok()
}

fn union_boundary(footprint: &skia::Path) -> Option<skia::Path> {
    let mesh = tessellate(footprint)?;
    type Vertex = (u32, u32);
    let key = |p: Point<Pixels>| -> Vertex {
        let canonical = |v: Pixels| {
            let v = f32::from(v);
            if v == 0.0 { 0 } else { v.to_bits() }
        };
        (canonical(p.x), canonical(p.y))
    };
    let mut edges = BTreeMap::<(Vertex, Vertex), usize>::new();
    for triangle in mesh.vertices.as_chunks::<3>().0.iter() {
        let [a, b, c] = [
            triangle[0].xy_position,
            triangle[1].xy_position,
            triangle[2].xy_position,
        ];
        if f32::from(b.x - a.x) * f32::from(c.y - a.y)
            == f32::from(b.y - a.y) * f32::from(c.x - a.x)
        {
            continue;
        }
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let a = key(triangle[a].xy_position);
            let b = key(triangle[b].xy_position);
            if a != b {
                *edges
                    .entry(if a < b { (a, b) } else { (b, a) })
                    .or_default() += 1;
            }
        }
    }
    let mut boundary = skia::PathBuilder::new();
    for ((a, b), count) in edges {
        if count == 1 {
            boundary.move_to(f32::from_bits(a.0), f32::from_bits(a.1));
            boundary.line_to(f32::from_bits(b.0), f32::from_bits(b.1));
        }
    }
    boundary.finish()
}
