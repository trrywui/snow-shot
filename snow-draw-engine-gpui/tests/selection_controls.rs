use snow_draw_engine_gpui::{CanvasDocument, CanvasSession, engine::*};

fn fixture(tool: ActiveTool, zoom: f64) -> (CanvasDocument, CanvasSession) {
    let mut document = CanvasDocument::default();
    document
        .command(|engine| engine.set_automatically_select_newly_created_elements(true))
        .unwrap();
    document
        .command(|engine| engine.set_automatic_selection_excluded_tools(0))
        .unwrap();
    let mut session = CanvasSession::new(&mut document).unwrap();
    session.set_surface(&mut document, 800, 600).unwrap();
    session
        .set_camera(
            &mut document,
            Camera {
                center: Point::new(0.0, 0.0),
                zoom,
            },
        )
        .unwrap();
    session.set_tool(&mut document, tool).unwrap();
    drag(
        &mut document,
        &mut session,
        Point::new(-80.0, -45.0),
        Point::new(80.0, 45.0),
    );
    session.set_tool(&mut document, ActiveTool::Select).unwrap();
    assert_eq!(document.engine().selected_ids().len(), 1);
    (document, session)
}

fn pointer(
    document: &mut CanvasDocument,
    session: &mut CanvasSession,
    kind: PointerEventType,
    world: Point<f64>,
) {
    let frame = session.scene().frame();
    session
        .input(
            document,
            InputEvent::Pointer(PointerEvent {
                pointer_id: 1,
                event_type: kind,
                device: PointerDevice::Mouse,
                position: canvas_to_view(world, &frame.camera, frame.surface),
                button: Some(PointerButton::Primary),
                buttons: PointerButtons(if kind == PointerEventType::Up {
                    0
                } else {
                    PointerButtons::PRIMARY
                }),
                modifiers: Modifiers::default(),
                pressure: None,
                timestamp_ms: 0,
            }),
        )
        .unwrap();
}

fn drag(
    document: &mut CanvasDocument,
    session: &mut CanvasSession,
    start: Point<f64>,
    end: Point<f64>,
) {
    for (kind, point) in [
        (PointerEventType::Down, start),
        (PointerEventType::Move, end),
        (PointerEventType::Up, end),
    ] {
        pointer(document, session, kind, point);
    }
    assert!(!document.engine().has_pointer_capture());
}

fn handle(session: &CanvasSession, kind: UiShapeKind) -> Point<f64> {
    session
        .scene()
        .overlays()
        .iter()
        .find_map(|item| match item {
            OverlayDisplayItem::Rectangle(rect) if rect.kind == kind => {
                Some(Point::new(rect.center_x, rect.center_y))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("{kind:?} must be visible for the selected element"))
}

fn rectangle(session: &CanvasSession) -> RectangleDisplayItem {
    session
        .scene()
        .ordered_nodes()
        .map(|node| node.item.as_ref())
        .find_map(|item| match item {
            SceneDisplayItem::Rectangle(rect) => Some(*rect),
            _ => None,
        })
        .unwrap()
}

#[test]
fn radius_and_rotation_controls_commit_and_undo_at_each_zoom() {
    for zoom in [0.5, 1.0, 2.5] {
        for kind in [
            UiShapeKind::SelectionCornerRadiusHandle,
            UiShapeKind::SelectionRotationHandle,
        ] {
            let (mut document, mut session) = fixture(ActiveTool::Shape, zoom);
            let before = document.engine().document_checkpoint().unwrap();
            let original = rectangle(&session);
            let start = handle(&session, kind);
            let end = if kind == UiShapeKind::SelectionRotationHandle {
                Point::new(-start.y, start.x)
            } else {
                Point::new(start.x + 15.0, start.y + 15.0)
            };
            drag(&mut document, &mut session, start, end);
            let changed = rectangle(&session);
            if kind == UiShapeKind::SelectionRotationHandle {
                assert!((changed.rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-8);
            } else {
                assert!(changed.corner_radii.top_left > original.corner_radii.top_left);
            }
            assert_eq!(
                (changed.width, changed.height),
                (original.width, original.height)
            );
            document.command(|engine| engine.undo()).unwrap();
            assert_eq!(document.engine().document_checkpoint().unwrap(), before);
        }
    }
}

#[test]
fn shape_text_plus_control_creates_a_label_and_requests_inline_editing() {
    for zoom in [0.5, 1.0, 2.5] {
        for offset in [Point::new(0.0, 0.0), Point::new(60.0, 30.0)] {
            let (mut document, mut session) = fixture(ActiveTool::Shape, zoom);
            let before = document.engine().document_checkpoint().unwrap();
            let start = handle(&session, UiShapeKind::ShapeTextHandle);
            let end = Point::new(start.x + offset.x, start.y + offset.y);
            drag(&mut document, &mut session, start, end);
            let viewport = session.viewport();
            let label = document
                .command(|engine| engine.take_text_edit_request(viewport))
                .unwrap()
                .expect("releasing the plus control requests inline label editing");
            assert_eq!(
                document.engine().text_element_info(label).unwrap().center,
                end
            );
            assert!(rectangle(&session).bound_text_id.is_some());
            document.command(|engine| engine.undo()).unwrap();
            assert_eq!(document.engine().document_checkpoint().unwrap(), before);
        }
    }
}

#[test]
fn speech_bubble_plus_control_adds_an_editable_tail_and_undo_restores_it() {
    for zoom in [0.5, 1.0, 2.5] {
        let (mut document, mut session) = fixture(ActiveTool::SpeechBubble, zoom);
        let before = document.engine().document_checkpoint().unwrap();
        let tail_count = |session: &CanvasSession| {
            session
                .scene()
                .overlays()
                .iter()
                .filter(|item| {
                    matches!(item, OverlayDisplayItem::Rectangle(rect)
                        if rect.kind == UiShapeKind::SpeechBubbleTailTipHandle)
                })
                .count()
        };
        let original_count = tail_count(&session);
        let start = handle(&session, UiShapeKind::SpeechBubbleAddTailHandle);
        let end = Point::new(150.0, 110.0);
        drag(&mut document, &mut session, start, end);
        assert_eq!(tail_count(&session), original_count + 1);
        assert!(session.scene().overlays().iter().any(|item| {
            matches!(item, OverlayDisplayItem::Rectangle(rect)
                if rect.kind == UiShapeKind::SpeechBubbleTailTipHandle
                    && (rect.center_x - end.x).abs() < 1e-8
                    && (rect.center_y - end.y).abs() < 1e-8)
        }));
        document.command(|engine| engine.undo()).unwrap();
        assert_eq!(document.engine().document_checkpoint().unwrap(), before);
    }
}

#[test]
fn magnifier_grip_moves_lens_independently_with_undo() {
    let (mut document, mut session) = fixture(ActiveTool::Magnifier, 1.5);
    let before = document.engine().document_checkpoint().unwrap();
    let magnifier = |session: &CanvasSession| {
        session
            .scene()
            .ordered_nodes()
            .map(|node| node.item.as_ref())
            .find_map(|item| match item {
                SceneDisplayItem::Magnifier(value) => Some(value.clone()),
                _ => None,
            })
            .unwrap()
    };
    let original = magnifier(&session);
    let start = handle(&session, UiShapeKind::MagnifierMoveHandle);
    let delta = Point::new(27.0, -18.0);
    drag(
        &mut document,
        &mut session,
        start,
        Point::new(start.x + delta.x, start.y + delta.y),
    );
    let moved = magnifier(&session);
    assert_eq!(moved.source_center, original.source_center);
    assert_eq!(moved.lens.center_x, original.lens.center_x + delta.x);
    assert_eq!(moved.lens.center_y, original.lens.center_y + delta.y);
    assert_eq!(moved.factor, original.factor);
    document.command(|engine| engine.undo()).unwrap();
    assert_eq!(document.engine().document_checkpoint().unwrap(), before);
}
