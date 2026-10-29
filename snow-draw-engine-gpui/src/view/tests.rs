#![cfg(target_os = "windows")]
use super::*;
use gpui::{TestAppContext, VisualTestContext, size};

#[path = "render_tests.rs"]
mod render_tests;

#[path = "text_entry_tests.rs"]
mod text_entry_tests;

#[path = "text_caret_tests.rs"]
mod text_caret_tests;

#[path = "frame_tests.rs"]
mod frame_tests;

#[path = "completion_tests.rs"]
mod completion_tests;

#[gpui::test]
fn send_cache_payload_drop_is_deferred_from_the_host_callback(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Probe(Arc<AtomicBool>);
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    cx.update(|cx| {
        crate::cleanup::retire_send_resources(Probe(dropped.clone()), cx);
        assert!(!dropped.load(Ordering::SeqCst));
    });
    cx.run_until_parked();
    assert!(dropped.load(Ordering::SeqCst));
}

#[gpui::test]
fn document_retirement_is_bounded_and_runs_on_the_owner_thread(cx: &mut TestAppContext) {
    cx.update(|cx| {
        for _ in 0..5 {
            crate::retire_canvas_document(CanvasDocument::default(), cx);
        }
        let stats = crate::canvas_cleanup_diagnostics(cx);
        assert_eq!(stats.pending, 2);
        assert_eq!(stats.completed, 3);
        assert_eq!(stats.overflow_drops, 3);
    });
    cx.run_until_parked();
    for _ in 0..3 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(20));
        cx.run_until_parked();
    }
    cx.update(|cx| {
        let stats = crate::canvas_cleanup_diagnostics(cx);
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.completed, 5);
    });
}

#[gpui::test]
fn host_key_resolution_preserves_text_and_releases_remapped_pan(cx: &mut TestAppContext) {
    use std::{cell::Cell, rc::Rc};
    let (view, mut visual) = fixture(cx, CanvasDocument::default());
    let calls = Rc::new(Cell::new(0));
    let count = calls.clone();
    visual.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.set_command_key_resolver(Some(Rc::new(move |key| {
                count.set(count.get() + 1);
                match key.key.as_str() {
                    "q" => Some("r".into()),
                    "f1" => Some("space".into()),
                    _ => None,
                }
            })));
            v.key_down(
                &KeyDownEvent {
                    keystroke: gpui::Keystroke::parse("q").unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
            assert_eq!(
                v.document
                    .engine()
                    .viewport_active_tool(v.session.viewport())
                    .unwrap(),
                draw::ActiveTool::Shape
            );
            v.key_down(
                &KeyDownEvent {
                    keystroke: gpui::Keystroke::parse("f1").unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
            assert!(v.space_down);
            v.set_command_key_resolver(None);
            v.key_up(
                &KeyUpEvent {
                    keystroke: gpui::Keystroke::parse("f1").unwrap(),
                },
                window,
                cx,
            );
            assert!(!v.space_down);
            let count = calls.clone();
            v.set_command_key_resolver(Some(Rc::new(move |_| {
                count.set(count.get() + 1);
                Some("delete".into())
            })));
            v.begin_text(draw::Point::new(0.0, 0.0), None, window, cx);
            let before = calls.get();
            v.key_down(
                &KeyDownEvent {
                    keystroke: gpui::Keystroke::parse("q").unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
            assert_eq!(calls.get(), before);
            v.replace_text_in_range(None, "q你好😀", window, cx);
            assert_eq!(v.draft.as_ref().unwrap().buffer.text, "q你好😀");
            v.key_down(
                &KeyDownEvent {
                    keystroke: gpui::Keystroke {
                        key: "q".into(),
                        key_char: Some("@".into()),
                        modifiers: Modifiers {
                            control: true,
                            alt: true,
                            ..Default::default()
                        },
                    },
                    is_held: false,
                    prefer_character_input: true,
                },
                window,
                cx,
            );
            assert_eq!(calls.get(), before, "AltGr consulted command resolver");
            assert_eq!(v.draft.as_ref().unwrap().buffer.text, "q你好😀");
            v.replace_text_in_range(None, "@", window, cx);
            assert_eq!(v.draft.as_ref().unwrap().buffer.text, "q你好😀@");
            v.set_command_key_resolver(Some(Rc::new(|key| {
                matches!(key.key.as_str(), "f1" | "pageup").then(|| "escape".into())
            })));
            for key in ["f1", "pageup"] {
                if v.draft.is_none() {
                    v.begin_text(draw::Point::new(0.0, 0.0), None, window, cx);
                }
                v.key_down(
                    &KeyDownEvent {
                        keystroke: gpui::Keystroke::parse(key).unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
                assert!(v.draft.is_none(), "named key {key} bypassed resolver");
            }
        })
    });
}

#[gpui::test]
fn cursor_layers_crosshair_dpi_cache_and_debug_flags_are_view_state(cx: &mut TestAppContext) {
    use std::{cell::Cell, rc::Rc};
    let (view, mut visual) = fixture(cx, crate::showcase_document().unwrap());
    let calls = Rc::new(Cell::new(0));
    let count = calls.clone();
    let before = view.read_with(&visual, |v, _| {
        v.document.engine().document_checkpoint().unwrap()
    });
    visual.update(|_, cx| {
        view.update(cx, |v, cx| {
            v.set_cursor_for_layer(CanvasCursorLayer::CanvasTool, CursorStyle::OpenHand, cx);
            v.set_cursor_for_layer(
                CanvasCursorLayer::Host,
                CursorStyle::OperationNotAllowed,
                cx,
            );
            assert_eq!(v.effective_cursor(), CursorStyle::OperationNotAllowed);
            v.clear_cursor_for_layer(CanvasCursorLayer::Host, cx);
            assert_eq!(v.effective_cursor(), CursorStyle::OpenHand);
            v.clear_cursor_for_layer(CanvasCursorLayer::CanvasTool, cx);
            v.set_crosshair_factory(
                Some(Rc::new(move |scale| {
                    count.set(count.get() + 1);
                    Some(CanvasCrosshair {
                        radius: 18.0 / scale,
                        ..Default::default()
                    })
                })),
                cx,
            );
            v.set_show_dirty_regions(true, cx);
        })
    });
    paint(&mut visual);
    paint(&mut visual);
    assert_eq!(calls.get(), 1);
    visual.simulate_scale_factor_change(1.5);
    paint(&mut visual);
    assert_eq!(calls.get(), 2);
    assert_eq!(
        view.read_with(&visual, |v, _| v
            .document
            .engine()
            .document_checkpoint()
            .unwrap()),
        before
    );
    assert!(view.read_with(&visual, |v, _| v.show_dirty_regions()));
}

#[gpui::test]
fn custom_crosshair_paints_current_hover_geometry_and_tracks_moves(cx: &mut TestAppContext) {
    let (view, mut visual) = fixture(cx, CanvasDocument::default());
    let color: Hsla = rgb(0xff00e1).into();
    visual.update(|_, cx| {
        view.update(cx, |v, cx| {
            v.set_crosshair_factory(
                Some(std::rc::Rc::new(move |_| {
                    Some(CanvasCrosshair {
                        color,
                        ..Default::default()
                    })
                })),
                cx,
            );
        });
    });
    for position in [point(px(180.0), px(170.0)), point(px(290.0), px(260.0))] {
        visual.simulate_mouse_move(position, None, Modifiers::default());
        paint(&mut visual);
        visual.update(|window, _| {
            let quads: Vec<_> = window
                .painted_quads()
                .into_iter()
                .filter(|quad| quad.background == gpui::Background::from(color))
                .collect();
            assert_eq!(quads.len(), 4, "current frame crosshair was not painted");
            let scale = window.scale_factor();
            for quad in quads {
                let x = quad.bounds.origin.x.0;
                let y = quad.bounds.origin.y.0;
                assert!((x - f32::from(position.x) * scale).abs() <= 19.0 * scale);
                assert!((y - f32::from(position.y) * scale).abs() <= 19.0 * scale);
            }
        });
    }
}

#[gpui::test]
fn view_cache_reset_is_deferred_until_frame_preparation(cx: &mut TestAppContext) {
    let (view, mut visual) = fixture(cx, crate::showcase_document().unwrap());
    let builds = view.read_with(&visual, |v, _| v.gpu_frame.as_ref().unwrap().revision);
    let before = view.read_with(&visual, |v, _| {
        v.document.engine().document_checkpoint().unwrap()
    });
    visual.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.clear_render_state(window, cx);
            assert!(v.gpu_frame.is_some());
            assert_eq!(v.gpu_frame.as_ref().unwrap().revision, builds);
        })
    });
    paint(&mut visual);
    assert!(
        view.read_with(&visual, |v, _| v.gpu_frame.as_ref().unwrap().revision
            > builds)
    );
    assert_eq!(
        view.read_with(&visual, |v, _| v
            .document
            .engine()
            .document_checkpoint()
            .unwrap()),
        before
    );
    assert!(
        view.read_with(&visual, |v, _| v.generation == v.rendered_generation
            && !v.clear_renderer_pending)
    );
}

fn fixture(
    cx: &mut TestAppContext,
    document: CanvasDocument,
) -> (Entity<CanvasView>, VisualTestContext) {
    // These integration views use a real D3D device. Permit its Windows
    // completion callbacks while driving logical work through the test scheduler.
    cx.executor().allow_parking();
    struct TestGpuHost {
        _host: gpui_windows::gpu_extensions::OffscreenGpuHost,
    }
    impl gpui::Global for TestGpuHost {}
    if !cx.update(|cx| cx.has_global::<TestGpuHost>()) {
        let host = gpui_windows::gpu_extensions::OffscreenGpuHost::new().unwrap();
        cx.set_gpu_service(host.service());
        cx.update(|cx| cx.set_global(TestGpuHost { _host: host }));
    }
    let window = cx.open_window(size(px(640.0), px(480.0)), |window, cx| {
        CanvasView::new(document, window, cx).unwrap()
    });
    let view = window.root(cx).unwrap();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, _| window.activate_window());
    paint(&mut visual);
    (view, visual)
}
fn paint(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    retire_gpu(cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    // Draining logical tasks does not retire native event-driven GPU work. Give
    // the requested publication a ready queue without relying on timer polling.
    retire_gpu(cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
}
fn retire_gpu(cx: &mut VisualTestContext) {
    let service = cx.update(|window, _| window.gpu_service().unwrap());
    let mut completion = service.completion().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !completion.poll().unwrap() {
        assert!(
            std::time::Instant::now() < deadline,
            "native GPU retirement timed out"
        );
        std::thread::yield_now();
    }
    // No GPUI background/foreground tasks run here. Tests can isolate immediate
    // logical publication from the separately tested native queue backpressure.
}

#[gpui::test]
fn repeated_dense_edits_coalesce_and_finish_with_the_latest_generation(cx: &mut TestAppContext) {
    let mut document = CanvasDocument::default();
    for base in (0..1000).step_by(128) {
        let ops:Vec<_>=(base..(base+128).min(1000)).map(|i|serde_json::json!({"type":"rectangle","bounds":[(i%40)*12,(i/40)*12,8,8],"style":{"fill":[40,100,190,255]}})).collect();
        document
            .command(|e| {
                e.apply_annotation_json(
                    &serde_json::to_vec(&serde_json::json!({"version":1,"operations":ops}))
                        .unwrap(),
                )
            })
            .unwrap();
    }
    let (view, mut visual) = fixture(cx, document);
    let (original, render_count) = view.read_with(&visual, |v, _| {
        (
            v.gpu_frame.clone().unwrap(),
            v.gpu_frame.as_ref().unwrap().revision,
        )
    });
    visual.update(|window, cx| {
        view.update(cx, |v, cx| {
            let id = v
                .session
                .scene()
                .ordered_nodes()
                .map(|node| node.item.as_ref())
                .find_map(|item| {
                    if let draw::SceneDisplayItem::Rectangle(r) = item {
                        Some(draw::ElementId {
                            index: r.id.index,
                            generation: r.id.generation,
                        })
                    } else {
                        None
                    }
                })
                .unwrap();
            v.session
                .mutate(&mut v.document, |e, s| {
                    e.select_element_with_viewport_changes(s, id)
                })
                .unwrap();
            for i in 0..25 {
                v.session
                    .mutate(&mut v.document, |e, s| {
                        e.set_selected_opacity_with_viewport_changes(
                            s,
                            if i % 2 == 0 { 0.5 } else { 1.0 },
                        )
                    })
                    .unwrap();
                v.changed(window, cx);
            }
            assert!(!v.logical_frame_ready);
            assert_eq!(v.gpu_frame.as_ref().unwrap().revision, render_count);
            assert!(
                Arc::ptr_eq(&v.gpu_frame.as_ref().unwrap().tiles, &original.tiles),
                "pending edits removed the last completed image"
            );
            assert!(!v.logical_frame_ready);
        })
    });
    paint(&mut visual);
    view.read_with(&visual, |v, _| {
        assert_eq!(v.rendered_generation, v.generation);
        assert!(!v.is_rendering());
        assert!(v.error.is_none());
        assert_eq!(
            v.gpu_frame.as_ref().unwrap().revision,
            render_count + 1,
            "queued every intermediate edit instead of coalescing"
        );
    });
}

#[gpui::test]
fn native_capture_cancels_outside_release_and_discards_inflight_preview(cx: &mut TestAppContext) {
    let (view, mut visual) = fixture(cx, CanvasDocument::default());
    visual.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: point(px(200.0), px(180.0)),
        ..Default::default()
    });
    visual.simulate_mouse_move(
        point(px(390.0), px(310.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    assert!(view.read_with(&visual, |view, _| {
        view.document.engine().has_pointer_capture()
    }));
    let pending = view.read_with(&visual, |view, _| view.generation);
    visual.update(|window, cx| view.update(cx, |view, cx| view.cancel_gesture(window, cx)));
    assert!(view.read_with(&visual, |view, _| view.generation > pending));
    paint(&mut visual);
    assert!(view.read_with(
        &visual,
        |view, _| !view.document.engine().has_document_content()
            && !view.document.engine().has_pointer_capture()
            && view.generation == view.rendered_generation
    ));
    visual.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: point(px(220.0), px(200.0)),
        ..Default::default()
    });
    visual.simulate_mouse_move(
        point(px(-30.0), px(-20.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: point(px(-30.0), px(-20.0)),
        ..Default::default()
    });
    paint(&mut visual);
    assert!(view.read_with(&visual, |view, _| {
        !view.document.engine().has_pointer_capture()
    }));
}

#[gpui::test]
fn editing_without_changes_preserves_narrow_text_and_ime_commits_once(cx: &mut TestAppContext) {
    let mut document = CanvasDocument::default();
    document.command(|engine|engine.apply_annotation_json(r#"{"version":1,"operations":[{"type":"text","bounds":[100,100,60,130],"text":"A😀Z narrow wrapped text","style":{"font_size":20}}]}"#.as_bytes())).unwrap();
    let before = document.engine().document_checkpoint().unwrap();
    let (view, mut visual) = fixture(cx, document);
    let id = view.read_with(&visual, |view, _| {
        view.session
            .scene()
            .ordered_nodes()
            .map(|node| node.item.as_ref())
            .find_map(|item| match item {
                draw::SceneDisplayItem::Text(text) => Some(draw::ElementId {
                    index: text.id.index,
                    generation: text.id.generation,
                }),
                _ => None,
            })
            .unwrap()
    });
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.begin_text(draw::Point::new(130.0, 165.0), Some(id), window, cx);
            assert_eq!(view.draft.as_ref().unwrap().width, 60.0);
            view.finish_text(true, window, cx);
            assert_eq!(
                view.document.engine().document_checkpoint().unwrap(),
                before
            );
            view.begin_text(draw::Point::new(130.0, 165.0), Some(id), window, cx);
            view.replace_and_mark_text_in_range(Some(1..3), "ni", Some(2..2), window, cx);
            view.replace_and_mark_text_in_range(None, "你", Some(1..1), window, cx);
            view.replace_text_in_range(None, "你好", window, cx);
            assert!(
                view.draft
                    .as_ref()
                    .unwrap()
                    .buffer
                    .text
                    .starts_with("A你好Z")
            );
            let length = view
                .draft
                .as_ref()
                .unwrap()
                .buffer
                .text
                .encode_utf16()
                .count();
            let bounds = view
                .bounds_for_range(0..length, view.bounds, window, cx)
                .unwrap();
            assert!(bounds.size.height > px(20.0));
            view.finish_text(true, window, cx);
            view.document.command(|engine| engine.undo()).unwrap();
            assert_eq!(
                view.document.engine().document_checkpoint().unwrap(),
                before
            );
        })
    });
}

#[gpui::test]
fn dpi_and_zoom_render_at_device_resolution_without_idle_rendering(cx: &mut TestAppContext) {
    let (view, mut visual) = fixture(cx, CanvasDocument::default());
    for scale in [1.25, 1.5, 2.0, 1.0] {
        let reference = view.read_with(&visual, |view, _| view.session.scene().frame());
        let (generation, count) = view.read_with(&visual, |view, _| {
            (view.generation, view.gpu_frame.as_ref().unwrap().revision)
        });
        visual.simulate_scale_factor_change(scale);
        paint(&mut visual);
        view.read_with(&visual, |view, _| {
            assert_eq!(view.session.scene().frame(), reference);
            assert!(
                view.generation > generation,
                "DPI change reused the old raster"
            );
            assert_eq!(view.rendered_generation, view.generation);
            assert_eq!(view.gpu_frame.as_ref().unwrap().revision, count + 1);
        });
        assert_device_resolution(&view, &mut visual);
        visual.update(|window, cx| view.update(cx, |view, cx| view.zoom(1.5, window, cx)));
        paint(&mut visual);
        assert_device_resolution(&view, &mut visual);
    }
}

fn frame_pixels(view: &CanvasView) -> image::RgbaImage {
    crate::gpu_tests::read_frame(
        &view.gpu_canvas.as_ref().unwrap().service(),
        view.gpu_frame.as_ref().unwrap(),
    )
    .unwrap()
}

fn assert_device_resolution(view: &Entity<CanvasView>, visual: &mut VisualTestContext) {
    let generation = visual.update(|window, cx| {
        view.read_with(cx, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            let live = view.session.scene().frame();
            assert_eq!(view.image_frame.unwrap(), live);
            let size = view.gpu_frame.as_ref().unwrap().size;
            assert_eq!(
                size,
                [
                    (live.surface.width as f32 * window.scale_factor()).ceil() as u32,
                    (live.surface.height as f32 * window.scale_factor()).ceil() as u32
                ]
            );
            view.generation
        })
    });
    let image = view.read_with(visual, |view, _| view.gpu_frame.clone().unwrap());
    paint(visual);
    paint(visual);
    view.read_with(visual, |view, _| {
        assert_eq!(view.generation, generation);
        assert!(
            Arc::ptr_eq(&view.gpu_frame.as_ref().unwrap().tiles, &image.tiles),
            "idle paint replaced GPU tiles"
        );
    });
}

#[gpui::test]
fn device_raster_keeps_single_pixel_edges_with_source_scaling(cx: &mut TestAppContext) {
    let mut document = CanvasDocument::default();
    document.set_background(Some(
        SourceImage::new(image::RgbaImage::from_pixel(
            320,
            240,
            image::Rgba([255; 4]),
        ))
        .unwrap()
        .with_placement(draw::Point::new(0.0, 0.0), 0.5)
        .unwrap(),
    ));
    document.command(|e| e.apply_annotation_json(br#"{"version":1,"operations":[{"type":"rectangle","bounds":[20,20,0.5,30],"style":{"fill":[0,0,0,255],"stroke_width":0}}]}"#)).unwrap();
    let (view, mut visual) = fixture(cx, document);
    visual.simulate_scale_factor_change(2.0);
    {
        visual.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session
                    .set_camera(
                        &mut view.document,
                        draw::Camera {
                            center: draw::Point::new(320.0, 240.0),
                            zoom: 1.0,
                        },
                    )
                    .unwrap();
                view.set_options(Default::default(), window, cx).unwrap();
            })
        });
        paint(&mut visual);
        assert_device_resolution(&view, &mut visual);
        view.read_with(&visual, |view, _| {
            let frame = view.image_frame.unwrap();
            let position =
                draw::canvas_to_view(draw::Point::new(20.0, 30.0), &frame.camera, frame.surface);
            let x = (position.x * f64::from(view.scale)).round() as usize;
            let y = (position.y * f64::from(view.scale)).round() as usize;
            let pixels = frame_pixels(view);
            let offset = (y * pixels.width() as usize + x) * 4;
            let pixels = pixels.as_raw();
            assert_eq!(&pixels[offset - 4..offset], &[255; 4]);
            assert_eq!(&pixels[offset..offset + 4], &[0, 0, 0, 255]);
            assert_eq!(&pixels[offset + 4..offset + 8], &[255; 4]);
        });
    }
}

#[gpui::test]
fn device_raster_matches_snapshot_with_filter_support_at_fractional_dpi(cx: &mut TestAppContext) {
    let mut document = CanvasDocument::default();
    document.set_background(Some(
        SourceImage::new(image::RgbaImage::from_fn(700, 520, |x, y| {
            image::Rgba([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8, 255])
        }))
        .unwrap(),
    ));
    document.command(|e| e.apply_annotation_json(br#"{"version":1,"operations":[{"type":"rectangle","bounds":[-12,5,22,120],"style":{"fill":[0,0,0,255]}},{"type":"rectangle_filter","bounds":[-20,0,100,240],"style":{"filter":"gaussian_blur","strength":0.2}},{"type":"rectangle","bounds":[1,1,2,2],"style":{"opacity":0}},{"type":"rectangle_filter","bounds":[-20,0,100,240],"style":{"filter":"mosaic","strength":0.3}},{"type":"text","bounds":[150,100,300,60],"text":"Device pixels","style":{"font_size":24}}]}"#)).unwrap();
    let (view, mut visual) = fixture(cx, document);
    visual.simulate_resize(size(px(641.0), px(479.0)));
    for scale in [1.25, 1.5, 2.0] {
        visual.simulate_scale_factor_change(scale);
        visual.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session
                    .set_camera(
                        &mut view.document,
                        draw::Camera {
                            center: draw::Point::new(320.5, 239.5),
                            zoom: 1.0,
                        },
                    )
                    .unwrap();
                view.changed(window, cx);
            })
        });
        paint(&mut visual);
        assert_device_resolution(&view, &mut visual);
        view.read_with(&visual, |view, _| {
            let mut renderer =
                crate::GpuCanvas::new(view.gpu_canvas.as_ref().unwrap().service()).unwrap();
            let expected = renderer
                .prepare(
                    view.session.scene(),
                    view.document.background(),
                    view.document.resource_revision(),
                    scale,
                    &view.options.presentation,
                    &view.smart_service.snapshot(),
                    &Default::default(),
                )
                .unwrap();
            let expected = crate::gpu_tests::read_frame(&renderer.service(), &expected).unwrap();
            assert_eq!(
                frame_pixels(view),
                expected,
                "fractional device scale {scale}"
            );
        });
    }
}

#[gpui::test]
fn wrapped_line_pointer_caret_keeps_visual_affinity(cx: &mut TestAppContext) {
    let (mut document, mut session) = {
        let mut d = CanvasDocument::default();
        let s = CanvasSession::new(&mut d).unwrap();
        (d, s)
    };
    document.command(|e|e.apply_annotation_json(br#"{"version":1,"operations":[{"type":"text","bounds":[100,100,60,160],"text":"one two three four five","style":{"font_size":20}}]}"#)).unwrap();
    session.set_surface(&mut document, 1000, 1000).unwrap();
    let id = session
        .scene()
        .ordered_nodes()
        .map(|node| node.item.as_ref())
        .find_map(|item| {
            if let draw::SceneDisplayItem::Text(t) = item {
                Some(draw::ElementId {
                    index: t.id.index,
                    generation: t.id.generation,
                })
            } else {
                None
            }
        })
        .unwrap();
    let (view, mut visual) = fixture(cx, document);
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.begin_text(draw::Point::new(130.0, 180.0), Some(id), window, cx);
            let draft = view.draft.as_ref().unwrap();
            assert!(draft.layout.lines.len() > 1);
            let byte = draft.layout.lines[1].start;
            let x = -draft.width / 2.0;
            let y = -draft.layout.height / 2.0 + draft.layout.line_height * 1.2;
            let pointer = view.text_world_position(x, y).unwrap();
            let index = view.text_index_at(pointer).unwrap();
            assert_eq!(index, byte);
            view.draft.as_mut().unwrap().buffer.select(index, false);
            let (_, paint_y) = view.text_local_position(index).unwrap();
            let draft = view.draft.as_ref().unwrap();
            assert!(
                (paint_y - (-draft.layout.height / 2.0 + draft.layout.line_height)).abs() < 0.01
            );
            view.move_text_vertical(false, false);
            assert_eq!(view.draft.as_ref().unwrap().caret_affinity.unwrap().1, 0);
            view.move_text_vertical(true, true);
            assert_eq!(view.draft.as_ref().unwrap().caret_affinity.unwrap().1, 1);
        })
    });
}

#[gpui::test]
fn preparation_failure_invalidates_pending_render_and_export(cx: &mut TestAppContext) {
    let (view, mut visual) = fixture(cx, CanvasDocument::default());
    visual.update(|window, cx| view.update(cx, |view, cx| {
        view.document.command(|e| e.apply_annotation_json(br#"{"version":1,"operations":[{"type":"rectangle","bounds":[10,10,40,30]}]}"#)).unwrap();
        view.changed(window, cx);
        let scheduled = view.generation;
        let mut other = CanvasDocument::default();
        view.session = CanvasSession::new(&mut other).unwrap();
        view.invalidate_render();
        view.changed(window, cx);
        view.prepare_display_frame(window,cx);
        assert!(view.generation > scheduled);
        assert!(view.snapshot().is_err());
        assert!(view.error.is_some());
    }));
    paint(&mut visual);
    assert!(view.read_with(&visual, |view, _| view.error.is_some()
        && !view.is_rendering()
        && !view.logical_frame_ready));
}

#[gpui::test]
fn png_clipboard_uses_strict_snapshot_and_text_popup_keeps_draft(cx: &mut TestAppContext) {
    let (view, mut visual) = fixture(cx, CanvasDocument::default());
    for scale in [1.0, 1.5, 2.0] {
        visual.simulate_scale_factor_change(scale);
        paint(&mut visual);
        let expected_width = visual.update(|window, cx| {
            view.update(cx, |view, cx| view.copy_image(window, cx));
            (640.0 * window.scale_factor()).ceil() as u32
        });
        paint(&mut visual);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while view.read_with(&visual, |view, _| view.clipboard_task.is_some()) {
            assert!(
                std::time::Instant::now() < deadline,
                "GPU clipboard readback timed out"
            );
            visual
                .executor()
                .advance_clock(std::time::Duration::from_millis(1));
            visual.run_until_parked();
            std::thread::yield_now();
        }
        view.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error)
        });
        visual.update(|_, cx| {
            let clipboard = cx.read_from_clipboard().unwrap();
            let gpui::ClipboardEntry::Image(image) = &clipboard.entries()[0] else {
                panic!("expected PNG");
            };
            assert_eq!(
                image::load_from_memory(image.bytes()).unwrap().width(),
                expected_width
            );
        });
    }
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            let options = crate::CanvasViewOptions {
                text_editing_bounds: Some(crate::CanvasRect::new(10.0, 0.0, 100.0, 400.0).unwrap()),
                ..Default::default()
            };
            view.set_options(options, window, cx).unwrap();
            view.begin_text(draw::Point::new(100.0, 100.0), None, window, cx);
            view.replace_text_in_range(
                None,
                "wrapped text across a bounded host image",
                window,
                cx,
            );
            assert!(view.draft.as_ref().unwrap().layout.lines.len() > 1);
            view.begin_text_style_popup_interaction();
            let mut style = view.draft.as_ref().unwrap().style.clone();
            style.bold = true;
            view.set_active_text_style(style, window, cx).unwrap();
            view.end_text_style_popup_interaction(window, cx);
            assert!(view.draft.as_ref().unwrap().style.bold);
            view.finish_text(false, window, cx);
            assert!(!view.document.engine().has_document_content());
        })
    });
}

#[gpui::test]
fn style_only_edits_commit_once_and_delete_label_preserves_owner(cx: &mut TestAppContext) {
    let mut document = CanvasDocument::default();
    document.command(|e| e.apply_annotation_json(br#"{"version":1,"operations":[{"type":"text","bounds":[100,100,120,40],"text":"Style"},{"type":"arrow","points":[[100,220],[300,220]]}]}"#)).unwrap();
    let (view, mut visual) = fixture(cx, document);
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            let text = view
                .session
                .scene()
                .ordered_nodes()
                .map(|node| node.item.as_ref())
                .find_map(|item| {
                    if let draw::SceneDisplayItem::Text(t) = item {
                        Some(draw::ElementId {
                            index: t.id.index,
                            generation: t.id.generation,
                        })
                    } else {
                        None
                    }
                })
                .unwrap();
            let before = view.document.engine().document_checkpoint().unwrap();
            view.begin_text(draw::Point::new(160.0, 120.0), Some(text), window, cx);
            let mut style = view.draft.as_ref().unwrap().style.clone();
            style.bold = true;
            view.set_active_text_style(style, window, cx).unwrap();
            view.finish_text(true, window, cx);
            assert!(view.document.engine().text_element_info(text).unwrap().bold);
            view.document.command(|e| e.undo()).unwrap();
            assert_eq!(
                view.document.engine().document_checkpoint().unwrap(),
                before
            );
            let arrow = view
                .session
                .scene()
                .ordered_nodes()
                .map(|node| node.item.as_ref())
                .find_map(|item| {
                    if let draw::SceneDisplayItem::Arrow(t) = item {
                        Some(draw::ElementId {
                            index: t.id.index,
                            generation: t.id.generation,
                        })
                    } else {
                        None
                    }
                })
                .unwrap();
            view.document
                .command(|e| e.select_element_with_viewport_changes(view.session.viewport(), arrow))
                .unwrap();
            view.edit_selected_label(window, cx);
            assert!(matches!(
                view.draft.as_ref().unwrap().target,
                draw::TextCommitTarget::NewArrow(_)
            ));
            view.options.delete_text_after_cursor = false;
            view.key_down(
                &KeyDownEvent {
                    keystroke: gpui::Keystroke::parse("delete").unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
            assert_eq!(
                view.document.engine().document_checkpoint().unwrap(),
                before
            );
        })
    });
}

#[gpui::test]
fn content_clip_rejects_new_creation_but_keeps_outside_selection_handles(cx: &mut TestAppContext) {
    let mut document = CanvasDocument::default();
    document
        .command(|e| {
            e.apply_annotation_json(
                br#"{"version":1,"operations":[{"type":"rectangle","bounds":[100,100,160,100]}]}"#,
            )
        })
        .unwrap();
    let (view, mut visual) = fixture(cx, document);
    let outside = visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            let id = view
                .session
                .scene()
                .ordered_nodes()
                .map(|node| node.item.as_ref())
                .find_map(|item| {
                    if let draw::SceneDisplayItem::Rectangle(r) = item {
                        Some(draw::ElementId {
                            index: r.id.index,
                            generation: r.id.generation,
                        })
                    } else {
                        None
                    }
                })
                .unwrap();
            view.document
                .command(|e| e.select_element_with_viewport_changes(view.session.viewport(), id))
                .unwrap();
            view.session.refresh(&view.document).unwrap();
            let options = crate::CanvasViewOptions {
                presentation: crate::CanvasPresentation {
                    content_clip: Some(crate::CanvasRect::new(100.0, 100.0, 50.0, 50.0).unwrap()),
                    ..Default::default()
                },
                ..Default::default()
            };
            view.set_options(options, window, cx).unwrap();
            view.prepare_logical_frame().unwrap();
            let handle = view
                .session
                .scene()
                .overlays()
                .iter()
                .find_map(|item| {
                    if let draw::OverlayDisplayItem::Rectangle(r) = item {
                        (r.kind == draw::UiShapeKind::SelectionResizeHandle && r.center_x > 150.0)
                            .then_some(*r)
                    } else {
                        None
                    }
                })
                .unwrap();
            let frame = view.session.scene().frame();
            let p = draw::canvas_to_view(
                draw::Point::new(handle.center_x, handle.center_y),
                &frame.camera,
                frame.surface,
            );
            let handle = view.bounds.origin + point(px(p.x as f32), px(p.y as f32));
            assert!(view.hits_existing_selection(handle));
            view.set_tool(draw::ActiveTool::Shape, window, cx);
            let p =
                draw::canvas_to_view(draw::Point::new(350.0, 250.0), &frame.camera, frame.surface);
            view.bounds.origin + point(px(p.x as f32), px(p.y as f32))
        })
    });
    let before = view.read_with(&visual, |v, _| {
        v.document.engine().document_checkpoint().unwrap()
    });
    visual.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: outside,
        ..Default::default()
    });
    visual.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: outside,
        ..Default::default()
    });
    assert_eq!(
        view.read_with(&visual, |v, _| v
            .document
            .engine()
            .document_checkpoint()
            .unwrap()),
        before
    );
}
