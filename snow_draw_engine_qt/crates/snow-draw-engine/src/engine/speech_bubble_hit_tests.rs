use super::*;
use snow_draw_engine_display::SceneDisplayItem;
use snow_draw_engine_document::{
    speech_bubble_body_contains, speech_bubble_canvas_to_local, speech_bubble_hit_test,
};
use snow_draw_engine_editor::SpeechBubbleTextMeasurement;

fn passive_growth(
    shape: HighlightShape,
    rotation: f64,
    body_painted: bool,
) -> (
    Engine,
    ViewportId,
    ElementId,
    ElementId,
    SpeechBubbleData,
    TextData,
) {
    let (mut engine, viewport, owner) = create(shape, 180.0, 90.0);
    let label = bind(&mut engine, viewport, owner, 30.0, 20.0, 3.0);
    let mut bubble = body(&engine, owner);
    bubble.body.rotation = rotation;
    if !body_painted {
        bubble.body.stroke.a = 0;
    }
    let mut transaction = Transaction::new("rotate bubble hit fixture");
    transaction.update_speech_bubble(owner, bubble);
    engine
        .apply_editor_command(
            viewport,
            EditorCommand::ApplyTransaction(ApplyTransactionCommand {
                transaction,
                history_undo_snapshot: None,
                created_selection_target: None,
            }),
        )
        .unwrap();
    engine
        .reset_editing_state_with_viewport_changes(viewport)
        .unwrap();
    engine.invalidate_arrow_text_measurements(viewport).unwrap();
    let original_bubble = body(&engine, owner);
    let original_text = engine.model.text(label).unwrap().clone();
    let original_history = engine.serialize_document_history().unwrap();
    let request = engine
        .speech_bubble_text_layout_requests(viewport)
        .unwrap()
        .into_iter()
        .find(|request| request.owner_id == owner)
        .unwrap();
    let constraints = speech_bubble_text_constraints(request.body, 20.0, 3.0, 100.0).unwrap();
    engine
        .apply_speech_bubble_text_layouts(
            viewport,
            &[(
                label,
                SpeechBubbleTextMeasurement {
                    key: request.key,
                    size: TextLayoutSize::with_content(
                        constraints.content_width,
                        100.0,
                        constraints.content_width - 1.0,
                        100.0,
                    ),
                    minimum_grapheme_width: 20.0,
                    text_ink_reserve: 3.0,
                },
            )],
        )
        .unwrap();
    assert_eq!(body(&engine, owner), original_bubble);
    assert_eq!(engine.model.text(label).unwrap(), &original_text);
    assert_eq!(
        engine.serialize_document_history().unwrap(),
        original_history
    );
    let state = engine
        .editor
        .presentation_state(&engine.model, &engine.viewport_slot(viewport).unwrap().view);
    let displayed_bubble = state
        .speech_bubble_previews
        .into_iter()
        .find(|(id, _)| *id == owner)
        .unwrap()
        .1;
    let displayed_text = state
        .speech_bubble_text_previews
        .into_iter()
        .find(|(id, _)| *id == label)
        .unwrap()
        .1;
    let patch = engine.acquire_patch(viewport, None).unwrap();
    let items: Vec<_> = patch
        .scene
        .ops
        .iter()
        .flat_map(|op| &op.insert_items)
        .collect();
    assert!(items.iter().any(|item| matches!(item,
        SceneDisplayItem::SpeechBubble(item)
            if item.body.center_x == displayed_bubble.body.center.x
                && item.body.center_y == displayed_bubble.body.center.y
                && item.body.height == displayed_bubble.body.height)));
    assert!(items.iter().any(|item| matches!(item,
        SceneDisplayItem::Text(item)
            if item.center_x == displayed_text.center.x
                && item.center_y == displayed_text.center.y
                && item.height == displayed_text.height())));
    (
        engine,
        viewport,
        owner,
        label,
        displayed_bubble,
        displayed_text,
    )
}

fn assert_displayed_target(info: &TextElementInfo, bubble: &SpeechBubbleData, text: &TextData) {
    assert_eq!(info.bubble_body, Some(bubble.body));
    assert_eq!(info.center, text.center);
    assert_eq!(info.width, text.width());
    assert_eq!(info.height, text.height());
    assert_eq!(info.content_height, text.layout.ink_or_wrap().1);
    assert_eq!(info.rotation, text.rotation);
    assert_eq!(info.text_ink_reserve, text.text_ink_reserve);
}

#[test]
fn speech_bubble_passive_growth_hits_rendered_body_and_label_and_reopens_displayed_layout() {
    for shape in [
        HighlightShape::Rectangle,
        HighlightShape::Ellipse,
        HighlightShape::Diamond,
    ] {
        for rotation in [0.0, 0.57] {
            let (mut engine, viewport, owner, label, bubble, text) =
                passive_growth(shape, rotation, true);
            let body_point =
                speech_bubble_local_to_canvas(&bubble, Point::new(0.0, bubble.body.height * 0.45));
            assert!(speech_bubble_body_contains(
                &bubble,
                speech_bubble_canvas_to_local(&bubble, body_point),
            ));
            assert!(!speech_bubble_body_contains(
                &body(&engine, owner),
                speech_bubble_canvas_to_local(&body(&engine, owner), body_point),
            ));
            pointer(&mut engine, viewport, PointerEventType::Move, body_point);
            let state = engine
                .editor
                .presentation_state(&engine.model, &engine.viewport_slot(viewport).unwrap().view);
            let hovered = state.hovered_rect.unwrap();
            assert_eq!(hovered.center, bubble.body.center);
            assert!(hovered.height > bubble.body.height && hovered.width > bubble.body.width);
            pointer(&mut engine, viewport, PointerEventType::Down, body_point);
            pointer(&mut engine, viewport, PointerEventType::Up, body_point);
            assert_eq!(
                engine.selected_ids(),
                vec![owner],
                "visible {shape:?} body rotated {rotation} selects its owner"
            );
            let (info, _) = engine
                .speech_bubble_text_target(viewport, None)
                .unwrap()
                .unwrap();
            assert_displayed_target(&info, &bubble, &text);
            engine
                .reset_editing_state_with_viewport_changes(viewport)
                .unwrap();
            let text_point =
                speech_bubble_local_to_canvas(&bubble, Point::new(0.0, text.height() * 0.4));
            assert!(!speech_bubble_body_contains(
                &body(&engine, owner),
                speech_bubble_canvas_to_local(&body(&engine, owner), text_point),
            ));
            assert_eq!(
                engine.hit_text_at(viewport, text_point).unwrap(),
                Some(label)
            );
            let (info, _) = engine
                .speech_bubble_text_target(viewport, Some(text_point))
                .unwrap()
                .unwrap();
            assert_displayed_target(&info, &bubble, &text);
            pointer(&mut engine, viewport, PointerEventType::Down, text_point);
            pointer(&mut engine, viewport, PointerEventType::Up, text_point);
            assert_eq!(
                engine.selected_ids(),
                vec![owner],
                "visible label selects its owner"
            );
        }
    }
}

#[test]
fn speech_bubble_passive_growth_marquee_matches_displayed_body_once() {
    let (mut engine, viewport, owner, _, bubble, _) =
        passive_growth(HighlightShape::Rectangle, 0.0, true);
    let start = Point::new(-140.0, 60.0);
    let end = Point::new(140.0, 90.0);
    let original_history = engine.serialize_document_history().unwrap();
    pointer(&mut engine, viewport, PointerEventType::Down, start);
    pointer(&mut engine, viewport, PointerEventType::Move, end);
    let state = engine
        .editor
        .presentation_state(&engine.model, &engine.viewport_slot(viewport).unwrap().view);
    assert_eq!(state.marquee_candidate_elements.len(), 1);
    assert_eq!(state.marquee_candidate_elements[0].id, owner);
    assert_eq!(
        state.marquee_candidate_elements[0].rect.center,
        bubble.body.center
    );
    assert_eq!(
        state.marquee_candidate_elements[0].rect.height,
        bubble.body.height
    );
    pointer(&mut engine, viewport, PointerEventType::Up, end);
    assert_eq!(engine.selected_ids(), vec![owner]);
    assert_eq!(
        engine.serialize_document_history().unwrap(),
        original_history
    );
}

#[test]
fn speech_bubble_passive_growth_text_target_respects_visible_occluders() {
    let (mut engine, viewport, owner, _, bubble, text) =
        passive_growth(HighlightShape::Rectangle, 0.0, true);
    let point = speech_bubble_local_to_canvas(&bubble, Point::new(0.0, text.height() * 0.4));
    let id = engine.model.peek_next_element_id();
    let mut rect = body(&engine, owner).body;
    rect.center = point;
    rect.width = 40.0;
    rect.height = 40.0;
    rect.fill.a = 255;
    let mut transaction = Transaction::new("insert bubble text occluder");
    transaction.insert_rectangle(id, ElementMeta::default(), rect);
    engine
        .apply_editor_command(
            viewport,
            EditorCommand::ApplyTransaction(ApplyTransactionCommand {
                transaction,
                history_undo_snapshot: None,
                created_selection_target: None,
            }),
        )
        .unwrap();
    assert!(
        engine
            .speech_bubble_text_target(viewport, Some(point))
            .unwrap()
            .is_none()
    );
    pointer(&mut engine, viewport, PointerEventType::Down, point);
    pointer(&mut engine, viewport, PointerEventType::Up, point);
    assert_eq!(engine.selected_ids(), vec![id]);
}

#[test]
fn speech_bubble_passive_growth_hits_visible_label_with_unpainted_body() {
    for shape in [
        HighlightShape::Rectangle,
        HighlightShape::Ellipse,
        HighlightShape::Diamond,
    ] {
        let (mut engine, viewport, owner, label, bubble, text) = passive_growth(shape, 0.57, false);
        let point = speech_bubble_local_to_canvas(&bubble, Point::new(0.0, text.height() * 0.4));
        assert!(!speech_bubble_hit_test(&bubble, point, 8.0));
        assert_eq!(engine.hit_text_at(viewport, point).unwrap(), Some(label));
        let (info, _) = engine
            .speech_bubble_text_target(viewport, Some(point))
            .unwrap()
            .unwrap();
        assert_displayed_target(&info, &bubble, &text);
        pointer(&mut engine, viewport, PointerEventType::Down, point);
        pointer(&mut engine, viewport, PointerEventType::Up, point);
        assert_eq!(engine.selected_ids(), vec![owner]);
    }
}
