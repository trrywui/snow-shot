use super::*;
use gpui::{TestAppContext, VisualTestContext, size};

fn overlay_rect(kind: draw::UiShapeKind, width: f64, height: f64) -> draw::UiRectangleDisplayItem {
    draw::UiRectangleDisplayItem {
        kind,
        center_x: 120.0,
        center_y: 90.0,
        width,
        height,
        rotation: 0.0,
        fill: draw::ColorRgba8::default(),
        fill_style: draw::DisplayFillStyle::Solid,
        stroke: draw::ColorRgba8::default(),
        stroke_width: 1.0,
        corner_radii: draw::CornerRadii::default(),
    }
}

fn world_from_local(item: draw::UiRectangleDisplayItem, x: f64, y: f64) -> draw::Point<f64> {
    let (sin, cos) = item.rotation.sin_cos();
    draw::Point::new(
        item.center_x + x * cos - y * sin,
        item.center_y + x * sin + y * cos,
    )
}

#[test]
fn selection_clip_hit_geometry_preserves_frame_edges_at_each_zoom_and_rotation() {
    for kind in [
        draw::UiShapeKind::SelectionFrame,
        draw::UiShapeKind::SelectionMultiFrame,
        draw::UiShapeKind::MagnifierSelectionFrame,
    ] {
        for zoom in [0.5, 1.0, 2.0] {
            for rotation in [0.0, 0.7] {
                let item = draw::UiRectangleDisplayItem {
                    rotation,
                    ..overlay_rect(kind, 100.0, 80.0)
                };
                let overlays = [draw::OverlayDisplayItem::Rectangle(item)];
                for offset_px in [-5.9, 0.0, 5.9] {
                    for (x, y) in [
                        (50.0 + offset_px / zoom, 0.0),
                        (-50.0 - offset_px / zoom, 0.0),
                        (0.0, 40.0 + offset_px / zoom),
                        (0.0, -40.0 - offset_px / zoom),
                    ] {
                        assert!(selection_input::hits_selection_overlay(
                            &overlays,
                            world_from_local(item, x, y),
                            zoom,
                        ));
                    }
                }
                for (x, y) in [(0.0, 0.0), (50.0 + 7.0 / zoom, 0.0), (50.0, 40.0)] {
                    assert!(!selection_input::hits_selection_overlay(
                        &overlays,
                        world_from_local(item, x, y),
                        zoom,
                    ));
                }
            }
        }
    }
}

#[test]
fn selection_clip_hit_geometry_includes_text_move_ring_without_including_text_interior() {
    let frame = overlay_rect(draw::UiShapeKind::SelectionFrame, 128.0, 108.0);
    let text = overlay_rect(draw::UiShapeKind::TextActualFrame, 100.0, 80.0);
    let overlays = [
        draw::OverlayDisplayItem::Rectangle(frame),
        draw::OverlayDisplayItem::Rectangle(text),
    ];
    for (x, y) in [(57.0, 0.0), (-57.0, 0.0), (0.0, 47.0), (0.0, -47.0)] {
        assert!(selection_input::hits_selection_overlay(
            &overlays,
            world_from_local(frame, x, y),
            1.0,
        ));
    }
    for (x, y) in [(0.0, 0.0), (50.0, 0.0), (0.0, 40.0), (72.0, 0.0)] {
        assert!(!selection_input::hits_selection_overlay(
            &overlays,
            world_from_local(frame, x, y),
            1.0,
        ));
    }
}

#[test]
fn selection_clip_hit_geometry_uses_twelve_pixel_minimum_handle_target() {
    for zoom in [0.5, 1.0, 2.0] {
        let handle = overlay_rect(
            draw::UiShapeKind::SelectionResizeHandle,
            8.0 / zoom,
            8.0 / zoom,
        );
        let overlays = [draw::OverlayDisplayItem::Rectangle(handle)];
        assert!(selection_input::hits_selection_overlay(
            &overlays,
            world_from_local(handle, 6.0 / zoom, 0.0),
            zoom,
        ));
        assert!(!selection_input::hits_selection_overlay(
            &overlays,
            world_from_local(handle, 6.1 / zoom, 0.0),
            zoom,
        ));
    }
}

fn fixture(cx: &mut TestAppContext, annotation: &[u8]) -> (Entity<CanvasView>, VisualTestContext) {
    let mut document = CanvasDocument::default();
    document
        .command(|e| e.apply_annotation_json(annotation))
        .unwrap();
    let window = cx.open_window(size(px(640.0), px(480.0)), |window, cx| {
        CanvasView::new(document, window, cx).unwrap()
    });
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            let id = view
                .session
                .scene()
                .ordered_nodes()
                .map(|node| node.item.as_ref())
                .find_map(|item| {
                    let id = match item {
                        draw::SceneDisplayItem::Rectangle(rect) => rect.id,
                        draw::SceneDisplayItem::Text(text) => text.id,
                        _ => return None,
                    };
                    Some(draw::ElementId {
                        index: id.index,
                        generation: id.generation,
                    })
                })
                .unwrap();
            view.document
                .command(|e| e.select_element_with_viewport_changes(view.session.viewport(), id))
                .unwrap();
            view.session
                .set_camera(
                    &mut view.document,
                    draw::Camera {
                        center: draw::Point::new(320.0, 240.0),
                        zoom: 1.0,
                    },
                )
                .unwrap();
            view.set_options(
                crate::CanvasViewOptions {
                    presentation: crate::CanvasPresentation {
                        content_clip: Some(
                            crate::CanvasRect::new(100.0, 100.0, 50.0, 50.0).unwrap(),
                        ),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                window,
                cx,
            )
            .unwrap();
        });
    });
    (view, visual)
}

fn rect(view: &CanvasView, kind: draw::UiShapeKind) -> draw::UiRectangleDisplayItem {
    view.session
        .scene()
        .overlays()
        .iter()
        .find_map(|item| match item {
            draw::OverlayDisplayItem::Rectangle(item) if item.kind == kind => Some(*item),
            _ => None,
        })
        .unwrap()
}

fn drag(visual: &mut VisualTestContext, start: Point<Pixels>, delta: Point<Pixels>) {
    visual.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: start,
        ..Default::default()
    });
    visual.simulate_event(MouseMoveEvent {
        position: start + delta,
        pressed_button: Some(MouseButton::Left),
        ..Default::default()
    });
    visual.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: start + delta,
        ..Default::default()
    });
}

#[gpui::test]
fn selection_outside_content_clip_supports_edge_resize_and_undo(cx: &mut TestAppContext) {
    let (view, mut visual) = fixture(
        cx,
        br#"{"version":1,"operations":[{"type":"rectangle","bounds":[100,100,160,100]}]}"#,
    );
    let (before, frame, start) = view.read_with(&visual, |view, _| {
        let frame = rect(view, draw::UiShapeKind::SelectionFrame);
        let start = point(
            px((frame.center_x + frame.width / 2.0 + 3.0) as f32),
            px(frame.center_y as f32),
        ) + view.bounds.origin;
        assert!(view.hits_existing_selection(start));
        (
            view.document.engine().document_checkpoint().unwrap(),
            frame,
            start,
        )
    });
    visual.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position: start,
        ..Default::default()
    });
    assert!(!view.read_with(&visual, |view, _| {
        view.document.engine().has_pointer_capture()
    }));
    visual.simulate_event(MouseUpEvent {
        button: MouseButton::Right,
        position: start,
        ..Default::default()
    });
    drag(&mut visual, start, point(px(40.0), px(0.0)));
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert!(rect(view, draw::UiShapeKind::SelectionFrame).width > frame.width + 30.0);
            assert!(!view.document.engine().has_pointer_capture());
            view.command(|e, _| e.undo_with_viewport_changes(), window, cx);
            assert_eq!(
                view.document.engine().document_checkpoint().unwrap(),
                before
            );
        })
    });
}

#[gpui::test]
fn selection_outside_content_clip_supports_text_ring_move(cx: &mut TestAppContext) {
    let (view, mut visual) = fixture(cx, br#"{"version":1,"operations":[{"type":"text","bounds":[100,100,120,40],"text":"Move me"}]}"#);
    let (frame, start) = view.read_with(&visual, |view, _| {
        let frame = rect(view, draw::UiShapeKind::SelectionFrame);
        let text = rect(view, draw::UiShapeKind::TextActualFrame);
        let right = text.center_x + text.width / 2.0;
        let start = view.bounds.origin + point(px((right + 7.0) as f32), px(text.center_y as f32));
        assert!(view.hits_existing_selection(start));
        (frame, start)
    });
    drag(&mut visual, start, point(px(40.0), px(25.0)));
    view.read_with(&visual, |view, _| {
        let moved = rect(view, draw::UiShapeKind::SelectionFrame);
        assert!((moved.center_x - frame.center_x - 40.0).abs() < 0.01);
        assert!((moved.center_y - frame.center_y - 25.0).abs() < 0.01);
        assert!((moved.width - frame.width).abs() < 0.01);
        assert!(!view.document.engine().has_pointer_capture());
    });
}
