use super::*;
use snow_draw_engine_core::{ColorRgba8, SurfaceSize};
use snow_draw_engine_display::{RetainedSceneDelta, SceneDisplayItem};
use snow_draw_engine_document::{ElementMeta, Transaction};
use snow_draw_engine_interaction::{
    Modifiers, PointerButton, PointerButtons, PointerDevice, PointerEvent, PointerEventType,
};

fn create(engine: &mut Engine, view: ViewportId) {
    engine.apply_annotation_json(br#"{"version":1,"operations":[{"type":"rectangle","bounds":[10,10,80,40],"style":{"fill":[30,60,90,255]}}]}"#).unwrap();
    engine.set_viewport_surface_size(view, 800, 600).unwrap();
    engine
        .set_viewport_camera(
            view,
            Camera {
                center: Point::new(400.0, 300.0),
                zoom: 1.0,
            },
        )
        .unwrap();
}
fn content(delta: &RetainedSceneDelta) -> Vec<SceneDisplayItem> {
    let nodes: HashMap<_, _> = delta.upserts.iter().map(|n| (n.id, n)).collect();
    delta
        .order
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .map(|id| (*nodes[id].item).clone())
        .collect()
}
fn pointer(kind: PointerEventType, x: f64, y: f64) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        pointer_id: 1,
        event_type: kind,
        device: PointerDevice::Mouse,
        position: Point::new(x, y),
        button: matches!(kind, PointerEventType::Down | PointerEventType::Up)
            .then_some(PointerButton::Primary),
        buttons: PointerButtons(if kind == PointerEventType::Up {
            0
        } else {
            PointerButtons::PRIMARY
        }),
        modifiers: Modifiers::default(),
        pressure: None,
        timestamp_ms: 0,
    })
}

#[test]
fn retained_viewport_is_lazy_and_keeps_qt_patch_consumers_separate() {
    let mut engine = Engine::default();
    let retained = engine.create_retained_viewport(Default::default()).unwrap();
    let legacy = engine.create_viewport(Default::default()).unwrap();
    create(&mut engine, retained);
    engine.set_viewport_surface_size(legacy, 800, 600).unwrap();
    engine
        .set_viewport_camera(
            legacy,
            Camera {
                center: Point::new(400.0, 300.0),
                zoom: 1.0,
            },
        )
        .unwrap();
    assert_eq!(
        engine
            .retained_scene_diagnostics(retained)
            .unwrap()
            .publications,
        0
    );
    assert_eq!(
        engine.viewports[&retained]
            .composer
            .current_cursor()
            .scene_revision
            .0,
        0
    );
    let delta = engine.acquire_retained_scene_delta(retained, None).unwrap();
    let old = engine.acquire_patch(legacy, None).unwrap();
    assert_eq!(content(&delta), old.scene.ops[0].insert_items);
    assert_eq!(
        delta.frame.surface,
        SurfaceSize {
            width: 800,
            height: 600
        }
    );
    assert_eq!(
        engine.acquire_patch(retained, None).unwrap_err(),
        ErrorCode::InvalidState
    );
    assert!(engine.acquire_retained_scene_delta(legacy, None).is_err());
}

#[test]
fn retained_scopes_keep_logic_current_and_publish_one_latest_frame() {
    let mut engine = Engine::default();
    let view = engine.create_retained_viewport(Default::default()).unwrap();
    create(&mut engine, view);
    let initial = engine.acquire_retained_scene_delta(view, None).unwrap();
    let id = engine.model.paint_order()[0];
    engine.begin_presentation_update().unwrap();
    for value in 0..100 {
        let mut rectangle = *engine.model.rectangle(id).unwrap();
        rectangle.center.x += 1.0;
        rectangle.fill = ColorRgba8 {
            r: value,
            g: 60,
            b: 90,
            a: 255,
        };
        let mut tx = Transaction::new("move");
        tx.update_rectangle(id, rectangle);
        engine
            .commit_transaction(
                view,
                ApplyTransactionCommand {
                    transaction: tx,
                    history_undo_snapshot: None,
                    created_selection_target: None,
                },
            )
            .unwrap();
    }
    assert!(
        engine
            .acquire_retained_scene_delta(view, Some(initial.revision))
            .is_err()
    );
    assert_eq!(
        engine
            .retained_scene_diagnostics(view)
            .unwrap()
            .publications,
        1
    );
    engine.end_presentation_update().unwrap();
    let delta = engine
        .acquire_retained_scene_delta(view, Some(initial.revision))
        .unwrap();
    assert!(!delta.reset);
    assert_eq!(delta.upserts.len(), 1);
    assert_eq!(
        engine
            .retained_scene_diagnostics(view)
            .unwrap()
            .publications,
        2
    );
    assert_eq!(
        engine.viewports[&view]
            .composer
            .current_cursor()
            .scene_revision
            .0,
        0
    );
    engine.undo_with_viewport_changes().unwrap();
    let undo = engine
        .acquire_retained_scene_delta(view, Some(delta.revision))
        .unwrap();
    assert_eq!(undo.upserts.len(), 1);
    engine.redo_with_viewport_changes().unwrap();
    assert_eq!(
        engine
            .acquire_retained_scene_delta(view, Some(undo.revision))
            .unwrap()
            .upserts[0]
            .item,
        delta.upserts[0].item
    );
}

#[test]
fn retained_creation_preview_commit_cancel_and_multiple_views_are_coherent() {
    let mut engine = Engine::default();
    let a = engine.create_retained_viewport(Default::default()).unwrap();
    let b = engine.create_retained_viewport(Default::default()).unwrap();
    engine.set_viewport_surface_size(a, 800, 600).unwrap();
    engine.set_viewport_surface_size(b, 800, 600).unwrap();
    engine
        .set_viewport_active_tool(a, ActiveTool::Shape)
        .unwrap();
    let initial = engine.acquire_retained_scene_delta(a, None).unwrap();
    engine
        .process_input(a, pointer(PointerEventType::Down, 100.0, 100.0))
        .unwrap();
    engine
        .process_input(a, pointer(PointerEventType::Move, 180.0, 150.0))
        .unwrap();
    let preview = engine
        .acquire_retained_scene_delta(a, Some(initial.revision))
        .unwrap();
    assert_eq!(preview.upserts.len(), 1);
    let before = engine.retained_scene_diagnostics(a).unwrap();
    let events: Vec<_> = (0..100)
        .map(|i| pointer(PointerEventType::Move, 180.0 + f64::from(i), 150.0))
        .collect();
    engine
        .process_pointer_move_batch_with_viewport_changes(a, &events)
        .unwrap();
    let moved = engine
        .acquire_retained_scene_delta(a, Some(preview.revision))
        .unwrap();
    assert_eq!(moved.upserts.len(), 1);
    assert_eq!(
        engine
            .retained_scene_diagnostics(a)
            .unwrap()
            .full_compositions,
        before.full_compositions
    );
    engine
        .process_input(a, pointer(PointerEventType::Up, 279.0, 150.0))
        .unwrap();
    let snapshot_a = engine.acquire_retained_scene_delta(a, None).unwrap();
    let snapshot_b = engine.acquire_retained_scene_delta(b, None).unwrap();
    assert_eq!(content(&snapshot_a), content(&snapshot_b));
    engine
        .process_input(a, pointer(PointerEventType::Down, 300.0, 300.0))
        .unwrap();
    engine
        .process_input(a, pointer(PointerEventType::Move, 340.0, 330.0))
        .unwrap();
    let active = engine.acquire_retained_scene_delta(a, None).unwrap();
    assert_eq!(active.upserts.len(), 2);
    engine.process_input(a, InputEvent::FocusLost).unwrap();
    let cancelled = engine
        .acquire_retained_scene_delta(a, Some(active.revision))
        .unwrap();
    assert_eq!(cancelled.removals.len(), 1);
    assert_eq!(
        content(&engine.acquire_retained_scene_delta(a, None).unwrap()),
        content(&snapshot_a)
    );
}

#[test]
fn retained_document_reset_replaces_nodes_even_with_reused_document_revisions() {
    let mut engine = Engine::default();
    let view = engine.create_retained_viewport(Default::default()).unwrap();
    create(&mut engine, view);
    let initial = engine.acquire_retained_scene_delta(view, None).unwrap();
    engine.clear_document_preserving_viewports().unwrap();
    let cleared = engine
        .acquire_retained_scene_delta(view, Some(initial.revision))
        .unwrap();
    assert!(cleared.reset);
    assert!(cleared.upserts.is_empty());
    let mut tx = Transaction::new("after reset");
    let id = engine.model.peek_next_element_id();
    let rect = snow_draw_engine_document::FilterData::default();
    tx.insert_filter(id, ElementMeta::default(), rect);
    engine
        .commit_transaction(
            view,
            ApplyTransactionCommand {
                transaction: tx,
                history_undo_snapshot: None,
                created_selection_target: None,
            },
        )
        .unwrap();
    let after = engine
        .acquire_retained_scene_delta(view, Some(cleared.revision))
        .unwrap();
    assert!(after.revision > cleared.revision);
    assert_eq!(
        engine.viewports[&view]
            .composer
            .current_cursor()
            .scene_revision
            .0,
        0
    );
}
