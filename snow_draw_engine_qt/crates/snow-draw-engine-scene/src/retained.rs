//! Uncropped retained logic scene. Only the renderer decides visible GPU work.
use super::*;
use snow_draw_engine_core::PathCommand;
use snow_draw_engine_display::{
    RetainedNodeId, RetainedNodeRole, RetainedSceneDelta, RetainedSceneDiagnostics,
    RetainedSceneNode, RetainedSceneOrder, ScenePose,
};
use snow_draw_engine_document::DocumentDelta;
use std::collections::{HashSet, VecDeque};

const JOURNAL_LIMIT: usize = 64;
const JOURNAL_NODE_LIMIT: usize = 4096;

#[cfg(test)]
#[path = "retained_tests.rs"]
mod tests;

#[derive(Debug, Default)]
pub struct RetainedSceneComposer {
    nodes: HashMap<RetainedNodeId, Arc<RetainedSceneNode>>,
    owner_nodes: HashMap<ElementId, Vec<RetainedNodeId>>,
    order: Option<RetainedSceneOrder>,
    pending: HashSet<ElementId>,
    preview_owners: HashSet<ElementId>,
    topology_dirty: bool,
    document_dirty: bool,
    initialized: bool,
    frame: FrameView,
    document_revision: u64,
    order_generation: u64,
    scene_input_revision: u64,
    overlay_input_revision: u64,
    revision: u64,
    scene_revision: u64,
    overlay_revision: u64,
    decoration_revision: u64,
    overlays: Arc<[OverlayDisplayItem]>,
    decoration: DecorationView,
    spotlights: Arc<[DisplaySpotlightCutout]>,
    journal: VecDeque<Arc<RetainedSceneDelta>>,
    journal_nodes: usize,
    diagnostics: RetainedSceneDiagnostics,
}

impl RetainedSceneComposer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn invalidate_document(&mut self) {
        self.initialized = false;
        self.pending.clear();
        self.document_dirty = true;
    }

    pub fn document_changed(&mut self, delta: &DocumentDelta) {
        self.document_dirty = true;
        self.pending.extend(
            delta
                .touched
                .iter()
                .chain(&delta.created)
                .chain(&delta.removed)
                .copied(),
        );
        self.topology_dirty |= delta.z_order_changed
            || delta.relations_changed
            || !delta.created.is_empty()
            || !delta.removed.is_empty();
    }

    pub fn diagnostics(&self) -> RetainedSceneDiagnostics {
        self.diagnostics
    }

    /// May be called after any number of logical mutations; only the latest
    /// presentation is composed. Owner-thread input/capture remains synchronous.
    pub fn refresh(
        &mut self,
        cache: &DocumentSceneCache,
        model: &DocumentModel,
        session: &EditorSession,
        view: &EditorViewportState,
    ) {
        let state = session.view_state(view);
        let frame = FrameView {
            surface: state.surface,
            camera: state.camera,
            clear_color: state.clear_color,
        };
        let scene_input = session.scene_input_revision();
        let overlay_input = session.overlay_input_revision();
        let document_revision = model.document_revision().0;
        if self.initialized
            && !self.document_dirty
            && self.document_revision == document_revision
            && self.frame == frame
            && self.scene_input_revision == scene_input
            && self.overlay_input_revision == overlay_input
        {
            return;
        }
        let presentation = session.presentation_state(model, view);
        let reset = !self.initialized;
        let full = reset
            || self.topology_dirty
            || cache.order_plan_generation() != self.order_generation
            || (self.document_revision != document_revision && !self.document_dirty);
        let content_changed =
            full || self.document_dirty || self.scene_input_revision != scene_input;
        let mut upserts = Vec::new();
        let mut removals = Vec::new();
        let mut order_changed = false;
        if content_changed {
            let previews = preview_owners(model, &presentation);
            self.pending.extend(self.preview_owners.iter().copied());
            self.pending.extend(previews.iter().copied());
            expand_dependents(model, &mut self.pending);
            let owners: Vec<_> = if full {
                model.paint_order().to_vec()
            } else {
                self.pending.iter().copied().collect()
            };
            self.diagnostics.owners_composed += owners.len() as u64;
            let items = compose_retained_items(cache, model, &presentation, &owners);
            let incoming: HashSet<_> = items.iter().filter_map(node_id).collect();
            let previous: HashSet<_> = if full {
                self.nodes.keys().copied().collect()
            } else {
                owners
                    .iter()
                    .filter_map(|id| self.owner_nodes.get(id))
                    .flatten()
                    .copied()
                    .collect()
            };
            // A topology change can also be a new/cancelled transient primitive.
            // Only those transitions need full order projection, never local edits.
            let effect_topology_changed = items.iter().any(|item| {
                node_id(item)
                    .and_then(|id| self.nodes.get(&id))
                    .is_some_and(|old| {
                        let owner = element_id(old.id);
                        crate::scene_order::OrderNode::new(owner, &old.item)
                            != crate::scene_order::OrderNode::new(owner, item)
                    })
            });
            if !full && (incoming != previous || effect_topology_changed) {
                self.diagnostics.owners_composed += model.paint_order().len() as u64;
                let all = compose_retained_items(cache, model, &presentation, model.paint_order());
                self.replace_items(
                    all,
                    cache,
                    model,
                    &presentation,
                    &mut upserts,
                    &mut removals,
                );
                order_changed = true;
            } else {
                for id in previous.difference(&incoming) {
                    self.nodes.remove(id);
                    removals.push(*id);
                }
                for owner in &owners {
                    self.owner_nodes.remove(owner);
                }
                if full {
                    self.replace_items(
                        items,
                        cache,
                        model,
                        &presentation,
                        &mut upserts,
                        &mut removals,
                    );
                    order_changed = true;
                } else {
                    for item in items {
                        self.upsert(item, &mut upserts);
                    }
                }
            }
            self.preview_owners = previews;
        }
        let overlays: Arc<[_]> =
            compose_overlay_items(session.snap_config(), &presentation, frame).into();
        let overlay_changed = reset || self.overlays != overlays;
        if overlay_changed {
            self.overlays = overlays;
            self.overlay_revision += 1;
        }
        let decoration =
            display_decoration_with_spotlight(model, &presentation, cache.has_visible_spotlight());
        // Uncropped cutouts use a query frame spanning the entire supported
        // finite document coordinate domain; geometry itself remains in canvas units.
        let cutout_frame = FrameView {
            camera: snow_draw_engine_core::Camera {
                center: Point::default(),
                zoom: 1.0e-12,
            },
            surface: snow_draw_engine_core::SurfaceSize {
                width: 1,
                height: 1,
            },
            ..frame
        };
        let spotlights: Arc<[_]> = if content_changed {
            compose_spotlight_cutouts(cache, model, &presentation, cutout_frame).into()
        } else {
            self.spotlights.clone()
        };
        let decoration_changed =
            reset || self.decoration != decoration || self.spotlights != spotlights;
        if decoration_changed {
            self.decoration = decoration;
            self.spotlights = spotlights;
            self.decoration_revision += 1;
        }
        let changed = reset
            || !upserts.is_empty()
            || !removals.is_empty()
            || order_changed
            || overlay_changed
            || decoration_changed
            || self.frame != frame;
        self.document_revision = document_revision;
        self.order_generation = cache.order_plan_generation();
        self.scene_input_revision = scene_input;
        self.overlay_input_revision = overlay_input;
        self.document_dirty = false;
        self.topology_dirty = false;
        self.pending.clear();
        self.initialized = true;
        self.frame = frame;
        if !changed {
            return;
        }
        let base_revision = self.revision;
        self.revision += 1;
        if reset || !upserts.is_empty() || !removals.is_empty() || order_changed {
            self.scene_revision += 1;
        }
        self.diagnostics.publications += 1;
        let delta = Arc::new(RetainedSceneDelta {
            base_revision,
            revision: self.revision,
            scene_revision: self.scene_revision,
            reset,
            frame,
            upserts,
            removals,
            order: order_changed.then(|| self.order.clone().expect("initialized order")),
            overlay_revision: self.overlay_revision,
            overlays: overlay_changed.then(|| self.overlays.clone()),
            decoration_revision: self.decoration_revision,
            decoration: decoration_changed.then_some(self.decoration),
            spotlights: decoration_changed.then(|| self.spotlights.clone()),
        });
        self.journal_nodes += journal_weight(&delta);
        self.journal.push_back(delta);
        while self.journal.len() > JOURNAL_LIMIT || self.journal_nodes > JOURNAL_NODE_LIMIT {
            let old = self.journal.pop_front().expect("nonempty journal");
            self.journal_nodes -= journal_weight(&old);
        }
    }

    fn replace_items(
        &mut self,
        items: Vec<SceneDisplayItem>,
        cache: &DocumentSceneCache,
        model: &DocumentModel,
        presentation: &EditorPresentationState,
        upserts: &mut Vec<Arc<RetainedSceneNode>>,
        removals: &mut Vec<RetainedNodeId>,
    ) {
        self.diagnostics.full_compositions += 1;
        let incoming: HashSet<_> = items.iter().filter_map(node_id).collect();
        for id in self
            .nodes
            .keys()
            .filter(|id| !incoming.contains(id))
            .copied()
            .collect::<Vec<_>>()
        {
            self.nodes.remove(&id);
            removals.push(id);
        }
        self.owner_nodes.clear();
        let order = RetainedSceneOrder {
            nodes: items.iter().filter_map(node_id).collect::<Vec<_>>().into(),
            effect_runs: compose_scene_render_plan(cache, model, presentation, &items).into(),
        };
        self.diagnostics.topology_rebuilds += 1;
        self.order = Some(order);
        for item in items {
            self.upsert(item, upserts);
        }
    }

    fn upsert(&mut self, item: SceneDisplayItem, output: &mut Vec<Arc<RetainedSceneNode>>) {
        let Some(id) = node_id(&item) else {
            return;
        };
        self.owner_nodes.entry(element_id(id)).or_default().push(id);
        self.diagnostics.nodes_compared += 1;
        let old = self.nodes.get(&id);
        if old.is_some_and(|old| *old.item == item) {
            return;
        }
        let (pose, local, rigid_geometry) = local_item(&item, old.map(Arc::as_ref));
        let (geometry, material) = separated(&local);
        let previous = old.map(|old| separated(&old.local_item));
        let revision = self.revision + 1;
        let node = Arc::new(RetainedSceneNode {
            id,
            geometry_revision: old
                .filter(|_| rigid_geometry || previous.as_ref().is_some_and(|p| p.0 == geometry))
                .map_or(revision, |old| old.geometry_revision),
            material_revision: old
                .filter(|_| previous.as_ref().is_some_and(|p| p.1 == material))
                .map_or(revision, |old| old.material_revision),
            pose_revision: old
                .filter(|old| old.pose == pose)
                .map_or(revision, |old| old.pose_revision),
            pose,
            local_item: old
                .filter(|old| *old.local_item == local)
                .map_or_else(|| Arc::new(local), |old| old.local_item.clone()),
            bounds: canvas_bounds(&item),
            item: Arc::new(item),
        });
        self.nodes.insert(id, node.clone());
        output.push(node);
    }

    pub fn acquire(&self, cursor: Option<u64>) -> Arc<RetainedSceneDelta> {
        let mut result = RetainedSceneDelta {
            base_revision: cursor.unwrap_or(0),
            revision: self.revision,
            scene_revision: self.scene_revision,
            reset: false,
            frame: self.frame,
            upserts: Vec::new(),
            removals: Vec::new(),
            order: None,
            overlay_revision: self.overlay_revision,
            overlays: None,
            decoration_revision: self.decoration_revision,
            decoration: None,
            spotlights: None,
        };
        if cursor == Some(self.revision) {
            return Arc::new(result);
        }
        if let Some(cursor) = cursor
            && let Some(index) = self
                .journal
                .iter()
                .position(|delta| delta.base_revision == cursor)
            && !self.journal.iter().skip(index).any(|delta| delta.reset)
        {
            if index + 1 == self.journal.len() {
                return self.journal[index].clone();
            }
            let mut upserts = HashMap::new();
            let mut removals = HashSet::new();
            for delta in self.journal.iter().skip(index) {
                for id in &delta.removals {
                    upserts.remove(id);
                    removals.insert(*id);
                }
                for node in &delta.upserts {
                    removals.remove(&node.id);
                    upserts.insert(node.id, node.clone());
                }
                if delta.order.is_some() {
                    result.order = delta.order.clone();
                }
                if delta.overlays.is_some() {
                    result.overlays = delta.overlays.clone();
                }
                if delta.decoration.is_some() {
                    result.decoration = delta.decoration;
                    result.spotlights = delta.spotlights.clone();
                }
            }
            result.upserts = upserts.into_values().collect();
            result.removals = removals.into_iter().collect();
        } else {
            result.reset = true;
            result.upserts = self
                .order
                .as_ref()
                .map(|order| {
                    order
                        .nodes
                        .iter()
                        .filter_map(|id| self.nodes.get(id).cloned())
                        .collect()
                })
                .unwrap_or_default();
            result.order = self.order.clone();
            result.overlays = Some(self.overlays.clone());
            result.decoration = Some(self.decoration);
            result.spotlights = Some(self.spotlights.clone());
        }
        Arc::new(result)
    }
}

fn journal_weight(delta: &RetainedSceneDelta) -> usize {
    delta.upserts.len()
        + delta.removals.len()
        + delta
            .order
            .as_ref()
            .map_or(0, |order| order.nodes.len() + order.effect_runs.len())
        + delta.overlays.as_ref().map_or(0, |items| items.len())
        + delta.spotlights.as_ref().map_or(0, |items| items.len())
}

fn element_id(id: RetainedNodeId) -> ElementId {
    ElementId {
        index: id.element.index,
        generation: id.element.generation,
    }
}

fn node_id(item: &SceneDisplayItem) -> Option<RetainedNodeId> {
    let (element, role) = match item {
        SceneDisplayItem::Rectangle(v) => (v.id, RetainedNodeRole::Content),
        SceneDisplayItem::Filter(v) => (v.id, RetainedNodeRole::Content),
        SceneDisplayItem::Arrow(v) => (v.id, RetainedNodeRole::Content),
        SceneDisplayItem::Text(v) => (v.id, RetainedNodeRole::Content),
        SceneDisplayItem::SerialNumber(v) => (v.id, RetainedNodeRole::Content),
        SceneDisplayItem::SerialNumberConnector(v) => (v.id, RetainedNodeRole::Connector),
        SceneDisplayItem::Magnifier(v) => (v.lens.id, RetainedNodeRole::Content),
        SceneDisplayItem::SpeechBubble(v) => (v.body.id, RetainedNodeRole::Content),
        SceneDisplayItem::Stroke | SceneDisplayItem::Image => return None,
    };
    Some(RetainedNodeId { element, role })
}

fn preview_owners(model: &DocumentModel, p: &EditorPresentationState) -> HashSet<ElementId> {
    let mut ids: HashSet<_> = p
        .preview_elements
        .iter()
        .map(|p| p.id)
        .chain(p.preview_arrows.iter().map(|p| p.id))
        .chain(p.preview_magnifiers.iter().map(|p| p.id))
        .chain(p.speech_bubble_previews.iter().map(|p| p.0))
        .chain(p.speech_bubble_text_previews.iter().map(|p| p.0))
        .chain(p.arrow_text_previews.iter().map(|p| p.0))
        .collect();
    if p.creation_preview.is_some() {
        ids.insert(model.peek_next_element_id());
    }
    if let Some(draft) = &p.active_text_draft {
        ids.insert(draft.display_id());
    }
    if let Some((id, _)) = &p.free_draw_replacement {
        ids.insert(*id);
    }
    if let Some((id, _)) = &p.distance_creation_text {
        ids.insert(*id);
    }
    if let Some(p) = &p.shape_bound_text_preview {
        ids.insert(p.owner_id);
        ids.insert(p.text_id);
    }
    if let Some(transaction) = &p.duplicate_preview {
        for operation in transaction.operations() {
            if let snow_draw_engine_document::Operation::InsertElement { id, .. } = operation {
                ids.insert(*id);
            }
        }
    }
    ids
}

fn expand_dependents(model: &DocumentModel, ids: &mut HashSet<ElementId>) {
    let mut queue: Vec<_> = ids.iter().copied().collect();
    while let Some(id) = queue.pop() {
        let adjacent = model
            .bound_arrow_ids(id)
            .iter()
            .copied()
            .chain(model.serials_for_text(id).iter().copied())
            .chain(model.shapes_for_text(id).iter().copied())
            .chain(model.text_owner_id(id))
            .chain(model.bound_text_id_for_arrow(id))
            .chain(model.bound_text_id_for_shape(id))
            .chain(model.bound_text_id_for_serial_number(id))
            .chain(model.bound_text_id_for_speech_bubble(id));
        for dependency in adjacent {
            if ids.insert(dependency) {
                queue.push(dependency);
            }
        }
    }
}

fn canvas_bounds(item: &SceneDisplayItem) -> [f64; 4] {
    let frame = FrameView {
        surface: snow_draw_engine_core::SurfaceSize {
            width: 0,
            height: 0,
        },
        camera: snow_draw_engine_core::Camera {
            center: Point::default(),
            zoom: 1.0,
        },
        ..FrameView::default()
    };
    scene_display_item_bounds(item, frame)
        .map_or([0.0; 4], |r| [r.min_x, r.min_y, r.max_x, r.max_y])
}

fn local_item(
    item: &SceneDisplayItem,
    old: Option<&RetainedSceneNode>,
) -> (ScenePose, SceneDisplayItem, bool) {
    if let SceneDisplayItem::Arrow(arrow) = item {
        if let Some(old) = old
            && let SceneDisplayItem::Arrow(previous) = old.item.as_ref()
            && let SceneDisplayItem::Arrow(local) = old.local_item.as_ref()
            && let Some(transform) = rigid_arrow_transform(previous, arrow)
        {
            let mut local = local.clone();
            local.stroke = arrow.stroke;
            local.arrow_outline_color = arrow.arrow_outline_color;
            local.fill = arrow.fill;
            local.opacity = arrow.opacity;
            local.blend_mode = arrow.blend_mode;
            return (
                compose_pose(transform, old.pose),
                SceneDisplayItem::Arrow(local),
                true,
            );
        }
        // The initial chunk coordinates are a valid local reference frame.
        // Keeping their Arc payloads preserves append-only chunk identity while
        // later rigid edits modify only the pose of that retained reference.
        return (ScenePose::default(), item.clone(), false);
    }
    let mut local = item.clone();
    let pose_fields = match &mut local {
        SceneDisplayItem::Rectangle(v) => Some((&mut v.center_x, &mut v.center_y, &mut v.rotation)),
        SceneDisplayItem::Text(v) => Some((&mut v.center_x, &mut v.center_y, &mut v.rotation)),
        SceneDisplayItem::SerialNumber(v) => {
            Some((&mut v.center_x, &mut v.center_y, &mut v.rotation))
        }
        SceneDisplayItem::Filter(v) if !v.is_pen_filter => {
            Some((&mut v.center_x, &mut v.center_y, &mut v.rotation))
        }
        _ => None,
    };
    let mut pose = ScenePose::default();
    if let Some((x, y, rotation)) = pose_fields {
        let (s, c) = rotation.sin_cos();
        pose = ScenePose {
            xx: c,
            yx: s,
            xy: -s,
            yy: c,
            tx: *x,
            ty: *y,
        };
        *x = 0.0;
        *y = 0.0;
        *rotation = 0.0;
    }
    (pose, local, false)
}

fn transform_point(pose: ScenePose, point: [f64; 2]) -> [f64; 2] {
    [
        pose.xx * point[0] + pose.xy * point[1] + pose.tx,
        pose.yx * point[0] + pose.yy * point[1] + pose.ty,
    ]
}
fn compose_pose(a: ScenePose, b: ScenePose) -> ScenePose {
    let [tx, ty] = transform_point(a, [b.tx, b.ty]);
    ScenePose {
        xx: a.xx * b.xx + a.xy * b.yx,
        yx: a.yx * b.xx + a.yy * b.yx,
        xy: a.xx * b.xy + a.xy * b.yy,
        yy: a.yx * b.xy + a.yy * b.yy,
        tx,
        ty,
    }
}
fn inverse_pose(pose: ScenePose) -> ScenePose {
    // All retained arrow poses are rigid, so the inverse linear part is its transpose.
    ScenePose {
        xx: pose.xx,
        xy: pose.yx,
        yx: pose.xy,
        yy: pose.yy,
        tx: -pose.xx * pose.tx - pose.yx * pose.ty,
        ty: -pose.xy * pose.tx - pose.yy * pose.ty,
    }
}
fn command_points(command: PathCommand) -> ([[f64; 2]; 3], usize) {
    match command {
        PathCommand::MoveTo { point } | PathCommand::LineTo { point } => {
            ([point, [0.0; 2], [0.0; 2]], 1)
        }
        PathCommand::QuadTo { control, end } => ([control, end, [0.0; 2]], 2),
        PathCommand::CubicTo {
            control_1,
            control_2,
            end,
        } => ([control_1, control_2, end], 3),
    }
}
fn arrow_pose(arrow: &ArrowDisplayItem) -> ScenePose {
    let mut first = None;
    let mut second = None;
    let points = arrow.points.iter().copied().chain(
        arrow
            .geometry
            .chunks
            .iter()
            .flat_map(|chunk| chunk.commands.iter().copied())
            .chain(arrow.path_commands.iter().copied())
            .flat_map(|command| {
                let (points, count) = command_points(command);
                points.into_iter().take(count)
            }),
    );
    for point in points {
        let origin = *first.get_or_insert(point);
        if (point[0] - origin[0]).hypot(point[1] - origin[1]) > 1.0e-9 {
            second = Some(point);
            break;
        }
    }
    let [tx, ty] = first.unwrap_or([0.0; 2]);
    let angle = if arrow.label_bounds.is_some() {
        0.0
    } else {
        second.map_or(0.0, |point| (point[1] - ty).atan2(point[0] - tx))
    };
    let (sin, cos) = angle.sin_cos();
    ScenePose {
        xx: cos,
        yx: sin,
        xy: -sin,
        yy: cos,
        tx,
        ty,
    }
}
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1.0e-8 + a.abs().max(b.abs()) * 1.0e-12
}
fn points_match(a: [f64; 2], b: [f64; 2], transform: ScenePose) -> bool {
    let a = transform_point(transform, a);
    near(a[0], b[0]) && near(a[1], b[1])
}
fn commands_match(a: PathCommand, b: PathCommand, transform: ScenePose) -> bool {
    if std::mem::discriminant(&a) != std::mem::discriminant(&b) {
        return false;
    }
    let (a, count) = command_points(a);
    let (b, _) = command_points(b);
    a[..count]
        .iter()
        .zip(&b[..count])
        .all(|(a, b)| points_match(*a, *b, transform))
}
fn geometry_matches(
    a: &snow_draw_engine_core::PathGeometry,
    b: &snow_draw_engine_core::PathGeometry,
    transform: ScenePose,
) -> bool {
    if a.closed != b.closed {
        return false;
    }
    let mut a = a.chunks.iter().flat_map(|chunk| chunk.commands.iter());
    let mut b = b.chunks.iter().flat_map(|chunk| chunk.commands.iter());
    loop {
        match (a.next(), b.next()) {
            (Some(a), Some(b)) if commands_match(*a, *b, transform) => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}
fn rigid_arrow_transform(a: &ArrowDisplayItem, b: &ArrowDisplayItem) -> Option<ScenePose> {
    if a.brush_mode != b.brush_mode
        || a.id != b.id
        || a.bound_text_id != b.bound_text_id
        || a.bound_text_overlay != b.bound_text_overlay
        || a.arrow_type != b.arrow_type
        || a.arrow_shaft_type != b.arrow_shaft_type
        || a.arrow_outline_width != b.arrow_outline_width
        || a.arrow_ratio != b.arrow_ratio
        || a.start_arrowhead != b.start_arrowhead
        || a.end_arrowhead != b.end_arrowhead
        || a.stroke_width != b.stroke_width
        || a.stroke_style != b.stroke_style
        || a.stroke_rendering_style != b.stroke_rendering_style
        || a.stroke_rendering_seed != b.stroke_rendering_seed
        || a.fill_style != b.fill_style
        || a.is_free_draw != b.is_free_draw
        || a.points.len() != b.points.len()
        || a.path_commands.len() != b.path_commands.len()
        || a.arrowhead_primitives.len() != b.arrowhead_primitives.len()
    {
        return None;
    }
    let (before, after) = (arrow_pose(a), arrow_pose(b));
    let transform = if before == after {
        ScenePose::default()
    } else {
        compose_pose(after, inverse_pose(before))
    };
    if !a
        .points
        .iter()
        .zip(&b.points)
        .all(|(a, b)| points_match(*a, *b, transform))
        || !a
            .path_commands
            .iter()
            .zip(&b.path_commands)
            .all(|(a, b)| commands_match(*a, *b, transform))
        || !geometry_matches(&a.geometry, &b.geometry, transform)
    {
        return None;
    }
    match (&a.pressure_geometry, &b.pressure_geometry) {
        (Some(a), Some(b)) if geometry_matches(a, b, transform) => {}
        (None, None) => {}
        _ => return None,
    }
    match (a.label_bounds, b.label_bounds) {
        (Some(a), Some(b))
            if points_match([a.min_x, a.min_y], [b.min_x, b.min_y], transform)
                && points_match([a.max_x, a.max_y], [b.max_x, b.max_y], transform) => {}
        (None, None) => {}
        _ => return None,
    }
    for (a, b) in a.arrowhead_primitives.iter().zip(&b.arrowhead_primitives) {
        if a.kind != b.kind
            || a.diameter != b.diameter
            || a.roughness_cap != b.roughness_cap
            || a.fill_mode != b.fill_mode
            || a.dash_mode != b.dash_mode
            || a.points.len() != b.points.len()
            || !a
                .points
                .iter()
                .zip(&b.points)
                .all(|(a, b)| points_match(*a, *b, transform))
            || (a.kind == ArrowheadDisplayPrimitiveKind::Circle
                && !points_match(a.center, b.center, transform))
        {
            return None;
        }
    }
    Some(transform)
}

// Compare only changed nodes. Removing material from a copy makes the geometry
// discriminator exhaustive as new geometry fields are added to display items.
fn separated(item: &SceneDisplayItem) -> (SceneDisplayItem, Vec<u64>) {
    let mut geometry = item.clone();
    let mut material = Vec::new();
    fn color(value: &mut ColorRgba8, out: &mut Vec<u64>) {
        out.push(u32::from_le_bytes([value.r, value.g, value.b, value.a]) as u64);
        *value = ColorRgba8::default();
    }
    fn alpha(value: &mut f64, out: &mut Vec<u64>) {
        out.push(value.to_bits());
        *value = 1.0;
    }
    fn rectangle(v: &mut RectangleDisplayItem, out: &mut Vec<u64>) {
        color(&mut v.fill, out);
        color(&mut v.stroke, out);
        alpha(&mut v.opacity, out);
        out.push(v.blend_mode as u64);
        v.blend_mode = Default::default();
    }
    fn arrow(v: &mut ArrowDisplayItem, out: &mut Vec<u64>) {
        color(&mut v.arrow_outline_color, out);
        color(&mut v.fill, out);
        color(&mut v.stroke, out);
        alpha(&mut v.opacity, out);
        out.push(v.blend_mode as u64);
        v.blend_mode = Default::default();
        // Document cache refreshes can assign a new transport revision to
        // identical path chunks after a color edit. That is not new geometry.
        fn normalize(value: &mut Arc<snow_draw_engine_core::PathGeometry>) {
            if value.revision != 0 {
                *value = Arc::new(snow_draw_engine_core::PathGeometry {
                    revision: 0,
                    ..(**value).clone()
                });
            }
        }
        normalize(&mut v.geometry);
        if let Some(pressure) = &mut v.pressure_geometry {
            normalize(pressure);
        }
    }
    match &mut geometry {
        SceneDisplayItem::Rectangle(v) => rectangle(v, &mut material),
        SceneDisplayItem::Arrow(v) => arrow(v, &mut material),
        SceneDisplayItem::Text(v) => {
            color(&mut v.color, &mut material);
            color(&mut v.fill, &mut material);
            color(&mut v.stroke, &mut material);
            alpha(&mut v.opacity, &mut material);
        }
        SceneDisplayItem::SerialNumber(v) => {
            color(&mut v.color, &mut material);
            color(&mut v.fill, &mut material);
            alpha(&mut v.opacity, &mut material);
        }
        SceneDisplayItem::SerialNumberConnector(v) => {
            color(&mut v.stroke, &mut material);
            alpha(&mut v.opacity, &mut material);
        }
        SceneDisplayItem::Filter(v) => {
            alpha(&mut v.opacity, &mut material);
            material.extend([
                v.filter.filter_type as u64,
                v.filter.render_phase as u64,
                v.filter.strength.to_bits(),
                v.filter.mosaic_block_size.to_bits(),
                v.filter.blur_sigma.to_bits(),
                v.filter.sampling_radius.to_bits(),
            ]);
            v.filter = Default::default();
        }
        SceneDisplayItem::Magnifier(v) => {
            rectangle(&mut v.lens, &mut material);
            if let Some(leader) = &mut v.leader {
                arrow(leader, &mut material);
            }
        }
        SceneDisplayItem::SpeechBubble(v) => {
            rectangle(&mut v.body, &mut material);
            arrow(&mut v.path, &mut material);
        }
        SceneDisplayItem::Stroke | SceneDisplayItem::Image => {}
    }
    (geometry, material)
}
