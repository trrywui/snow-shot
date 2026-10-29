use super::*;
use snow_draw_engine::{
    DisplayItemId, RetainedNodeRole, RetainedSceneOrder, SceneDisplayItem, ScenePose,
};

fn node(index: u32, revision: u64) -> Arc<RetainedSceneNode> {
    let element = DisplayItemId {
        index,
        generation: 1,
    };
    let item = Arc::new(SceneDisplayItem::Rectangle(
        snow_draw_engine::RectangleDisplayItem {
            id: element,
            width: 8.0,
            height: 8.0,
            center_x: f64::from(index),
            center_y: revision as f64,
            ..Default::default()
        },
    ));
    Arc::new(RetainedSceneNode {
        id: RetainedNodeId {
            element,
            role: RetainedNodeRole::Content,
        },
        geometry_revision: 1,
        material_revision: 1,
        pose_revision: revision,
        pose: ScenePose::default(),
        local_item: item.clone(),
        item,
        bounds: [f64::from(index) - 4.0, -4.0, f64::from(index) + 4.0, 4.0],
    })
}

fn delta(count: u32) -> RetainedSceneDelta {
    let upserts: Vec<_> = (0..count).map(|index| node(index, 1)).collect();
    RetainedSceneDelta {
        base_revision: 0,
        revision: 1,
        scene_revision: 1,
        reset: true,
        frame: FrameView::default(),
        order: Some(RetainedSceneOrder {
            nodes: upserts.iter().map(|n| n.id).collect::<Vec<_>>().into(),
            effect_runs: Arc::default(),
        }),
        upserts,
        removals: Vec::new(),
        overlay_revision: 1,
        overlays: Some(Arc::default()),
        decoration_revision: 1,
        decoration: Some(DecorationView::default()),
        spotlights: Some(Arc::default()),
    }
}

fn update(base: u64, upserts: Vec<Arc<RetainedSceneNode>>) -> RetainedSceneDelta {
    RetainedSceneDelta {
        base_revision: base,
        revision: base + 1,
        scene_revision: base + 1,
        reset: false,
        upserts,
        order: None,
        overlays: None,
        decoration: None,
        spotlights: None,
        ..delta(0)
    }
}

#[test]
fn sparse_scene_updates_copy_one_bucket_and_preserve_snapshots_without_flattening() {
    let mut scene = RetainedScene::default();
    scene.apply_delta(&delta(1024)).unwrap();
    let snapshot = scene.clone();
    let next = node(513, 2);
    let id = next.id;
    scene.apply_delta(&update(1, vec![next.clone()])).unwrap();
    assert_eq!(scene.node(id).unwrap().pose_revision, 2);
    assert_eq!(snapshot.node(id).unwrap().pose_revision, 1);
    assert_eq!(scene.order_ids().len(), 1024);
    assert_eq!(scene.nodes().count(), 1024);
    let old = &snapshot.gpu;
    let new = &scene.gpu;
    let copied = old
        .buckets
        .iter()
        .zip(new.buckets.iter())
        .filter(|(old, new)| !Arc::ptr_eq(old, new))
        .count();
    assert_eq!(copied, 1);
    assert!(Arc::ptr_eq(&old.order, &new.order));
    assert_eq!(scene.retained_identity(), snapshot.retained_identity());
    assert_eq!(scene.changes_since(1).unwrap(), vec![id]);
    assert_eq!(scene.ordered_nodes().count(), 1024);
    assert!(
        scene
            .ordered_nodes()
            .zip(snapshot.ordered_nodes())
            .all(|(next, old)| next.id == old.id)
    );
}

#[test]
fn sparse_journal_preserves_removals_and_rejects_invalid_updates_atomically() {
    let mut scene = RetainedScene::default();
    scene.apply_delta(&delta(4)).unwrap();
    let changed = node(0, 2);
    scene
        .apply_delta(&update(1, vec![changed.clone()]))
        .unwrap();
    assert_eq!(scene.changes_since(1).unwrap(), vec![changed.id]);
    let removed = node(3, 1).id;
    let mut removal = update(2, Vec::new());
    removal.removals.push(removed);
    removal.order = Some(RetainedSceneOrder {
        nodes: scene
            .order_ids()
            .iter()
            .filter(|id| **id != removed)
            .copied()
            .collect::<Vec<_>>()
            .into(),
        effect_runs: Arc::default(),
    });
    scene.apply_delta(&removal).unwrap();
    // Removing a node changes painter order: GPU consumers must synchronize
    // their order index and retain removed nodes' old damage bounds.
    assert!(scene.changes_since(1).is_none());
    assert!(scene.changes_since(2).is_none());
    assert!(scene.node(removed).is_none());
    assert_eq!(scene.retained_revision(), Some(3));
    assert!(scene.apply_delta(&update(3, vec![node(99, 4)])).is_err());
    assert_eq!(scene.retained_revision(), Some(3));
    assert!(scene.node(node(99, 4).id).is_none());
    assert_eq!(scene.nodes().count(), 3);
}

#[test]
fn sparse_journal_requires_resync_after_bounded_eviction() {
    let mut scene = RetainedScene::default();
    scene.apply_delta(&delta(1)).unwrap();
    for revision in 1..80 {
        scene
            .apply_delta(&update(revision, vec![node(0, revision + 1)]))
            .unwrap();
    }
    assert!(scene.changes_since(1).is_none());
    assert_eq!(scene.changes_since(79).unwrap(), vec![node(0, 80).id]);
    assert!(scene.changes_since(80).unwrap().is_empty());
    assert!(scene.gpu.journal.len() <= JOURNAL_ENTRIES);
}
