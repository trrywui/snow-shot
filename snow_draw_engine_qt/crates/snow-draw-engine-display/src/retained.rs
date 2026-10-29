//! Renderer-independent retained scene transport. No graphics resources live here.
use crate::{
    DecorationView, DisplayItemId, DisplaySpotlightCutout, FrameView, OverlayDisplayItem,
    SceneDisplayItem, SceneRenderRun,
};
use std::sync::Arc;

/// A connector and its owning shape deliberately have different identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RetainedNodeId {
    pub element: DisplayItemId,
    pub role: RetainedNodeRole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RetainedNodeRole {
    Content,
    Connector,
}

/// Column-major affine matrix: x' = xx*x + xy*y + tx, y' = yx*x + yy*y + ty.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScenePose {
    pub xx: f64,
    pub yx: f64,
    pub xy: f64,
    pub yy: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Default for ScenePose {
    fn default() -> Self {
        Self {
            xx: 1.0,
            yx: 0.0,
            xy: 0.0,
            yy: 1.0,
            tx: 0.0,
            ty: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RetainedSceneNode {
    pub id: RetainedNodeId,
    /// Changes only when coverage geometry or text shaping changes.
    pub geometry_revision: u64,
    /// Changes only when color, opacity, blend, or effect parameters change.
    pub material_revision: u64,
    pub pose_revision: u64,
    pub pose: ScenePose,
    /// Centered geometry for rigid primitives; identity-pose canvas geometry otherwise.
    pub local_item: Arc<SceneDisplayItem>,
    /// Legacy neutral description, also useful for hit-test/debug validation.
    /// Unchanged descriptions and geometry are shared between snapshots.
    pub item: Arc<SceneDisplayItem>,
    /// Conservative canvas-space paint bounds, before viewport clipping.
    pub bounds: [f64; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct RetainedSceneOrder {
    pub nodes: Arc<[RetainedNodeId]>,
    /// Indices refer to `nodes`; identities are assigned before viewport culling.
    pub effect_runs: Arc<[SceneRenderRun]>,
}

/// One atomic scene publication. A reset replaces all consumer state. Otherwise
/// remove IDs, apply upserts, and replace only the optional layers that are present.
#[derive(Clone, Debug)]
pub struct RetainedSceneDelta {
    pub base_revision: u64,
    pub revision: u64,
    /// Document-content revision, independent of overlay and frame publication.
    pub scene_revision: u64,
    pub reset: bool,
    pub frame: FrameView,
    pub upserts: Vec<Arc<RetainedSceneNode>>,
    pub removals: Vec<RetainedNodeId>,
    pub order: Option<RetainedSceneOrder>,
    pub overlay_revision: u64,
    pub overlays: Option<Arc<[OverlayDisplayItem]>>,
    pub decoration_revision: u64,
    pub decoration: Option<DecorationView>,
    pub spotlights: Option<Arc<[DisplaySpotlightCutout]>>,
}

/// Deterministic counters for verifying incremental work, independent of timers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetainedSceneDiagnostics {
    pub publications: u64,
    pub full_compositions: u64,
    pub owners_composed: u64,
    pub nodes_compared: u64,
    pub topology_rebuilds: u64,
}
