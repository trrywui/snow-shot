use super::duplicate_drag_tests::pointer;
use super::*;
use snow_draw_engine_core::arrow::{ArrowType, StrokeStyle};
use snow_draw_engine_document::{
    ArrowData, ElementData, HighlightShape, MagnifierData, Transaction,
};
use snow_draw_engine_editor::magnifier_move_grip;
use snow_draw_engine_interaction::PointerEventType;

#[derive(Clone, Copy, Debug)]
enum Companion {
    Rectangle,
    Arrow,
    Magnifier,
}

fn fixture(shape: HighlightShape, companion: Companion) -> (Engine, ViewportId, [ElementId; 2]) {
    let mut engine = Engine::default();
    let viewport = engine.create_viewport(ViewportConfig::default()).unwrap();
    engine
        .set_viewport_surface_size(viewport, 800, 600)
        .unwrap();
    engine
        .set_viewport_active_tool(viewport, ActiveTool::Select)
        .unwrap();
    let first = engine.model.peek_next_element_id();
    let second = ElementId {
        index: first.index + 1,
        generation: first.generation,
    };
    let value = MagnifierData {
        source_center: Point::new(0.0, 0.0),
        magnified_center: Point::new(130.0, 0.0),
        width: 80.0,
        height: 60.0,
        shape,
        ..Default::default()
    };
    let mut transaction = Transaction::new("magnifier group move fixture");
    transaction.insert_magnifier(first, Default::default(), value);
    match companion {
        Companion::Rectangle => {
            let mut rectangle = value.source_rect();
            rectangle.center = Point::new(300.0, 0.0);
            rectangle.height = 100.0;
            transaction.insert_rectangle(second, Default::default(), rectangle);
        }
        Companion::Arrow => {
            let arrow = ArrowData::from_global_points(
                &[Point::new(260.0, -20.0), Point::new(340.0, 20.0)],
                value.stroke,
                2.0,
                StrokeStyle::Solid,
                ArrowType::Straight,
                None,
                None,
            )
            .unwrap();
            transaction.insert_arrow(second, Default::default(), arrow);
        }
        Companion::Magnifier => {
            transaction.insert_magnifier(
                second,
                Default::default(),
                MagnifierData {
                    source_center: Point::new(300.0, 0.0),
                    magnified_center: Point::new(430.0, 0.0),
                    ..value
                },
            );
        }
    }
    apply(&mut engine, viewport, transaction);
    let snapshot = serde_json::from_value(serde_json::json!({
        "selectedIds": [first, second], "primaryId": first
    }))
    .unwrap();
    engine
        .editor
        .restore_history_selection(&engine.model, &snapshot);
    engine.refresh_all_viewports().unwrap();
    (engine, viewport, [first, second])
}

fn apply(engine: &mut Engine, viewport: ViewportId, transaction: Transaction) {
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
}

fn drag(engine: &mut Engine, viewport: ViewportId, from: Point<f64>, delta: Point<f64>) {
    for (kind, point) in [
        (PointerEventType::Down, from),
        (
            PointerEventType::Move,
            Point::new(from.x + delta.x, from.y + delta.y),
        ),
        (
            PointerEventType::Up,
            Point::new(from.x + delta.x, from.y + delta.y),
        ),
    ] {
        pointer(
            engine,
            viewport,
            kind,
            point.x + 400.0,
            point.y + 300.0,
            false,
        );
    }
}

fn assert_group_translated(
    engine: &Engine,
    ids: [ElementId; 2],
    originals: &[ElementData; 2],
    delta: Point<f64>,
) {
    assert_eq!(engine.selected_ids(), ids);
    for (id, original) in ids.into_iter().zip(originals) {
        let expected = match original {
            ElementData::Magnifier(value) => ElementData::Magnifier(value.translated(delta)),
            ElementData::Rectangle(value) => {
                let mut value = *value;
                value.center.x += delta.x;
                value.center.y += delta.y;
                ElementData::Rectangle(value)
            }
            ElementData::Arrow(value) => {
                let mut value = value.clone();
                value.x += delta.x;
                value.y += delta.y;
                ElementData::Arrow(value)
            }
            _ => unreachable!(),
        };
        assert_eq!(engine.model.element(id).unwrap().data, expected);
    }
}

#[test]
fn magnifier_group_selected_sources_and_companion_bodies_move_every_member() {
    for shape in [
        HighlightShape::Rectangle,
        HighlightShape::Ellipse,
        HighlightShape::Diamond,
    ] {
        for companion in [Companion::Rectangle, Companion::Arrow, Companion::Magnifier] {
            for from in [Point::new(0.0, 0.0), Point::new(300.0, 0.0)] {
                let (mut engine, viewport, ids) = fixture(shape, companion);
                let originals = ids.map(|id| engine.model.element(id).unwrap().data.clone());
                let delta = Point::new(23.0, 17.0);
                drag(&mut engine, viewport, from, delta);
                assert_group_translated(&engine, ids, &originals, delta);
                assert!(engine.undo().unwrap());
                assert_group_translated(&engine, ids, &originals, Point::new(0.0, 0.0));
                assert!(engine.redo().unwrap());
                assert_group_translated(&engine, ids, &originals, delta);
            }
        }
    }
}

#[test]
fn magnifier_group_lens_and_grip_move_only_the_target_lens() {
    for companion in [Companion::Rectangle, Companion::Arrow, Companion::Magnifier] {
        for grip in [false, true] {
            let (mut engine, viewport, ids) = fixture(HighlightShape::Rectangle, companion);
            let original = *engine.model.magnifier(ids[0]).unwrap();
            let other = engine.model.element(ids[1]).unwrap().data.clone();
            let from = if grip {
                magnifier_move_grip(original, 1.0)
            } else {
                original.magnified_center
            };
            let delta = Point::new(23.0, 17.0);
            drag(&mut engine, viewport, from, delta);
            let expected = MagnifierData {
                magnified_center: Point::new(
                    original.magnified_center.x + delta.x,
                    original.magnified_center.y + delta.y,
                ),
                ..original
            };
            assert_eq!(*engine.model.magnifier(ids[0]).unwrap(), expected);
            assert_eq!(engine.model.element(ids[1]).unwrap().data, other);
            assert_eq!(engine.selected_ids(), ids);
            assert!(engine.undo().unwrap());
            assert_eq!(*engine.model.magnifier(ids[0]).unwrap(), original);
        }
    }
}

#[test]
fn magnifier_group_coincident_source_wins_but_actual_shape_outside_hits_the_lens() {
    for (shape, outside_source) in [
        (HighlightShape::Ellipse, Point::new(35.0, 25.0)),
        (HighlightShape::Diamond, Point::new(30.0, 20.0)),
    ] {
        for source in [true, false] {
            let (mut engine, viewport, ids) = fixture(shape, Companion::Rectangle);
            let mut original = *engine.model.magnifier(ids[0]).unwrap();
            original.magnified_center = original.source_center;
            let mut transaction = Transaction::new("coincident source and lens");
            transaction.update_magnifier(ids[0], original);
            apply(&mut engine, viewport, transaction);
            let originals = ids.map(|id| engine.model.element(id).unwrap().data.clone());
            let delta = Point::new(23.0, 17.0);
            let from = if source {
                original.source_center
            } else {
                outside_source
            };
            drag(&mut engine, viewport, from, delta);
            if source {
                assert_group_translated(&engine, ids, &originals, delta);
            } else {
                assert_eq!(
                    engine.model.magnifier(ids[0]).unwrap().source_center,
                    original.source_center
                );
                assert_eq!(
                    engine.model.magnifier(ids[0]).unwrap().magnified_center,
                    delta
                );
                assert_eq!(engine.model.element(ids[1]).unwrap().data, originals[1]);
                assert_eq!(engine.selected_ids(), ids);
            }
        }
    }
}

#[test]
fn magnifier_group_unselected_top_element_keeps_paint_order_priority() {
    for center in [
        Point::new(0.0, 0.0),
        Point::new(130.0, 0.0),
        Point::new(300.0, 0.0),
    ] {
        let (mut engine, viewport, ids) = fixture(HighlightShape::Rectangle, Companion::Rectangle);
        let originals = ids.map(|id| engine.model.element(id).unwrap().data.clone());
        let top = engine.model.peek_next_element_id();
        let mut rectangle = engine.model.magnifier(ids[0]).unwrap().source_rect();
        rectangle.center = center;
        rectangle.width = 30.0;
        rectangle.height = 30.0;
        let mut transaction = Transaction::new("unselected top annotation");
        transaction.insert_rectangle(top, Default::default(), rectangle);
        apply(&mut engine, viewport, transaction);
        let delta = Point::new(23.0, 17.0);
        drag(&mut engine, viewport, center, delta);
        assert_eq!(engine.selected_ids(), vec![top]);
        assert_eq!(
            engine.model.rectangle(top).unwrap().center,
            Point::new(center.x + delta.x, center.y + delta.y)
        );
        for (id, original) in ids.into_iter().zip(originals) {
            assert_eq!(engine.model.element(id).unwrap().data, original);
        }
    }
}

#[test]
fn magnifier_group_empty_space_inside_selection_frame_still_moves_the_group() {
    let (mut engine, viewport, ids) = fixture(HighlightShape::Rectangle, Companion::Rectangle);
    let originals = ids.map(|id| engine.model.element(id).unwrap().data.clone());
    let delta = Point::new(23.0, 17.0);
    drag(&mut engine, viewport, Point::new(235.0, 0.0), delta);
    assert_group_translated(&engine, ids, &originals, delta);
}

#[test]
fn magnifier_group_empty_space_outside_selection_frame_does_not_move_any_member() {
    let (mut engine, viewport, ids) = fixture(HighlightShape::Rectangle, Companion::Rectangle);
    let originals = ids.map(|id| engine.model.element(id).unwrap().data.clone());
    drag(
        &mut engine,
        viewport,
        Point::new(235.0, 130.0),
        Point::new(23.0, 17.0),
    );
    assert!(engine.selected_ids().is_empty());
    for (id, original) in ids.into_iter().zip(originals) {
        assert_eq!(engine.model.element(id).unwrap().data, original);
    }
}

#[test]
fn magnifier_group_many_mixed_previews_preserve_selection_order_and_geometry() {
    let mut engine = Engine::default();
    let viewport = engine.create_viewport(ViewportConfig::default()).unwrap();
    engine
        .set_viewport_surface_size(viewport, 800, 600)
        .unwrap();
    engine
        .set_viewport_active_tool(viewport, ActiveTool::Select)
        .unwrap();
    let mut transaction = Transaction::new("many mixed magnifier preview fixture");
    let mut originals = Vec::new();
    let mut ids = Vec::new();
    for index in 0..64 {
        let id = engine.model.allocate_element_id();
        let value = MagnifierData {
            source_center: Point::new(f64::from(index) * 200.0, 0.0),
            magnified_center: Point::new(f64::from(index) * 200.0 + 100.0, 80.0),
            width: 40.0,
            height: 30.0,
            rotation: f64::from(index % 4) * 0.1,
            factor: 1.5 + f64::from(index % 3),
            shape: [
                HighlightShape::Rectangle,
                HighlightShape::Ellipse,
                HighlightShape::Diamond,
            ][index as usize % 3],
            ..Default::default()
        };
        transaction.insert_magnifier(id, Default::default(), value);
        originals.push((id, value));
        ids.push(id);
        let companion = engine.model.allocate_element_id();
        if index % 2 == 0 {
            let mut rectangle = value.source_rect();
            rectangle.center.y = 200.0;
            transaction.insert_rectangle(companion, Default::default(), rectangle);
        } else {
            let arrow = ArrowData::from_global_points(
                &[
                    Point::new(value.source_center.x, 200.0),
                    Point::new(value.source_center.x + 60.0, 220.0),
                ],
                value.stroke,
                2.0,
                StrokeStyle::Solid,
                ArrowType::Straight,
                None,
                None,
            )
            .unwrap();
            transaction.insert_arrow(companion, Default::default(), arrow);
        }
        ids.push(companion);
    }
    apply(&mut engine, viewport, transaction);
    ids.reverse();
    originals.reverse();
    let snapshot = serde_json::from_value(serde_json::json!({
        "selectedIds": ids, "primaryId": ids[0]
    }))
    .unwrap();
    engine
        .editor
        .restore_history_selection(&engine.model, &snapshot);
    engine.refresh_all_viewports().unwrap();

    for completion in [PointerEventType::Cancel, PointerEventType::Up] {
        pointer(
            &mut engine,
            viewport,
            PointerEventType::Down,
            400.0,
            300.0,
            false,
        );
        pointer(
            &mut engine,
            viewport,
            PointerEventType::Move,
            423.0,
            317.0,
            false,
        );
        let presentation = engine
            .editor
            .presentation_state(&engine.model, &engine.viewport_slot(viewport).unwrap().view);
        assert_eq!(engine.selected_ids(), ids);
        assert_eq!(presentation.preview_magnifiers.len(), originals.len());
        assert_eq!(
            presentation.selected_magnifiers,
            presentation.preview_magnifiers
        );
        for (preview, (id, original)) in presentation.preview_magnifiers.iter().zip(&originals) {
            assert_eq!(preview.id, *id);
            assert_eq!(
                preview.magnifier,
                original.translated(Point::new(23.0, 17.0))
            );
            assert_eq!(*engine.model.magnifier(*id).unwrap(), *original);
        }
        pointer(&mut engine, viewport, completion, 423.0, 317.0, false);
        let presentation = engine
            .editor
            .presentation_state(&engine.model, &engine.viewport_slot(viewport).unwrap().view);
        assert!(presentation.preview_magnifiers.is_empty());
        for (selected, (id, original)) in presentation.selected_magnifiers.iter().zip(&originals) {
            let expected = if completion == PointerEventType::Up {
                original.translated(Point::new(23.0, 17.0))
            } else {
                *original
            };
            assert_eq!(selected.id, *id);
            assert_eq!(selected.magnifier, expected);
            assert_eq!(*engine.model.magnifier(*id).unwrap(), expected);
        }
    }
    assert!(engine.undo().unwrap());
    for (id, original) in &originals {
        assert_eq!(*engine.model.magnifier(*id).unwrap(), *original);
    }
    assert!(engine.redo().unwrap());
    for (id, original) in &originals {
        assert_eq!(
            *engine.model.magnifier(*id).unwrap(),
            original.translated(Point::new(23.0, 17.0))
        );
    }
}
