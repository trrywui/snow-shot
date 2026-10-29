use anyhow::{Context, Result, ensure};
use gpui::GpuService;
use gpui_windows::gpu_extensions::{DirectXGpuService, OffscreenGpuHost};
use snow_draw_engine_gpui::{CanvasDocument, CanvasSession, GpuCanvas, engine::*};
use std::{
    sync::Arc,
    time::{Duration, Instant},
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Opacity,
    Pose,
}

impl EditKind {
    fn label(self) -> &'static str {
        match self {
            Self::Opacity => "opacity",
            Self::Pose => "pose",
        }
    }
}

fn fixture(width: u32, height: u32) -> Result<(CanvasDocument, CanvasSession, RetainedNodeId)> {
    let mut document = CanvasDocument::default();
    let mut session = CanvasSession::new(&mut document)?;
    session.set_surface(&mut document, width, height)?;
    session.set_camera(
        &mut document,
        Camera {
            center: Point::new(width as f64 / 2.0, height as f64 / 2.0),
            zoom: 1.0,
        },
    )?;
    session.set_deferred_composition(true);
    let spacing = if (width, height) == (960, 640) {
        [9.0, 6.0]
    } else {
        [width as f64 / 100.0, height as f64 / 100.0]
    };
    for start in (0..10_000).step_by(128) {
        let operations:Vec<_>=(start..(start+128).min(10_000)).map(|i|serde_json::json!({
            "type":"rectangle","bounds":[(i%100) as f64*spacing[0],(i/100) as f64*spacing[1],5,3],
            "style":{"fill":[40,100,190,255],"stroke_width":1}
        })).collect();
        document.command(|engine| {
            engine.apply_annotation_json(
                &serde_json::to_vec(&serde_json::json!({"version":1,"operations":operations}))
                    .unwrap(),
            )
        })?;
    }
    session.flush(&document)?;
    let node = session
        .scene()
        .nodes()
        .find(|(id, node)| {
            id.element.index == 5050 && matches!(node.item.as_ref(), SceneDisplayItem::Rectangle(_))
        })
        .context("rectangle fixture")?
        .1;
    let node_id = node.id;
    let id = ElementId {
        index: node_id.element.index,
        generation: node_id.element.generation,
    };
    session.mutate(&mut document, |engine, viewport| {
        engine.select_element_with_viewport_changes(viewport, id)
    })?;
    session.flush(&document)?;
    Ok((document, session, node_id))
}

fn rectangle(scene: &CanvasSession, id: RetainedNodeId) -> Result<RectangleDisplayItem> {
    match scene
        .scene()
        .node(id)
        .context("retained rectangle")?
        .item
        .as_ref()
    {
        SceneDisplayItem::Rectangle(rectangle) => Ok(*rectangle),
        _ => anyhow::bail!("Fixture node is no longer a rectangle"),
    }
}

fn apply_edit(
    document: &mut CanvasDocument,
    session: &mut CanvasSession,
    id: RetainedNodeId,
    initial: RectangleDisplayItem,
    edit_kind: EditKind,
    index: usize,
) -> Result<()> {
    let before = session
        .scene()
        .node(id)
        .context("rectangle before edit")?
        .clone();
    match edit_kind {
        EditKind::Opacity => {
            session.mutate(document, |engine, viewport| {
                engine.set_selected_opacity_with_viewport_changes(
                    viewport,
                    if index.is_multiple_of(2) { 0.5 } else { 1.0 },
                )
            })?;
        }
        EditKind::Pose => {
            let current = rectangle(session, id)?;
            // Tiny selected rectangles have overlapping resize handles. The
            // existing secondary-button gesture moves their body directly.
            // Use bounds origin + [0.5, 0.5] so dense neighbors' inclusive
            // stroke hit tolerances cannot win the initial hit test.
            let from = Point::new(
                current.center_x - current.width / 2.0 + 0.5,
                current.center_y - current.height / 2.0 + 0.5,
            );
            let to = Point::new(
                initial.center_x - initial.width / 2.0
                    + 0.5
                    + if index.is_multiple_of(2) { 1.0 } else { 2.0 },
                from.y,
            );
            // Cross the real 4 px drag threshold, then finish at the desired
            // 1/2 px offset. Apply every input sample before one publication.
            for (sample, (event_type, position)) in [
                (PointerEventType::Down, from),
                (PointerEventType::Move, Point::new(from.x + 8.0, from.y)),
                (PointerEventType::Move, to),
                (PointerEventType::Up, to),
            ]
            .into_iter()
            .enumerate()
            {
                let update = session.input(
                    document,
                    InputEvent::Pointer(PointerEvent {
                        pointer_id: 1,
                        event_type,
                        device: PointerDevice::Mouse,
                        position,
                        button: matches!(event_type, PointerEventType::Down | PointerEventType::Up)
                            .then_some(PointerButton::Secondary),
                        buttons: PointerButtons(if event_type == PointerEventType::Up {
                            0
                        } else {
                            PointerButtons::SECONDARY
                        }),
                        modifiers: Default::default(),
                        pressure: None,
                        timestamp_ms: (index * 4 + sample) as u64,
                    }),
                )?;
                ensure!(
                    update.interaction.consumed,
                    "Pointer sample was not consumed"
                );
            }
        }
    }
    session.flush(document)?;
    let after = session.scene().node(id).context("rectangle after edit")?;
    ensure!(
        after.geometry_revision == before.geometry_revision,
        "Local edit rebuilt unchanged rectangle geometry"
    );
    match edit_kind {
        EditKind::Opacity => ensure!(
            after.material_revision != before.material_revision
                && after.pose_revision == before.pose_revision,
            "Opacity edit did not update only the selected material"
        ),
        EditKind::Pose => {
            ensure!(
                after.pose_revision != before.pose_revision
                    && after.material_revision == before.material_revision,
                "Pointer gesture did not update only rectangle 5050's pose"
            );
            let current = rectangle(session, id)?;
            let offset = if index.is_multiple_of(2) { 1.0 } else { 2.0 };
            ensure!(
                (current.center_x - initial.center_x - offset).abs() < 1e-9
                    && (current.center_y - initial.center_y).abs() < 1e-9,
                "Pointer gesture did not reach the requested 1/2 px pose"
            );
            ensure!(
                (current.center_x / 256.0).floor() == (initial.center_x / 256.0).floor()
                    && (current.center_y / 256.0).floor() == (initial.center_y / 256.0).floor(),
                "Local pose edit left its original output tile"
            );
        }
    }
    Ok(())
}

fn run(
    service: Arc<dyn GpuService>,
    width: u32,
    height: u32,
    edit_kind: EditKind,
) -> Result<serde_json::Value> {
    let fixture_start = Instant::now();
    let (mut document, mut session, id) = fixture(width, height)?;
    let initial = rectangle(&session, id)?;
    let fixture_ms = fixture_start.elapsed().as_secs_f64() * 1000.0;
    let mut canvas = GpuCanvas::new(service.clone())?;
    canvas.set_profiling(true);
    let prepare = |canvas: &mut GpuCanvas, session: &CanvasSession, document: &CanvasDocument| {
        canvas.prepare(
            session.scene(),
            document.background(),
            document.resource_revision(),
            1.0,
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )
    };
    let cold_start = Instant::now();
    let cold_frame = prepare(&mut canvas, &session, &document)?;
    complete(&service)?;
    let cold_ms = cold_start.elapsed().as_secs_f64() * 1000.0;
    let cold = canvas.take_completed_profiles()?;
    let mut edits = Vec::new();
    let mut timings = Vec::new();
    for index in 0..128 {
        let before = document
            .engine()
            .retained_scene_diagnostics(session.viewport())
            .map_err(snow_draw_engine_gpui::CanvasError::from)?;
        let start = Instant::now();
        apply_edit(&mut document, &mut session, id, initial, edit_kind, index)?;
        let applied_ms = start.elapsed().as_secs_f64() * 1000.0;
        let frame = prepare(&mut canvas, &session, &document)?;
        complete(&service)?;
        let total = start.elapsed().as_secs_f64() * 1000.0;
        let after = document
            .engine()
            .retained_scene_diagnostics(session.viewport())
            .map_err(snow_draw_engine_gpui::CanvasError::from)?;
        let gpu = canvas.take_completed_profiles()?;
        ensure!(
            frame.profile.mesh_upload_bytes == 0 && frame.profile.source_upload_bytes == 0,
            "{} edit uploaded unchanged meshes or source pixels",
            edit_kind.label()
        );
        ensure!(
            after.owners_composed - before.owners_composed <= 2,
            "Local edit visited unrelated owners"
        );
        ensure!(
            after.nodes_compared - before.nodes_compared <= 2,
            "Local edit compared unrelated nodes"
        );
        if index >= 8 {
            timings.push(total);
        }
        edits.push(serde_json::json!({"input_to_gpu_completion_ms":total,"input_and_delta_ms":applied_ms,"owners_composed":after.owners_composed-before.owners_composed,"nodes_compared":after.nodes_compared-before.nodes_compared,"profile":frame.profile,"completed_gpu":gpu}));
    }
    timings.sort_by(f64::total_cmp);
    let idle = prepare(&mut canvas, &session, &document)?;
    ensure!(
        idle.profile.rendered_tiles == 0 && idle.profile.uploaded_bytes == 0,
        "Idle renderer submitted work"
    );
    let p95 = timings[(timings.len() * 95).div_ceil(100) - 1];
    Ok(
        serde_json::json!({"size":[width,height],"edit_kind":edit_kind.label(),"elements":10_000,"samples":128,"warmup_samples":8,"gesture":(edit_kind == EditKind::Pose).then(|| serde_json::json!({"button":"secondary","interior_offset_px":[0.5,0.5],"activation_dx_px":8.0,"final_dx_px":[1.0,2.0],"input_samples_per_edit":4,"publications_per_edit":1})),"fixture_setup_ms":fixture_ms,"cold_render_to_gpu_completion_ms":cold_ms,"cold_profile":cold_frame.profile,"cold_gpu":cold,"warmed_p95_ms":p95,"target_ms":8.33,"meets_target":p95<=8.33,"edits":edits,"idle_profile":idle.profile}),
    )
}

fn main() -> Result<()> {
    let destination = std::env::args().nth(1).context("output JSON path")?;
    if destination == "--validate-input" {
        // Deterministic gesture verification without starting a GPU device or
        // collecting timings; safe to run while performance work is paused.
        for (width, height) in [(960, 640), (1920, 1080), (3840, 2160)] {
            let (mut document, mut session, id) = fixture(width, height)?;
            let initial = rectangle(&session, id)?;
            for edit_kind in [EditKind::Opacity, EditKind::Pose] {
                for index in 0..4 {
                    apply_edit(&mut document, &mut session, id, initial, edit_kind, index)?;
                }
            }
        }
        println!("Validated opacity and pointer pose edits at all three fixture sizes");
        return Ok(());
    }
    let device_start = Instant::now();
    let host = OffscreenGpuHost::new()?;
    let device_init_ms = device_start.elapsed().as_secs_f64() * 1000.0;
    let service = host.service();
    let capabilities = service.capabilities();
    let adapter = serde_json::json!({"name":capabilities.adapter_name.as_ref(),"vendor_id":capabilities.vendor_id,"device_id":capabilities.device_id,"backend":format!("{:?}",capabilities.backend),"rgba8_sample_counts":capabilities.rgba8_sample_counts.as_ref()});
    let readback_before = service.diagnostics().readback_bytes;
    let mut cases = Vec::new();
    for (width, height) in [(960, 640), (1920, 1080), (3840, 2160)] {
        for edit_kind in [EditKind::Opacity, EditKind::Pose] {
            cases.push(run(service.clone(), width, height, edit_kind)?);
        }
    }
    let display_readback_bytes = service.diagnostics().readback_bytes - readback_before;
    ensure!(
        display_readback_bytes == 0,
        "Display path read back GPU pixels"
    );
    std::fs::write(
        destination,
        serde_json::to_vec_pretty(
            &serde_json::json!({"adapter":adapter,"profile":"canvas-perf","device_initialization_ms":device_init_ms,"display_readback_bytes":display_readback_bytes,"cases":cases,"native_input_to_present":"measured separately by the native window benchmark"}),
        )?,
    )?;
    Ok(())
}
