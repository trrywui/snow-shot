use super::*;

fn drag_move(cx: &mut VisualTestContext, position: Point<Pixels>) {
    // simulate_mouse_move drains the executor, hiding input that overlaps a render.
    cx.update(|window, cx| {
        window.dispatch_event(
            gpui::PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            }),
            cx,
        );
    });
}

fn assert_drag_paints_current_frames(
    view: &Entity<CanvasView>,
    cx: &mut VisualTestContext,
    origin: Point<Pixels>,
) {
    use std::{cell::RefCell, rc::Rc};
    let published = Rc::new(RefCell::new(Vec::new()));
    let frames = published.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(view, move |view, event, cx| {
            if let CanvasEvent::Rendered = event {
                let view = view.read(cx);
                assert_eq!(view.rendered_generation, view.generation);
                frames
                    .borrow_mut()
                    .push((view.rendered_generation, view.gpu_frame.clone().unwrap()));
            }
        })
    });
    for step in 0..3 {
        let previous = view.read_with(cx, |v, _| v.gpu_frame.clone().unwrap());
        let delta = 15.0 + step as f32 * 30.0;
        let first = origin + point(px(delta), px(delta));
        drag_move(cx, first);
        let generation = view.read_with(cx, |v, _| v.generation);
        drag_move(cx, first + point(px(4.0), px(4.0)));
        drag_move(cx, first + point(px(8.0), px(8.0)));
        cx.update(|_, cx| view.update(cx, |v, _| v.prepare_logical_frame().unwrap()));
        let latest = view.read_with(cx, |v, _| v.generation);
        assert!(latest > generation);
        // Establish native queue headroom, then paint without executing CPU workers.
        retire_gpu(cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let frames = published.borrow();
        let (painted, image) = frames.last().unwrap();
        assert_eq!(*painted, latest, "paint published stale geometry");
        assert_ne!(image.revision, previous.revision);
        view.read_with(cx, |v, _| {
            assert!(v.document.engine().has_pointer_capture());
            assert_eq!(v.rendered_generation, v.generation);
            assert!(!v.is_rendering());
            assert!(v.error.is_none());
            let service = v.gpu_canvas.as_ref().unwrap().service();
            let mut renderer = crate::GpuCanvas::new(service.clone()).unwrap();
            let expected = renderer
                .prepare(
                    v.session.scene(),
                    v.document.background(),
                    v.document.resource_revision(),
                    v.scale,
                    &v.options.presentation,
                    &v.smart_service.snapshot(),
                    &Default::default(),
                )
                .unwrap();
            assert_eq!(
                frame_pixels(v),
                crate::gpu_tests::read_frame(&service, &expected).unwrap(),
                "GPU pixels do not match the live scene"
            );
        });
    }
}

#[gpui::test]
fn live_creation_paints_latest_geometry_without_waiting_for_workers(cx: &mut TestAppContext) {
    let (view, mut visual) = fixture(cx, CanvasDocument::default());
    visual.update(|window, cx| {
        view.update(cx, |v, cx| v.set_tool(draw::ActiveTool::Shape, window, cx));
    });
    paint(&mut visual);
    let origin = point(px(220.0), px(180.0));
    visual.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: origin,
        ..Default::default()
    });
    assert_drag_paints_current_frames(&view, &mut visual, origin);
}

#[gpui::test]
fn live_selection_paints_latest_move_resize_and_rotation_without_waiting_for_workers(
    cx: &mut TestAppContext,
) {
    for handle in [
        None,
        Some(draw::UiShapeKind::SelectionResizeHandle),
        Some(draw::UiShapeKind::SelectionRotationHandle),
    ] {
        let mut document = CanvasDocument::default();
        document.command(|e| e.apply_annotation_json(br#"{"version":1,"operations":[{"type":"rectangle","bounds":[300,200,160,100],"style":{"fill":[40,100,190,255]}}]}"#)).unwrap();
        let (view, mut visual) = fixture(cx, document);
        let origin = visual.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.set_tool(draw::ActiveTool::Select, window, cx);
                let draw::SceneDisplayItem::Rectangle(rect) = v
                    .session
                    .scene()
                    .ordered_nodes()
                    .next()
                    .unwrap()
                    .item
                    .as_ref()
                else {
                    panic!("expected rectangle");
                };
                let id = draw::ElementId {
                    index: rect.id.index,
                    generation: rect.id.generation,
                };
                v.command(
                    |e, s| e.select_element_with_viewport_changes(s, id),
                    window,
                    cx,
                );
                v.prepare_logical_frame().unwrap();
                let world = if let Some(kind) = handle {
                    v.session
                        .scene()
                        .overlays()
                        .iter()
                        .find_map(|item| {
                            if let draw::OverlayDisplayItem::Rectangle(r) = item {
                                (r.kind == kind).then_some(draw::Point::new(r.center_x, r.center_y))
                            } else {
                                None
                            }
                        })
                        .unwrap()
                } else {
                    draw::Point::new(380.0, 250.0)
                };
                let frame = v.session.scene().frame();
                let local = draw::canvas_to_view(world, &frame.camera, frame.surface);
                v.bounds.origin + point(px(local.x as f32), px(local.y as f32))
            })
        });
        paint(&mut visual);
        visual.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: origin,
            ..Default::default()
        });
        assert_drag_paints_current_frames(&view, &mut visual, origin);
    }
}

#[gpui::test]
fn explicit_invalidation_replaces_pending_and_displayed_frames(cx: &mut TestAppContext) {
    use std::{cell::RefCell, rc::Rc};
    for completed in [false, true] {
        for action in ["cancel", "document", "background", "presentation", "cache"] {
            let (view, mut visual) = fixture(cx, CanvasDocument::default());
            visual.update(|window, cx| {
                view.update(cx, |v, cx| v.set_tool(draw::ActiveTool::Shape, window, cx));
            });
            paint(&mut visual);
            visual.simulate_event(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(220.0), px(180.0)),
                ..Default::default()
            });
            drag_move(&mut visual, point(px(300.0), px(260.0)));
            let obsolete = view.read_with(&visual, |v, _| v.generation);
            if completed {
                visual.update(|window, cx| window.draw(cx).clear(cx));
            }
            let published = Rc::new(RefCell::new(Vec::new()));
            let frames = published.clone();
            let _subscription = visual.update(|_, cx| {
                cx.subscribe(&view, move |view, event, cx| {
                    if let CanvasEvent::Rendered = event {
                        frames.borrow_mut().push(view.read(cx).rendered_generation);
                    }
                })
            });
            visual.update(|window, cx| {
                view.update(cx, |v, cx| match action {
                    "cancel" => v.cancel_gesture(window, cx),
                    "document" => v
                        .replace_document(CanvasDocument::default(), window, cx)
                        .unwrap(),
                    "background" => v.set_background(
                        SourceImage::new(image::RgbaImage::from_pixel(
                            64,
                            64,
                            image::Rgba([20, 80, 180, 255]),
                        ))
                        .unwrap(),
                        window,
                        cx,
                    ),
                    "presentation" => {
                        let mut options = v.options.clone();
                        options.presentation.content_visible = false;
                        v.set_options(options, window, cx).unwrap();
                    }
                    "cache" => v.clear_render_state(window, cx),
                    _ => unreachable!(),
                });
            });
            paint(&mut visual);
            view.read_with(&visual, |v, _| {
                assert_eq!(
                    &*published.borrow(),
                    &[v.generation],
                    "{action}, completed={completed}"
                );
                assert_ne!(v.rendered_generation, obsolete);
                assert_eq!(v.rendered_generation, v.generation);
                assert!(!v.is_rendering());
                assert!(v.error.is_none(), "{:?}", v.error);
            });
        }
    }
}

#[gpui::test]
fn discarded_text_draft_cannot_publish_its_pending_preview(cx: &mut TestAppContext) {
    use std::{cell::Cell, rc::Rc};
    let (view, mut visual) = fixture(cx, CanvasDocument::default());
    visual.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.begin_text(draw::Point::new(480.0, 320.0), None, window, cx);
            v.replace_text_in_range(None, "Discard this draft", window, cx);
        });
    });
    paint(&mut visual);
    visual.update(|window, cx| {
        view.update(cx, |v, cx| v.replace_text_in_range(None, "!", window, cx));
    });
    let obsolete = view.read_with(&visual, |v, _| v.generation);
    let published = Rc::new(Cell::new(0));
    let count = published.clone();
    let _subscription = visual.update(|_, cx| {
        cx.subscribe(&view, move |view, event, cx| {
            if let CanvasEvent::Rendered = event {
                assert_ne!(view.read(cx).rendered_generation, obsolete);
                count.set(count.get() + 1);
            }
        })
    });
    visual.update(|window, cx| view.update(cx, |v, cx| v.finish_text(false, window, cx)));
    paint(&mut visual);
    assert_eq!(published.get(), 1);
    view.read_with(&visual, |v, _| {
        assert!(v.draft.is_none() && !v.document.engine().has_document_content());
        assert_eq!(v.rendered_generation, v.generation);
        assert!(v.error.is_none());
    });
}
