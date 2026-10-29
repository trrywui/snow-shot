//! Straight-RGBA eligible donor assembly, independent of display composites/LODs.
use super::*;
use std::{cell::RefCell, rc::Rc};

const PAGE: u32 = 256;
const CACHE_BYTES: usize = 64 * 1024 * 1024;
const INVALID: u32 = u32::MAX;

#[derive(Clone)]
struct Part {
    page: [u32; 4],
    area: [u32; 4],
}
struct Layer {
    source: SourceImage,
    axes: Vec<[u32; 4]>,
    parts: Vec<Part>,
}

/// Worker-prepared placement/coverage tables. Pixel data stays shared with the
/// immutable source; preparation never flattens a layered image or copies pages.
#[derive(Clone)]
pub struct GpuDonorInput {
    layers: Arc<[Layer]>,
    size: [u32; 2],
    origin: crate::engine::Point<f64>,
    units: f64,
    copy_leaf: bool,
}
impl GpuDonorInput {
    pub fn new(
        source: &SourceImage,
        region: [u32; 4],
        cancel: &CanvasCancellation,
    ) -> Result<Self, CanvasError> {
        cancel.check()?;
        let [x, y, w, h] = region;
        if w == 0
            || h == 0
            || x.checked_add(w).is_none_or(|r| r > source.width())
            || y.checked_add(h).is_none_or(|b| b > source.height())
        {
            return Err(crate::engine::ErrorCode::InvalidArgument.into());
        }
        if w as usize * h as usize > MAX_PIXELS || w > 16384 || h > 16384 {
            return Err(CanvasError::ResourceLimit);
        }
        let units = source.units_per_pixel();
        let origin = crate::engine::Point::new(
            source.origin().x + x as f64 * units,
            source.origin().y + y as f64 * units,
        );
        let leaf = crate::ImageSourceLayer::new(source.clone());
        let copy_leaf = source.layers().is_none();
        let mut layers = Vec::new();
        let mut samples = 0_u64;
        for layer in source.layers().unwrap_or(std::slice::from_ref(&leaf)) {
            cancel.check()?;
            if !layer.smart_erase_source {
                continue;
            }
            if layer.image.gpu_texture().is_some() && layer.image.gpu_straight.is_none() {
                return Err(CanvasError::Unsupported(
                    "GPU donors without exact straight RGBA",
                ));
            }
            let placement = layer.placement();
            if placement.width <= 0.0 || placement.height <= 0.0 {
                continue;
            }
            let axis = |horizontal: bool| -> Vec<[u32; 4]> {
                let (count, start, placed, span, size, src_origin, coverage) = if horizontal {
                    (
                        w,
                        origin.x,
                        placement.x,
                        placement.width,
                        layer.image.width(),
                        layer.image.origin().x,
                        layer.coverage.map(|c| (c.x, c.x + c.width)),
                    )
                } else {
                    (
                        h,
                        origin.y,
                        placement.y,
                        placement.height,
                        layer.image.height(),
                        layer.image.origin().y,
                        layer.coverage.map(|c| (c.y, c.y + c.height)),
                    )
                };
                (0..count)
                    .map(|i| {
                        if copy_leaf {
                            let at = i + if horizontal { x } else { y };
                            return [at, at, 0, 1];
                        }
                        let world = start + (i as f64 + 0.5) * units;
                        if coverage.is_some_and(|(a, b)| world < a || world >= b) {
                            return [INVALID, INVALID, 0, 0];
                        }
                        // Preserve the CPU sampler's evaluation order, including the
                        // source placement round-trip at large world coordinates.
                        let mapped = src_origin
                            + (world - placed) * size as f64 * layer.image.units_per_pixel() / span;
                        let texel = (mapped - src_origin) / layer.image.units_per_pixel();
                        if texel < 0.0 || texel >= size as f64 {
                            return [INVALID, INVALID, 0, 0];
                        }
                        let fixed = ((texel - 0.5) * 65536.0).round() / 65536.0;
                        let first = fixed.floor();
                        [
                            first.clamp(0.0, size as f64 - 1.0) as u32,
                            (first + 1.0).clamp(0.0, size as f64 - 1.0) as u32,
                            ((fixed - first) * 65536.0) as u32,
                            1,
                        ]
                    })
                    .collect()
            };
            let mut axes = axis(true);
            axes.extend(axis(false));
            let resident = layer.image.gpu_straight.is_some();
            let x_groups = groups(&axes[..w as usize], resident);
            let y_groups = groups(&axes[w as usize..], resident);
            let mut parts = Vec::new();
            for &(py, top, bottom) in &y_groups {
                for &(px, left, right) in &x_groups {
                    samples += u64::from(right - left) * u64::from(bottom - top);
                    if samples > 128 * 1024 * 1024 {
                        return Err(CanvasError::ResourceLimit);
                    }
                    let page = if resident {
                        [0, 0, layer.image.width(), layer.image.height()]
                    } else {
                        [
                            px * PAGE,
                            py * PAGE,
                            (PAGE + 1).min(layer.image.width() - px * PAGE),
                            (PAGE + 1).min(layer.image.height() - py * PAGE),
                        ]
                    };
                    parts.push(Part {
                        page,
                        area: [left, top, right - left, bottom - top],
                    });
                }
            }
            if !parts.is_empty() {
                layers.push(Layer {
                    source: layer.image.clone(),
                    axes,
                    parts,
                });
            }
        }
        cancel.check()?;
        Ok(Self {
            layers: layers.into(),
            size: [w, h],
            origin,
            units,
            copy_leaf,
        })
    }
}

fn groups(axis: &[[u32; 4]], resident: bool) -> Vec<(u32, u32, u32)> {
    let mut result = Vec::<(u32, u32, u32)>::new();
    for (i, value) in axis.iter().enumerate() {
        if value[3] == 0 {
            continue;
        }
        let page = if resident { 0 } else { value[0] / PAGE };
        if let Some(last) = result.last_mut()
            && last.0 == page
            && last.2 == i as u32
        {
            last.2 += 1;
        } else {
            result.push((page, i as u32, i as u32 + 1));
        }
    }
    result
}

#[derive(Default, Clone, Debug, serde::Serialize)]
pub struct GpuDonorProfile {
    pub layers: usize,
    pub page_hits: usize,
    pub page_upload_bytes: usize,
    pub axis_upload_bytes: usize,
    pub uniform_upload_bytes: usize,
    pub submitted_dispatches: usize,
    pub poll_count: usize,
    pub retained_bytes: usize,
    pub cache_bytes: usize,
    pub peak_device_bytes: usize,
    pub gpu_ms: Option<f64>,
}
pub struct GpuDonorOutput {
    pub source: SourceImage,
    pub profile: GpuDonorProfile,
}

struct PageEntry {
    key: (u64, u32, u32),
    pixels: Arc<Buffer>,
}
#[derive(Default)]
struct PageCache {
    pages: VecDeque<PageEntry>,
    bytes: usize,
}
impl PageCache {
    fn get(&mut self, key: (u64, u32, u32)) -> Option<Arc<Buffer>> {
        let index = self.pages.iter().position(|p| p.key == key)?;
        let entry = self.pages.remove(index)?;
        let result = entry.pixels.clone();
        self.pages.push_back(entry);
        Some(result)
    }
    fn put(&mut self, entry: PageEntry) {
        while self.bytes + entry.pixels.bytes > CACHE_BYTES {
            let Some(old) = self.pages.pop_front() else {
                break;
            };
            self.bytes -= old.pixels.bytes;
        }
        self.bytes += entry.pixels.bytes;
        self.pages.push_back(entry);
    }
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct DonorParameters {
    extent: [u32; 4],
    page: [u32; 4],
    area: [u32; 4],
}
struct DonorPipeline {
    assemble: Option<ID3D11ComputeShader>,
    copy: ID3D11ComputeShader,
    present: ID3D11ComputeShader,
    uniform: ID3D11Buffer,
    allocation: Arc<Allocation>,
}
impl DonorPipeline {
    fn new(device: &ID3D11Device, memory: &Arc<GpuMemory>) -> Result<Self> {
        let mut doubles = D3D11_FEATURE_DATA_DOUBLES::default();
        let mut options = D3D11_FEATURE_DATA_D3D11_OPTIONS::default();
        let doubles_ok = unsafe {
            device
                .CheckFeatureSupport(
                    D3D11_FEATURE_DOUBLES,
                    (&mut doubles as *mut D3D11_FEATURE_DATA_DOUBLES).cast(),
                    std::mem::size_of_val(&doubles) as u32,
                )
                .is_ok()
        };
        let options_ok = unsafe {
            device
                .CheckFeatureSupport(
                    D3D11_FEATURE_D3D11_OPTIONS,
                    (&mut options as *mut D3D11_FEATURE_DATA_D3D11_OPTIONS).cast(),
                    std::mem::size_of_val(&options) as u32,
                )
                .is_ok()
        };
        let precise = doubles_ok
            && options_ok
            && doubles.DoublePrecisionFloatShaderOps.as_bool()
            && options.ExtendedDoublesShaderInstructions.as_bool();
        let shader = |code: &[u8]| -> Result<ID3D11ComputeShader> {
            let mut result = None;
            unsafe {
                device.CreateComputeShader(code, None, Some(&mut result))?;
            }
            result.context("Donor shader missing")
        };
        let allocation = memory.reserve(std::mem::size_of::<DonorParameters>())?;
        let mut uniform = None;
        unsafe {
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: std::mem::size_of::<DonorParameters>() as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut uniform),
            )?;
        }
        Ok(Self {
            assemble: precise
                .then(|| {
                    shader(include_bytes!(concat!(
                        env!("OUT_DIR"),
                        "/assemble_donors.cso"
                    )))
                })
                .transpose()?,
            copy: shader(include_bytes!(concat!(env!("OUT_DIR"), "/copy_donors.cso")))?,
            present: shader(include_bytes!(concat!(
                env!("OUT_DIR"),
                "/present_donors.cso"
            )))?,
            uniform: uniform.context("Donor constants missing")?,
            allocation,
        })
    }
}

/// Owner-thread eligible-layer compositor with a 64 MiB straight-pixel page
/// cache. It shares the window device, epoch and total allocation budget.
pub struct GpuDonorAssembler {
    service: Arc<dyn GpuService>,
    epoch: u64,
    memory: Arc<GpuMemory>,
    pipeline: Arc<DonorPipeline>,
    cache: Rc<RefCell<PageCache>>,
    profiling: bool,
}
impl GpuDonorAssembler {
    pub fn new(service: Arc<dyn GpuService>) -> Result<Self, CanvasError> {
        ensure_gpu(&service)?;
        let memory =
            GpuMemory::shared(&service, None, DEFAULT_GPU_MEMORY_BUDGET).map_err(gpu_error)?;
        let pipeline = native(&service)
            .map_err(gpu_error)?
            .with_context(|gpu| DonorPipeline::new(gpu.device, &memory))
            .map_err(gpu_error)?;
        Ok(Self {
            epoch: service.epoch(),
            service,
            memory,
            pipeline: Arc::new(pipeline),
            cache: Default::default(),
            profiling: false,
        })
    }
    pub fn set_profiling(&mut self, enabled: bool) {
        self.profiling = enabled;
    }
    pub fn supports_filtered_layers(&self) -> bool {
        self.pipeline.assemble.is_some()
    }
    pub(crate) fn supports_service(&self, service: &Arc<dyn GpuService>) -> bool {
        self.service.service_id() == service.service_id() && self.epoch == service.epoch()
    }
    pub fn clear_cache(&mut self) {
        *self.cache.borrow_mut() = PageCache::default();
    }
    pub fn start(
        &self,
        input: GpuDonorInput,
        cancel: CanvasCancellation,
    ) -> Result<GpuDonorJob, CanvasError> {
        cancel.check()?;
        if !self.supports_service(&self.service) {
            return Err(CanvasError::Gpu("Donor device epoch changed".into()));
        }
        if !input.copy_leaf && self.pipeline.assemble.is_none() {
            return Err(CanvasError::Unsupported(
                "precise layered GPU donors without extended double-precision shaders",
            ));
        }
        for layer in input.layers.iter() {
            if let Some(texture) = layer.source.gpu_texture()
                && (texture.service_id() != self.service.service_id()
                    || texture.epoch() != self.epoch)
            {
                return Err(CanvasError::Gpu(
                    "Donor source belongs to a retired or foreign device".into(),
                ));
            }
        }
        native(&self.service)
            .map_err(gpu_error)?
            .with_context(|gpu| {
                let raw = Arc::new(Buffer::new(
                    gpu.device,
                    input.size[0] as usize * input.size[1] as usize,
                    4,
                    &self.memory,
                )?);
                let image = ComputeImage::new(gpu.device, input.size, &self.memory)?;
                raw.allocation.mark_gpu_use();
                unsafe {
                    gpu.immediate_context
                        .ClearUnorderedAccessViewUint(&raw.output, &[0; 4]);
                }
                Ok(GpuDonorJob {
                    service: self.service.clone(),
                    epoch: self.epoch,
                    memory: self.memory.clone(),
                    pipeline: self.pipeline.clone(),
                    cache: self.cache.clone(),
                    profile: GpuDonorProfile {
                        layers: input.layers.len(),
                        ..Default::default()
                    },
                    input,
                    cancel,
                    raw,
                    image,
                    axes: Vec::new(),
                    layer: 0,
                    part: 0,
                    pending: None,
                    finished: false,
                    profiling: self.profiling,
                    timings: Vec::new(),
                    submitted: None,
                })
            })
            .map_err(gpu_error)
    }
}

/// Submit at most eight source-page draws per poll, checking the two-millisecond
/// encoding budget after each. Completion is nonblocking; no donor readback occurs.
pub struct GpuDonorJob {
    service: Arc<dyn GpuService>,
    epoch: u64,
    memory: Arc<GpuMemory>,
    pipeline: Arc<DonorPipeline>,
    cache: Rc<RefCell<PageCache>>,
    input: GpuDonorInput,
    cancel: CanvasCancellation,
    raw: Arc<Buffer>,
    image: ComputeImage,
    axes: Vec<Arc<Buffer>>,
    layer: usize,
    part: usize,
    pending: Option<Box<dyn GpuCompletion>>,
    finished: bool,
    profile: GpuDonorProfile,
    profiling: bool,
    timings: Vec<GpuTiming>,
    submitted: Option<SourceImage>,
}
impl GpuDonorJob {
    /// Internal same-context dependency: all writes are queued, but completion
    /// is observed by the downstream preparation fence before publication.
    pub(crate) fn submitted_source(&mut self) -> Result<Option<SourceImage>, CanvasError> {
        self.cancel.check()?;
        if self.finished || self.pending.is_none() {
            return Ok(None);
        }
        if self.service.epoch() != self.epoch {
            return Err(CanvasError::Gpu("Donor device epoch changed".into()));
        }
        if self.submitted.is_none() {
            self.submitted = Some(
                self.make_source(native(&self.service).map_err(gpu_error)?)
                    .map_err(gpu_error)?,
            );
        }
        Ok(self.submitted.clone())
    }
    pub fn has_pending_submissions(&self) -> bool {
        !self.finished && self.pending.is_none()
    }
    pub fn poll(&mut self) -> Poll<Result<GpuDonorOutput, CanvasError>> {
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
    fn poll_inner(&mut self) -> Result<Option<GpuDonorOutput>> {
        ensure!(
            self.service.epoch() == self.epoch,
            "Donor device epoch changed"
        );
        let service = self.service.clone();
        let native = native(&service)?;
        if let Some(pending) = &mut self.pending {
            if !pending.poll()? {
                return Ok(None);
            }
            return self.finish(native).map(Some);
        }
        native.with_context(|gpu| {
            let timing = if self.profiling {
                Some(GpuTiming::start(gpu.device, gpu.immediate_context)?)
            } else {
                None
            };
            let started = Instant::now();
            for _ in 0..8 {
                self.cancel.check().map_err(anyhow::Error::from)?;
                if self.layer == self.input.layers.len() {
                    break;
                }
                self.submit_page(gpu.device, gpu.immediate_context)?;
                if started.elapsed().as_millis() >= 2 {
                    break;
                }
            }
            if self.layer == self.input.layers.len() {
                let params = DonorParameters {
                    extent: [self.input.size[0], self.input.size[1], 0, 0],
                    ..Default::default()
                };
                self.bind(gpu.immediate_context, &params);
                self.raw.allocation.mark_gpu_use();
                self.image.allocation.mark_gpu_use();
                unsafe {
                    gpu.immediate_context
                        .CSSetShaderResources(0, Some(&[Some(self.raw.view.clone()), None]));
                    gpu.immediate_context.CSSetUnorderedAccessViews(
                        0,
                        2,
                        Some([None, Some(self.image.output.clone())].as_ptr()),
                        None,
                    );
                    gpu.immediate_context
                        .CSSetShader(&self.pipeline.present, None);
                    gpu.immediate_context.Dispatch(
                        (self.input.size[0] * self.input.size[1]).div_ceil(64),
                        1,
                        1,
                    );
                }
                self.unbind(gpu.immediate_context);
                self.profile.submitted_dispatches += 1;
            }
            if let Some(timing) = timing {
                timing.end(gpu.immediate_context);
                self.timings.push(timing);
            }
            Ok(())
        })?;
        if self.layer == self.input.layers.len() {
            self.pending = Some(native.completion()?);
        }
        Ok(None)
    }
    fn bind(&mut self, context: &ID3D11DeviceContext, params: &DonorParameters) {
        self.pipeline.allocation.mark_gpu_use();
        unsafe {
            context.UpdateSubresource(
                &self.pipeline.uniform,
                0,
                None,
                bytes(std::slice::from_ref(params)).as_ptr().cast(),
                0,
                0,
            );
            context.CSSetConstantBuffers(0, Some(&[Some(self.pipeline.uniform.clone())]));
        }
        self.profile.uniform_upload_bytes += std::mem::size_of::<DonorParameters>();
    }
    fn unbind(&self, context: &ID3D11DeviceContext) {
        unsafe {
            context.CSSetShaderResources(0, Some(&[None, None]));
            context.CSSetUnorderedAccessViews(0, 2, Some([None, None].as_ptr()), None);
        }
    }
    fn submit_page(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext) -> Result<()> {
        if self.part == 0 {
            let layer = &self.input.layers[self.layer];
            let axes = Arc::new(Buffer::new(device, layer.axes.len(), 16, &self.memory)?);
            axes.upload(context, bytes(&layer.axes))?;
            self.profile.axis_upload_bytes += axes.bytes;
            self.axes.push(axes);
        }
        let layer = &self.input.layers[self.layer];
        let part = &layer.parts[self.part];
        let pixels = if let Some(raw) = &layer.source.gpu_straight {
            self.profile.page_hits += 1;
            raw.clone()
        } else {
            let key = (layer.source.pixels_identity(), part.page[0], part.page[1]);
            let cached = self.cache.borrow_mut().get(key);
            if let Some(page) = cached {
                self.profile.page_hits += 1;
                page
            } else {
                let [x, y, w, h] = part.page;
                let source = layer.source.smart_erase_pixels();
                let mut bytes = Vec::with_capacity(w as usize * h as usize * 4);
                for row in y..y + h {
                    let left = (row as usize * source.width() as usize + x as usize) * 4;
                    bytes.extend_from_slice(&source.as_raw()[left..left + w as usize * 4]);
                }
                let page = Arc::new(Buffer::new(
                    device,
                    w as usize * h as usize,
                    4,
                    &self.memory,
                )?);
                page.upload(context, &bytes)?;
                self.profile.page_upload_bytes += page.bytes;
                self.cache.borrow_mut().put(PageEntry {
                    key,
                    pixels: page.clone(),
                });
                page
            }
        };
        let params = DonorParameters {
            extent: [
                self.input.size[0],
                self.input.size[1],
                u32::from(self.input.copy_leaf),
                0,
            ],
            page: part.page,
            area: part.area,
        };
        let count = part.area[2] * part.area[3];
        let last = self.part + 1 == layer.parts.len();
        let axes = self
            .axes
            .last()
            .context("Donor coordinates missing")?
            .clone();
        pixels.allocation.mark_gpu_use();
        axes.allocation.mark_gpu_use();
        self.bind(context, &params);
        unsafe {
            context.CSSetShaderResources(
                0,
                Some(&[Some(pixels.view.clone()), Some(axes.view.clone())]),
            );
            context.CSSetUnorderedAccessViews(
                0,
                2,
                Some([Some(self.raw.output.clone()), None].as_ptr()),
                None,
            );
            context.CSSetShader(
                if self.input.copy_leaf {
                    &self.pipeline.copy
                } else {
                    self.pipeline
                        .assemble
                        .as_ref()
                        .context("Precise donor shader missing")?
                },
                None,
            );
            context.Dispatch(count.div_ceil(64), 1, 1);
        }
        self.unbind(context);
        self.profile.submitted_dispatches += 1;
        self.part += 1;
        if last {
            self.layer += 1;
            self.part = 0;
        }
        Ok(())
    }
    fn make_source(&self, native: &DirectXGpuService) -> Result<SourceImage> {
        let mut source = SourceImage::from_gpu_texture(Arc::new(ResultTexture {
            texture: native.wrap_texture(self.image.resource.clone())?,
            _allocation: self.image.allocation.clone(),
        }))?
        .with_placement(self.input.origin, self.input.units)?;
        source.gpu_straight = Some(self.raw.clone());
        Ok(source)
    }
    fn finish(&mut self, native: &DirectXGpuService) -> Result<GpuDonorOutput> {
        self.cancel.check()?;
        let source = if let Some(source) = self.submitted.take() {
            source
        } else {
            self.make_source(native)?
        };
        for axes in &mut self.axes {
            if let Some(axes) = Arc::get_mut(axes)
                && let Some(allocation) = Arc::get_mut(&mut axes.allocation)
            {
                allocation.mark_gpu_complete();
            }
        }
        self.axes.clear();
        if self.profiling {
            self.profile.gpu_ms = native.with_context(|gpu| {
                let mut total = 0.0;
                for timing in &self.timings {
                    match timing.poll(gpu.immediate_context)? {
                        Some(Some(ms)) => total += ms,
                        _ => return Ok(None),
                    }
                }
                Ok(Some(total))
            })?;
        }
        self.profile.retained_bytes = self.raw.bytes * 2;
        self.profile.cache_bytes = self.cache.borrow().bytes;
        self.profile.peak_device_bytes = self.memory.peak();
        self.finished = true;
        Ok(GpuDonorOutput {
            source,
            profile: self.profile.clone(),
        })
    }
}

#[cfg(test)]
#[path = "gpu_donor_tests.rs"]
mod tests;
