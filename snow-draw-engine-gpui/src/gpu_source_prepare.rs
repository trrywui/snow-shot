//! Resident coverage/domain/model preparation for straight GPU source grids.
use super::*;

const WORDS: usize = META_WORDS;
const INVALID: u32 = u32::MAX;

/// Rasterize/convert on a worker; owner-thread preparation only uploads the mask.
pub struct GpuSourceMask {
    size: [u32; 2],
    words: Vec<u32>,
    crop: [u32; 4],
    bounds: [u32; 4],
}
impl GpuSourceMask {
    pub fn new(
        mask: &[u8],
        size: [u32; 2],
        cancel: &CanvasCancellation,
    ) -> Result<Self, CanvasError> {
        let [w, h] = size;
        if w == 0 || h == 0 || w > 16384 || h > 16384 || w as usize * h as usize > MAX_PIXELS {
            return Err(CanvasError::ResourceLimit);
        }
        if mask.len() != w as usize * h as usize {
            return Err(crate::engine::ErrorCode::InvalidArgument.into());
        }
        let mut bounds = [w, h, 0, 0];
        let mut words = Vec::with_capacity(mask.len());
        for y in 0..h {
            cancel.check()?;
            for x in 0..w {
                let filled = mask[(y * w + x) as usize] != 0;
                words.push(u32::from(filled));
                if filled {
                    bounds[0] = bounds[0].min(x);
                    bounds[1] = bounds[1].min(y);
                    bounds[2] = bounds[2].max(x + 1);
                    bounds[3] = bounds[3].max(y + 1);
                }
            }
        }
        if bounds[2] == 0 || bounds[3] == 0 {
            return Err(crate::engine::ErrorCode::InvalidArgument.into());
        }
        let left = bounds[0].saturating_sub(2);
        let top = bounds[1].saturating_sub(2);
        let right = bounds[2].saturating_add(2).min(w);
        let bottom = bounds[3].saturating_add(2).min(h);
        Ok(Self {
            size,
            words,
            crop: [left, top, right - left, bottom - top],
            bounds,
        })
    }
    pub(crate) fn bounds(&self) -> [u32; 4] {
        self.bounds
    }
    pub(crate) fn sub_mask(&self, region: [u32; 4]) -> Vec<u8> {
        let [x, y, w, h] = region;
        let mut result = Vec::with_capacity(w as usize * h as usize);
        for row in y..y + h {
            result.extend(
                self.words
                    [(row * self.size[0] + x) as usize..(row * self.size[0] + x + w) as usize]
                    .iter()
                    .map(|v| u8::from(*v != 0)),
            );
        }
        result
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct GpuSourcePrepareProfile {
    pub submitted_dispatches: usize,
    pub poll_count: usize,
    pub mask_upload_bytes: usize,
    pub metadata_upload_bytes: usize,
    pub uniform_upload_bytes: usize,
    pub control_readback_bytes: usize,
    pub peak_device_bytes: usize,
    pub gpu_ms: Option<f64>,
    pub prepared_levels: usize,
}
/// Source pixels and flags remain on the GPU. Counts/bounds permit adaptive ROI
/// decisions before allocating multiscale solver storage.
pub struct GpuSourcePrepared {
    source: SourceImage,
    flags: Arc<Buffer>,
    crop: [u32; 4],
    span: u32,
    pub masked_pixels: u32,
    pub known_pixels: u32,
    pub donor_pixels: u32,
    pub domain_reach: u32,
    pub hole_bounds: [u32; 4],
    constant: Option<u32>,
    periodic: Option<u32>,
    pyramid: Option<GpuPyramid>,
    needs_more: bool,
    pub profile: GpuSourcePrepareProfile,
}
impl GpuSourcePrepared {
    pub fn solver(&self) -> crate::SmartEraseSolver {
        if self.masked_pixels == 0 {
            crate::SmartEraseSolver::Unchanged
        } else if self.constant.is_some() {
            crate::SmartEraseSolver::Affine
        } else if self.periodic.is_some() {
            crate::SmartEraseSolver::Periodic
        } else {
            crate::SmartEraseSolver::Multiscale
        }
    }
    pub fn with_search_span(mut self, span: u32) -> Result<Self, CanvasError> {
        if span == 0 || span > MAX_PIXELS as u32 {
            return Err(crate::engine::ErrorCode::InvalidArgument.into());
        }
        self.span = span;
        Ok(self)
    }
}
impl GpuSmartErase {
    /// Submit preparation directly behind queued donor writes. The donor job
    /// must have submitted every page; neither image is read back or waited on.
    pub fn prepare_assembled(
        &self,
        donors: &mut crate::GpuDonorJob,
        mask: GpuSourceMask,
        cancel: CanvasCancellation,
    ) -> Result<GpuSourcePrepareJob, CanvasError> {
        let source = donors
            .submitted_source()?
            .ok_or(crate::engine::ErrorCode::InvalidState)?;
        self.prepare_source(source, mask, cancel)
    }
    /// Queue after source assembly on this same context, without waiting for its
    /// completion. The source and all dependencies must remain immutable.
    pub fn prepare_source(
        &self,
        source: SourceImage,
        mask: GpuSourceMask,
        cancel: CanvasCancellation,
    ) -> Result<GpuSourcePrepareJob, CanvasError> {
        cancel.check()?;
        if !self.supports_service(&self.service) {
            return Err(CanvasError::Gpu("Preparation device epoch changed".into()));
        }
        validate_source_device(&source, &self.service)?;
        if source.dimensions() != mask.size {
            return Err(crate::engine::ErrorCode::InvalidArgument.into());
        }
        native(&self.service)
            .map_err(gpu_error)?
            .with_context(|gpu| {
                GpuSourcePrepareJob::new(
                    self,
                    source,
                    mask,
                    cancel,
                    gpu.device,
                    gpu.immediate_context,
                )
            })
            .map_err(gpu_error)
    }
    pub fn start_prepared(
        &self,
        prepared: GpuSourcePrepared,
        cancel: CanvasCancellation,
    ) -> Result<GpuSourceSolveJob, CanvasError> {
        cancel.check()?;
        validate_source_device(&prepared.source, &self.service)?;
        if !self.supports_service(&self.service) {
            return Err(CanvasError::Gpu(
                "Reconstruction device epoch changed".into(),
            ));
        }
        if prepared.needs_more {
            return Err(CanvasError::ResourceLimit);
        }
        if prepared.masked_pixels != 0 && (prepared.known_pixels == 0 || prepared.donor_pixels == 0)
        {
            return Err(CanvasError::Unsupported(
                "Smart Erase without unmasked source donors",
            ));
        }
        if prepared.solver() == crate::SmartEraseSolver::Multiscale {
            let origin = prepared.source.origin();
            let units = prepared.source.units_per_pixel();
            let input = GpuSmartEraseInput {
                pixels: Arc::new(RgbaImage::new(0, 0)),
                flags: Vec::new(),
                gpu: Some(Box::new(GpuGrid {
                    source: prepared.source,
                    flags: prepared.flags,
                    pyramid: prepared.pyramid,
                })),
                crop: prepared.crop,
                origin,
                units,
                search_span: prepared.span,
            };
            Ok(GpuSourceSolveJob {
                kind: SolveKind::Patch(Box::new(self.start(input, cancel)?)),
            })
        } else {
            let job = native(&self.service)
                .map_err(gpu_error)?
                .with_context(|gpu| SimpleJob::new(self, prepared, cancel, gpu.device))
                .map_err(gpu_error)?;
            Ok(GpuSourceSolveJob {
                kind: SolveKind::Simple(Box::new(job)),
            })
        }
    }
}

pub struct GpuSourcePrepareJob {
    service: Arc<dyn GpuService>,
    epoch: u64,
    memory: Arc<GpuMemory>,
    pipeline: Arc<Pipeline>,
    source: SourceImage,
    crop: [u32; 4],
    cancel: CanvasCancellation,
    buffers: Vec<Arc<Buffer>>,
    steps: VecDeque<Step>,
    staging: ID3D11Buffer,
    staging_allocation: Arc<Allocation>,
    pending: Option<Box<dyn GpuCompletion>>,
    finished: bool,
    profile: GpuSourcePrepareProfile,
    profiling: bool,
    timings: Vec<GpuTiming>,
    pyramid: Vec<ResidentLevel>,
}
impl GpuSourcePrepareJob {
    fn new(
        runtime: &GpuSmartErase,
        source: SourceImage,
        mask: GpuSourceMask,
        cancel: CanvasCancellation,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
    ) -> Result<Self> {
        let [w, h] = mask.size;
        let n = w as usize * h as usize;
        let raw = source
            .gpu_straight
            .as_ref()
            .context("Straight pixels missing")?
            .clone();
        ensure!(raw.bytes == n * 4, "Preparation source size differs");
        let mut buffers = vec![raw];
        for count in [n, n, n, n, n, h as usize * 8, WORDS] {
            buffers.push(Arc::new(Buffer::new(device, count, 4, &runtime.memory)?));
        }
        // 0 raw; 1 mask/nearest rows; 2 seeds/radii; 3 base flags; 4 output
        // flags; 5 nearest holes; 6 row statistics; 7 metadata.
        buffers[1].upload(context, bytes(&mask.words))?;
        let mut header = [0_u32; WORDS];
        header[12] = INVALID;
        buffers[7].upload(context, bytes(&header))?;
        let allocation = runtime.memory.reserve(WORDS * 4)?;
        let mut staging = None;
        unsafe {
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: (WORDS * 4) as u32,
                    Usage: D3D11_USAGE_STAGING,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut staging),
            )?;
        }
        let level = stub(mask.size);
        let mut steps = VecDeque::new();
        steps.push_back(
            Step::new(Kernel::SourceFlags, &level)
                .inputs(&[(4, 0), (5, 1)])
                .outputs(&[(2, 3), (3, 2), (4, 4)]),
        );
        let mut rows = Step::new(Kernel::NearestRows, &level)
            .inputs(&[(4, 2)])
            .outputs(&[(2, 1)]);
        rows.threads = h;
        steps.push_back(rows);
        let mut columns = Step::new(Kernel::NearestColumns, &level)
            .inputs(&[(4, 1)])
            .outputs(&[(2, 5)]);
        columns.threads = w;
        steps.push_back(columns);
        let mut erosion = Step::new(Kernel::Erode, &level)
            .inputs(&[(4, 3)])
            .outputs(&[(2, 2), (3, 6)]);
        erosion.threads = h;
        steps.push_back(erosion);
        let mut init = Step::new(Kernel::DomainInit, &level).inputs(&[(4, 6)]);
        init.threads = 1;
        steps.push_back(init);
        for _ in 0..6 {
            steps.push_back(
                Step::new(Kernel::DomainPixels, &level)
                    .inputs(&[(4, 3), (5, 5)])
                    .outputs(&[(2, 4)]),
            );
            let mut erosion = Step::new(Kernel::Erode, &level)
                .inputs(&[(4, 4)])
                .outputs(&[(2, 2), (3, 6)]);
            erosion.threads = h;
            erosion.parameters.options[3] = 1;
            steps.push_back(erosion);
            let mut advance = Step::new(Kernel::DomainAdvance, &level).inputs(&[(4, 6)]);
            advance.threads = 1;
            steps.push_back(advance);
        }
        let mut periodic = Step::new(Kernel::DetectPeriodic, &level).inputs(&[(4, 0), (5, 4)]);
        let mut margin = Step::new(Kernel::DomainMargin, &level);
        margin.threads = 1;
        steps.push_back(margin);
        periodic.threads = 126 * 64;
        steps.push_back(periodic);
        let mut pyramid = Vec::new();
        let mut previous: Option<(usize, usize, [u32; 2])> = None;
        let mut size = mask.size;
        loop {
            let index = pyramid.len();
            let n = size[0] as usize * size[1] as usize;
            let rgb = buffers.len();
            buffers.push(Arc::new(Buffer::new(device, n, 16, &runtime.memory)?));
            let flags = if index == 0 {
                3
            } else {
                let at = buffers.len();
                buffers.push(Arc::new(Buffer::new(device, n, 4, &runtime.memory)?));
                at
            };
            let radii = if index == 0 {
                2
            } else {
                let at = buffers.len();
                buffers.push(Arc::new(Buffer::new(device, n, 4, &runtime.memory)?));
                at
            };
            let offsets = buffers.len();
            buffers.push(Arc::new(Buffer::new(
                device,
                size[1] as usize * 2,
                4,
                &runtime.memory,
            )?));
            let mut level = stub(size);
            level.index = index;
            let mut load = if let Some((prgb, pflags, previous_size)) = previous {
                let mut load = Step::new(Kernel::Downsample, &level)
                    .inputs(&[(0, prgb), (4, pflags)])
                    .outputs(&[(0, rgb), (2, flags)]);
                load.parameters.previous[2..].copy_from_slice(&previous_size);
                load
            } else {
                Step::new(Kernel::Load, &level)
                    .inputs(&[(4, 0), (5, 4)])
                    .outputs(&[(0, rgb), (2, flags)])
            };
            load.parameters.options[3] = 2;
            steps.push_back(load);
            let mut erode = Step::new(Kernel::Erode, &level)
                .inputs(&[(4, flags)])
                .outputs(&[(2, radii), (3, 6)]);
            erode.threads = size[1];
            erode.parameters.options[3] = 2;
            steps.push_back(erode);
            let mut prefix = Step::new(Kernel::Prefix, &level)
                .inputs(&[(4, 6)])
                .outputs(&[(2, offsets)]);
            prefix.threads = 1;
            prefix.parameters.options[3] = 2;
            steps.push_back(prefix);
            pyramid.push(ResidentLevel {
                size,
                rgb: buffers[rgb].clone(),
                flags: buffers[flags].clone(),
                radii: buffers[radii].clone(),
                offsets: buffers[offsets].clone(),
            });
            previous = Some((rgb, flags, size));
            if pyramid.len() == 6 || size[0].min(size[1]) < 40 {
                break;
            }
            size = size.map(|v| v.div_ceil(2));
        }
        Ok(Self {
            service: runtime.service.clone(),
            epoch: runtime.epoch,
            memory: runtime.memory.clone(),
            pipeline: runtime.pipeline.clone(),
            source,
            crop: mask.crop,
            cancel,
            buffers,
            steps,
            staging: staging.context("Preparation staging missing")?,
            staging_allocation: allocation,
            pending: None,
            finished: false,
            profile: GpuSourcePrepareProfile {
                mask_upload_bytes: n * 4,
                metadata_upload_bytes: WORDS * 4,
                ..Default::default()
            },
            profiling: runtime.profiling,
            timings: Vec::new(),
            pyramid,
        })
    }
    pub(crate) fn set_donor_context(
        &mut self,
        context: crate::smart_erase::region::DonorContext,
    ) -> Result<(), CanvasError> {
        if self.profile.submitted_dispatches != 0
            || self.source.dimensions() != [context.region[2], context.region[3]]
        {
            return Err(crate::engine::ErrorCode::InvalidState.into());
        }
        let step = self
            .steps
            .iter_mut()
            .find(|s| matches!(s.kernel, Kernel::DomainMargin))
            .ok_or(crate::engine::ErrorCode::InvalidState)?;
        step.parameters.previous = [
            context.region[0],
            context.region[1],
            context.source_size[0],
            context.source_size[1],
        ];
        step.parameters.crop = [
            context.region[2],
            context.region[3],
            u32::from(context.can_expand),
            0,
        ];
        Ok(())
    }
    pub fn has_pending_submissions(&self) -> bool {
        !self.finished && !self.steps.is_empty()
    }
    pub fn poll(&mut self) -> Poll<Result<GpuSourcePrepared, CanvasError>> {
        if self.finished {
            return Poll::Ready(Err(crate::engine::ErrorCode::InvalidState.into()));
        }
        if let Err(error) = self.cancel.check() {
            self.finished = true;
            return Poll::Ready(Err(error));
        }
        self.profile.poll_count += 1;
        match self.poll_inner() {
            Ok(None) => Poll::Pending,
            Ok(Some(output)) => Poll::Ready(Ok(output)),
            Err(error) => {
                self.finished = true;
                Poll::Ready(Err(if self.cancel.check().is_err() {
                    CanvasError::Cancelled
                } else {
                    gpu_error(error)
                }))
            }
        }
    }
    fn poll_inner(&mut self) -> Result<Option<GpuSourcePrepared>> {
        ensure!(
            self.epoch == self.service.epoch(),
            "Preparation device epoch changed"
        );
        let service = self.service.clone();
        let native = native(&service)?;
        if let Some(pending) = &mut self.pending {
            if !pending.poll()? {
                return Ok(None);
            }
            let Some(header) = native
                .with_context(|gpu| read_staging(gpu.immediate_context, &self.staging, WORDS))?
            else {
                return Ok(None);
            };
            self.cancel.check()?;
            self.profile.control_readback_bytes = WORDS * 4;
            self.profile.peak_device_bytes = self.memory.peak();
            self.profile.gpu_ms = timing_total(native, &self.timings, self.profiling)?;
            let needs_more = header[11] & 2 != 0;
            let has_pyramid = header[14] != 0 && header[11] == 0 && header[10] >= 126;
            let flags = if has_pyramid {
                self.pyramid[0].flags.clone()
            } else {
                self.buffers[4].clone()
            };
            let pyramid = if has_pyramid {
                let mut levels = std::mem::take(&mut self.pyramid);
                levels.truncate(header[0] as usize);
                ensure!(!levels.is_empty(), "Prepared pyramid is empty");
                self.profile.prepared_levels = levels.len();
                Some(GpuPyramid {
                    levels,
                    meta: self.buffers[7].clone(),
                    metadata: header.clone(),
                })
            } else {
                self.pyramid.clear();
                None
            };
            let result = GpuSourcePrepared {
                source: self.source.clone(),
                flags,
                crop: self.crop,
                span: self.source.width().max(self.source.height()),
                masked_pixels: header[14],
                known_pixels: header[15],
                domain_reach: header[2],
                hole_bounds: [header[4], header[5], header[6], header[7]],
                donor_pixels: header[8],
                constant: (header[11] & 1 != 0).then_some(header[12]),
                periodic: (header[10] < 126).then_some(header[10]),
                profile: self.profile.clone(),
                pyramid,
                needs_more,
            };
            retire_unique(&mut self.buffers);
            self.finished = true;
            return Ok(Some(result));
        }
        native.with_context(|gpu| {
            let timing = if self.profiling {
                Some(GpuTiming::start(gpu.device, gpu.immediate_context)?)
            } else {
                None
            };
            let start = Instant::now();
            for _ in 0..8 {
                self.cancel.check()?;
                let Some(step) = self.steps.pop_front() else {
                    break;
                };
                submit_compute(
                    gpu.immediate_context,
                    &self.pipeline,
                    &self.buffers,
                    None,
                    Some(7),
                    &step,
                )?;
                self.profile.submitted_dispatches += 1;
                self.profile.uniform_upload_bytes += std::mem::size_of::<Parameters>();
                if start.elapsed().as_millis() >= 2 {
                    break;
                }
            }
            if let Some(timing) = timing {
                timing.end(gpu.immediate_context);
                self.timings.push(timing);
            }
            if self.steps.is_empty() {
                self.staging_allocation.mark_gpu_use();
                unsafe {
                    gpu.immediate_context
                        .CopyResource(&self.staging, &self.buffers[7].resource);
                }
            }
            Ok(())
        })?;
        if self.steps.is_empty() {
            self.pending = Some(native.completion()?);
        }
        Ok(None)
    }
}
fn stub(size: [u32; 2]) -> Level {
    Level {
        index: 0,
        size,
        search_span: size[0].max(size[1]),
        rgb: 0,
        flags: 0,
        donor_max: 0,
        offsets: 0,
        result: 0,
        matches: 0,
    }
}
fn retire_unique(buffers: &mut Vec<Arc<Buffer>>) {
    for buffer in buffers.iter_mut() {
        if let Some(buffer) = Arc::get_mut(buffer)
            && let Some(allocation) = Arc::get_mut(&mut buffer.allocation)
        {
            allocation.mark_gpu_complete();
        }
    }
    buffers.clear();
}
fn timing_total(
    native: &DirectXGpuService,
    timings: &[GpuTiming],
    enabled: bool,
) -> Result<Option<f64>> {
    if !enabled {
        return Ok(None);
    }
    native.with_context(|gpu| {
        let mut sum = 0.0;
        for timing in timings {
            match timing.poll(gpu.immediate_context)? {
                Some(Some(ms)) => sum += ms,
                _ => return Ok(None),
            }
        }
        Ok(Some(sum))
    })
}

enum SolveKind {
    Patch(Box<GpuSmartEraseJob>),
    Simple(Box<SimpleJob>),
}
pub struct GpuSourceSolveJob {
    kind: SolveKind,
}
impl GpuSourceSolveJob {
    pub fn has_pending_submissions(&self) -> bool {
        match &self.kind {
            SolveKind::Patch(job) => job.has_pending_submissions(),
            SolveKind::Simple(job) => !job.finished && !job.steps.is_empty(),
        }
    }
    pub fn poll(&mut self) -> Poll<Result<GpuSmartEraseOutput, CanvasError>> {
        match &mut self.kind {
            SolveKind::Patch(job) => job.poll(),
            SolveKind::Simple(job) => job.poll(),
        }
    }
}
struct SimpleJob {
    service: Arc<dyn GpuService>,
    epoch: u64,
    memory: Arc<GpuMemory>,
    pipeline: Arc<Pipeline>,
    source: SourceImage,
    crop: [u32; 4],
    cancel: CanvasCancellation,
    buffers: Vec<Arc<Buffer>>,
    images: Vec<ComputeImage>,
    steps: VecDeque<Step>,
    pending: Option<Box<dyn GpuCompletion>>,
    finished: bool,
    profile: GpuSmartEraseProfile,
    profiling: bool,
    timings: Vec<GpuTiming>,
}
impl SimpleJob {
    fn new(
        runtime: &GpuSmartErase,
        prepared: GpuSourcePrepared,
        cancel: CanvasCancellation,
        device: &ID3D11Device,
    ) -> Result<Self> {
        let solver = prepared.solver();
        let [w, h] = prepared.source.dimensions();
        let level = stub([w, h]);
        let crop = prepared.crop;
        let n = crop[2] as usize * crop[3] as usize;
        let mut buffers = vec![
            prepared
                .source
                .gpu_straight
                .as_ref()
                .context("Prepared source missing")?
                .clone(),
            prepared.flags,
        ];
        buffers.push(Arc::new(Buffer::new(device, n, 4, &runtime.memory)?));
        buffers.push(Arc::new(Buffer::new(device, n, 4, &runtime.memory)?));
        let mut steps = VecDeque::new();
        if let Some(candidate) = prepared
            .periodic
            .filter(|_| solver == crate::SmartEraseSolver::Periodic)
        {
            buffers.push(Arc::new(Buffer::new(
                device,
                w as usize * h as usize,
                4,
                &runtime.memory,
            )?));
            let mut step = Step::new(Kernel::PeriodicLines, &level)
                .inputs(&[(4, 1)])
                .outputs(&[(2, 4)]);
            step.parameters.options[0] = candidate;
            step.threads = if candidate < 63 { h } else { w } * (candidate % 63 + 2);
            steps.push_back(step);
        }
        let mut finish = Step::new(Kernel::Finish, &level)
            .inputs(&[(4, 0), (5, 1)])
            .outputs(&[(2, 2), (3, 3)]);
        finish.parameters.crop = crop;
        finish.threads = crop[2] * crop[3];
        finish.image = Some(1);
        if solver == crate::SmartEraseSolver::Periodic {
            finish.inputs[6] = Some(4);
            finish.parameters.options[3] = 2;
        } else {
            finish.parameters.options[3] = 1;
            finish.parameters.previous[0] = prepared.constant.unwrap_or(0);
        }
        steps.push_back(finish);
        let mut original = Step::new(Kernel::Original, &level).inputs(&[(4, 2)]);
        original.parameters.extent[..2].copy_from_slice(&crop[2..]);
        original.threads = crop[2] * crop[3];
        original.image = Some(0);
        steps.push_back(original);
        let images = vec![
            ComputeImage::new(device, [crop[2], crop[3]], &runtime.memory)?,
            ComputeImage::new(device, [crop[2], crop[3]], &runtime.memory)?,
        ];
        Ok(Self {
            service: runtime.service.clone(),
            epoch: runtime.epoch,
            memory: runtime.memory.clone(),
            pipeline: runtime.pipeline.clone(),
            source: prepared.source,
            crop,
            cancel,
            buffers,
            images,
            steps,
            pending: None,
            finished: false,
            profile: GpuSmartEraseProfile {
                solver: Some(solver),
                levels: 1,
                ..Default::default()
            },
            profiling: runtime.profiling,
            timings: Vec::new(),
        })
    }
    fn poll(&mut self) -> Poll<Result<GpuSmartEraseOutput, CanvasError>> {
        if self.finished {
            return Poll::Ready(Err(crate::engine::ErrorCode::InvalidState.into()));
        }
        if let Err(error) = self.cancel.check() {
            self.finished = true;
            return Poll::Ready(Err(error));
        }
        self.profile.poll_count += 1;
        match self.poll_inner() {
            Ok(None) => Poll::Pending,
            Ok(Some(output)) => Poll::Ready(Ok(output)),
            Err(error) => {
                self.finished = true;
                Poll::Ready(Err(if self.cancel.check().is_err() {
                    CanvasError::Cancelled
                } else {
                    gpu_error(error)
                }))
            }
        }
    }
    fn poll_inner(&mut self) -> Result<Option<GpuSmartEraseOutput>> {
        ensure!(
            self.epoch == self.service.epoch(),
            "Simple solver device epoch changed"
        );
        let service = self.service.clone();
        let native = native(&service)?;
        if let Some(pending) = &mut self.pending {
            if !pending.poll()? {
                return Ok(None);
            }
            self.cancel.check()?;
            let origin = crate::engine::Point::new(
                self.source.origin().x + self.crop[0] as f64 * self.source.units_per_pixel(),
                self.source.origin().y + self.crop[1] as f64 * self.source.units_per_pixel(),
            );
            let mut images = Vec::new();
            for (i, image) in self.images.iter().enumerate() {
                let mut source = SourceImage::from_gpu_texture(Arc::new(ResultTexture {
                    texture: native.wrap_texture(image.resource.clone())?,
                    _allocation: image.allocation.clone(),
                }))?
                .with_placement(origin, self.source.units_per_pixel())?;
                source.gpu_straight = Some(self.buffers[i + 2].clone());
                images.push(source);
            }
            self.profile.gpu_ms = timing_total(native, &self.timings, self.profiling)?;
            self.profile.retained_bytes = self.crop[2] as usize * self.crop[3] as usize * 16;
            self.profile.peak_device_bytes = self.memory.peak();
            retire_unique(&mut self.buffers);
            self.images.clear();
            self.finished = true;
            return Ok(Some(GpuSmartEraseOutput {
                original: images.remove(0),
                reconstructed: images.remove(0),
                profile: self.profile.clone(),
            }));
        }
        native.with_context(|gpu| {
            let timing = if self.profiling {
                Some(GpuTiming::start(gpu.device, gpu.immediate_context)?)
            } else {
                None
            };
            let started = Instant::now();
            for _ in 0..8 {
                self.cancel.check()?;
                let Some(step) = self.steps.pop_front() else {
                    break;
                };
                submit_compute(
                    gpu.immediate_context,
                    &self.pipeline,
                    &self.buffers,
                    step.image.map(|i| &self.images[i]),
                    None,
                    &step,
                )?;
                self.profile.submitted_dispatches += 1;
                self.profile.uniform_upload_bytes += std::mem::size_of::<Parameters>();
                self.profile.uploaded_bytes += std::mem::size_of::<Parameters>();
                if started.elapsed().as_millis() >= 2 {
                    break;
                }
            }
            if let Some(timing) = timing {
                timing.end(gpu.immediate_context);
                self.timings.push(timing);
            }
            Ok(())
        })?;
        if self.steps.is_empty() {
            self.pending = Some(native.completion()?);
        }
        Ok(None)
    }
}

#[cfg(test)]
#[path = "gpu_source_prepare_tests.rs"]
mod tests;
