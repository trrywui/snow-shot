use snow_draw_engine_core::{Camera, EngineConfig, Point};
use snow_draw_engine_document::{ElementId, ElementMeta, TextData, TextLayoutSize, Transaction};
use snow_draw_engine_editor::{ActiveTool, EditorSession, EditorViewportState};
use snow_draw_engine_interaction::{
    CursorCommand, CursorStyle, InputEvent, Modifiers, PointerButton, PointerButtons,
    PointerCaptureCommand, PointerDevice, PointerEvent, PointerEventType,
};
use snow_draw_engine_model::DocumentModel;

fn fixture(
    tool: ActiveTool,
    selected: bool,
    rotation: f64,
    zoom: f64,
) -> (DocumentModel, EditorSession, EditorViewportState, ElementId) {
    let mut document = DocumentModel::new();
    let id = document.allocate_element_id();
    let mut transaction = Transaction::new("insert text");
    transaction.insert_text(
        id,
        ElementMeta::default(),
        TextData {
            center: Point::new(30.0, -20.0),
            text: "Edit existing text".into(),
            layout: TextLayoutSize::new(100.0, 80.0),
            auto_resize: false,
            rotation,
            ..Default::default()
        },
    );
    document.apply_transaction(transaction).unwrap();
    let mut editor = EditorSession::new(EngineConfig::default()).unwrap();
    editor.set_active_tool(tool).unwrap();
    if selected {
        editor.select_element(&document, id).unwrap();
    }
    let mut view = EditorViewportState::default();
    view.set_surface_size(400, 300);
    view.set_camera(Camera {
        center: Point::new(30.0, -20.0),
        zoom,
    })
    .unwrap();
    (document, editor, view, id)
}

fn pointer(kind: PointerEventType, button: PointerButton, modifiers: Modifiers) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        pointer_id: 1,
        event_type: kind,
        device: PointerDevice::Mouse,
        position: Point::new(200.0, 150.0),
        button: Some(button),
        buttons: PointerButtons(if kind == PointerEventType::Up {
            0
        } else if button == PointerButton::Primary {
            PointerButtons::PRIMARY
        } else {
            PointerButtons::SECONDARY
        }),
        modifiers,
        pressure: None,
        timestamp_ms: 0,
    })
}

#[test]
fn accepted_text_click_publishes_one_existing_edit_request_without_a_document_command() {
    for (tool, selected) in [
        (ActiveTool::Text, false),
        (ActiveTool::Text, true),
        (ActiveTool::Select, true),
    ] {
        for (rotation, zoom) in [(0.0, 0.5), (0.7, 1.0), (-0.4, 2.0)] {
            let (document, mut editor, mut view, id) = fixture(tool, selected, rotation, zoom);
            let update = editor
                .process_input(
                    &document,
                    &mut view,
                    pointer(
                        PointerEventType::Down,
                        PointerButton::Primary,
                        Modifiers::default(),
                    ),
                )
                .unwrap();
            assert!(update.interaction.consumed);
            assert_eq!(update.interaction.capture, PointerCaptureCommand::NoChange);
            assert_eq!(
                update.interaction.cursor,
                CursorCommand::Set(CursorStyle::Text)
            );
            assert!(update.command.is_none());
            assert_eq!(editor.selected_ids(), vec![id]);
            assert_eq!(editor.take_text_edit_request(), Some(id));
            assert_eq!(editor.take_text_edit_request(), None);
            assert!(!editor.take_new_text_draft_request());
            editor
                .process_input(
                    &document,
                    &mut view,
                    pointer(
                        PointerEventType::Up,
                        PointerButton::Primary,
                        Modifiers::default(),
                    ),
                )
                .unwrap();
            assert_eq!(editor.take_text_edit_request(), None);
        }
    }
}

#[test]
fn selection_and_copy_gestures_do_not_publish_text_edit_requests() {
    for selected in [false, true] {
        for (button, modifiers) in [
            (
                PointerButton::Primary,
                Modifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            (
                PointerButton::Primary,
                Modifiers {
                    alt: true,
                    ..Default::default()
                },
            ),
            (PointerButton::Secondary, Modifiers::default()),
        ] {
            let (document, mut editor, mut view, _) = fixture(ActiveTool::Text, selected, 0.0, 1.0);
            let update = editor
                .process_input(
                    &document,
                    &mut view,
                    pointer(PointerEventType::Down, button, modifiers),
                )
                .unwrap();
            assert!(update.interaction.consumed);
            assert_eq!(editor.take_text_edit_request(), None);
            assert!(!editor.take_new_text_draft_request());
            if modifiers.alt {
                assert_eq!(
                    update.interaction.capture,
                    PointerCaptureCommand::Capture(1)
                );
            }
        }
    }
}

#[test]
fn select_tool_first_click_selects_and_next_click_requests_editing() {
    let (document, mut editor, mut view, id) = fixture(ActiveTool::Select, false, 0.0, 1.0);
    for kind in [PointerEventType::Down, PointerEventType::Up] {
        editor
            .process_input(
                &document,
                &mut view,
                pointer(kind, PointerButton::Primary, Modifiers::default()),
            )
            .unwrap();
        assert_eq!(editor.take_text_edit_request(), None);
    }
    assert_eq!(editor.selected_ids(), vec![id]);
    editor
        .process_input(
            &document,
            &mut view,
            pointer(
                PointerEventType::Down,
                PointerButton::Primary,
                Modifiers::default(),
            ),
        )
        .unwrap();
    assert_eq!(editor.take_text_edit_request(), Some(id));
}
