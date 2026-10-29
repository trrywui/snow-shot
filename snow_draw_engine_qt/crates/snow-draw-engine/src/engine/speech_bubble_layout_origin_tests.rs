use super::*;
use snow_draw_engine_editor::{
    RectangleShapeStyle, SHAPE_STYLE_MIXED_SHAPE, SHAPE_STYLE_MIXED_STROKE_WIDTH, ShapeKind,
    ShapeStyle, ShapeStylePatch, SpeechBubbleTextMeasurement,
};

fn fixture() -> (Engine, ViewportId, ElementId, ElementId) {
    let (mut engine, viewport, owner) = create(HighlightShape::Rectangle, 180.0, 90.0);
    let label = bind(&mut engine, viewport, owner, 30.0, 20.0, 3.0);
    (engine, viewport, owner, label)
}

fn change_body_style(engine: &mut Engine, viewport: ViewportId) {
    let mut style = ShapeStyle::from_rectangle_shape_style(RectangleShapeStyle {
        shape: HighlightShape::Diamond,
        ..EngineConfig::default().style_defaults.editor.speech_bubble
    });
    style.stroke_width = 8.0;
    engine
        .set_viewport_shape_style_patch(
            viewport,
            ShapeStylePatch {
                kind: ShapeKind::SpeechBubble,
                style,
                properties: SHAPE_STYLE_MIXED_SHAPE | SHAPE_STYLE_MIXED_STROKE_WIDTH,
            },
        )
        .unwrap();
}

fn measurement(
    engine: &Engine,
    viewport: ViewportId,
    owner: ElementId,
) -> (ElementId, SpeechBubbleTextMeasurement) {
    let request = engine
        .speech_bubble_text_layout_requests(viewport)
        .unwrap()
        .into_iter()
        .find(|request| request.owner_id == owner)
        .unwrap();
    let c = speech_bubble_text_constraints(request.body, 20.0, 3.0, 160.0).unwrap();
    (
        request.text_id,
        SpeechBubbleTextMeasurement {
            key: request.key,
            size: TextLayoutSize::with_content(
                c.content_width,
                160.0,
                c.content_width - 1.0,
                160.0,
            ),
            minimum_grapheme_width: 20.0,
            text_ink_reserve: 3.0,
        },
    )
}

fn undo_count(engine: &Engine) -> usize {
    let value: serde_json::Value =
        serde_json::from_slice(&engine.serialize_document_history().unwrap()).unwrap();
    value["history"]["undoStack"].as_array().unwrap().len()
}

fn command(transaction: Transaction) -> ApplyTransactionCommand {
    ApplyTransactionCommand {
        transaction,
        history_undo_snapshot: None,
        created_selection_target: None,
    }
}

#[test]
fn speech_bubble_style_metrics_amend_originating_history_with_growth_and_tail_deletion() {
    let (mut engine, viewport, owner, label) = fixture();
    let add = handle(
        &mut engine,
        viewport,
        UiShapeKind::SpeechBubbleAddTailHandle,
    );
    pointer(&mut engine, viewport, PointerEventType::Down, add);
    pointer(
        &mut engine,
        viewport,
        PointerEventType::Up,
        Point::new(0.0, 75.0),
    );
    let before = body(&engine, owner);
    let before_text = engine.model.text(label).unwrap().clone();
    assert_eq!(before.tails.len(), 1);
    let before_tip = speech_bubble_local_to_canvas(&before, before.tails[0].tip);
    let history_before = undo_count(&engine);

    engine.begin_presentation_update().unwrap();
    change_body_style(&mut engine, viewport);
    let metrics = measurement(&engine, viewport, owner);
    engine
        .apply_speech_bubble_text_layouts(viewport, &[metrics])
        .unwrap();
    engine.end_presentation_update().unwrap();
    let after = body(&engine, owner);
    let after_text = engine.model.text(label).unwrap().clone();
    assert_eq!(undo_count(&engine), history_before + 1);
    assert!(after.body.height > before.body.height);
    assert!(after.tails.is_empty());
    let c = speech_bubble_text_constraints(after.body, 20.0, 3.0, 160.0).unwrap();
    assert!((after_text.width() - c.content_width).abs() < 1e-8);
    assert!((after_text.height() - c.content_height).abs() < 1e-8);
    assert_eq!(after_text.layout.ink_or_wrap().1, 160.0);
    engine.history.validate_session(&engine.model).unwrap();
    let saved = engine.serialize_document_session().unwrap();
    let restored =
        Engine::from_serialized_document_session_with_config(&saved, EngineConfig::default())
            .unwrap();
    assert_eq!(body(&restored, owner), after);
    assert_eq!(restored.model.text(label).unwrap(), &after_text);

    engine.undo_with_viewport_changes().unwrap();
    assert_eq!(body(&engine, owner), before);
    assert_eq!(engine.model.text(label).unwrap(), &before_text);
    assert_eq!(
        speech_bubble_local_to_canvas(&body(&engine, owner), before.tails[0].tip),
        before_tip
    );
    engine.redo_with_viewport_changes().unwrap();
    assert_eq!(body(&engine, owner), after);
    assert_eq!(engine.model.text(label).unwrap(), &after_text);
}

#[test]
fn speech_bubble_text_font_metrics_amend_originating_text_transaction() {
    let (mut engine, viewport, owner, label) = fixture();
    let before = body(&engine, owner);
    let before_text = engine.model.text(label).unwrap().clone();
    let count = undo_count(&engine);
    let mut changed = before_text.clone();
    changed.font_size *= 2.0;
    changed.bold = true;
    changed.italic = true;
    changed.stroke_width = 6.0;
    changed.stroke.a = 255;
    let mut transaction = Transaction::new("update bound bubble typography");
    transaction.update_text(label, changed);
    engine
        .commit_transaction(viewport, command(transaction))
        .unwrap();
    let metrics = measurement(&engine, viewport, owner);
    engine
        .apply_speech_bubble_text_layouts(viewport, &[metrics])
        .unwrap();
    let after = body(&engine, owner);
    let after_text = engine.model.text(label).unwrap().clone();
    assert_eq!(undo_count(&engine), count + 1);
    assert!(after.body.height > before.body.height);
    assert!(after_text.bold && after_text.italic);
    assert_eq!(after_text.layout.ink_or_wrap().1, 160.0);
    engine.history.validate_session(&engine.model).unwrap();
    engine.undo_with_viewport_changes().unwrap();
    assert_eq!(body(&engine, owner), before);
    assert_eq!(engine.model.text(label).unwrap(), &before_text);
    engine.redo_with_viewport_changes().unwrap();
    assert_eq!(body(&engine, owner), after);
    assert_eq!(engine.model.text(label).unwrap(), &after_text);
}

#[test]
fn speech_bubble_partial_metric_batches_amend_one_shared_origin() {
    let (mut engine, viewport, first, first_label) = fixture();
    engine
        .duplicate_selected_with_viewport_changes(viewport, Point::new(240.0, 0.0))
        .unwrap();
    let second = engine.selected_ids()[0];
    let second_label = body(&engine, second).text_element_id.unwrap();
    let originals = [body(&engine, first), body(&engine, second)];
    let count = undo_count(&engine);
    let mut transaction = Transaction::new("update two bubble bodies");
    for (owner, value) in [first, second].into_iter().zip(&originals) {
        let mut value = value.clone();
        value.body.highlight_shape = HighlightShape::Diamond;
        value.body.stroke_width = 8.0;
        transaction.update_speech_bubble(owner, value);
    }
    engine
        .commit_transaction(viewport, command(transaction))
        .unwrap();
    let first_metrics = measurement(&engine, viewport, first);
    let second_metrics = measurement(&engine, viewport, second);
    engine
        .apply_speech_bubble_text_layouts(viewport, &[first_metrics])
        .unwrap();
    assert!(body(&engine, first).body.height > originals[0].body.height);
    assert_eq!(body(&engine, second).body.height, originals[1].body.height);
    engine
        .apply_speech_bubble_text_layouts(viewport, &[second_metrics])
        .unwrap();
    let final_values = [body(&engine, first), body(&engine, second)];
    assert!(final_values[1].body.height > originals[1].body.height);
    assert_eq!(undo_count(&engine), count + 1);
    assert_eq!(
        engine
            .model
            .text(first_label)
            .unwrap()
            .layout
            .ink_or_wrap()
            .1,
        160.0
    );
    assert_eq!(
        engine
            .model
            .text(second_label)
            .unwrap()
            .layout
            .ink_or_wrap()
            .1,
        160.0
    );
    engine.history.validate_session(&engine.model).unwrap();
    engine.undo_with_viewport_changes().unwrap();
    assert_eq!([body(&engine, first), body(&engine, second)], originals);
    engine.redo_with_viewport_changes().unwrap();
    assert_eq!([body(&engine, first), body(&engine, second)], final_values);
}

#[test]
fn speech_bubble_passive_or_stale_metrics_cannot_amend_another_history_state() {
    for scenario in [
        "unrelated",
        "undo_redo",
        "font",
        "restore",
        "clone",
        "capture",
        "metadata",
        "reorder",
    ] {
        let (mut engine, viewport, owner, label) = fixture();
        change_body_style(&mut engine, viewport);
        let mut metrics = measurement(&engine, viewport, owner);
        match scenario {
            "unrelated" => {
                let mut transaction = Transaction::new("unrelated watermark");
                let mut watermark = engine.model.watermark_config().clone();
                watermark.text = "other change".into();
                transaction.update_watermark(watermark);
                engine
                    .commit_transaction(viewport, command(transaction))
                    .unwrap();
            }
            "undo_redo" => {
                engine.undo_with_viewport_changes().unwrap();
                engine
                    .apply_speech_bubble_text_layouts(viewport, &[metrics])
                    .unwrap();
                engine.redo_with_viewport_changes().unwrap();
            }
            "font" => {
                engine.invalidate_arrow_text_measurements(viewport).unwrap();
                engine
                    .apply_speech_bubble_text_layouts(viewport, &[metrics])
                    .unwrap();
                metrics = measurement(&engine, viewport, owner);
            }
            "restore" => {
                let bytes = engine.serialize_document_history().unwrap();
                engine
                    .restore_document_history_preserving_editor_styles(&bytes)
                    .unwrap();
            }
            "metadata" | "reorder" => {
                engine.invalidate_arrow_text_measurements(viewport).unwrap();
                let mut transaction = Transaction::new("nongeometry bubble update");
                if scenario == "metadata" {
                    transaction.update_element_meta(
                        owner,
                        ElementMeta {
                            locked: true,
                            ..Default::default()
                        },
                    );
                } else {
                    let mut watermark = engine.model.watermark_config().clone();
                    watermark.text = "force a real order transaction".into();
                    transaction.update_watermark(watermark);
                    transaction.reorder_elements(vec![owner, label], 0);
                }
                engine
                    .commit_transaction(viewport, command(transaction))
                    .unwrap();
                metrics = measurement(&engine, viewport, owner);
            }
            "clone" => engine = engine.clone_document_session(),
            "capture" => {
                let from = handle(&mut engine, viewport, UiShapeKind::SelectionResizeHandle);
                pointer(&mut engine, viewport, PointerEventType::Down, from);
            }
            _ => unreachable!(),
        }
        let persisted = body(&engine, owner);
        let persisted_text = engine.model.text(label).unwrap().clone();
        let count = undo_count(&engine);
        engine
            .apply_speech_bubble_text_layouts(viewport, &[metrics])
            .unwrap();
        assert_eq!(body(&engine, owner), persisted, "{scenario}");
        assert_eq!(
            engine.model.text(label).unwrap(),
            &persisted_text,
            "{scenario}"
        );
        assert_eq!(undo_count(&engine), count, "{scenario}");
        engine.history.validate_session(&engine.model).unwrap();
    }
}

#[test]
fn speech_bubble_failed_transaction_preserves_pending_origin() {
    let (mut engine, viewport, owner, _) = fixture();
    change_body_style(&mut engine, viewport);
    let metrics = measurement(&engine, viewport, owner);
    let before = body(&engine, owner);
    let mut invalid = Transaction::new("invalid intervening update");
    invalid.update_text(
        ElementId {
            index: 9999,
            generation: 1,
        },
        TextData::default(),
    );
    assert!(
        engine
            .commit_transaction(viewport, command(invalid))
            .is_err()
    );
    engine
        .apply_speech_bubble_text_layouts(viewport, &[metrics])
        .unwrap();
    assert!(body(&engine, owner).body.height > before.body.height);
    engine.history.validate_session(&engine.model).unwrap();
}
