use std::time::Instant;

/// The reconstruction branch that produced a completed source-grid patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SmartEraseSolver {
    Unchanged,
    Affine,
    Periodic,
    Multiscale,
}

/// Non-overlapping CPU stage times. Matching/voting include all pyramid levels.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct SmartEraseStageTimings {
    pub mask_ms: f64,
    pub donor_assembly_ms: f64,
    pub prepare_ms: f64,
    pub affine_ms: f64,
    pub domain_ms: f64,
    pub periodic_ms: f64,
    pub pyramid_ms: f64,
    pub lab_ms: f64,
    pub guides_ms: f64,
    pub donors_ms: f64,
    pub initialize_ms: f64,
    pub matching_ms: f64,
    pub voting_ms: f64,
    pub output_ms: f64,
}

/// Work counts in coarse-to-fine execution order, independent of wall-clock timing.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SmartEraseLevelProfile {
    pub size: [usize; 2],
    pub masked_pixels: usize,
    pub donor_patches: usize,
    pub patch_radius: usize,
    pub passes: usize,
}

/// Opt-in diagnostics for one service lookup/solve, including failed attempts.
/// A cache hit reports no reconstruction work. Timings never enter result identity.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct SmartEraseProfile {
    pub cache_hit: bool,
    pub solver: Option<SmartEraseSolver>,
    pub source_region: [u32; 4],
    pub attempts: usize,
    pub donor_halo: u32,
    pub domain_reach: usize,
    pub working_pixels: usize,
    pub masked_pixels: usize,
    pub donor_pixels: usize,
    pub estimated_scratch_bytes: usize,
    pub total_ms: f64,
    pub stages: SmartEraseStageTimings,
    pub levels: Vec<SmartEraseLevelProfile>,
}

pub(crate) enum Stage {
    Mask,
    DonorAssembly,
    Prepare,
    Affine,
    Domain,
    Periodic,
    Pyramid,
    Lab,
    Guides,
    Donors,
    Initialize,
    Matching,
    Voting,
    Output,
}

#[derive(Default)]
pub(crate) struct Profiler<'a> {
    pub data: Option<&'a mut SmartEraseProfile>,
}

impl Profiler<'_> {
    pub fn start(&self) -> Option<Instant> {
        self.data.as_ref().map(|_| Instant::now())
    }

    pub fn finish(&mut self, stage: Stage, start: Option<Instant>) {
        let (Some(data), Some(start)) = (&mut self.data, start) else {
            return;
        };
        let value = match stage {
            Stage::Mask => &mut data.stages.mask_ms,
            Stage::DonorAssembly => &mut data.stages.donor_assembly_ms,
            Stage::Prepare => &mut data.stages.prepare_ms,
            Stage::Affine => &mut data.stages.affine_ms,
            Stage::Domain => &mut data.stages.domain_ms,
            Stage::Periodic => &mut data.stages.periodic_ms,
            Stage::Pyramid => &mut data.stages.pyramid_ms,
            Stage::Lab => &mut data.stages.lab_ms,
            Stage::Guides => &mut data.stages.guides_ms,
            Stage::Donors => &mut data.stages.donors_ms,
            Stage::Initialize => &mut data.stages.initialize_ms,
            Stage::Matching => &mut data.stages.matching_ms,
            Stage::Voting => &mut data.stages.voting_ms,
            Stage::Output => &mut data.stages.output_ms,
        };
        *value += start.elapsed().as_secs_f64() * 1000.0;
    }
}
