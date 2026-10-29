//! Separate cold, interactive, reconstruction and streaming-export measurements.
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::Poll,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use gpui::GpuService;
use gpui_windows::gpu_extensions::{DirectXGpuService, OffscreenGpuHost};
use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use snow_draw_engine_gpui::{
    CanvasDocument, CanvasRect, CanvasSession, CanvasSnapshot, GpuCanvas, GpuExportFormat,
    GpuExportJob, SmartEraseJob, SmartEraseSnapshot, SourceImage, engine::*, prepare_text_metrics,
};

fn complete(service: &Arc<dyn GpuService>) -> Result<()> {
    let native = service
        .as_any()
        .downcast_ref::<DirectXGpuService>()
        .context("D3D11 service")?;
    let mut completion = native.completion()?;
    let deadline = Instant::now() + Duration::from_secs(60);
    while !completion.poll()? {
        ensure!(Instant::now() < deadline, "GPU completion timed out");
        std::thread::yield_now();
    }
    Ok(())
}

fn fixture(size: [u32; 2], operations: Vec<Value>) -> Result<(CanvasDocument, CanvasSession)> {
    let mut document = CanvasDocument::default();
    let mut session = CanvasSession::new(&mut document)?;
    session.set_surface(&mut document, size[0], size[1])?;
    session.set_camera(
        &mut document,
        Camera {
            center: Point::new(f64::from(size[0]) / 2.0, f64::from(size[1]) / 2.0),
            zoom: 1.0,
        },
    )?;
    session.set_deferred_composition(true);
    for operations in operations.chunks(128) {
        let batch = serde_json::to_vec(&json!({"version":1,"operations":operations}))?;
        document.command(|engine| engine.apply_annotation_json(&batch))?;
    }
    prepare_text_metrics(&mut document, &mut session)?;
    Ok((document, session))
}

fn render(
    canvas: &mut GpuCanvas,
    document: &CanvasDocument,
    session: &CanvasSession,
    smart: &SmartEraseSnapshot,
) -> Result<Value> {
    let start = Instant::now();
    let frame = canvas.prepare(
        session.scene(),
        document.background(),
        document.resource_revision(),
        1.0,
        &Default::default(),
        smart,
        &Default::default(),
    )?;
    complete(&canvas.service())?;
    Ok(
        json!({"prepare_to_gpu_completion_ms":start.elapsed().as_secs_f64()*1000.0,
        "submission":frame.profile,"completed_gpu":canvas.take_completed_profiles()?}),
    )
}

fn percentile(values: &[Value], field: &str) -> Option<f64> {
    let mut times: Vec<_> = values.iter().filter_map(|v| v[field].as_f64()).collect();
    times.sort_by(f64::total_cmp);
    (!times.is_empty()).then(|| times[(times.len() * 95).div_ceil(100) - 1])
}

fn dense_text(service: Arc<dyn GpuService>) -> Result<Value> {
    let setup = Instant::now();
    let operations = (0..1000)
        .map(|index| {
            json!({"type":"text",
        "bounds":[(index%40)*48,(index/40)*40,48,38],"text":format!("{index:04} 漢A"),
        "style":{"font_size":10,"stroke":[30,50,90,255]}})
        })
        .collect();
    let (mut document, mut session) = fixture([1920, 1080], operations)?;
    let setup_ms = setup.elapsed().as_secs_f64() * 1000.0;
    let mut canvas = GpuCanvas::new(service)?;
    canvas.set_profiling(true);
    let cold = render(&mut canvas, &document, &session, &Default::default())?;
    let id = *session
        .scene()
        .order_ids()
        .first()
        .context("text fixture")?;
    session.mutate(&mut document, |engine, viewport| {
        engine.select_element_with_viewport_changes(
            viewport,
            ElementId {
                index: id.element.index,
                generation: id.element.generation,
            },
        )
    })?;
    session.flush(&document)?;
    let mut edits = Vec::new();
    for index in 0..12 {
        let start = Instant::now();
        session.mutate(&mut document, |engine, viewport| {
            engine.set_selected_opacity_with_viewport_changes(
                viewport,
                if index % 2 == 0 { 0.6 } else { 1.0 },
            )
        })?;
        session.flush(&document)?;
        let mut result = render(&mut canvas, &document, &session, &Default::default())?;
        result["input_to_gpu_completion_ms"] = json!(start.elapsed().as_secs_f64() * 1000.0);
        edits.push(result);
    }
    Ok(
        json!({"workload":"dense_text","size":[1920,1080],"text_nodes":1000,
        "setup_and_measurement_ms":setup_ms,"cold":cold,
        "warm_p95_ms":percentile(&edits[2..],"input_to_gpu_completion_ms"),"edits":edits}),
    )
}

fn long_stroke(service: Arc<dyn GpuService>) -> Result<Value> {
    let points: Vec<_> = (0..8192)
        .map(|index| {
            [
                40.0 + f64::from(index) * 1800.0 / 8191.0,
                540.0 + (f64::from(index) * 0.015).sin() * 180.0,
            ]
        })
        .collect();
    let anchor = points[4096];
    let (mut document, mut session) = fixture(
        [1920, 1080],
        vec![json!({"type":"freehand","points":points,
        "style":{"stroke":[30,90,180,255],"stroke_width":3}})],
    )?;
    let id = *session
        .scene()
        .order_ids()
        .first()
        .context("stroke fixture")?;
    session.set_tool(&mut document, ActiveTool::Select)?;
    session.mutate(&mut document, |engine, viewport| {
        engine.select_element_with_viewport_changes(
            viewport,
            ElementId {
                index: id.element.index,
                generation: id.element.generation,
            },
        )
    })?;
    session.flush(&document)?;
    let mut canvas = GpuCanvas::new(service)?;
    canvas.set_profiling(true);
    let cold = render(&mut canvas, &document, &session, &Default::default())?;
    let mut edits = Vec::new();
    for index in 0..16 {
        let before = session.scene().node(id).context("retained stroke")?.clone();
        let start = Instant::now();
        let (from, to) = if index % 2 == 0 {
            (0.0, 12.0)
        } else {
            (12.0, 0.0)
        };
        for (event_type, offset) in [
            (PointerEventType::Down, from),
            (PointerEventType::Move, to),
            (PointerEventType::Up, to),
        ] {
            session.input(
                &mut document,
                InputEvent::Pointer(PointerEvent {
                    pointer_id: 1,
                    event_type,
                    device: PointerDevice::Mouse,
                    position: Point::new(anchor[0] + offset, anchor[1]),
                    button: matches!(event_type, PointerEventType::Down | PointerEventType::Up)
                        .then_some(PointerButton::Primary),
                    buttons: PointerButtons(if event_type == PointerEventType::Up {
                        0
                    } else {
                        PointerButtons::PRIMARY
                    }),
                    modifiers: Default::default(),
                    pressure: None,
                    timestamp_ms: 0,
                }),
            )?;
        }
        session.flush(&document)?;
        let after = session.scene().node(id).context("retained moved stroke")?;
        ensure!(
            after.pose_revision != before.pose_revision,
            "Pointer drag did not move the stroke"
        );
        ensure!(
            after.geometry_revision == before.geometry_revision,
            "Rigid stroke motion rebuilt logical geometry"
        );
        let mut result = render(&mut canvas, &document, &session, &Default::default())?;
        ensure!(
            result["submission"]["mesh_upload_bytes"] == 0,
            "Rigid stroke motion uploaded unchanged meshes"
        );
        result["input_to_gpu_completion_ms"] = json!(start.elapsed().as_secs_f64() * 1000.0);
        edits.push(result);
    }
    Ok(
        json!({"workload":"long_stroke_pose","size":[1920,1080],"points":8192,"cold":cold,
        "warm_p95_ms":percentile(&edits[2..],"input_to_gpu_completion_ms"),"edits":edits}),
    )
}

fn pan_zoom(service: Arc<dyn GpuService>) -> Result<Value> {
    let operations = (0..1200)
        .map(|index| {
            json!({"type":"rectangle","bounds":[(index%40)*48,(index/40)*36,34,22],
        "style":{"fill":[30,100,190,180],"stroke_width":1}})
        })
        .collect();
    let (mut document, mut session) = fixture([1920, 1080], operations)?;
    let mut canvas = GpuCanvas::new(service)?;
    canvas.set_profiling(true);
    let cold = render(&mut canvas, &document, &session, &Default::default())?;
    let mut edits = Vec::new();
    for index in 0..16 {
        let start = Instant::now();
        session.set_camera(
            &mut document,
            Camera {
                center: Point::new(960.0 + f64::from(index % 4) * 4.0, 540.0),
                zoom: 1.0 + f64::from(index % 4) * 0.02,
            },
        )?;
        session.flush(&document)?;
        let mut result = render(&mut canvas, &document, &session, &Default::default())?;
        result["input_to_gpu_completion_ms"] = json!(start.elapsed().as_secs_f64() * 1000.0);
        edits.push(result);
    }
    Ok(
        json!({"workload":"pan_zoom","size":[1920,1080],"nodes":1200,"cold":cold,
        "warm_p95_ms":percentile(&edits[2..],"input_to_gpu_completion_ms"),"edits":edits}),
    )
}

fn overlapping_filters(service: Arc<dyn GpuService>) -> Result<Value> {
    let mut operations: Vec<_> = (0..96)
        .map(|index| {
            json!({"type":"rectangle","bounds":[(index%12)*100,(index/12)*84,82,64],
        "style":{"fill":[30+(index%4)*40,100,180,220],"stroke_width":1}})
        })
        .collect();
    for (index, filter) in [
        "gaussian_blur",
        "mosaic",
        "grayscale",
        "gaussian_blur",
        "inversion",
        "brightness",
    ]
    .into_iter()
    .enumerate()
    {
        operations.push(json!({"type":"rectangle_filter","bounds":[140+index*24,100+index*20,760,440],"style":{"filter":filter,"strength":0.15}}));
    }
    let (mut document, mut session) = fixture([1280, 720], operations)?;
    let source = SourceImage::new(RgbaImage::from_fn(1280, 720, |x, y| {
        Rgba([(x % 251) as u8, (y % 241) as u8, 110, 255])
    }))?;
    document.set_background(Some(source));
    let id = session
        .scene()
        .nodes()
        .find(|(_, node)| {
            matches!(node.item.as_ref(), SceneDisplayItem::Rectangle(_))
                && (390.0..500.0).contains(&node.bounds[0])
                && (240.0..340.0).contains(&node.bounds[1])
        })
        .context("underlying rectangle")?
        .0;
    let id = ElementId {
        index: id.element.index,
        generation: id.element.generation,
    };
    session.mutate(&mut document, |engine, viewport| {
        engine.select_element_with_viewport_changes(viewport, id)
    })?;
    session.flush(&document)?;
    let mut canvas = GpuCanvas::new(service)?;
    canvas.set_profiling(true);
    let cold = render(&mut canvas, &document, &session, &Default::default())?;
    let mut edits = Vec::new();
    for index in 0..8 {
        let start = Instant::now();
        session.mutate(&mut document, |engine, viewport| {
            engine.set_selected_opacity_with_viewport_changes(
                viewport,
                if index % 2 == 0 { 0.6 } else { 1.0 },
            )
        })?;
        session.flush(&document)?;
        let mut result = render(&mut canvas, &document, &session, &Default::default())?;
        result["input_to_gpu_completion_ms"] = json!(start.elapsed().as_secs_f64() * 1000.0);
        edits.push(result);
    }
    Ok(
        json!({"workload":"overlapping_filters","size":[1280,720],"filters":6,"cold":cold,
        "warm_p95_ms":percentile(&edits[2..],"input_to_gpu_completion_ms"),"edits":edits}),
    )
}

fn smart_completion(service: Arc<dyn GpuService>) -> Result<Value> {
    let (mut document, mut session) = fixture(
        [512, 384],
        vec![
            json!({"type":"rectangle_filter","bounds":[240,176,32,32],"style":{"filter":"gaussian_blur","strength":0.5}}),
        ],
    )?;
    document.set_background(Some(SourceImage::new(RgbaImage::from_fn(
        512,
        384,
        |x, y| {
            Rgba(if (240..272).contains(&x) && (176..208).contains(&y) {
                [240, 5, 9, 255]
            } else {
                [70, 120, 180, 255]
            })
        },
    ))?));
    let id = *session.scene().order_ids().first().context("smart mask")?;
    session.mutate(&mut document, |engine, viewport| {
        engine.select_element_with_viewport_changes(
            viewport,
            ElementId {
                index: id.element.index,
                generation: id.element.generation,
            },
        )
    })?;
    let mut style = document
        .engine()
        .viewport_style_toolbar_state(session.viewport())
        .map_err(snow_draw_engine_gpui::CanvasError::from)?
        .filter_style;
    style.filter_type = CanvasFilterType::SmartErase;
    session.mutate(&mut document, |engine, viewport| {
        engine.set_viewport_filter_style(viewport, style, FILTER_STYLE_PROPERTY_ALL)
    })?;
    session.flush(&document)?;
    let filters: Vec<_> = session
        .scene()
        .nodes()
        .filter_map(|(_, node)| {
            if let SceneDisplayItem::Filter(filter) = node.item.as_ref() {
                Some(filter.clone())
            } else {
                None
            }
        })
        .collect();
    let job = SmartEraseJob::new(
        1,
        document.background().cloned(),
        filters,
        Default::default(),
    );
    let (cpu_ms, results) = std::thread::spawn(move || {
        let start = Instant::now();
        let result = job.run(&Default::default());
        (start.elapsed().as_secs_f64() * 1000.0, result)
    })
    .join()
    .map_err(|_| anyhow::anyhow!("Smart Erase worker panicked"))?;
    let results = results?;
    ensure!(
        results.warnings.is_empty(),
        "Smart Erase warnings: {:?}",
        results.warnings
    );
    let mut canvas = GpuCanvas::new(service)?;
    canvas.set_profiling(true);
    let before = render(&mut canvas, &document, &session, &Default::default())?;
    let completion = render(&mut canvas, &document, &session, &results.snapshot)?;
    Ok(
        json!({"workload":"smart_erase_completion","size":[512,384],"cpu_worker_solve_ms":cpu_ms,
        "patch_count":results.snapshot.result_count(),"patch_bytes":results.snapshot.retained_bytes(),"unresolved_frame":before,"completed_frame":completion}),
    )
}

struct CountingWriter(Arc<AtomicUsize>);
impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.fetch_add(bytes.len(), Ordering::Relaxed);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn large_export(service: Arc<dyn GpuService>) -> Result<Value> {
    let (mut document, session) = fixture([2048, 1024], Vec::new())?;
    document.set_background(Some(SourceImage::new(RgbaImage::from_fn(
        2048,
        1024,
        |x, y| Rgba([(x % 251) as u8, (y % 241) as u8, 110, 220]),
    ))?));
    let snapshot = CanvasSnapshot::capture(&document, &session)?
        .with_region(CanvasRect::new(0.0, 0.0, 2048.0, 1024.0)?, [8192, 4096])?;
    let bytes = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();
    let mut job = GpuExportJob::to_writer(
        service.clone(),
        snapshot,
        1.0,
        Default::default(),
        GpuExportFormat::Png,
        CountingWriter(bytes.clone()),
        Default::default(),
    )?;
    let readback_before = service.diagnostics();
    let mut peak_staging = 0;
    let mut polls = 0;
    loop {
        let result = job.poll();
        polls += 1;
        peak_staging = peak_staging.max(service.diagnostics().pending_readback_bytes);
        if let Poll::Ready(result) = result {
            result?;
            break;
        }
        ensure!(
            start.elapsed() < Duration::from_secs(120),
            "Streaming export timed out"
        );
        std::thread::yield_now();
    }
    Ok(
        json!({"workload":"large_streaming_export","output_size":[8192,4096],"decoded_output_bytes":8192_u64*4096*4,
        "gpu_readback_and_encode_ms":start.elapsed().as_secs_f64()*1000.0,"encoded_bytes":bytes.load(Ordering::Relaxed),"peak_pending_readback_bytes":peak_staging,"readback_bytes":service.diagnostics().readback_bytes-readback_before.readback_bytes,"staging_allocations":service.diagnostics().readback_allocations-readback_before.readback_allocations,"staging_reuses":service.diagnostics().reused_readbacks-readback_before.reused_readbacks,"owner_poll_count":polls,
        "full_output_rgba_allocated":false,"poll_policy":"owner thread yields between nonblocking polls; no UI timer included"}),
    )
}

fn main() -> Result<()> {
    let destination = std::env::args().nth(1).context("output JSON path")?;
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let capabilities = service.capabilities();
    let mut cases = Vec::new();
    // Errors are recorded with their group, without presenting incomplete timings as success.
    for (name, run) in [
        (
            "dense_text",
            dense_text as fn(Arc<dyn GpuService>) -> Result<Value>,
        ),
        ("long_stroke_pose", long_stroke),
        ("pan_zoom", pan_zoom),
        ("overlapping_filters", overlapping_filters),
        ("smart_erase_completion", smart_completion),
        ("large_streaming_export", large_export),
    ] {
        cases.push(match run(service.clone()) {
            Ok(value) => value,
            Err(error) => json!({"workload":name,"error":format!("{error:#}")}),
        });
    }
    let failed = cases.iter().any(|case| case.get("error").is_some());
    std::fs::write(
        destination,
        serde_json::to_vec_pretty(&json!({"profile":"canvas-perf",
        "adapter":{"name":capabilities.adapter_name.as_ref(),"vendor_id":capabilities.vendor_id,"device_id":capabilities.device_id},
        "native_input_to_present":"not measured by this offscreen benchmark","cases":cases}))?,
    )?;
    ensure!(
        !failed,
        "One or more GPU workloads failed; see the output JSON"
    );
    Ok(())
}
