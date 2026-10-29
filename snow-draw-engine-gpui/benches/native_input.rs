//! Native message queue -> GPUI CanvasView -> Present1 submission benchmark.
#[cfg(target_os = "windows")]
mod native {
    mod view_erase {
        include!("native_input/view_erase.rs");
    }
    use anyhow::{Context as _, Result, ensure};
    use gpui::{prelude::*, *};
    use snow_draw_engine_gpui::{
        CanvasDocument, CanvasSession, CanvasView, GpuSmartErase, GpuSmartEraseInput,
        GpuSmartEraseJob, GpuSmartEraseProfile, SourceImage, engine as draw,
    };
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        task::Poll,
        time::{Duration, Instant as WallInstant},
    };
    use windows::{
        Win32::{
            Foundation::{HWND, LPARAM, WPARAM},
            UI::WindowsAndMessaging::*,
        },
        core::HSTRING,
    };

    #[derive(Default)]
    struct InputState {
        handled: u64,
        callback_started: Option<WallInstant>,
    }

    struct BenchView {
        canvas: Entity<CanvasView>,
        input: Rc<RefCell<InputState>>,
        erasure: Option<view_erase::Fixture>,
    }
    impl Render for BenchView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(self.canvas.clone())
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key != "f24" {
                        return;
                    }
                    let opacity = {
                        let mut input = this.input.borrow_mut();
                        input.callback_started = Some(WallInstant::now());
                        input.handled += 1;
                        if input.handled.is_multiple_of(2) {
                            1.0
                        } else {
                            0.5
                        }
                    };
                    if let Some(fixture) = &this.erasure {
                        let index = this.input.borrow().handled;
                        this.canvas.update(cx, |canvas, cx| {
                            fixture.apply(canvas, index, window, cx);
                        });
                        cx.stop_propagation();
                        return;
                    }
                    this.canvas.update(cx, |canvas, cx| {
                        canvas.command(
                            |engine, viewport| {
                                engine.set_selected_opacity_with_viewport_changes(viewport, opacity)
                            },
                            window,
                            cx,
                        )
                    });
                    cx.stop_propagation();
                }))
        }
    }

    fn fixture() -> Result<(CanvasDocument, draw::ElementId)> {
        let mut document = CanvasDocument::default();
        for start in (0..10_000).step_by(128) {
            let operations: Vec<_> = (start..(start + 128).min(10_000)).map(|index| serde_json::json!({
                "type":"rectangle", "bounds":[(index % 100) as f64 * 9.6, (index / 100) as f64 * 6.4, 5, 3],
                "style":{"fill":[40,100,190,255],"stroke_width":1}
            })).collect();
            let bytes =
                serde_json::to_vec(&serde_json::json!({"version":1,"operations":operations}))?;
            document.command(|engine| engine.apply_annotation_json(&bytes))?;
        }
        let mut session = CanvasSession::new(&mut document)?;
        session.flush(&document)?;
        let node = session
            .scene()
            .nodes()
            .find(|(_, node)| matches!(node.item.as_ref(), draw::SceneDisplayItem::Rectangle(_)))
            .context("rectangle fixture")?
            .1;
        let selected = draw::ElementId {
            index: node.id.element.index,
            generation: node.id.element.generation,
        };
        ensure!(
            session.close(&mut document)?.is_none(),
            "fixture viewport held pointer capture"
        );
        Ok((document, selected))
    }

    async fn presented(
        service: &Arc<dyn GpuService>,
        count: u64,
        after: WallInstant,
        cx: &AsyncApp,
    ) -> Result<GpuServiceDiagnostics> {
        let deadline = WallInstant::now() + Duration::from_secs(30);
        loop {
            let diagnostics = service.diagnostics();
            if diagnostics.presentations > count
                && diagnostics
                    .last_presentation_end
                    .is_some_and(|end| end >= after)
            {
                return Ok(diagnostics);
            }
            ensure!(
                WallInstant::now() < deadline,
                "native presentation timed out: {diagnostics:?}"
            );
            cx.background_executor()
                .timer(Duration::from_millis(1))
                .await;
        }
    }

    fn poll_compute(
        job: &mut Option<GpuSmartEraseJob>,
        profile: &mut Option<GpuSmartEraseProfile>,
        max_poll_ms: &mut f64,
    ) -> Result<()> {
        let Some(active) = job else { return Ok(()) };
        let start = WallInstant::now();
        let result = active.poll();
        *max_poll_ms = max_poll_ms.max(start.elapsed().as_secs_f64() * 1000.0);
        if let Poll::Ready(result) = result {
            *profile = Some(result?.profile);
            *job = None;
        }
        Ok(())
    }

    async fn wait_for_work(
        cx: &AsyncApp,
        job: &Option<GpuSmartEraseJob>,
        runtime: &Option<GpuSmartErase>,
        use_events: bool,
    ) -> Result<bool> {
        if job
            .as_ref()
            .is_some_and(GpuSmartEraseJob::has_pending_submissions)
        {
            cx.foreground_executor().spawn(async {}).await;
        } else if use_events
            && job.is_some()
            && let Some(wake) = runtime.as_ref().unwrap().completion_wake()?
        {
            wake.await;
            return Ok(true);
        } else {
            cx.background_executor()
                .timer(Duration::from_millis(1))
                .await;
        }
        Ok(false)
    }

    fn native_handle(title: &str) -> Result<HWND> {
        unsafe { FindWindowW(None, &HSTRING::from(title)) }.context("finding benchmark HWND")
    }

    fn percentile(values: &mut [f64], percent: usize) -> f64 {
        values.sort_by(f64::total_cmp);
        values[(values.len() * percent).div_ceil(100).saturating_sub(1)]
    }

    async fn measure(
        window: WindowHandle<BenchView>,
        selected: draw::ElementId,
        title: String,
        output: std::path::PathBuf,
        mode: String,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        let initial = WallInstant::now();
        cx.background_executor()
            .timer(Duration::from_millis(300))
            .await;
        let (service, input, refresh, scale, logical_size) =
            window.update(cx, |root, window, cx| {
                let canvas = root.canvas.clone();
                let input = root.input.clone();
                canvas.update(cx, |canvas, cx| {
                    canvas.fit(window, cx);
                    canvas.command(
                        |engine, viewport| {
                            engine.select_element_with_viewport_changes(viewport, selected)
                        },
                        window,
                        cx,
                    );
                    canvas.focus_handle(cx).focus(window, cx);
                });
                (
                    window.gpu_service(),
                    input,
                    window.refresh_interval(cx),
                    window.scale_factor(),
                    window.viewport_size(),
                )
            })?;
        let service = service?;
        if let Err(error) = presented(&service, 0, initial, cx).await {
            let state =
                window.read_with(cx, |root, cx| root.canvas.read(cx).render_diagnostics())?;
            return Err(error.context(state));
        }
        if mode.starts_with("view-") {
            return view_erase::measure(window, &service, input, &title, output, cx).await;
        }
        let compute_load = mode == "smart-erase" || mode == "smart-erase-polling";
        let use_events = mode != "smart-erase-polling";
        let display_start = service.diagnostics();
        let handle = native_handle(&title)?;
        let runtime = if compute_load {
            let mut runtime = GpuSmartErase::new(service.clone())?;
            runtime.set_profiling(true);
            Some(runtime)
        } else {
            None
        };
        let source = SourceImage::new(image::RgbaImage::from_fn(512, 384, |x, y| {
            let n = (((x * 17 + y * 31) ^ (x * y)) % 49) as u8;
            image::Rgba([90 + n, 120 + n, 60 + n, 255])
        }))?;
        let mask: Vec<u8> = (0..512 * 384)
            .map(|i| u8::from((192..320).contains(&(i % 512)) && (144..240).contains(&(i / 512))))
            .collect();
        let mut samples = Vec::new();
        let mut queue_latencies = Vec::new();
        let mut callback_latencies = Vec::new();
        for index in 0..72 {
            let mut event_wakes = 0;
            let mut compute_profile = None;
            let mut max_poll_ms = 0.0_f64;
            let compute_input = runtime
                .as_ref()
                .map(|_| GpuSmartEraseInput::new(&source, &mask, &Default::default()))
                .transpose()?;
            let compute_start = WallInstant::now();
            let mut compute = match (&runtime, compute_input) {
                (Some(runtime), Some(input)) => Some(runtime.start(input, Default::default())?),
                _ => None,
            };
            let owner_start_ms = compute_start.elapsed().as_secs_f64() * 1000.0;
            poll_compute(&mut compute, &mut compute_profile, &mut max_poll_ms)?;
            let active_at_post = compute.is_some();
            let before = service.diagnostics();
            let expected = input.borrow().handled + 1;
            let posted = WallInstant::now();
            unsafe {
                PostMessageW(Some(handle), WM_KEYDOWN, WPARAM(0x87), LPARAM(1))?;
                PostMessageW(Some(handle), WM_KEYUP, WPARAM(0x87), LPARAM(0xc0000001))?;
            }
            let deadline = posted + Duration::from_secs(30);
            let callback = loop {
                poll_compute(&mut compute, &mut compute_profile, &mut max_poll_ms)?;
                {
                    let input = input.borrow();
                    if input.handled >= expected {
                        break input.callback_started.context("input timestamp")?;
                    }
                }
                ensure!(
                    WallInstant::now() < deadline,
                    "F24 native input was not dispatched"
                );
                event_wakes +=
                    usize::from(wait_for_work(cx, &compute, &runtime, use_events).await?);
            };
            let active_at_callback = compute.is_some();
            let after = loop {
                poll_compute(&mut compute, &mut compute_profile, &mut max_poll_ms)?;
                let diagnostics = service.diagnostics();
                if diagnostics.presentations > before.presentations
                    && diagnostics
                        .last_presentation_end
                        .is_some_and(|end| end >= callback)
                {
                    break diagnostics;
                }
                ensure!(
                    WallInstant::now() < deadline,
                    "presentation under compute load timed out"
                );
                event_wakes +=
                    usize::from(wait_for_work(cx, &compute, &runtime, use_events).await?);
            };
            let active_at_presentation_observation = compute.is_some();
            let end = after
                .last_presentation_end
                .context("presentation timestamp")?;
            let queued_ms = end.duration_since(posted).as_secs_f64() * 1000.0;
            let callback_ms = end.duration_since(callback).as_secs_f64() * 1000.0;
            if index >= 8 {
                queue_latencies.push(queued_ms);
                callback_latencies.push(callback_ms);
            }
            while compute.is_some() {
                poll_compute(&mut compute, &mut compute_profile, &mut max_poll_ms)?;
                ensure!(
                    WallInstant::now() < deadline,
                    "background reconstruction timed out"
                );
                if compute.is_some() {
                    event_wakes +=
                        usize::from(wait_for_work(cx, &compute, &runtime, use_events).await?);
                }
            }
            if compute_load {
                ensure!(
                    active_at_post && active_at_callback,
                    "input did not overlap reconstruction"
                );
                ensure!(
                    compute_profile.as_ref().is_some_and(|p| p.levels >= 2),
                    "multiscale load missing"
                );
            }
            samples.push(serde_json::json!({
                "native_queue_to_present_submission_ms": queued_ms,
                "native_callback_to_present_submission_ms": callback_ms,
                "compositor_cpu_ms": (after.compositor_cpu_ns - before.compositor_cpu_ns) as f64 / 1e6,
                "present_call_cpu_ms": (after.present_cpu_ns - before.present_cpu_ns) as f64 / 1e6,
                "presented_pixels": after.presented_pixels - before.presented_pixels,
                "full_presentations": after.full_presentations - before.full_presentations,
                "reconstruction_active_at_post":active_at_post,
                "reconstruction_active_at_callback":active_at_callback,
                "reconstruction_active_at_presentation_observation":active_at_presentation_observation,
                "reconstruction_owner_start_ms":owner_start_ms,
                "reconstruction_max_owner_poll_ms":max_poll_ms,
                "reconstruction_event_wakes":event_wakes,
                "reconstruction_total_ms": if compute_load {Some(compute_start.elapsed().as_secs_f64()*1000.0)} else {None},
                "reconstruction":compute_profile,
            }));
        }
        let error = window.read_with(cx, |root, cx| {
            root.canvas.read(cx).error().map(ToString::to_string)
        })?;
        ensure!(error.is_none(), "Canvas error: {error:?}");
        let idle_before = service.diagnostics();
        cx.background_executor()
            .timer(Duration::from_millis(200))
            .await;
        let idle_after = service.diagnostics();
        ensure!(
            idle_after.presentations == idle_before.presentations,
            "idle window submitted a new presentation"
        );

        let before_resize = service.diagnostics();
        let resized_at = WallInstant::now();
        window.update(cx, |_, window, _| {
            window.resize(size(px(1152.0), px(680.0)))
        })?;
        let resized = presented(&service, before_resize.presentations, resized_at, cx).await?;
        ensure!(
            resized.full_presentations > before_resize.full_presentations,
            "resize did not reconstruct the window"
        );
        let _was_visible_before_minimize = unsafe { ShowWindow(handle, SW_MINIMIZE) };
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
        let before_restore = service.diagnostics();
        let restored_at = WallInstant::now();
        let _was_visible_before_restore = unsafe { ShowWindow(handle, SW_RESTORE) };
        let restored = presented(&service, before_restore.presentations, restored_at, cx).await?;
        ensure!(
            restored.full_presentations > before_restore.full_presentations,
            "restore did not reconstruct the window"
        );

        let queue_p95 = percentile(&mut queue_latencies, 95);
        let callback_p95 = percentile(&mut callback_latencies, 95);
        let capabilities = service.capabilities();
        let report = serde_json::json!({
            "profile":"canvas-perf", "elements":10_000, "warmup_samples":8,
            "load":if compute_load {"smart_erase"} else {"none"},
            "completion":if use_events {"events_when_supported"} else {"timer"},
            "adapter":{"name":capabilities.adapter_name.as_ref(), "vendor_id":capabilities.vendor_id, "device_id":capabilities.device_id},
            "logical_size":[f32::from(logical_size.width), f32::from(logical_size.height)], "dpi_scale":scale,
            "physical_size":[f32::from(logical_size.width)*scale, f32::from(logical_size.height)*scale],
            "refresh_interval_ms":refresh.map(|interval| interval.as_secs_f64()*1000.0),
            "native_queue_to_present_submission_p95_ms":queue_p95,
            "native_callback_to_present_submission_p95_ms":callback_p95,
            "target_ms":8.33, "meets_native_queue_target":queue_p95<=8.33,
            "idle_presentations":idle_after.presentations-idle_before.presentations,
            "resize_and_minimize_restore":"passed",
            "startup_readback_bytes":display_start.readback_bytes,
            "display_readback_bytes":service.diagnostics().readback_bytes - display_start.readback_bytes,
            "measurement":"Posted WM_KEYDOWN -> GPUI native dispatch -> CanvasView edit -> Present1 return. Excludes physical scanout; GPU retirement is measured by gpu_profile.",
            "reconstruction_measurement":"Optional 512x384 source-grid GPU reconstruction shares the window device. Ready batches yield to the event loop; completion events wake the owner where native fences are supported, with timer polling otherwise. Source/mask preparation precedes posting; initial allocation is reported separately. This isolates compute coexistence, not the complete CanvasView Smart Erase preparation pipeline.",
            "samples":samples,
        });
        std::fs::write(output, serde_json::to_vec_pretty(&report)?)?;
        Ok(())
    }

    pub fn run() -> Result<()> {
        let output = std::env::args_os()
            .nth(1)
            .context("output JSON path")?
            .into();
        let mode = std::env::args().nth(2).unwrap_or_else(|| "idle".into());
        ensure!(
            matches!(
                mode.as_str(),
                "idle"
                    | "smart-erase"
                    | "smart-erase-polling"
                    | "view-texture"
                    | "view-layered"
                    | "view-affine"
                    | "view-periodic"
                    | "view-texture-compare"
                    | "view-layered-compare"
            ),
            "Unknown native benchmark mode"
        );
        let (mut document, selected) = fixture()?;
        let erasure = view_erase::Fixture::new(&mode)?;
        if let Some(fixture) = &erasure {
            document.set_background(Some(fixture.source.clone()));
        }
        let failed = Arc::new(AtomicBool::new(false));
        let exit_failed = failed.clone();
        let title = format!("Snow native GPU input benchmark {}", std::process::id());
        gpui_platform::application().run(move |cx| {
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                            None,
                            size(px(1280.0), px(720.0)),
                            cx,
                        ))),
                        titlebar: Some(TitlebarOptions {
                            title: Some(title.clone().into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    |window, cx| {
                        cx.new(|cx| BenchView {
                            canvas: cx.new(|cx| {
                                CanvasView::new(document, window, cx)
                                    .expect("canvas initialization")
                            }),
                            input: Rc::default(),
                            erasure,
                        })
                    },
                )
                .expect("native benchmark window");
            cx.activate(true);
            cx.spawn(async move |cx| {
                if let Err(error) = measure(window, selected, title, output, mode, cx).await {
                    failed.store(true, Ordering::Relaxed);
                    eprintln!("Native input benchmark failed: {error:#}");
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
        ensure!(
            !exit_failed.load(Ordering::Relaxed),
            "native input benchmark failed"
        );
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    native::run()
}
#[cfg(not(target_os = "windows"))]
fn main() {
    panic!("native_input currently requires the Windows GPU backend");
}
