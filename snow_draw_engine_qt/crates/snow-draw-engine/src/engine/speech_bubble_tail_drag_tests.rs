use super::*;
use snow_draw_engine_document::{SpeechBubbleTail, speech_bubble_canvas_to_local};
use snow_draw_engine_editor::SpeechBubbleTextMeasurement;

fn passive_tail_fixture(
    shape: HighlightShape,
    rotation: f64,
) -> (
    Engine,
    ViewportId,
    ElementId,
    SpeechBubbleData,
    SpeechBubbleData,
) {
    let (mut engine, viewport, owner) = create(shape, 180.0, 90.0);
    let label = bind(&mut engine, viewport, owner, 30.0, 20.0, 3.0);
    let mut original = body(&engine, owner);
    original.body.rotation = rotation;
    original.tails = vec![
        SpeechBubbleTail {
            id: 7,
            tip: Point::new(0.0, original.body.height / 2.0 + 25.0),
            base_width: 12.0,
        },
        SpeechBubbleTail {
            id: 42,
            tip: Point::new(original.body.width / 2.0 + 90.0, 0.0),
            base_width: 12.0,
        },
    ];
    let mut tx = Transaction::new("passive tail fixture");
    tx.update_speech_bubble(owner, original.clone());
    engine
        .apply_editor_command(
            viewport,
            EditorCommand::ApplyTransaction(ApplyTransactionCommand {
                transaction: tx,
                history_undo_snapshot: None,
                created_selection_target: None,
            }),
        )
        .unwrap();
    engine.invalidate_arrow_text_measurements(viewport).unwrap();
    let request = engine
        .speech_bubble_text_layout_requests(viewport)
        .unwrap()
        .remove(0);
    let c = speech_bubble_text_constraints(request.body, 20.0, 3.0, 120.0).unwrap();
    engine
        .apply_speech_bubble_text_layouts(
            viewport,
            &[(
                label,
                SpeechBubbleTextMeasurement {
                    key: request.key,
                    size: TextLayoutSize::with_content(
                        c.content_width,
                        120.0,
                        c.content_width - 1.0,
                        120.0,
                    ),
                    minimum_grapheme_width: 20.0,
                    text_ink_reserve: 3.0,
                },
            )],
        )
        .unwrap();
    let displayed = engine
        .editor
        .speech_bubble_display_snapshot(&engine.model, owner)
        .unwrap();
    assert_eq!(body(&engine, owner), original);
    assert_eq!(displayed.tails.len(), 1);
    assert_eq!(displayed.tails[0].id, 42);
    (engine, viewport, owner, original, displayed)
}

fn assert_point_close(actual: Point<f64>, expected: Point<f64>) {
    assert!(
        (actual.x - expected.x).hypot(actual.y - expected.y) < 1e-8,
        "{actual:?} != {expected:?}"
    );
}

#[test]
fn speech_bubble_passive_growth_tail_drag_uses_surviving_identity_and_replays_history() {
    for shape in [
        HighlightShape::Rectangle,
        HighlightShape::Ellipse,
        HighlightShape::Diamond,
    ] {
        for rotation in [0.0, 0.57] {
            let (mut engine, viewport, owner, original, displayed) =
                passive_tail_fixture(shape, rotation);
            let control = handle(
                &mut engine,
                viewport,
                UiShapeKind::SpeechBubbleTailTipHandle,
            );
            let delta = Point::new(
                30.0 * rotation.cos() - 10.0 * rotation.sin(),
                30.0 * rotation.sin() + 10.0 * rotation.cos(),
            );
            let before_tip = speech_bubble_local_to_canvas(&displayed, displayed.tails[0].tip);
            pointer(&mut engine, viewport, PointerEventType::Down, control);
            let state = engine
                .editor
                .presentation_state(&engine.model, &engine.viewport_slot(viewport).unwrap().view);
            let preview = &state
                .speech_bubble_previews
                .iter()
                .find(|(id, _)| *id == owner)
                .unwrap()
                .1;
            assert_eq!(preview.body, displayed.body);
            assert_eq!(preview.tails, displayed.tails);
            pointer(
                &mut engine,
                viewport,
                PointerEventType::Up,
                Point::new(control.x + delta.x, control.y + delta.y),
            );
            let edited = body(&engine, owner);
            assert_eq!(edited.tails.len(), 1);
            assert_eq!(edited.tails[0].id, 42);
            assert_point_close(
                speech_bubble_local_to_canvas(&edited, edited.tails[0].tip),
                Point::new(before_tip.x + delta.x, before_tip.y + delta.y),
            );
            assert_eq!(edited.body, displayed.body);
            engine.history.validate_session(&engine.model).unwrap();
            engine.undo().unwrap();
            assert_eq!(body(&engine, owner), original);
            engine.redo().unwrap();
            assert_eq!(body(&engine, owner), edited);
        }
    }
}

#[test]
fn speech_bubble_passive_growth_tail_click_and_cancel_preserve_document_and_history() {
    for shape in [
        HighlightShape::Rectangle,
        HighlightShape::Ellipse,
        HighlightShape::Diamond,
    ] {
        let (mut engine, viewport, owner, original, displayed) = passive_tail_fixture(shape, 0.57);
        let history = engine.serialize_document_history().unwrap();
        for kind in [
            UiShapeKind::SpeechBubbleTailTipHandle,
            UiShapeKind::SpeechBubbleTailBaseHandle,
        ] {
            let control = handle(&mut engine, viewport, kind);
            pointer(&mut engine, viewport, PointerEventType::Down, control);
            pointer(&mut engine, viewport, PointerEventType::Up, control);
            assert_eq!(body(&engine, owner), original);
            assert_eq!(engine.serialize_document_history().unwrap(), history);
            pointer(&mut engine, viewport, PointerEventType::Down, control);
            pointer(
                &mut engine,
                viewport,
                PointerEventType::Move,
                Point::new(control.x + 35.0, control.y - 15.0),
            );
            pointer(&mut engine, viewport, PointerEventType::Up, control);
            assert_eq!(body(&engine, owner), original);
            assert_eq!(engine.serialize_document_history().unwrap(), history);
            pointer(&mut engine, viewport, PointerEventType::Down, control);
            pointer(
                &mut engine,
                viewport,
                PointerEventType::Move,
                Point::new(control.x + 35.0, control.y - 15.0),
            );
            pointer(&mut engine, viewport, PointerEventType::Cancel, control);
            assert!(!engine.has_pointer_capture());
            assert_eq!(body(&engine, owner), original);
            assert_eq!(engine.serialize_document_history().unwrap(), history);
            assert_eq!(
                engine
                    .editor
                    .speech_bubble_display_snapshot(&engine.model, owner)
                    .unwrap(),
                displayed
            );
        }
    }
}

#[test]
fn speech_bubble_passive_growth_base_controls_edit_the_surviving_tail() {
    for shape in [
        HighlightShape::Rectangle,
        HighlightShape::Ellipse,
        HighlightShape::Diamond,
    ] {
        for side in 0..2 {
            let (mut engine, viewport, owner, original, displayed) =
                passive_tail_fixture(shape, 0.57);
            let patch = engine.acquire_patch(viewport, None).unwrap();
            let control = patch
                .overlay
                .ops
                .iter()
                .flat_map(|op| &op.insert_items)
                .filter_map(|item| match item {
                    OverlayDisplayItem::Rectangle(rect)
                        if rect.kind == UiShapeKind::SpeechBubbleTailBaseHandle =>
                    {
                        Some(Point::new(rect.center_x, rect.center_y))
                    }
                    _ => None,
                })
                .nth(side)
                .unwrap();
            let tip = displayed.tails[0].tip;
            let angle = tip.y.atan2(tip.x) + if side == 0 { -0.25 } else { 0.25 };
            let target = speech_bubble_local_to_canvas(
                &displayed,
                Point::new(250.0 * angle.cos(), 250.0 * angle.sin()),
            );
            let bases = speech_bubble_tail_base_points(&displayed, &displayed.tails[0]);
            let anchor = speech_bubble_local_to_canvas(&displayed, bases[side]);
            pointer(&mut engine, viewport, PointerEventType::Down, control);
            pointer(
                &mut engine,
                viewport,
                PointerEventType::Up,
                Point::new(
                    target.x + control.x - anchor.x,
                    target.y + control.y - anchor.y,
                ),
            );
            let edited = body(&engine, owner);
            assert_eq!(edited.tails.len(), 1);
            assert_eq!(edited.tails[0].id, 42);
            assert!(
                (snow_draw_engine_document::speech_bubble_tail_effective_half_angle(
                    &edited,
                    &edited.tails[0]
                ) - 0.25)
                    .abs()
                    < 1e-9
            );
            assert_eq!(edited.tails[0].tip, displayed.tails[0].tip);
            engine.undo().unwrap();
            assert_eq!(body(&engine, owner), original);
            engine.redo().unwrap();
            assert_eq!(body(&engine, owner), edited);
        }
    }
}

#[test]
fn speech_bubble_passive_growth_add_tail_uses_displayed_body_for_creation_and_deletion() {
    for shape in [
        HighlightShape::Rectangle,
        HighlightShape::Ellipse,
        HighlightShape::Diamond,
    ] {
        let (mut engine, viewport, owner, original, displayed) = passive_tail_fixture(shape, 0.57);
        let history = engine.serialize_document_history().unwrap();
        let add = handle(
            &mut engine,
            viewport,
            UiShapeKind::SpeechBubbleAddTailHandle,
        );
        let inside =
            speech_bubble_local_to_canvas(&displayed, Point::new(0.0, displayed.body.height * 0.4));
        assert!(!snow_draw_engine_document::speech_bubble_body_contains(
            &original,
            speech_bubble_canvas_to_local(&original, inside)
        ));
        pointer(&mut engine, viewport, PointerEventType::Down, add);
        pointer(&mut engine, viewport, PointerEventType::Up, inside);
        assert_eq!(body(&engine, owner), original);
        assert_eq!(engine.serialize_document_history().unwrap(), history);
        pointer(&mut engine, viewport, PointerEventType::Down, add);
        pointer(&mut engine, viewport, PointerEventType::Up, add);
        let edited = body(&engine, owner);
        assert_eq!(edited.body, displayed.body);
        assert_eq!(edited.tails.len(), 2);
        assert_eq!(edited.tails[0], displayed.tails[0]);
        assert_point_close(
            speech_bubble_local_to_canvas(&edited, edited.tails[1].tip),
            add,
        );
        engine.undo().unwrap();
        assert_eq!(body(&engine, owner), original);
        engine.redo().unwrap();
        assert_eq!(body(&engine, owner), edited);
    }
}
