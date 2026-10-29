use crate::CanvasError;
mod gpu_store;
use gpu_store::GpuSceneStore;
use snow_draw_engine::{
    DecorationView, DisplaySpotlightCutout, FrameView, OverlayDisplayItem, PatchCursor,
    RetainedNodeId, RetainedSceneDelta, RetainedSceneNode, SceneRenderRun,
};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT_RENDER_IDENTITY: AtomicU64 = AtomicU64::new(1);

/// ID-keyed shared scene storage. Overlay changes do not copy document content.
#[derive(Clone, Default)]
pub struct RetainedScene {
    cursor: Option<PatchCursor>,
    frame: FrameView,
    overlays: Arc<[OverlayDisplayItem]>,
    decoration: DecorationView,
    spotlights: Arc<[DisplaySpotlightCutout]>,
    runs: Arc<[SceneRenderRun]>,
    damage: Arc<[snow_draw_engine::DirtyRegion]>,
    overlay_damage: Arc<[snow_draw_engine::DirtyRegion]>,
    gpu: GpuSceneStore,
}
impl RetainedScene {
    pub fn retained_revision(&self) -> Option<u64> {
        self.cursor.map(|_| self.gpu.revision)
    }
    pub fn retained_identity(&self) -> u64 {
        self.gpu.identity
    }
    pub fn node(&self, id: RetainedNodeId) -> Option<&Arc<RetainedSceneNode>> {
        self.gpu.node(id)
    }
    pub fn nodes(&self) -> impl Iterator<Item = (&RetainedNodeId, &Arc<RetainedSceneNode>)> {
        self.gpu.nodes()
    }
    /// Traverse painter order without allocating a flat item projection.
    pub fn ordered_nodes(&self) -> impl DoubleEndedIterator<Item = &Arc<RetainedSceneNode>> {
        self.gpu.order.iter().filter_map(|id| self.gpu.node(*id))
    }
    pub fn order_ids(&self) -> &[RetainedNodeId] {
        &self.gpu.order
    }
    /// Includes removed IDs; callers retain their old bounds until damage is
    /// resolved. None requests a complete synchronization after reset/eviction.
    pub fn changes_since(&self, revision: u64) -> Option<Vec<RetainedNodeId>> {
        self.cursor?;
        self.gpu.changes_since(revision)
    }
    pub fn apply_delta(&mut self, delta: &RetainedSceneDelta) -> Result<bool, CanvasError> {
        if self.retained_revision() == Some(delta.revision) {
            return Ok(false);
        }
        if !delta.reset && self.retained_revision() != Some(delta.base_revision) {
            return Err(stale());
        }
        let mut store = self.gpu.clone();
        store.apply(delta)?;
        let mut damage = Vec::new();
        for id in delta
            .removals
            .iter()
            .copied()
            .chain(delta.upserts.iter().map(|node| node.id))
        {
            if let Some(old) = self.gpu.node(id) {
                damage.push(canvas_damage(old.bounds, delta.frame));
            }
            if let Some(new) = store.node(id) {
                damage.push(canvas_damage(new.bounds, delta.frame));
            }
        }
        if delta.reset || self.frame != delta.frame {
            damage = snow_draw_engine::full_surface_dirty_region(delta.frame.surface);
        }
        self.frame = delta.frame;
        self.cursor = Some(PatchCursor {
            scene_revision: snow_draw_engine::SceneRevision(delta.scene_revision),
            decoration_revision: snow_draw_engine::DecorationRevision(delta.decoration_revision),
            overlay_revision: snow_draw_engine::OverlayRevision(delta.overlay_revision),
        });
        if let Some(order) = &delta.order {
            self.runs = order.effect_runs.clone();
        }
        if let Some(overlays) = &delta.overlays {
            self.overlays = overlays.clone();
        }
        if let Some(decoration) = delta.decoration {
            self.decoration = decoration;
        }
        if let Some(spotlights) = &delta.spotlights {
            self.spotlights = spotlights.clone();
        }
        self.overlay_damage = if delta.overlays.is_some() {
            snow_draw_engine::full_surface_dirty_region(delta.frame.surface).into()
        } else {
            Arc::default()
        };
        self.damage = damage.into();
        self.gpu = store;
        Ok(true)
    }
    pub fn cursor(&self) -> Option<PatchCursor> {
        self.cursor
    }
    pub fn frame(&self) -> FrameView {
        self.frame
    }
    pub(crate) fn with_frame(&self, frame: FrameView) -> Self {
        Self {
            frame,
            ..self.clone()
        }
    }
    pub fn overlays(&self) -> &[OverlayDisplayItem] {
        &self.overlays
    }
    pub fn decoration(&self) -> DecorationView {
        self.decoration
    }
    pub fn spotlights(&self) -> &[DisplaySpotlightCutout] {
        &self.spotlights
    }
    pub fn runs(&self) -> &[SceneRenderRun] {
        &self.runs
    }
    /// Latest engine delta damage in viewport logical pixels. Diagnostic only;
    /// consumers must not assume this covers multiple coalesced scene revisions.
    pub fn dirty_regions(&self) -> &[snow_draw_engine::DirtyRegion] {
        &self.damage
    }
    pub fn overlay_dirty_regions(&self) -> &[snow_draw_engine::DirtyRegion] {
        &self.overlay_damage
    }
}
fn stale() -> CanvasError {
    snow_draw_engine::ErrorCode::StaleRevision.into()
}
fn canvas_damage(bounds: [f64; 4], frame: FrameView) -> snow_draw_engine::DirtyRegion {
    let first = snow_draw_engine::canvas_to_view(
        snow_draw_engine::Point::new(bounds[0], bounds[1]),
        &frame.camera,
        frame.surface,
    );
    let last = snow_draw_engine::canvas_to_view(
        snow_draw_engine::Point::new(bounds[2], bounds[3]),
        &frame.camera,
        frame.surface,
    );
    snow_draw_engine::DirtyRegion::new(first.x, first.y, last.x, last.y)
}
