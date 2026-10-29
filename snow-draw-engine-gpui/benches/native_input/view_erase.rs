use super::*;
use snow_draw_engine_gpui::{
    CanvasRect, ImageSourceLayer, SmartEraseExecution, SmartEraseProfile, SmartEraseService,
    SmartEraseSolver,
};

pub(super) struct Fixture {
    pub source: SourceImage,
    mode: String,
    mask: [u32; 2],
}
impl Fixture {
    pub fn new(mode: &str) -> Result<Option<Self>> {
        if !mode.starts_with("view-") {
            return Ok(None);
        }
        let dimension = |index, default| -> Result<u32> {
            std::env::args()
                .nth(index)
                .filter(|arg| arg != "--bench")
                .map(|arg| arg.parse().context("native mask dimension"))
                .transpose()
                .map(|value| value.unwrap_or(default))
        };
        let mask = [dimension(3, 128)?, dimension(4, 96)?];
        ensure!(
            (1..=256).contains(&mask[0]) && (1..=192).contains(&mask[1]),
            "native mask exceeds fixture bounds"
        );
        let kind = mode.strip_suffix("-compare").unwrap_or(mode);
        let source = SourceImage::new(image::RgbaImage::from_fn(512, 384, |x, y| {
            let n = (((x * 17 + y * 31) ^ (x * y)) % 49) as u8;
            image::Rgba(match kind {
                "view-affine" => [70, 120, 180, 255],
                "view-periodic" => {
                    let n = ((x / 8 + y / 8) % 2 * 24) as u8;
                    [140 + n, 160 + n, 180 + n, 255]
                }
                _ => [90 + n, 120 + n, 60 + n, 255],
            })
        }))?;
        let source = if kind == "view-layered" {
            SourceImage::from_layers(&[
                ImageSourceLayer::new(source),
                ImageSourceLayer {
                    image: SourceImage::new(image::RgbaImage::from_fn(257, 193, |x, y| {
                        let n = ((x * 31 + y * 13) % 43) as u8;
                        image::Rgba([70 + n, 110 + n, 150 + n, 128])
                    }))?,
                    canvas_rect: Some(CanvasRect::new(0.25, 0.5, 511.5, 383.25)?),
                    coverage: None,
                    smart_erase_source: true,
                },
            ])?
            .context("layered native fixture")?
        } else {
            source
        };
        Ok(Some(Self {
            source,
            mode: mode.into(),
            mask,
        }))
    }
    fn filter(&self, index: u64) -> draw::FilterDisplayItem {
        // Each comparison pair has identical geometry and distinct element IDs.
        let index = if self.comparison() {
            (index - 1) / 2
        } else {
            index
        };
        draw::FilterDisplayItem {
            center_x: 192.0 + (index % 64) as f64 + f64::from(self.mask[0]) / 2.0,
            center_y: 144.0 + (index / 64) as f64 + f64::from(self.mask[1]) / 2.0,
            width: f64::from(self.mask[0]),
            height: f64::from(self.mask[1]),
            opacity: 1.0,
            filter: draw::FilterRenderSpec::resolve(draw::DisplayFilterType::SmartErase, 0.5),
            ..Default::default()
        }
    }
    fn comparison(&self) -> bool {
        self.mode.ends_with("-compare")
    }
    fn execution(&self, index: u64) -> SmartEraseExecution {
        // Two successive samples of each mode balance ordering and frame phase.
        if self.comparison() && !(index / 2).is_multiple_of(2) {
            SmartEraseExecution::Cpu
        } else if self.comparison() {
            SmartEraseExecution::GpuPreferred
        } else {
            SmartEraseExecution::Automatic
        }
    }
    pub fn apply(
        &self,
        canvas: &mut CanvasView,
        index: u64,
        window: &mut Window,
        cx: &mut gpui::Context<CanvasView>,
    ) {
        let execution = self.execution(index);
        if canvas.options().smart_erase_execution != execution {
            let mut options = canvas.options().clone();
            options.smart_erase_execution = execution;
            canvas
                .set_options(options, window, cx)
                .expect("native reconstruction policy");
        }
        let previous = canvas.session().scene().nodes().find_map(|(_, node)| {
            matches!(node.item.as_ref(), draw::SceneDisplayItem::Filter(_)).then_some(
                draw::ElementId {
                    index: node.id.element.index,
                    generation: node.id.element.generation,
                },
            )
        });
        let filter = self.filter(index);
        let mut operations = Vec::new();
        if let Some(id) = previous {
            operations.push(serde_json::json!({"type":"delete", "ids":[id]}));
        }
        // The annotation interface intentionally omits Smart Erase. Create an
        // ordinary rectangle filter and convert it with the public style command.
        operations.push(serde_json::json!({"type":"rectangle_filter", "bounds":[filter.center_x-filter.width/2.0,filter.center_y-filter.height/2.0,filter.width,filter.height]}));
        let bytes =
            serde_json::to_vec(&serde_json::json!({"version":1,"operations":operations})).unwrap();
        canvas.command(
            |engine, viewport| {
                let (_, created) = engine.apply_annotation_json(&bytes)?;
                let created: serde_json::Value =
                    serde_json::from_slice(&created).map_err(|_| draw::ErrorCode::Internal)?;
                let id = serde_json::from_value(created["created_element_ids"][0].clone())
                    .map_err(|_| draw::ErrorCode::Internal)?;
                engine.select_element_with_viewport_changes(viewport, id)?;
                let mut style = engine.viewport_style_toolbar_state(viewport)?.filter_style;
                style.filter_type = draw::CanvasFilterType::SmartErase;
                engine.set_viewport_filter_style(viewport, style, draw::FILTER_STYLE_PROPERTY_ALL)
            },
            window,
            cx,
        );
    }
}

pub(super) async fn measure(
    window: WindowHandle<BenchView>,
    service: &Arc<dyn GpuService>,
    input: Rc<RefCell<InputState>>,
    title: &str,
    output: std::path::PathBuf,
    cx: &mut AsyncApp,
) -> Result<()> {
    let handle = native_handle(title)?;
    let initial = service.diagnostics();
    let (logical_size, scale, refresh) = window.update(cx, |_, window, cx| {
        (
            window.viewport_size(),
            window.scale_factor(),
            window.refresh_interval(cx),
        )
    })?;
    let (comparison, mask, cpu_reference) = window.read_with(cx, |root, _| -> Result<_> {
        let fixture = root.erasure.as_ref().unwrap();
        let mut profile = SmartEraseProfile::default();
        if fixture.comparison() {
            SmartEraseService::default().solve_profiled(
                &fixture.filter(1),
                &fixture.source,
                &Default::default(),
                &mut profile,
            )?;
            ensure!(
                profile.solver == Some(SmartEraseSolver::Multiscale),
                "texture comparison must exercise CPU multiscale reconstruction"
            );
        }
        Ok((
            fixture.comparison(),
            fixture.mask,
            fixture.comparison().then_some(profile),
        ))
    })??;
    let warmup = if comparison { 16 } else { 8 };
    let measured = if comparison { 128 } else { 64 };
    let mut samples = Vec::new();
    for index in 1..=(warmup + measured) as u64 {
        let execution = window.read_with(cx, |root, _| {
            root.erasure.as_ref().unwrap().execution(index)
        })?;
        let before = service.diagnostics();
        let posted = WallInstant::now();
        unsafe {
            PostMessageW(Some(handle), WM_KEYDOWN, WPARAM(0x87), LPARAM(1))?;
            PostMessageW(Some(handle), WM_KEYUP, WPARAM(0x87), LPARAM(0xc0000001))?;
        }
        let deadline = posted + Duration::from_secs(30);
        let (after, gpu, observed) = loop {
            let state = window.read_with(cx, |root, cx| {
                let canvas = root.canvas.read(cx);
                let fixture = root.erasure.as_ref().unwrap();
                let expected = fixture.filter(index);
                let results = canvas.smart_erase_snapshot();
                let patch = canvas.session().scene().nodes().find_map(|(_, node)| {
                    if let draw::SceneDisplayItem::Filter(filter) = node.item.as_ref()
                        && filter.center_x == expected.center_x
                        && filter.center_y == expected.center_y
                        && filter.width == expected.width
                        && filter.height == expected.height
                    {
                        results.patch(filter, &fixture.source)
                    } else {
                        None
                    }
                });
                (
                    canvas.is_rendering(),
                    patch,
                    canvas.error().map(ToString::to_string),
                    results.result_count(),
                )
            })?;
            ensure!(state.2.is_none(), "Canvas error: {:?}", state.2);
            let diagnostics = service.diagnostics();
            if input.borrow().handled >= index
                && !state.0
                && let Some(patch) = state.1
            {
                ensure!(
                    diagnostics.presentations > before.presentations,
                    "result was not presented"
                );
                ensure!(
                    state.3 == index as usize,
                    "comparison reused a completed patch or evicted its bounded fixture"
                );
                break (
                    diagnostics,
                    patch.reconstructed().gpu_texture().is_some(),
                    WallInstant::now(),
                );
            }
            if WallInstant::now() >= deadline {
                let detail = window.read_with(cx, |root, cx| {
                    let canvas = root.canvas.read(cx);
                    let filters: Vec<_> = canvas
                        .session()
                        .scene()
                        .nodes()
                        .filter_map(|(_, n)| {
                            if let draw::SceneDisplayItem::Filter(f) = n.item.as_ref() {
                                Some(f.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    format!(
                        "{} results={} filters={filters:?} expected={:?}",
                        canvas.render_diagnostics(),
                        canvas.smart_erase_snapshot().result_count(),
                        root.erasure.as_ref().unwrap().filter(index)
                    )
                })?;
                anyhow::bail!(
                    "full-view erasure timed out: sample={index}, handled={}, {detail}",
                    input.borrow().handled
                );
            }
            cx.background_executor()
                .timer(Duration::from_millis(1))
                .await;
        };
        let callback = input
            .borrow()
            .callback_started
            .context("erasure callback")?;
        let end = after.last_presentation_end.context("result presentation")?;
        ensure!(end >= callback, "result presentation preceded input");
        samples.push(serde_json::json!({
            "native_queue_to_result_present_ms":end.duration_since(posted).as_secs_f64()*1000.0,
            "native_callback_to_result_present_ms":end.duration_since(callback).as_secs_f64()*1000.0,
            "completion_observed_ms":observed.duration_since(posted).as_secs_f64()*1000.0,
            "gpu_result":gpu,
            "requested_execution":match execution {SmartEraseExecution::Cpu=>"cpu",SmartEraseExecution::GpuPreferred=>"gpu_preferred",SmartEraseExecution::Automatic=>"automatic"},
            "paired_mask":comparison.then_some((index-1)/2),
            "presentations":after.presentations-before.presentations,
        }));
    }
    let (mode, retained) = window.read_with(cx, |root, cx| {
        (
            root.erasure.as_ref().unwrap().mode.clone(),
            root.canvas.read(cx).smart_erase_snapshot().retained_bytes(),
        )
    })?;
    let expect_gpu = mode.starts_with("view-layered")
        || (mode.starts_with("view-texture")
            && (comparison || mask[0] > 16 || mask[1] > 16 || !cfg!(target_arch = "x86_64")));
    ensure!(
        samples
            .iter()
            .all(|s| s["gpu_result"] == (expect_gpu && s["requested_execution"] != "cpu")),
        "unexpected full-view reconstruction backend"
    );
    let mut times: Vec<_> = samples
        .iter()
        .skip(warmup)
        .map(|s| s["native_queue_to_result_present_ms"].as_f64().unwrap())
        .collect();
    let median = percentile(&mut times, 50);
    let p95 = percentile(&mut times, 95);
    let mut comparison_results = Vec::new();
    let mut paired_differences = Vec::new();
    if comparison {
        for backend in ["cpu", "gpu_preferred"] {
            let mut times: Vec<_> = samples
                .iter()
                .skip(warmup)
                .filter(|s| s["requested_execution"] == backend)
                .map(|s| s["native_queue_to_result_present_ms"].as_f64().unwrap())
                .collect();
            ensure!(times.len() == 64, "unbalanced native backend comparison");
            let median = percentile(&mut times, 50);
            let p95 = percentile(&mut times, 95);
            comparison_results.push(serde_json::json!({"execution":backend,"samples":times.len(),"median_ms":median,"p95_ms":p95}));
        }
        for pair in samples[warmup..].as_chunks::<2>().0 {
            let cpu = pair
                .iter()
                .find(|s| s["requested_execution"] == "cpu")
                .context("paired CPU sample")?;
            let gpu = pair
                .iter()
                .find(|s| s["requested_execution"] == "gpu_preferred")
                .context("paired GPU sample")?;
            ensure!(
                cpu["paired_mask"] == gpu["paired_mask"],
                "comparison geometry differs"
            );
            paired_differences.push(
                gpu["native_queue_to_result_present_ms"].as_f64().unwrap()
                    - cpu["native_queue_to_result_present_ms"].as_f64().unwrap(),
            );
        }
    }
    // Allow the last nonblocking GPU-retirement notification to settle.
    cx.background_executor()
        .timer(Duration::from_millis(100))
        .await;
    let idle = service.diagnostics();
    cx.background_executor()
        .timer(Duration::from_millis(200))
        .await;
    let after = service.diagnostics();
    ensure!(
        after.presentations == idle.presentations,
        "idle erasure window kept presenting"
    );
    ensure!(
        after.readback_bytes == initial.readback_bytes,
        "full-view editing read back images"
    );
    let capabilities = service.capabilities();
    let report = serde_json::json!({
        "profile":"canvas-perf", "load":mode, "elements":10_000, "source_size":[512,384], "mask_size":mask,
        "warmup_samples":warmup, "measured_samples":measured,
        "adapter":{"name":capabilities.adapter_name.as_ref(),"vendor_id":capabilities.vendor_id,"device_id":capabilities.device_id},
        "cpu":std::env::var("PROCESSOR_IDENTIFIER").ok(),
        "logical_size":[f32::from(logical_size.width),f32::from(logical_size.height)],"dpi_scale":scale,
        "physical_size":[f32::from(logical_size.width)*scale,f32::from(logical_size.height)*scale],
        "refresh_interval_ms":refresh.map(|interval|interval.as_secs_f64()*1000.0),
        "native_queue_to_result_present_p50_ms":median,"native_queue_to_result_present_p95_ms":p95,
        "idle_presentations":after.presentations-idle.presentations,"display_readback_bytes":after.readback_bytes-initial.readback_bytes,
        "retained_result_bytes":retained,
        "cpu_reference":cpu_reference,"backend_comparison":comparison_results,
        "paired_gpu_minus_cpu_p50_ms":if comparison {Some(percentile(&mut paired_differences,50))} else {None},
        "measurement":"Posted F24 -> native dispatch -> replace one rectangle filter -> CanvasView worker preparation, GPU/CPU reconstruction, ready result publication and Present1 return. Sample 1 is the first view solve in this process with existing driver caches retained. Comparison pairs share exact mask geometry and have distinct element IDs; CPU/GPU order alternates by pair. Earlier results remain in the ordinary bounded cache, whose count is checked to reject result-cache hits. Fixed source and 10,000 shapes; excludes physical scanout. Completion observation uses 1 ms timer requests and is reported separately.",
        "samples":samples,
    });
    std::fs::write(output, serde_json::to_vec_pretty(&report)?)?;
    Ok(())
}
