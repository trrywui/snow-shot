/// Whether a cropped source-grid input omits available donor context. Natural
/// source edges do not require escalation. The service caps expansion at the
/// original 512-pixel neighborhood and applies the scratch limit before sampling.
#[derive(Clone, Copy)]
pub(crate) struct DonorContext {
    pub region: [u32; 4],
    pub source_size: [u32; 2],
    pub search_span: usize,
    pub can_expand: bool,
}

/// One ROI policy for worker-prepared and resident GPU donor sources.
pub(crate) struct DonorRegions {
    mask: [u32; 4],
    source: [u32; 2],
}
impl DonorRegions {
    pub fn new(mask: [u32; 4], source: [u32; 2]) -> Self {
        Self { mask, source }
    }
    pub fn initial_halo(&self) -> u32 {
        (((self.mask[2] - self.mask[0]).min(self.mask[3] - self.mask[1]) / 2).clamp(16, 96) + 32)
            .max(64)
    }
    fn unaligned(&self, halo: u32) -> [u32; 4] {
        let left = self.mask[0].saturating_sub(halo);
        let top = self.mask[1].saturating_sub(halo);
        let right = self.mask[2].saturating_add(halo).min(self.source[0]);
        let bottom = self.mask[3].saturating_add(halo).min(self.source[1]);
        [left, top, right - left, bottom - top]
    }
    pub fn maximum(&self) -> [u32; 4] {
        self.unaligned(512)
    }
    pub fn region(&self, halo: u32) -> [u32; 4] {
        let [x, y, w, h] = self.unaligned(halo);
        let [mx, my, mw, mh] = self.maximum();
        let left = mx + (x - mx) / 32 * 32;
        let top = my + (y - my) / 32 * 32;
        let right = (mx + (x + w - mx).div_ceil(32) * 32).min(mx + mw);
        let bottom = (my + (y + h - my).div_ceil(32) * 32).min(my + mh);
        [left, top, right - left, bottom - top]
    }
    pub fn context(&self, halo: u32) -> DonorContext {
        let region = self.region(halo);
        let maximum = self.maximum();
        DonorContext {
            region,
            source_size: self.source,
            search_span: maximum[2].max(maximum[3]) as usize,
            can_expand: halo < 512 && region != maximum,
        }
    }
}

impl DonorContext {
    pub fn needs_margin(&self, hole: [usize; 4], margin: usize) -> bool {
        if !self.can_expand {
            return false;
        }
        let [x, y, width, height] = self.region.map(|value| value as usize);
        let required = [
            (x + hole[0]).saturating_sub(margin),
            (y + hole[1]).saturating_sub(margin),
            (x + hole[2] + margin).min(self.source_size[0] as usize),
            (y + hole[3] + margin).min(self.source_size[1] as usize),
        ];
        required[0] < x || required[1] < y || required[2] > x + width || required[3] > y + height
    }
}

pub(crate) enum Reconstruction {
    Complete(image::RgbaImage),
    ExpandDonors,
    #[cfg(target_os = "windows")]
    Gpu(PreparedGpuGrid),
}

#[cfg(target_os = "windows")]
pub(crate) struct PreparedGpuGrid {
    pub region: [u32; 4],
    pub flags: Vec<u32>,
    pub search_span: u32,
}
