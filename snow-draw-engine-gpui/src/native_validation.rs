//! Reproducible validation through a real GPUI window and its native input adapter.
//! Events use GPUI's supported dispatch API; IME checks exercise EntityInputHandler.
use super::*;
use anyhow::{Result, ensure};

async fn settle(demo: &Entity<Demo>, cx: &mut AsyncApp) -> Result<()> {
    let start = Instant::now();
    loop {
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
        let state = demo.read_with(cx, |demo, cx| {
            (
                !demo.canvas.read(cx).is_rendering(),
                demo.canvas.read(cx).render_diagnostics(),
            )
        });
        if state.0 {
            return Ok(());
        }
        ensure!(
            start.elapsed() < Duration::from_secs(60),
            "Canvas did not settle: {}",
            state.1
        );
    }
}
fn canvas(demo: &Entity<Demo>, cx: &AsyncApp) -> Entity<CanvasView> {
    demo.read_with(cx, |demo, _| demo.canvas.clone())
}
trait NativeEvent {
    fn platform(self) -> PlatformInput;
}
impl NativeEvent for MouseDownEvent {
    fn platform(self) -> PlatformInput {
        PlatformInput::MouseDown(self)
    }
}
impl NativeEvent for MouseMoveEvent {
    fn platform(self) -> PlatformInput {
        PlatformInput::MouseMove(self)
    }
}
impl NativeEvent for MouseUpEvent {
    fn platform(self) -> PlatformInput {
        PlatformInput::MouseUp(self)
    }
}
fn dispatch(window: &mut Window, cx: &mut App, event: impl NativeEvent) {
    window.dispatch_event(event.platform(), cx);
}
fn click(window: &mut Window, cx: &mut App, position: Point<Pixels>) {
    dispatch(
        window,
        cx,
        MouseMoveEvent {
            position,
            ..Default::default()
        },
    );
    dispatch(
        window,
        cx,
        MouseDownEvent {
            button: MouseButton::Left,
            position,
            ..Default::default()
        },
    );
    dispatch(
        window,
        cx,
        MouseUpEvent {
            button: MouseButton::Left,
            position,
            ..Default::default()
        },
    );
}
fn drag(
    window: &mut Window,
    cx: &mut App,
    from: Point<Pixels>,
    to: Point<Pixels>,
    button: MouseButton,
) {
    dispatch(
        window,
        cx,
        MouseMoveEvent {
            position: from,
            ..Default::default()
        },
    );
    dispatch(
        window,
        cx,
        MouseDownEvent {
            button,
            position: from,
            ..Default::default()
        },
    );
    dispatch(
        window,
        cx,
        MouseMoveEvent {
            position: to,
            pressed_button: Some(button),
            ..Default::default()
        },
    );
    dispatch(
        window,
        cx,
        MouseUpEvent {
            button,
            position: to,
            ..Default::default()
        },
    );
}
fn key(window: &mut Window, cx: &mut App, key: &'static str) {
    window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
}
fn capture(
    window: AnyWindowHandle,
    directory: &std::path::Path,
    name: &str,
    cx: &mut AsyncApp,
) -> Result<image::RgbaImage> {
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        let image = window.render_to_image()?;
        image.save(directory.join(name))?;
        Ok::<_, anyhow::Error>(image)
    })?
}
pub(super) async fn run(
    window: AnyWindowHandle,
    demo: Entity<Demo>,
    directory: PathBuf,
    cx: &mut AsyncApp,
) -> Result<()> {
    let start = Instant::now();
    std::fs::create_dir_all(&directory)?;
    settle(&demo, cx).await?;
    let light = capture(window, &directory, "canvas-native.png", cx)?;
    let showcase_bounds = canvas(&demo, cx).read_with(cx, |v, _| v.surface_bounds());
    let mut checks = vec!["native GPU readback of showcase"];
    cx.update_window(window, |_, _, cx| {
        demo.update(cx, |demo, cx| {
            demo.dark = true;
            cx.notify();
        });
    })?;
    // Let notification effects and the normal frame cycle complete after theme
    // changes; consecutive manual draws in one update are not a settled frame.
    settle(&demo, cx).await?;
    let dark = capture(window, &directory, "canvas-dark.png", cx)?;
    let scale = cx.update_window(window, |_, window, _| window.scale_factor())?;
    let content_count = |image: &image::RgbaImage| {
        let left = (f32::from(showcase_bounds.left()) * scale).ceil() as u32 + 4;
        let top = (f32::from(showcase_bounds.top()) * scale).ceil() as u32 + 4;
        let right = ((f32::from(showcase_bounds.right()) * scale).floor() as u32)
            .saturating_sub(4)
            .min(image.width());
        let bottom = ((f32::from(showcase_bounds.bottom()) * scale).floor() as u32)
            .saturating_sub(4)
            .min(image.height());
        (top..bottom)
            .flat_map(|y| (left..right).map(move |x| (x, y)))
            .filter(|&(x, y)| image.get_pixel(x, y).0[..3].iter().any(|v| *v < 230))
            .count()
    };
    let light_content = content_count(&light);
    let dark_content = content_count(&dark);
    ensure!(
        light.dimensions() == dark.dimensions(),
        "Theme changed capture dimensions"
    );
    let left = (f32::from(showcase_bounds.left()) * scale).ceil() as u32 + 4;
    let top = (f32::from(showcase_bounds.top()) * scale).ceil() as u32 + 4;
    let right = ((f32::from(showcase_bounds.right()) * scale).floor() as u32)
        .saturating_sub(4)
        .min(light.width());
    let bottom = ((f32::from(showcase_bounds.bottom()) * scale).floor() as u32)
        .saturating_sub(4)
        .min(light.height());
    let mut canvas_difference = 0_u64;
    for y in top..bottom {
        for x in left..right {
            let a = light.get_pixel(x, y).0;
            let b = dark.get_pixel(x, y).0;
            canvas_difference += (0..3).map(|i| u64::from(a[i].abs_diff(b[i]))).sum::<u64>();
        }
    }
    let canvas_mean_rgb_difference = canvas_difference as f64
        / (f64::from(right.saturating_sub(left)) * f64::from(bottom.saturating_sub(top)) * 3.0);
    ensure!(
        light_content > 1000
            && dark_content >= light_content * 9 / 10
            && dark_content <= light_content * 11 / 10
            && canvas_mean_rgb_difference <= 1.0,
        "Theme capture changed document pixels: light={light_content}, dark={dark_content}, mean RGB error={canvas_mean_rgb_difference}"
    );
    checks.push("dark theme preserves visible document content after settled frame");
    // Use the demo's New button, then the canvas's own keyboard/pointer listeners.
    cx.update_window(window, |_, window, cx| {
        click(window, cx, point(px(238.0), px(28.0)));
        window.draw(cx).clear(cx);
    })?;
    settle(&demo, cx).await?;
    let view = canvas(&demo, cx);
    ensure!(
        !view.read_with(cx, |v, _| v.document().engine().has_document_content()),
        "New button did not clear the showcase"
    );
    checks.push("native toolbar New button");
    let bounds = view.read_with(cx, |v, _| v.surface_bounds());
    let at = |x, y| bounds.origin + point(px(x), px(y));
    cx.update_window(window, |_, window, cx| {
        click(window, cx, at(40.0, 40.0));
        key(window, cx, "r");
        drag(
            window,
            cx,
            at(150.0, 120.0),
            at(330.0, 240.0),
            MouseButton::Left,
        );
        drag(
            window,
            cx,
            at(430.0, 150.0),
            at(600.0, 270.0),
            MouseButton::Left,
        );
    })?;
    settle(&demo, cx).await?;
    let before = view
        .read_with(cx, |v, _| v.document().engine().document_checkpoint())
        .unwrap();
    ensure!(
        view.read_with(cx, |v, _| v.document().engine().has_document_content()),
        "Pointer drawing did not commit"
    );
    cx.update_window(window, |_, window, cx| {
        key(window, cx, "ctrl-z");
        key(window, cx, "ctrl-y");
    })?;
    ensure!(
        view.read_with(cx, |v, _| v.document().engine().document_checkpoint())
            .unwrap()
            == before,
        "Undo/redo changed the committed drawing"
    );
    checks.push("repeated pointer gestures and undo/redo");
    cx.update_window(window, |_, window, cx| {
        dispatch(
            window,
            cx,
            MouseDownEvent {
                button: MouseButton::Left,
                position: at(50.0, 350.0),
                ..Default::default()
            },
        );
        dispatch(
            window,
            cx,
            MouseMoveEvent {
                position: at(220.0, 440.0),
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            },
        );
        key(window, cx, "escape");
    })?;
    ensure!(
        !view.read_with(cx, |v, _| v.document().engine().has_pointer_capture()),
        "Escape retained capture"
    );
    ensure!(
        view.read_with(cx, |v, _| v.document().engine().document_checkpoint())
            .unwrap()
            == before,
        "Escape committed a preview"
    );
    checks.push("interrupted gesture capture and preview cancellation");
    cx.update_window(window, |_, window, cx| {
        drag(
            window,
            cx,
            at(20.0, 30.0),
            bounds.origin - point(px(20.0), px(20.0)),
            MouseButton::Left,
        );
    })?;
    ensure!(
        !view.read_with(cx, |v, _| v.document().engine().has_pointer_capture()),
        "Outside release retained capture"
    );
    checks.push("outside release with pointer capture");
    cx.update_window(window, |_, window, cx| {
        key(window, cx, "v");
        click(window, cx, at(150.0, 180.0));
        window.draw(cx).clear(cx);
    })?;
    let handle = view.read_with(cx, |v, _| {
        v.session().scene().overlays().iter().find_map(|overlay| {
            if let draw::OverlayDisplayItem::Rectangle(r) = overlay {
                (r.kind == draw::UiShapeKind::SelectionResizeHandle).then_some(*r)
            } else {
                None
            }
        })
    });
    let handle = handle.ok_or_else(|| anyhow::anyhow!("Selection resize handle is absent"))?;
    let point = view.read_with(cx, |v, _| {
        let f = v.session().scene().frame();
        let p = draw::canvas_to_view(
            draw::Point::new(handle.center_x, handle.center_y),
            &f.camera,
            f.surface,
        );
        at(p.x as f32, p.y as f32)
    });
    let before_resize = view
        .read_with(cx, |v, _| v.document().engine().document_checkpoint())
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        drag(
            window,
            cx,
            point,
            point + gpui::point(px(-35.0), px(-25.0)),
            MouseButton::Left,
        )
    })?;
    ensure!(
        view.read_with(cx, |v, _| v.document().engine().document_checkpoint())
            .unwrap()
            != before_resize,
        "Native resize did not change geometry"
    );
    checks.push("selection and resize handle");
    cx.update_window(window, |_, window, cx| {
        key(window, cx, "t");
        click(window, cx, at(470.0, 390.0));
        window.draw(cx).clear(cx);
    })?;
    ensure!(
        view.read_with(cx, |v, _| v.has_text_draft()),
        "Text click did not open draft"
    );
    cx.update_window(window, |_, window, cx| {
        view.update(cx, |v, cx| {
            v.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
            v.replace_and_mark_text_in_range(None, "你好 😀 GPUI", Some(10..10), window, cx);
        })
    })?;
    settle(&demo, cx).await?;
    capture(window, &directory, "canvas-ime-draft.png", cx)?;
    cx.update_window(window, |_, window, cx| {
        view.update(cx, |v, cx| {
            v.replace_text_in_range(None, "你好 😀 GPUI", window, cx)
        });
        key(window, cx, "ctrl-enter");
    })?;
    ensure!(
        !view.read_with(cx, |v, _| v.has_text_draft()),
        "Ctrl+Enter did not commit text"
    );
    checks.push("native-window UTF-16 preedit, emoji and atomic text commit");
    let camera = view.read_with(cx, |v, _| v.session().scene().frame().camera);
    cx.update_window(window, |_, window, cx| {
        drag(
            window,
            cx,
            at(650.0, 450.0),
            at(710.0, 490.0),
            MouseButton::Middle,
        )
    })?;
    ensure!(
        view.read_with(cx, |v, _| v.session().scene().frame().camera) != camera,
        "Middle-button pan did not change camera"
    );
    checks.push("viewport pan");
    cx.update_window(window, |_, window, cx| {
        key(window, cx, "ctrl-+");
    })?;
    ensure!(
        view.read_with(cx, |v, _| v.session().scene().frame().camera.zoom) > camera.zoom,
        "Zoom shortcut did not change scale"
    );
    checks.push("zoom shortcut");
    settle(&demo, cx).await?;
    capture(window, &directory, "canvas-interactions.png", cx)?;
    let before_host = view
        .read_with(cx, |v, _| v.document().engine().document_checkpoint())
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        view.update(cx, |v, cx| {
            v.set_command_key_resolver(Some(Rc::new(|key| match key.key.as_str() {
                "f8" => Some("r".into()),
                "f9" => Some("escape".into()),
                _ => None,
            })));
            v.set_crosshair_factory(
                Some(Rc::new(|scale| {
                    Some(snow_draw_engine_gpui::CanvasCrosshair {
                        radius: 24.0 / scale,
                        gap: 6.0 / scale,
                        color: rgb(0xff00e1).into(),
                        ..Default::default()
                    })
                })),
                cx,
            );
            v.set_show_dirty_regions(true, cx);
            v.set_cursor_for_layer(
                snow_draw_engine_gpui::CanvasCursorLayer::Host,
                CursorStyle::Crosshair,
                cx,
            );
        });
        key(window, cx, "f8");
        dispatch(
            window,
            cx,
            MouseMoveEvent {
                position: at(700.0, 420.0),
                ..Default::default()
            },
        );
        view.update(cx, |v, cx| v.clear_render_state(window, cx));
    })?;
    settle(&demo, cx).await?;
    ensure!(
        view.read_with(cx, |v, _| v
            .document()
            .engine()
            .viewport_active_tool(v.session().viewport())
            .unwrap())
            == draw::ActiveTool::Shape,
        "Host command-key resolver did not select the shape tool"
    );
    ensure!(
        view.read_with(cx, |v, _| v.document().engine().document_checkpoint())
            .unwrap()
            == before_host,
        "Host diagnostics/cache reset changed drawing history"
    );
    // Keep the synthetic local pointer position deterministic at readback.
    cx.update_window(window, |_, window, cx| {
        dispatch(
            window,
            cx,
            MouseMoveEvent {
                position: at(700.0, 420.0),
                ..Default::default()
            },
        )
    })?;
    let host = capture(window, &directory, "canvas-host-controls.png", cx)?;
    let pointer = at(700.0, 420.0);
    let center = [
        (f32::from(pointer.x) * scale) as i32,
        (f32::from(pointer.y) * scale) as i32,
    ];
    let mut crosshair_pixels = 0;
    for y in (center[1] - 30).max(0)..(center[1] + 31).min(host.height() as i32) {
        for x in (center[0] - 30).max(0)..(center[0] + 31).min(host.width() as i32) {
            let p = host.get_pixel(x as u32, y as u32).0;
            crosshair_pixels += usize::from(p[0] > 240 && p[1] < 20 && p[2] > 210);
        }
    }
    ensure!(
        crosshair_pixels >= 32,
        "Custom crosshair was absent from native readback ({crosshair_pixels} pixels)"
    );
    checks.push("host key resolver, DPI crosshair, dirty overlays and asynchronous cache reset");
    cx.update_window(window, |_, window, cx| {
        view.update(cx, |v, cx| v.set_tool(draw::ActiveTool::Text, window, cx));
        click(window, cx, at(700.0, 520.0));
        view.update(cx, |v, cx| v.replace_text_in_range(None, "á", window, cx));
        window.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke: Keystroke {
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
            }),
            cx,
        );
        view.update(cx, |v, cx| {
            v.replace_text_in_range(None, "@", window, cx);
            ensure!(
                v.text_for_range(0..2, &mut None, window, cx).as_deref() == Some("á@"),
                "Native AltGr preference changed draft contents"
            );
            Ok::<_, anyhow::Error>(())
        })?;
        key(window, cx, "f9");
        Ok::<_, anyhow::Error>(())
    })??;
    ensure!(
        !view.read_with(cx, |v, _| v.has_text_draft()),
        "Resolved function key did not cancel draft"
    );
    ensure!(
        view.read_with(cx, |v, _| v.document().engine().document_checkpoint())
            .unwrap()
            == before_host,
        "Cancelled native draft changed history"
    );
    checks.push("live draft function-key cancellation and AltGr native text protocol");

    let mut plain = CanvasDocument::default();
    plain.command(|e| e.apply_annotation_json(br#"{"version":1,"operations":[{"type":"rectangle","bounds":[200,180,170,120],"style":{"fill":[40,110,190,190]}},{"type":"rectangle","bounds":[320,230,170,120],"style":{"fill":[210,110,60,190]}}]}"#))?;
    cx.update_window(window, |_, window, cx| {
        view.update(cx, |v, cx| v.replace_document(plain, window, cx))
    })??;
    settle(&demo, cx).await?;
    let selected = view.read_with(cx, |v, _| {
        match v
            .session()
            .scene()
            .ordered_nodes()
            .next()
            .unwrap()
            .item
            .as_ref()
        {
            draw::SceneDisplayItem::Rectangle(r) => draw::ElementId {
                index: r.id.index,
                generation: r.id.generation,
            },
            _ => unreachable!(),
        }
    });
    cx.update_window(window, |_, window, cx| {
        view.update(cx, |v, cx| {
            v.set_tool(draw::ActiveTool::Select, window, cx);
            v.command(
                |e, s| e.select_element_with_viewport_changes(s, selected),
                window,
                cx,
            );
        });
    })?;
    settle(&demo, cx).await?;
    for opacity in [0.35, 0.75, 0.45, 0.85] {
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |v, cx| {
                v.command(
                    |e, s| e.set_selected_opacity_with_viewport_changes(s, opacity),
                    window,
                    cx,
                );
            });
        })?;
        settle(&demo, cx).await?;
        ensure!(
            view.read_with(cx, |v, _| v.error().is_none()),
            "Native repeated edit failed"
        );
    }
    ensure!(
        view.read_with(cx, |v, _| !v.session().scene().dirty_regions().is_empty()),
        "Repeated edit did not publish diagnostic damage"
    );
    capture(window, &directory, "canvas-retained-edits.png", cx)?;
    checks.push("repeated translucent plain-scene edits and native dirty diagnostics");
    let cleanup = cx.update(|cx| snow_draw_engine_gpui::canvas_cleanup_diagnostics(cx));
    ensure!(
        cleanup.pending == 0 && cleanup.completed >= 2,
        "Document retirement did not complete: {cleanup:?}"
    );
    checks.push("owner-thread retirement queue completed replaced documents");
    let (scale, size) = cx.update_window(window, |_, window, _| {
        (window.scale_factor(), window.viewport_size())
    })?;
    let report = serde_json::json!({"result":"passed","input_method":"GPUI Window::dispatch_event / dispatch_keystroke on Windows native window; EntityInputHandler composition protocol","os_ime_candidate_ui":"not automated","visual_assertions":{"light_content_pixels":light_content,"dark_content_pixels":dark_content,"canvas_mean_rgb_difference":canvas_mean_rgb_difference,"custom_crosshair_pixels":crosshair_pixels},"partial_path_pixel_equivalence":"Device-backed incremental/full-render regressions run separately; native run checks visible repeated edits","cleanup":{"pending":cleanup.pending,"completed":cleanup.completed,"overflow_drops":cleanup.overflow_drops,"last_drop_ms":cleanup.last_drop_ms,"maximum_drop_ms":cleanup.maximum_drop_ms},"scale":scale,"logical_size":[f32::from(size.width),f32::from(size.height)],"elapsed_ms":start.elapsed().as_secs_f64()*1000.0,"checks":checks});
    std::fs::write(
        directory.join("validation.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}
