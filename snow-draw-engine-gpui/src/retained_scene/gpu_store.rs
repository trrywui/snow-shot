use super::*;

use std::collections::{HashMap, VecDeque};

const BUCKETS: usize = 256;
const JOURNAL_ENTRIES: usize = 64;
const JOURNAL_IDS: usize = 4096;

#[cfg(test)]
#[path = "gpu_store_tests.rs"]
mod tests;

type NodeBucket = Arc<HashMap<RetainedNodeId, Arc<RetainedSceneNode>>>;

#[derive(Clone)]
struct Change {
    base: u64,
    result: u64,
    ids: Arc<[RetainedNodeId]>,
}

/// Fixed directory + copy-on-write buckets: snapshotting shares every node and
/// a local change copies only the affected buckets, never the complete scene.
#[derive(Clone)]
pub(super) struct GpuSceneStore {
    buckets: Arc<[NodeBucket; BUCKETS]>,
    pub(super) order: Arc<[RetainedNodeId]>,
    pub(super) identity: u64,
    pub(super) revision: u64,
    journal: VecDeque<Change>,
    journal_ids: usize,
}

impl Default for GpuSceneStore {
    fn default() -> Self {
        Self {
            buckets: Arc::new(std::array::from_fn(|_| Arc::default())),
            order: Arc::default(),
            identity: NEXT_RENDER_IDENTITY.fetch_add(1, Ordering::Relaxed),
            revision: 0,
            journal: VecDeque::new(),
            journal_ids: 0,
        }
    }
}

fn bucket(id: RetainedNodeId) -> usize {
    (id.element.index as usize ^ (id.element.generation as usize).wrapping_mul(17)) % BUCKETS
}

impl GpuSceneStore {
    pub(super) fn node(&self, id: RetainedNodeId) -> Option<&Arc<RetainedSceneNode>> {
        self.buckets[bucket(id)].get(&id)
    }
    pub(super) fn nodes(&self) -> impl Iterator<Item = (&RetainedNodeId, &Arc<RetainedSceneNode>)> {
        self.buckets.iter().flat_map(|bucket| bucket.iter())
    }

    pub(super) fn apply(&mut self, delta: &RetainedSceneDelta) -> Result<(), CanvasError> {
        if delta.reset
            && (delta.order.is_none()
                || delta.overlays.is_none()
                || delta.decoration.is_none()
                || delta.spotlights.is_none())
        {
            return Err(stale());
        }
        let topology_changed = !delta.removals.is_empty()
            || delta
                .upserts
                .iter()
                .any(|node| self.node(node.id).is_none());
        if delta.reset {
            self.buckets = Arc::new(std::array::from_fn(|_| Arc::default()));
        }
        if delta.reset || delta.order.is_some() {
            self.journal.clear();
            self.journal_ids = 0;
        }
        let mut changed = HashSet::new();
        let buckets = Arc::make_mut(&mut self.buckets);
        for id in &delta.removals {
            Arc::make_mut(&mut buckets[bucket(*id)]).remove(id);
            changed.insert(*id);
        }
        for node in &delta.upserts {
            if node.bounds.into_iter().any(|v| !v.is_finite())
                || node.bounds[0] > node.bounds[2]
                || node.bounds[1] > node.bounds[3]
            {
                return Err(stale());
            }
            Arc::make_mut(&mut buckets[bucket(node.id)]).insert(node.id, node.clone());
            changed.insert(node.id);
        }
        if let Some(order) = &delta.order {
            let mut ids = HashSet::new();
            if order
                .nodes
                .iter()
                .any(|id| !ids.insert(*id) || self.node(*id).is_none())
            {
                return Err(stale());
            }
            if ids.len() != self.nodes().count() {
                return Err(stale());
            }
            for run in order.effect_runs.iter() {
                if run.count == 0
                    || run
                        .start
                        .checked_add(run.count)
                        .is_none_or(|end| end as usize > order.nodes.len())
                {
                    return Err(stale());
                }
            }
            self.order = order.nodes.clone();
        } else if topology_changed {
            // Topology changes must explicitly publish order; ordinary updates
            // validate against the old bucket lookup without scanning order.
            return Err(stale());
        }
        if !delta.reset && delta.order.is_none() {
            let ids: Arc<[_]> = changed.into_iter().collect::<Vec<_>>().into();
            self.journal_ids += ids.len();
            self.journal.push_back(Change {
                base: delta.base_revision,
                result: delta.revision,
                ids,
            });
            while self.journal.len() > JOURNAL_ENTRIES || self.journal_ids > JOURNAL_IDS {
                let old = self.journal.pop_front().expect("nonempty journal");
                self.journal_ids -= old.ids.len();
            }
        }
        self.revision = delta.revision;
        Ok(())
    }

    pub(super) fn changes_since(&self, revision: u64) -> Option<Vec<RetainedNodeId>> {
        if revision == self.revision {
            return Some(Vec::new());
        }
        let index = self
            .journal
            .iter()
            .position(|entry| entry.base == revision)?;
        let mut current = revision;
        let mut ids = HashSet::new();
        for entry in self.journal.iter().skip(index) {
            if entry.base != current {
                return None;
            }
            current = entry.result;
            ids.extend(entry.ids.iter().copied());
        }
        (current == self.revision).then(|| ids.into_iter().collect())
    }
}
