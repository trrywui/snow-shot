use super::*;
use crate::CanvasRect;

#[gpui::test]
fn published_translucent_canvas_has_no_duplicate_clear_quad(cx: &mut TestAppContext) {
    let mut document = CanvasDocument::default();
    let mut config = draw::ViewportConfig::default();
    config.engine.clear_color = draw::ColorRgba8 {
        r: 80,
        g: 120,
        b: 160,
        a: 128,
    };
    document
        .command(|engine| engine.create_viewport(config))
        .unwrap();
    let (view, mut visual) = fixture(cx, document);
    visual.update(|window, _| {
        let scene = window.rendered_scene();
        assert!(
            !scene.gpu_textures.is_empty(),
            "document GPU frame was not published"
        );
        assert!(
            scene.quads.is_empty(),
            "a clear quad would composite translucent background twice"
        );
    });
    view.read_with(&visual, |view, _| {
        assert_eq!(view.image_frame.unwrap().clear_color.a, 128);
        assert_eq!(frame_pixels(view).get_pixel(320, 240).0, [40, 60, 80, 128]);
    });
}

fn pointer(cx: &mut VisualTestContext, position: Point<Pixels>, down: bool) {
    cx.update(|window, cx| {
        let event = if down {
            gpui::PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position,
                ..Default::default()
            })
        } else {
            gpui::PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            })
        };
        window.dispatch_event(event, cx);
    });
}

#[gpui::test]
fn creation_pixels_and_selection_are_current_in_the_same_frame(cx: &mut TestAppContext) {
    let (view, mut visual) = fixture(cx, CanvasDocument::default());
    visual.update(|window, cx| {
        view.update(cx, |v, cx| v.set_tool(draw::ActiveTool::Shape, window, cx));
    });
    paint(&mut visual);
    pointer(&mut visual, point(px(200.0), px(180.0)), true);
    for x in [240.0, 280.0, 320.0] {
        pointer(&mut visual, point(px(x), px(260.0)), false);
        // Paint without draining workers: a displayed frame must already match
        // the scene that supplies its selection controls.
        retire_gpu(&mut visual);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        view.read_with(&visual, |v, _| {
            assert!(!v.session.scene().order_ids().is_empty());
            assert_eq!(v.rendered_generation, v.generation);
            assert!(v.error.is_none());
        });
    }
}

#[gpui::test]
fn failed_frame_retains_controls_from_the_completed_scene(cx: &mut TestAppContext) {
    let (view, mut visual) = fixture(cx, CanvasDocument::default());
    visual.update(|window, cx| {
        view.update(cx, |v, cx| v.set_tool(draw::ActiveTool::Shape, window, cx))
    });
    pointer(&mut visual, point(px(200.0), px(180.0)), true);
    pointer(&mut visual, point(px(240.0), px(230.0)), false);
    paint(&mut visual);
    visual.update(|window, cx| {
        view.update(cx, |v, cx| {
            let painted = v.display_scene().clone();
            let frame = v.session.scene().frame();
            v.session
                .set_camera(
                    &mut v.document,
                    draw::Camera {
                        center: draw::Point::new(90.0, 90.0),
                        ..frame.camera
                    },
                )
                .unwrap();
            // Force frame preparation to fail after the live viewport advances.
            let mut other = CanvasDocument::default();
            v.session = CanvasSession::new(&mut other).unwrap();
            v.changed(window, cx);
            v.prepare_display_frame(window, cx);
            assert!(!v.logical_frame_ready);
            assert_eq!(v.display_scene().frame(), painted.frame());
            assert_eq!(v.display_scene().overlays(), painted.overlays());
            assert_ne!(v.session.scene().frame(), painted.frame());
        })
    });
}

#[gpui::test]
fn failed_text_frame_retains_selected_glyphs_ime_and_caret_publication(cx: &mut TestAppContext) {
    struct RelocatedCanvas(Entity<CanvasView>);
    impl Render for RelocatedCanvas {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                div()
                    .absolute()
                    .left(px(32.0))
                    .top(px(24.0))
                    .w(px(640.0))
                    .h(px(480.0))
                    .child(self.0.clone()),
            )
        }
    }
    for mode in ["selection", "ime", "caret", "finish"] {
        let (view, mut visual) = fixture(cx, CanvasDocument::default());
        visual.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.begin_text(draw::Point::new(0.0, 0.0), None, window, cx);
                v.replace_text_in_range(None, "Published text", window, cx);
                if matches!(mode, "selection" | "finish") {
                    v.set_selected_text_range(0..9, window, cx);
                } else if mode == "ime" {
                    v.draft.as_mut().unwrap().buffer.marked = Some(0..9);
                }
            })
        });
        paint(&mut visual);
        let (published, frame) = view.read_with(&visual, |v, _| {
            (
                v.text_selection_overlay.publication_snapshot(),
                v.gpu_frame.clone().unwrap(),
            )
        });
        visual.update(|window, cx| {
            view.update(cx, |v, cx| {
                if mode == "finish" {
                    v.finish_text(true, window, cx);
                } else {
                    v.replace_text_in_range(
                        None,
                        "new draft text that must not be displayed yet",
                        window,
                        cx,
                    );
                    if mode == "selection" {
                        v.set_selected_text_range(3..18, window, cx);
                    }
                    if mode == "ime" {
                        v.draft.as_mut().unwrap().buffer.marked = Some(4..14);
                    }
                }
                // Fail document composition after the new overlay candidate is prepared.
                v.options.presentation.content_clip = Some(CanvasRect {
                    x: f64::NAN,
                    y: 0.0,
                    width: 10.0,
                    height: 10.0,
                });
                v.options.presentation.content_visible = false;
                v.options.presentation.clear_background = false;
                v.invalidate_render();
                v.changed(window, cx);
            })
        });
        paint(&mut visual);
        view.read_with(&visual, |v, _| {
            assert!(
                v.error.is_some(),
                "{mode}: invalid document candidate succeeded"
            );
            assert_eq!(
                v.text_selection_overlay.publication_snapshot(),
                published,
                "{mode}: mixed text revisions"
            );
            assert!(Arc::ptr_eq(
                &frame.tiles,
                &v.gpu_frame.as_ref().unwrap().tiles
            ));
            assert!(v.display_presentation().content_visible);
            assert!(v.display_presentation().clear_background);
        });
        let (old_paths, old_tiles) = visual.update(|window, _| {
            let scene = window.rendered_scene();
            (
                scene
                    .paths
                    .iter()
                    .map(|path| path.bounds.origin)
                    .collect::<Vec<_>>(),
                scene
                    .gpu_textures
                    .iter()
                    .map(|texture| texture.bounds.origin)
                    .collect::<Vec<_>>(),
            )
        });
        assert!(
            !old_paths.is_empty(),
            "{mode}: no published text control paths"
        );
        visual.update(|window, cx| {
            window.replace_root(cx, |_, _| RelocatedCanvas(view.clone()));
        });
        paint(&mut visual);
        visual.update(|window, _| {
            let scale = window.scale_factor();
            let scene = window.rendered_scene();
            assert_eq!(
                scene.paths.len(),
                old_paths.len(),
                "{mode}: relocated path count"
            );
            for (path, origin) in scene.paths.iter().zip(old_paths) {
                assert!(
                    ((path.bounds.origin.x - origin.x).0 - 32.0 * scale).abs() < 0.001,
                    "{mode}: path x"
                );
                assert!(
                    ((path.bounds.origin.y - origin.y).0 - 24.0 * scale).abs() < 0.001,
                    "{mode}: path y"
                );
            }
            assert_eq!(scene.gpu_textures.len(), old_tiles.len());
            for (texture, origin) in scene.gpu_textures.iter().zip(old_tiles) {
                assert!(
                    ((texture.bounds.origin.x - origin.x).0 - 32.0 * scale).abs() < 0.001,
                    "{mode}: texture x"
                );
                assert!(
                    ((texture.bounds.origin.y - origin.y).0 - 24.0 * scale).abs() < 0.001,
                    "{mode}: texture y"
                );
            }
        });
        visual.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.options.presentation.content_clip = None;
                v.options.presentation.content_visible = true;
                v.options.presentation.clear_background = true;
                v.invalidate_render();
                v.changed(window, cx);
            })
        });
        paint(&mut visual);
        view.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{mode}: retry failed: {:?}", v.error);
            assert_ne!(
                v.text_selection_overlay.publication_snapshot(),
                published,
                "{mode}: retry retained stale controls"
            );
        });
    }
}

#[gpui::test]
fn smart_erase_paints_current_mask_and_only_imports_valid_worker_results(cx: &mut TestAppContext) {
    for mode in [
        "complete",
        "cancel_pending",
        "cancel_completed",
        "no_donors",
    ] {
        let mut document = CanvasDocument::default();
        document.set_background(Some(
            SourceImage::new(image::RgbaImage::from_pixel(
                160,
                120,
                image::Rgba([60, 90, 140, 255]),
            ))
            .unwrap(),
        ));
        let (view, mut visual) = fixture(cx, document);
        visual.update(|window, cx| {
            view.update(cx, |v, cx| {
                let viewport = v.session.viewport();
                let mut style = v
                    .document
                    .engine()
                    .viewport_style_toolbar_state(viewport)
                    .unwrap()
                    .filter_style;
                style.filter_type = draw::CanvasFilterType::SmartErase;
                v.document
                    .command(|e| {
                        e.set_viewport_filter_style(
                            viewport,
                            style,
                            draw::FILTER_STYLE_PROPERTY_ALL,
                        )
                    })
                    .unwrap();
                v.set_tool(draw::ActiveTool::RectangleFilter, window, cx);
            })
        });
        paint(&mut visual);
        let (start, end) = if mode == "no_donors" {
            (point(px(1.0), px(1.0)), point(px(639.0), px(479.0)))
        } else {
            (point(px(280.0), px(220.0)), point(px(340.0), px(280.0)))
        };
        pointer(&mut visual, start, true);
        pointer(&mut visual, end, false);
        visual.update(|window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: end,
                    ..Default::default()
                }),
                cx,
            );
            window.draw(cx).clear(cx);
        });
        // A previous frame can still occupy the GPU submission slots. Draw the
        // current preview once a slot retires, without running reconstruction
        // workers or advancing their timers.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while view.read_with(&visual, |v, _| v.rendered_generation != v.generation) {
            visual.update(|window, cx| window.draw(cx).clear(cx));
            assert!(
                std::time::Instant::now() < deadline,
                "preview GPU frame did not retire"
            );
            std::thread::yield_now();
        }
        let preview = view.read_with(&visual, |v, _| {
            assert_eq!(v.rendered_generation, v.generation);
            assert!(v.smart_task.is_some());
            assert_eq!(
                v.smart_service.snapshot().result_count(),
                0,
                "frame preparation ran reconstruction on the UI thread"
            );
            v.gpu_frame.clone().unwrap()
        });
        if mode == "cancel_completed" {
            let scheduler = visual.dispatcher.scheduler().clone();
            for _ in 0..100 {
                if scheduler.pending_task_counts().1 > 0 {
                    break;
                }
                assert!(scheduler.tick(), "reconstruction worker was never queued");
            }
            assert!(scheduler.pending_task_counts().1 > 0);
            while scheduler.tick_background_only() {}
        }
        if mode.starts_with("cancel_") {
            visual.update(|window, cx| {
                view.update(cx, |v, cx| {
                    v.replace_document(CanvasDocument::default(), window, cx)
                        .unwrap();
                })
            });
        }
        paint(&mut visual);
        view.read_with(&visual, |v, _| {
            assert!(!v.is_rendering(), "{}", v.render_diagnostics());
            assert_eq!(
                v.error.is_some(),
                mode == "no_donors",
                "{mode}: {:?}",
                v.error
            );
            assert_eq!(
                v.smart_service.snapshot().result_count(),
                usize::from(mode == "complete")
            );
            if mode != "no_donors" {
                assert_ne!(v.gpu_frame.as_ref().unwrap().revision, preview.revision);
            }
        });
        let completed = view.read_with(&visual, |v, _| v.gpu_frame.clone().unwrap());
        paint(&mut visual);
        view.read_with(&visual, |v, _| {
            assert!(!v.is_rendering());
            assert!(
                Arc::ptr_eq(&completed.tiles, &v.gpu_frame.as_ref().unwrap().tiles),
                "{mode}: idle retry"
            );
        });
    }
}

#[gpui::test]
fn smart_erase_multiscale_runs_on_gpu_and_cancellation_rejects_submitted_results(
    cx: &mut TestAppContext,
) {
    for (mode, cancel_gpu) in [
        ("fallback", false),
        ("cpu", false),
        ("cpu_layered", false),
        ("raster", false),
        ("raster", true),
        ("layered", false),
        ("layered", true),
        ("resident", false),
        ("resident", true),
    ] {
        let mut document = CanvasDocument::default();
        let mut source = SourceImage::new(image::RgbaImage::from_fn(192, 160, |x, y| {
            let n = (((x * 17 + y * 31) ^ (x * y)) % 49) as u8;
            image::Rgba([90 + n, 120 + n, 60 + n, 255])
        }))
        .unwrap()
        .with_placement(draw::Point::new(-96.0, -80.0), 1.0)
        .unwrap();
        if mode == "layered" || mode == "fallback" || mode == "cpu_layered" {
            source = SourceImage::from_layers(&[
                crate::ImageSourceLayer::new(source),
                crate::ImageSourceLayer {
                    image: SourceImage::new(image::RgbaImage::from_pixel(
                        16,
                        16,
                        image::Rgba([0, 255, 0, 255]),
                    ))
                    .unwrap(),
                    canvas_rect: Some(CanvasRect::new(-96.0, -80.0, 192.0, 160.0).unwrap()),
                    coverage: None,
                    smart_erase_source: false,
                },
            ])
            .unwrap()
            .unwrap();
        }
        document.set_background(Some(source));
        let (view, mut visual) = fixture(cx, document);
        visual.update(|window, cx| {
            view.update(cx, |v, cx| {
                if mode == "cpu" || mode == "cpu_layered" {
                    let mut options = v.options().clone();
                    options.smart_erase_execution = crate::SmartEraseExecution::Cpu;
                    v.set_options(options, window, cx).unwrap();
                }
                if mode == "resident" {
                    let service = window.gpu_service().unwrap();
                    let assembler = crate::GpuDonorAssembler::new(service).unwrap();
                    let source = v.document.background().unwrap();
                    let input =
                        crate::GpuDonorInput::new(source, [0, 0, 192, 160], &Default::default())
                            .unwrap();
                    let mut job = assembler.start(input, Default::default()).unwrap();
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                    let source = loop {
                        if let std::task::Poll::Ready(result) = job.poll() {
                            break result.unwrap().source;
                        }
                        assert!(std::time::Instant::now() < deadline);
                        std::thread::yield_now();
                    };
                    v.set_background(source, window, cx);
                }
                let viewport = v.session.viewport();
                let mut style = v
                    .document
                    .engine()
                    .viewport_style_toolbar_state(viewport)
                    .unwrap()
                    .filter_style;
                style.filter_type = draw::CanvasFilterType::SmartErase;
                v.document
                    .command(|engine| {
                        engine.set_viewport_filter_style(
                            viewport,
                            style,
                            draw::FILTER_STYLE_PROPERTY_ALL,
                        )
                    })
                    .unwrap();
                v.set_tool(draw::ActiveTool::RectangleFilter, window, cx);
            })
        });
        paint(&mut visual);
        let start = point(px(280.0), px(220.0));
        let end = point(px(340.0), px(280.0));
        pointer(&mut visual, start, true);
        pointer(&mut visual, end, false);
        visual.update(|window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: end,
                    ..Default::default()
                }),
                cx,
            );
            window.draw(cx).clear(cx);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        if mode == "fallback" {
            // Finish worker classification, then exhaust the real shared budget
            // before its owner task can allocate a compute pipeline.
            let scheduler = visual.dispatcher.scheduler().clone();
            while scheduler.pending_task_counts().1 == 0 {
                assert!(scheduler.tick());
            }
            while scheduler.tick_background_only() {}
            let service = visual.update(|window, _| window.gpu_service().unwrap());
            let mut done = service.completion().unwrap();
            while !done.poll().unwrap() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
            visual.update(|_, cx| {
                view.update(cx, |v, _| {
                    v.gpu_canvas.as_mut().unwrap().is_ready().unwrap();
                })
            });
            while service.poll_deferred().unwrap() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
            let memory = crate::gpu_memory::GpuMemory::shared(
                &service,
                None,
                crate::DEFAULT_GPU_MEMORY_BUDGET,
            )
            .unwrap();
            let pressure = memory.reserve(memory.limit() - memory.used()).unwrap();
            while scheduler.pending_task_counts().1 == 0 {
                assert!(scheduler.tick());
                assert!(std::time::Instant::now() < deadline);
            }
            view.read_with(&visual, |v, _| {
                assert!(
                    v.gpu_smart_erase.is_none(),
                    "budget failure did not reach CPU fallback"
                )
            });
            drop(pressure);
        }
        let mut cancelled = false;
        loop {
            paint(&mut visual);
            if cancel_gpu
                && !cancelled
                && view.read_with(&visual, |v, _| v.gpu_smart_erase.is_some())
            {
                visual.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        v.replace_document(CanvasDocument::default(), window, cx)
                            .unwrap();
                    })
                });
                cancelled = true;
            }
            if !view.read_with(&visual, |v, _| v.is_rendering()) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "GPU erasure view did not finish"
            );
            visual
                .executor()
                .advance_clock(std::time::Duration::from_millis(2));
            std::thread::yield_now();
        }
        view.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let snapshot = v.smart_service.snapshot();
            if cancel_gpu {
                assert!(cancelled);
                assert_eq!(snapshot.result_count(), 0);
            } else {
                assert_eq!(snapshot.result_count(), 1);
                let expect_gpu = mode != "fallback"
                    && mode != "cpu"
                    && mode != "cpu_layered"
                    && (mode != "layered"
                        || v.gpu_donors.as_ref().unwrap().supports_filtered_layers());
                assert_eq!(
                    snapshot.entries[0].filled.gpu_texture().is_some(),
                    expect_gpu,
                    "{mode}: unexpected reconstruction backend"
                );
                if mode == "layered" || mode == "resident" {
                    assert!(v.gpu_donors.is_some());
                }
                if mode == "cpu" || mode == "cpu_layered" {
                    assert!(v.gpu_smart_erase.is_none());
                    assert!(v.gpu_donors.is_none());
                }
            }
        });
        if mode == "raster" && !cancel_gpu {
            let previous = view.read_with(&visual, |v, _| {
                v.smart_service.snapshot().entries[0].clone()
            });
            visual.update(|window, cx| {
                view.update(cx, |v, cx| {
                    let mut options = v.options().clone();
                    options.smart_erase_execution = crate::SmartEraseExecution::Cpu;
                    v.set_options(options, window, cx).unwrap();
                })
            });
            while view.read_with(&visual, |v, _| v.is_rendering()) {
                paint(&mut visual);
                visual
                    .executor()
                    .advance_clock(std::time::Duration::from_millis(2));
                assert!(std::time::Instant::now() < deadline);
            }
            view.read_with(&visual, |v, _| {
                assert!(v.error.is_none());
                let snapshot = v.smart_service.snapshot();
                assert!(
                    Arc::ptr_eq(&previous, &snapshot.entries[0]),
                    "execution preference invalidated a valid completed patch"
                );
            });
        }
    }
}
