//! Isolate continuous shape-label dragging from input, scene publication, and GPU work.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use gpui::GpuService;
use gpui_windows::gpu_extensions::{DirectXGpuService, OffscreenGpuHost};
use serde_json::{Value, json};
use snow_draw_engine_gpui::{
    CanvasDocument, CanvasSession, GpuCanvas, engine::*, prepare_text_metrics,
};

fn pointer(
    document: &mut CanvasDocument,
    session: &mut CanvasSession,
    kind: PointerEventType,
    position: Point<f64>,
) -> Result<()> {
    pointer_button(document, session, kind, position, PointerButton::Primary)
}

fn pointer_button(
    document: &mut CanvasDocument,
    session: &mut CanvasSession,
    kind: PointerEventType,
    position: Point<f64>,
    button: PointerButton,
) -> Result<()> {
    session.input(
        document,
        InputEvent::Pointer(PointerEvent {
            pointer_id: 1,
            event_type: kind,
            device: PointerDevice::Mouse,
            position,
            button: matches!(kind, PointerEventType::Down | PointerEventType::Up).then_some(button),
            buttons: PointerButtons(if kind == PointerEventType::Up {
                0
            } else if button == PointerButton::Secondary {
                PointerButtons::SECONDARY
            } else {
                PointerButtons::PRIMARY
            }),
            modifiers: Default::default(),
            pressure: None,
            timestamp_ms: 0,
        }),
    )?;
    Ok(())
}

fn handle(session: &CanvasSession) -> Result<Point<f64>> {
    session
        .scene()
        .overlays()
        .iter()
        .find_map(|item| match item {
            OverlayDisplayItem::Rectangle(item) if item.kind == UiShapeKind::ShapeTextHandle => {
                Some(Point::new(item.center_x, item.center_y))
            }
            _ => None,
        })
        .context("selected shape text handle")
}

fn complete(service: &Arc<dyn GpuService>) -> Result<()> {
    let native = service
        .as_any()
        .downcast_ref::<DirectXGpuService>()
        .context("D3D11 service")?;
    let mut completion = native.completion()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !completion.poll()? {
        ensure!(Instant::now() < deadline, "GPU completion timed out");
        std::thread::yield_now();
    }
    Ok(())
}

fn render(
    canvas: &mut GpuCanvas,
    document: &CanvasDocument,
    session: &CanvasSession,
    scale: f32,
) -> Result<Value> {
    let start = Instant::now();
    let frame = canvas.prepare(
        session.scene(),
        None,
        document.resource_revision(),
        scale,
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )?;
    let submit_ms = start.elapsed().as_secs_f64() * 1000.0;
    complete(&canvas.service())?;
    Ok(
        json!({"prepare_ms":submit_ms,"render_to_completion_ms":start.elapsed().as_secs_f64()*1000.0,
        "profile":frame.profile,"completed_gpu":canvas.take_completed_profiles()?}),
    )
}

fn percentile(samples: &[Value], field: &str) -> f64 {
    let mut values: Vec<_> = samples.iter().filter_map(|v| v[field].as_f64()).collect();
    values.sort_by(f64::total_cmp);
    values[(values.len() * 95).div_ceil(100) - 1]
}

fn run(
    service: Arc<dyn GpuService>,
    gesture: &str,
    text: &str,
    scale: f32,
    background_nodes: u32,
) -> Result<Value> {
    let mut document = CanvasDocument::default();
    let mut session = CanvasSession::new(&mut document)?;
    session.set_surface(&mut document, 960, 640)?;
    session.set_camera(
        &mut document,
        Camera {
            center: Point::new(480.0, 320.0),
            zoom: 1.0,
        },
    )?;
    session.set_deferred_composition(true);
    document.command(|engine| {
        engine.set_viewport_active_tool(session.viewport(), ActiveTool::Select)
    })?;
    for start in (0..background_nodes).step_by(128) {
        let ops: Vec<_> = (start..(start+128).min(background_nodes)).map(|i| json!({"type":"rectangle","bounds":[(i%40)*24,(i/40)*24,12,12],"style":{"fill":[80,120,180,255]}})).collect();
        document.command(|engine| {
            engine.apply_annotation_json(
                &serde_json::to_vec(&json!({"version":1,"operations":ops})).unwrap(),
            )
        })?;
    }
    document.command(|engine| {
        engine.apply_annotation_json(
            br#"{"version":1,"operations":[{"type":"rectangle","bounds":[100,100,160,100]}]}"#,
        )
    })?;
    session.flush(&document)?;
    let owner_id = session
        .scene()
        .nodes()
        .find(|(id, _)| id.element.index == background_nodes)
        .context("shape owner")?
        .0
        .element;
    let owner = ElementId {
        index: owner_id.index,
        generation: owner_id.generation,
    };
    session.mutate(&mut document, |engine, viewport| {
        engine.select_element_with_viewport_changes(viewport, owner)
    })?;
    session.flush(&document)?;
    let label_center = Point::new(450.0, 350.0);
    let text_id = if gesture == "text_body" {
        None
    } else {
        let start = handle(&session)?;
        pointer(&mut document, &mut session, PointerEventType::Down, start)?;
        prepare_text_metrics(&mut document, &mut session)?;
        pointer(
            &mut document,
            &mut session,
            PointerEventType::Up,
            label_center,
        )?;
        Some(
            document
                .command(|engine| engine.take_text_edit_request(session.viewport()))?
                .context("bound label created")?,
        )
    };
    session.mutate(&mut document, |engine, viewport| {
        let mut style = engine.viewport_style_toolbar_state(viewport)?.text_style;
        style.font_size = 28.0;
        engine.commit_text_draft_with_viewport_changes(
            viewport,
            TextDraftCommit::new(
                text_id.map_or(TextCommitTarget::New, TextCommitTarget::Existing),
                label_center,
                text,
                TextLayoutSize::new(420.0, text.lines().count() as f64 * 36.4),
                style,
                false,
                false,
            ),
        )
    })?;
    session.flush(&document)?;
    let text_id = text_id.unwrap_or_else(|| {
        let id = session
            .scene()
            .nodes()
            .find(|(_, node)| matches!(node.item.as_ref(), SceneDisplayItem::Text(_)))
            .expect("created text")
            .0
            .element;
        ElementId {
            index: id.index,
            generation: id.generation,
        }
    });
    let selected = if gesture == "shape_handle" {
        owner
    } else {
        text_id
    };
    session.mutate(&mut document, |engine, viewport| {
        engine.select_element_with_viewport_changes(viewport, selected)
    })?;
    prepare_text_metrics(&mut document, &mut session)?;
    let mut canvas = GpuCanvas::new(service.clone())?;
    canvas.set_profiling(true);
    let cold = render(&mut canvas, &document, &session, scale).context("cold text rendering")?;
    let start = match gesture {
        "shape_handle" => handle(&session)?,
        // Hit the visible glyphs at the left of the fixed-width text box.
        _ => Point::new(250.0, 350.0),
    };
    let button = if gesture == "shape_handle" {
        PointerButton::Primary
    } else {
        PointerButton::Secondary
    };
    pointer_button(
        &mut document,
        &mut session,
        PointerEventType::Down,
        start,
        button,
    )?;
    ensure!(
        document.engine().has_pointer_capture(),
        "drag must capture the pointer"
    );
    // Selection dragging starts after crossing the pointer threshold.
    pointer_button(
        &mut document,
        &mut session,
        PointerEventType::Move,
        Point::new(start.x + 8.0, start.y),
        button,
    )?;
    let history = document.engine().history_state();
    let diagnostics =
        document.command(|engine| engine.retained_scene_diagnostics(session.viewport()))?;
    let readbacks = service.diagnostics().readback_bytes;
    let mut samples = Vec::new();
    for index in 0..40 {
        let position = Point::new(
            start.x + 40.0 + (index % 8) as f64 * 2.0,
            start.y + 40.0 + (index % 5) as f64 * 2.0,
        );
        let begin = Instant::now();
        pointer_button(
            &mut document,
            &mut session,
            PointerEventType::Move,
            position,
            button,
        )?;
        let input_ms = begin.elapsed().as_secs_f64() * 1000.0;
        let measure = Instant::now();
        prepare_text_metrics(&mut document, &mut session)?;
        let metrics_and_flush_ms = measure.elapsed().as_secs_f64() * 1000.0;
        let node = session.scene().nodes().find(|(_, node)| matches!(node.item.as_ref(), SceneDisplayItem::Text(item) if item.id.index==text_id.index)).context("dragged text remains present")?.1;
        let SceneDisplayItem::Text(item) = node.item.as_ref() else {
            unreachable!()
        };
        let expected = if gesture == "shape_handle" {
            position
        } else {
            Point::new(
                label_center.x + position.x - start.x,
                label_center.y + position.y - start.y,
            )
        };
        ensure!(
            (item.center_x - expected.x).abs() < 0.001
                && (item.center_y - expected.y).abs() < 0.001,
            "{gesture} must move the text: actual [{}, {}], expected [{}, {}]",
            item.center_x,
            item.center_y,
            expected.x,
            expected.y
        );
        let mut result = render(&mut canvas, &document, &session, scale)
            .with_context(|| format!("drag frame {index}"))?;
        result["input_ms"] = json!(input_ms);
        result["metrics_and_flush_ms"] = json!(metrics_and_flush_ms);
        result["input_to_completion_ms"] = json!(begin.elapsed().as_secs_f64() * 1000.0);
        samples.push(result);
        ensure!(
            document.engine().history_state() == history,
            "drag preview changed undo history"
        );
    }
    let after = document.command(|engine| engine.retained_scene_diagnostics(session.viewport()))?;
    let warm = &samples[8..];
    let summary: serde_json::Map<String, Value> = [
        "input_ms",
        "metrics_and_flush_ms",
        "prepare_ms",
        "render_to_completion_ms",
        "input_to_completion_ms",
    ]
    .into_iter()
    .map(|field| (format!("{field}_p95"), json!(percentile(warm, field))))
    .collect();
    ensure!(
        service.diagnostics().readback_bytes == readbacks,
        "display drag performed readback"
    );
    pointer(&mut document, &mut session, PointerEventType::Cancel, start)?;
    Ok(
        json!({"gesture":gesture,"text_chars":text.chars().count(),"scale":scale,"background_nodes":background_nodes,
        "cold":cold,"warm_p95":summary,"scene_before":format!("{diagnostics:?}"),"scene_after":format!("{after:?}"),"display_readback_bytes":0,"samples":samples}),
    )
}

fn main() -> Result<()> {
    let destination = std::env::args().nth(1).context("output JSON path")?;
    let mut adapter = String::new();
    let short = "Bound text label";
    // Explicit lines fit within 420 units, so the measured box and rendered
    // line heights agree without depending on a second host font measurer.
    let latin = vec!["Wrapped text label"; 32].join("\n");
    let cjk = vec!["形状绑定文字拖动"; 24].join("\n");
    let mut cases = Vec::new();
    for (gesture, text, scale, nodes) in [
        ("shape_handle", short, 1.0, 0),
        ("bound_text_body", short, 1.0, 0),
        ("text_body", short, 1.0, 0),
        ("shape_handle", latin.as_str(), 1.0, 0),
        ("shape_handle", cjk.as_str(), 1.0, 0),
        ("shape_handle", cjk.as_str(), 1.5, 0),
        ("shape_handle", cjk.as_str(), 2.0, 0),
        ("shape_handle", short, 1.5, 1000),
    ] {
        eprintln!(
            "{gesture}: {} characters, scale {scale}, {nodes} background nodes",
            text.chars().count()
        );
        // Use a fresh device so deferred retirement from a previous case cannot
        // consume this case's aggregate GPU budget or affect its cold timing.
        let host = OffscreenGpuHost::new()?;
        let service = host.service();
        adapter = service.capabilities().adapter_name.to_string();
        cases.push(match run(service, gesture, text, scale, nodes) {
            Ok(result) => result,
            Err(error) => json!({"gesture":gesture,"text_chars":text.chars().count(),"scale":scale,"background_nodes":nodes,"error":format!("{error:#}")}),
        });
    }
    std::fs::write(
        destination,
        serde_json::to_vec_pretty(
            &json!({"profile":"canvas-perf Release opt3","adapter":adapter,
        "surface_logical":[960,640],"samples":40,"warmup_samples":8,"measurement":"continuous pointer move through offscreen GPU completion; excludes native event dispatch, overlays and Present","cases":cases}),
        )?,
    )?;
    ensure!(
        !cases.iter().any(|case| case.get("error").is_some()),
        "One or more drag cases failed; see the output JSON"
    );
    Ok(())
}
