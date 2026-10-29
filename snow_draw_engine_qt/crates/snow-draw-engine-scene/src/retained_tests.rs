use super::*;
use snow_draw_engine_document::{ElementMeta, Transaction};

#[test]
fn long_path_rigid_edits_preserve_local_chunks_and_geometry_revision() {
    let commands: Vec<_> = (0..8192)
        .map(|index| {
            let point = [
                f64::from(index) * 0.7,
                (f64::from(index) * 0.01).sin() * 70.0,
            ];
            if index == 0 {
                PathCommand::MoveTo { point }
            } else {
                PathCommand::LineTo { point }
            }
        })
        .collect();
    let geometry = Arc::new(snow_draw_engine_core::PathGeometry::from_commands(
        1, commands, false,
    ));
    let arrow = ArrowDisplayItem {
        id: DisplayItemId {
            index: 41,
            generation: 1,
        },
        geometry: geometry.clone(),
        is_free_draw: true,
        stroke_width: 2.0,
        ..Default::default()
    };
    let mut composer = RetainedSceneComposer::new();
    let mut output = Vec::new();
    composer.upsert(SceneDisplayItem::Arrow(arrow.clone()), &mut output);
    let original = output.pop().unwrap();
    let SceneDisplayItem::Arrow(local) = original.local_item.as_ref() else {
        panic!()
    };
    assert!(Arc::ptr_eq(&local.geometry, &geometry));
    composer.revision = 1;
    let (s, c) = 0.7_f64.sin_cos();
    let expected_pose = ScenePose {
        xx: c,
        yx: s,
        xy: -s,
        yy: c,
        tx: 140.25,
        ty: -75.125,
    };
    let changed: Vec<_> = geometry
        .chunks
        .iter()
        .flat_map(|chunk| chunk.commands.iter())
        .map(|command| match *command {
            PathCommand::MoveTo { point } => PathCommand::MoveTo {
                point: transform_point(expected_pose, point),
            },
            PathCommand::LineTo { point } => PathCommand::LineTo {
                point: transform_point(expected_pose, point),
            },
            _ => unreachable!(),
        })
        .collect();
    let moved = ArrowDisplayItem {
        geometry: Arc::new(snow_draw_engine_core::PathGeometry::from_commands(
            2,
            changed.clone(),
            false,
        )),
        ..arrow
    };
    composer.upsert(SceneDisplayItem::Arrow(moved.clone()), &mut output);
    let rigid = output.pop().unwrap();
    assert_eq!(rigid.geometry_revision, original.geometry_revision);
    assert_eq!(rigid.material_revision, original.material_revision);
    assert_ne!(rigid.pose_revision, original.pose_revision);
    assert!(Arc::ptr_eq(&rigid.local_item, &original.local_item));
    for point in [[0.0, 0.0], [423.0, 17.0]] {
        let actual = transform_point(rigid.pose, point);
        let expected = transform_point(expected_pose, point);
        assert!(near(actual[0], expected[0]) && near(actual[1], expected[1]));
    }
    composer.revision = 2;
    let styled = ArrowDisplayItem {
        stroke: ColorRgba8 {
            r: 80,
            g: 40,
            b: 20,
            a: 255,
        },
        ..moved.clone()
    };
    composer.upsert(SceneDisplayItem::Arrow(styled), &mut output);
    let styled = output.pop().unwrap();
    assert_eq!(styled.geometry_revision, rigid.geometry_revision);
    assert_eq!(styled.pose_revision, rigid.pose_revision);
    assert_ne!(styled.material_revision, rigid.material_revision);
    let SceneDisplayItem::Arrow(local) = styled.local_item.as_ref() else {
        panic!()
    };
    assert!(Arc::ptr_eq(&local.geometry, &geometry));
    composer.revision = 3;
    let mut reshaped = changed;
    if let Some(PathCommand::LineTo { point }) = reshaped.last_mut() {
        point[1] += 1.0;
    }
    composer.upsert(
        SceneDisplayItem::Arrow(ArrowDisplayItem {
            geometry: Arc::new(snow_draw_engine_core::PathGeometry::from_commands(
                4, reshaped, false,
            )),
            ..moved
        }),
        &mut output,
    );
    let edited = output.pop().unwrap();
    assert_ne!(edited.geometry_revision, rigid.geometry_revision);
}

fn id(index: u32) -> ElementId {
    ElementId {
        index,
        generation: 1,
    }
}
fn rectangle(index: u32) -> RectangleData {
    RectangleData {
        text_element_id: None,
        rectangle_kind: snow_draw_engine_document::RectangleElementKind::Rectangle,
        highlight_shape: snow_draw_engine_document::HighlightShape::Rectangle,
        center: Point::new(f64::from(index) * 10.0, 10.0),
        width: 8.0,
        height: 8.0,
        rotation: 0.0,
        fill: ColorRgba8 {
            r: 20,
            g: 40,
            b: 80,
            a: 255,
        },
        fill_style: FillStyle::Solid,
        stroke: ColorRgba8::default(),
        stroke_width: 1.0,
        stroke_style: StrokeStyle::Solid,
        stroke_rendering_style: Default::default(),
        stroke_rendering_seed: 0,
        corner_radii: Default::default(),
        opacity: 1.0,
    }
}
struct Fixture {
    model: DocumentModel,
    cache: DocumentSceneCache,
    composer: RetainedSceneComposer,
    editor: EditorSession,
    view: EditorViewportState,
}
impl Fixture {
    fn new(count: u32) -> Self {
        let mut model = DocumentModel::new();
        let mut tx = Transaction::new("fixture");
        for index in 0..count {
            tx.insert_rectangle(id(index), ElementMeta::default(), rectangle(index));
        }
        model.apply_transaction(tx).unwrap();
        let mut cache = DocumentSceneCache::new();
        cache.sync(&model, None);
        let mut fixture = Self {
            model,
            cache,
            composer: Default::default(),
            editor: EditorSession::new(Default::default()).unwrap(),
            view: EditorViewportState::default(),
        };
        fixture.view.surface = snow_draw_engine_core::SurfaceSize {
            width: 800,
            height: 600,
        };
        fixture.refresh();
        fixture
    }
    fn refresh(&mut self) {
        self.composer
            .refresh(&self.cache, &self.model, &self.editor, &self.view);
    }
    fn apply(&mut self, tx: Transaction) {
        let result = self.model.apply_transaction(tx).unwrap();
        self.cache.sync(&self.model, Some(&result.changes));
        self.composer.document_changed(&result.changes);
    }
    fn update(&mut self, index: u32, change: impl FnOnce(&mut RectangleData)) {
        let mut rect = *self.model.rectangle(id(index)).unwrap();
        change(&mut rect);
        let mut tx = Transaction::new("update");
        tx.update_rectangle(id(index), rect);
        self.apply(tx);
    }
}

#[test]
fn ten_thousand_items_local_edits_are_sparse_and_separate_pose_and_material() {
    let mut f = Fixture::new(10_000);
    let initial = f.composer.acquire(None);
    assert_eq!(initial.upserts.len(), 10_000);
    let original = initial
        .upserts
        .iter()
        .find(|node| node.id.element.index == 7000)
        .unwrap();
    let before = f.composer.diagnostics();
    f.update(7000, |r| {
        r.center.x += 20.0;
        r.rotation = 0.7;
    });
    f.refresh();
    let moved = f.composer.acquire(Some(initial.revision));
    assert!(!moved.reset);
    assert_eq!(moved.upserts.len(), 1);
    assert!(moved.order.is_none());
    assert_eq!(
        moved.upserts[0].geometry_revision,
        original.geometry_revision
    );
    assert_eq!(
        moved.upserts[0].material_revision,
        original.material_revision
    );
    assert_ne!(moved.upserts[0].pose_revision, original.pose_revision);
    assert!(Arc::ptr_eq(
        &moved.upserts[0].local_item,
        &original.local_item
    ));
    let after = f.composer.diagnostics();
    assert_eq!(after.owners_composed - before.owners_composed, 1);
    assert_eq!(after.nodes_compared - before.nodes_compared, 1);
    assert_eq!(after.full_compositions, before.full_compositions);
    assert_eq!(after.topology_rebuilds, before.topology_rebuilds);
    f.update(7000, |r| {
        r.fill.r = 200;
        r.opacity = 0.3;
    });
    f.refresh();
    let material = f.composer.acquire(Some(moved.revision));
    assert_eq!(
        material.upserts[0].geometry_revision,
        original.geometry_revision
    );
    assert_ne!(
        material.upserts[0].material_revision,
        original.material_revision
    );
    assert_eq!(
        material.upserts[0].pose_revision,
        moved.upserts[0].pose_revision
    );
}

#[test]
fn coalesced_disjoint_changes_and_camera_only_updates_do_not_scan_content() {
    let mut f = Fixture::new(10_000);
    let first = f.composer.acquire(None);
    let before = f.composer.diagnostics();
    for value in 0..100 {
        f.update(1, |r| r.center.y = f64::from(value));
    }
    f.update(9998, |r| r.width = 12.0);
    f.refresh();
    let delta = f.composer.acquire(Some(first.revision));
    assert_eq!(delta.upserts.len(), 2);
    assert_eq!(
        f.composer.diagnostics().owners_composed - before.owners_composed,
        2
    );
    assert_eq!(
        f.composer.diagnostics().nodes_compared - before.nodes_compared,
        2
    );
    let before = f.composer.diagnostics();
    f.view.camera.center.x += 50000.0;
    f.refresh();
    let camera = f.composer.acquire(Some(delta.revision));
    assert!(camera.upserts.is_empty());
    assert!(camera.order.is_none());
    assert_eq!(
        f.composer.diagnostics().owners_composed,
        before.owners_composed
    );
    assert_eq!(
        f.composer.diagnostics().nodes_compared,
        before.nodes_compared
    );
}

#[test]
fn journal_merges_coalesced_publications_and_resets_after_eviction() {
    let mut f = Fixture::new(3);
    let initial = f.composer.acquire(None);
    for index in 0..3 {
        f.update(index, |r| r.center.y += 1.0);
        f.refresh();
    }
    let merged = f.composer.acquire(Some(initial.revision));
    assert!(!merged.reset);
    assert_eq!(merged.upserts.len(), 3);
    for _ in 0..70 {
        f.update(1, |r| r.center.x += 1.0);
        f.refresh();
    }
    let reset = f.composer.acquire(Some(initial.revision));
    assert!(reset.reset);
    assert_eq!(reset.upserts.len(), 3);
    assert_eq!(reset.order.as_ref().unwrap().nodes.len(), 3);
    assert!(f.composer.journal.len() <= JOURNAL_LIMIT);
    assert!(f.composer.journal_nodes <= JOURNAL_NODE_LIMIT);
    let idle = f.composer.acquire(Some(reset.revision));
    assert!(!idle.reset);
    assert!(idle.upserts.is_empty() && idle.order.is_none());
}

#[test]
fn removal_and_id_reuse_keep_generations_and_reset_state_correct() {
    let mut f = Fixture::new(2);
    let initial = f.composer.acquire(None);
    let mut tx = Transaction::new("remove");
    tx.remove_element(id(0));
    f.apply(tx);
    f.refresh();
    let removed = f.composer.acquire(Some(initial.revision));
    assert_eq!(removed.removals.len(), 1);
    assert_eq!(removed.removals[0].element.index, 0);
    let next = f.model.peek_next_element_id();
    let mut tx = Transaction::new("insert");
    tx.insert_rectangle(next, ElementMeta::default(), rectangle(0));
    f.apply(tx);
    f.refresh();
    let current = f.composer.acquire(None);
    assert_eq!(current.upserts.len(), 2);
    assert!(
        current
            .upserts
            .iter()
            .any(|node| node.id.element.index == next.index
                && node.id.element.generation == next.generation)
    );
}

#[test]
fn retained_order_and_effect_runs_match_full_uncropped_composition() {
    let mut f = Fixture::new(3);
    let mut tx = Transaction::new("filters");
    for (index, kind) in [
        (3, snow_draw_engine_document::CanvasFilterType::GaussianBlur),
        (4, snow_draw_engine_document::CanvasFilterType::Inversion),
    ] {
        tx.insert_filter(
            id(index),
            ElementMeta::default(),
            snow_draw_engine_document::FilterData {
                center: Point::new(10.0, 10.0),
                width: 50.0,
                height: 50.0,
                filter_type: kind,
                ..Default::default()
            },
        );
    }
    f.apply(tx);
    f.refresh();
    let presentation = f.editor.presentation_state(&f.model, &f.view);
    let expected = compose_retained_items(&f.cache, &f.model, &presentation, f.model.paint_order());
    let expected_plan = compose_scene_render_plan(&f.cache, &f.model, &presentation, &expected);
    let snapshot = f.composer.acquire(None);
    assert_eq!(
        snapshot
            .upserts
            .iter()
            .map(|n| (*n.item).clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        snapshot.order.as_ref().unwrap().effect_runs.as_ref(),
        expected_plan
    );
}

#[test]
fn bound_label_updates_only_its_owner_and_connector_with_stable_order() {
    let mut f = Fixture::new(1000);
    let label_id = id(1000);
    let mut shape = *f.model.rectangle(id(37)).unwrap();
    shape.text_element_id = Some(label_id);
    let label = TextData {
        text: "linked label".into(),
        center: Point::new(520.0, 140.0),
        layout: snow_draw_engine_document::TextLayoutSize::new(100.0, 30.0),
        ..Default::default()
    };
    let mut tx = Transaction::new("bind");
    tx.insert_text(label_id, ElementMeta::default(), label.clone());
    tx.update_rectangle(id(37), shape);
    f.apply(tx);
    f.refresh();
    let initial = f.composer.acquire(None);
    let before = f.composer.diagnostics();
    let mut label = label;
    label.center.y += 15.0;
    let mut tx = Transaction::new("move label");
    tx.update_text(label_id, label);
    f.apply(tx);
    f.refresh();
    let delta = f.composer.acquire(Some(initial.revision));
    assert_eq!(
        f.composer.diagnostics().owners_composed - before.owners_composed,
        2
    );
    assert_eq!(
        f.composer.diagnostics().nodes_compared - before.nodes_compared,
        3
    );
    assert!(delta.order.is_none());
    assert_eq!(delta.upserts.len(), 2);
    assert!(
        delta
            .upserts
            .iter()
            .any(|node| node.id.role == RetainedNodeRole::Connector)
    );
    let presentation = f.editor.presentation_state(&f.model, &f.view);
    let expected = compose_retained_items(&f.cache, &f.model, &presentation, f.model.paint_order());
    assert_eq!(
        f.composer
            .acquire(None)
            .upserts
            .iter()
            .map(|node| (*node.item).clone())
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn path_transport_revision_does_not_invalidate_material_only_geometry() {
    let mut item = ArrowDisplayItem {
        stroke: ColorRgba8 {
            r: 80,
            g: 40,
            b: 20,
            a: 255,
        },
        ..Default::default()
    };
    item.geometry = Arc::new(snow_draw_engine_core::PathGeometry::from_commands(
        1,
        vec![
            snow_draw_engine_core::PathCommand::MoveTo { point: [0.0, 0.0] },
            snow_draw_engine_core::PathCommand::LineTo {
                point: [50.0, 30.0],
            },
        ],
        false,
    ));
    let before = separated(&SceneDisplayItem::Arrow(item.clone()));
    item.stroke.r = 100;
    item.geometry = Arc::new(snow_draw_engine_core::PathGeometry {
        revision: 99,
        ..(*item.geometry).clone()
    });
    let after = separated(&SceneDisplayItem::Arrow(item));
    assert_eq!(before.0, after.0);
    assert_ne!(before.1, after.1);
}
