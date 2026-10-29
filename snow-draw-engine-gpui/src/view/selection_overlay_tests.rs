use super::*;

const BLUE: draw::ColorRgba8 = draw::ColorRgba8 {
    r: 0x40,
    g: 0x96,
    b: 0xff,
    a: 0xff,
};
const WHITE: draw::ColorRgba8 = draw::ColorRgba8 {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};

struct PaintedControl {
    paths: Vec<(gpui::Path<Pixels>, draw::ColorRgba8)>,
    center: Point<Pixels>,
    rotation: f64,
}

impl PaintedControl {
    fn new(item: draw::UiRectangleDisplayItem, zoom: f64) -> Self {
        let frame = draw::FrameView {
            camera: draw::Camera {
                center: draw::Point::new(-12.0, 7.0),
                zoom,
            },
            surface: draw::SurfaceSize {
                width: 500,
                height: 300,
            },
            ..Default::default()
        };
        let bounds = Bounds::new(point(px(17.0), px(23.0)), gpui::size(px(500.0), px(300.0)));
        let view = draw::canvas_to_view(
            draw::Point::new(item.center_x, item.center_y),
            &frame.camera,
            frame.surface,
        );
        Self {
            paths: rectangle_paths(&item, frame, bounds),
            center: bounds.origin + point(px(view.x as f32), px(view.y as f32)),
            rotation: item.rotation,
        }
    }

    // Query the actual GPUI fill/stroke tessellation sent to Window::paint_path.
    // Sampling in control-local logical pixels also tests camera and rotated mapping.
    fn ink_at(&self, x: f64, y: f64) -> Option<draw::ColorRgba8> {
        let (sin, cos) = self.rotation.sin_cos();
        let p = self.center
            + point(
                px((x * cos - y * sin) as f32),
                px((x * sin + y * cos) as f32),
            );
        self.paths.iter().rev().find_map(|(path, color)| {
            path.vertices
                .as_chunks::<3>()
                .0
                .iter()
                .any(|triangle| {
                    let a = triangle[0].xy_position;
                    let b = triangle[1].xy_position;
                    let c = triangle[2].xy_position;
                    let cross = |a: Point<Pixels>, b: Point<Pixels>, c: Point<Pixels>| {
                        f64::from(f32::from(b.x - a.x)) * f64::from(f32::from(c.y - a.y))
                            - f64::from(f32::from(b.y - a.y)) * f64::from(f32::from(c.x - a.x))
                    };
                    let area = cross(a, b, c);
                    if area.abs() < 1.0e-8 {
                        return false;
                    }
                    let edges = [cross(a, b, p), cross(b, c, p), cross(c, a, p)];
                    edges.iter().all(|v| *v >= -1.0e-5) || edges.iter().all(|v| *v <= 1.0e-5)
                })
                .then_some(*color)
        })
    }
}

fn control(
    kind: draw::UiShapeKind,
    size: f64,
    radius: f64,
    zoom: f64,
    rotation: f64,
) -> draw::UiRectangleDisplayItem {
    draw::UiRectangleDisplayItem {
        kind,
        center_x: 31.0,
        center_y: -9.0,
        width: size / zoom,
        height: size / zoom,
        rotation,
        fill: WHITE,
        stroke: BLUE,
        stroke_width: 1.0 / zoom,
        corner_radii: draw::CornerRadii::splat(radius / zoom),
        ..Default::default()
    }
}

#[test]
fn selection_controls_preserve_qt_rounding_at_every_zoom_and_rotation() {
    for zoom in [0.5, 1.0, 2.75] {
        for rotation in [0.0, 0.61, std::f64::consts::FRAC_PI_2] {
            let rounded = PaintedControl::new(
                control(
                    draw::UiShapeKind::SelectionResizeHandle,
                    8.0,
                    2.0,
                    zoom,
                    rotation,
                ),
                zoom,
            );
            assert_eq!(
                rounded.ink_at(-3.8, -3.8),
                None,
                "resize handle lost its 2px corner radius"
            );
            assert_eq!(rounded.ink_at(-3.8, 0.2), Some(BLUE));
            assert_eq!(rounded.ink_at(0.2, 0.1), Some(WHITE));
            for kind in [
                draw::UiShapeKind::SelectionRotationHandle,
                draw::UiShapeKind::SelectionCornerRadiusHandle,
            ] {
                let circle = PaintedControl::new(control(kind, 8.0, 4.0, zoom, rotation), zoom);
                assert_eq!(circle.ink_at(-3.5, -3.5), None, "{kind:?} must be circular");
                assert_eq!(circle.ink_at(0.1, 0.2), Some(WHITE));
                assert_eq!(circle.ink_at(3.8, 0.1), Some(BLUE));
            }
        }
    }
}

#[test]
fn selection_overlay_honors_independent_corner_radii() {
    let mut item = control(draw::UiShapeKind::BindingHighlight, 20.0, 0.0, 1.0, 0.37);
    item.height = 16.0;
    item.stroke.a = 0;
    item.corner_radii = draw::CornerRadii {
        top_left: 0.0,
        top_right: 4.0,
        bottom_right: 7.0,
        bottom_left: 2.0,
    };
    let painted = PaintedControl::new(item, 1.0);
    assert_eq!(painted.ink_at(-9.7, -7.7), Some(WHITE));
    assert_eq!(painted.ink_at(9.7, -7.7), None);
    assert_eq!(painted.ink_at(9.5, 7.5), None);
    assert_eq!(painted.ink_at(-9.5, 7.5), None);
    assert_eq!(painted.ink_at(0.1, 0.1), Some(WHITE));
}

#[test]
fn multi_text_and_magnifier_frames_use_qt_five_three_dashes() {
    for zoom in [0.5, 1.0, 2.75] {
        for rotation in [0.0, 0.61] {
            for kind in [
                draw::UiShapeKind::SelectionMultiFrame,
                draw::UiShapeKind::TextActualFrame,
                draw::UiShapeKind::MagnifierSelectionFrame,
                draw::UiShapeKind::SelectionFrame,
            ] {
                let mut item = control(kind, 80.0, 0.0, zoom, rotation);
                item.height = 40.0 / zoom;
                item.fill.a = 0;
                let painted = PaintedControl::new(item, zoom);
                assert_eq!(painted.ink_at(-37.1, -20.0), Some(BLUE));
                assert_eq!(painted.ink_at(-29.1, -20.0), Some(BLUE));
                assert_eq!(
                    painted.ink_at(-33.4, -20.0),
                    if kind == draw::UiShapeKind::SelectionFrame {
                        Some(BLUE)
                    } else {
                        None
                    },
                    "{kind:?} has incorrect dash gaps"
                );
                assert_eq!(painted.ink_at(0.1, 0.1), None);
            }
        }
    }
}

#[test]
fn text_and_speech_tail_controls_show_circular_plus_icons() {
    for zoom in [0.5, 1.0, 2.75] {
        for rotation in [0.0, 0.61] {
            for kind in [
                draw::UiShapeKind::ShapeTextHandle,
                draw::UiShapeKind::SpeechBubbleAddTailHandle,
            ] {
                let painted = PaintedControl::new(control(kind, 10.0, 5.0, zoom, rotation), zoom);
                assert_eq!(
                    painted.ink_at(0.1, 0.1),
                    Some(BLUE),
                    "{kind:?} is missing its plus icon"
                );
                assert_eq!(painted.ink_at(1.8, 0.1), Some(BLUE));
                assert_eq!(painted.ink_at(0.1, -1.8), Some(BLUE));
                assert_eq!(painted.ink_at(2.0, 2.0), Some(WHITE));
                assert_eq!(painted.ink_at(4.7, 4.7), None);
            }
        }
    }
}

#[test]
fn magnifier_move_control_shows_six_dot_grip() {
    for zoom in [0.5, 1.0, 2.75] {
        for rotation in [0.0, 0.61] {
            let painted = PaintedControl::new(
                control(
                    draw::UiShapeKind::MagnifierMoveHandle,
                    14.0,
                    7.0,
                    zoom,
                    rotation,
                ),
                zoom,
            );
            for x in [-1.5, 1.5] {
                for y in [-2.5, 0.0, 2.5] {
                    assert_eq!(
                        painted.ink_at(x, y),
                        Some(BLUE),
                        "magnifier grip dot is missing"
                    );
                }
            }
            assert_eq!(painted.ink_at(0.0, 0.0), Some(WHITE));
            assert_eq!(painted.ink_at(0.0, 2.5), Some(WHITE));
            assert_eq!(painted.ink_at(6.5, 6.5), None);
        }
    }
}

#[test]
fn text_hover_only_underlines_the_bottom_edge() {
    let mut item = control(draw::UiShapeKind::TextHoverUnderline, 80.0, 0.0, 2.0, 0.61);
    item.height = 20.0;
    let painted = PaintedControl::new(item, 2.0);
    assert_eq!(painted.ink_at(0.1, 20.0), Some(BLUE));
    assert_eq!(painted.ink_at(0.1, -20.0), None);
    assert_eq!(painted.ink_at(-40.0, 0.1), None);
    assert_eq!(painted.ink_at(0.1, 0.1), None);
}

#[test]
fn selection_large_offscreen_dashed_frames_keep_visible_edges_and_phase() {
    for zoom in [0.5, 1.0, 2.75] {
        let mut item = control(
            draw::UiShapeKind::SelectionMultiFrame,
            400_000.0,
            0.0,
            zoom,
            0.0,
        );
        item.height = 40.0 / zoom;
        item.fill.a = 0;
        let painted = PaintedControl::new(item, zoom);
        assert_eq!(painted.ink_at(-247.5, -20.0), Some(BLUE));
        assert_eq!(painted.ink_at(-242.0, -20.0), None);
        assert!(
            painted
                .paths
                .iter()
                .map(|(path, _)| path.vertices.len())
                .sum::<usize>()
                < 10_000
        );
    }
}
