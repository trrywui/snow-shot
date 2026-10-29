//! Bounded owner-thread D3D11 reconstruction. No reconstruction image is read
//! back: one small metadata copy supplies active levels and sparse target counts.
use std::{collections::VecDeque, sync::Arc, task::Poll, time::Instant};

use anyhow::{Context, Result, ensure};
use gpui::{GpuCompletion, GpuService, GpuTexture};
use gpui_windows::gpu_extensions::DirectXGpuService;
use image::RgbaImage;
use windows::Win32::Graphics::{
    Direct3D11::*,
    Dxgi::{Common::*, DXGI_ERROR_WAS_STILL_DRAWING},
};

use crate::{
    CanvasCancellation, CanvasError, DEFAULT_GPU_MEMORY_BUDGET, SourceImage,
    gpu_memory::{Allocation, GpuMemory},
    gpu_renderer::GpuTiming,
};

const MAX_PIXELS: usize = 2 * 1024 * 1024;
const META_BASE: usize = 16;
const META_WORDS: usize = META_BASE + 6 * 12;
const TARGET_BATCH: u32 = 4096;
const PROPAGATIONS: u32 = 4;

#[path = "gpu_donors.rs"]
pub(crate) mod donors;
#[path = "gpu_source_prepare.rs"]
pub(crate) mod source_prepare;
use crate::gpu_wake::{GpuCompletionWake, Notifier};

/// An already cropped, eligible source neighborhood and binary reconstruction
/// domain. Construct/prepare on a CPU worker; submit on the GPUI device owner.
/// GPU sources use `prepare_source`/`start_prepared` to retain their pixels/flags.
pub struct GpuSmartEraseInput {
    pixels: Arc<RgbaImage>,
    flags: Vec<u32>,
    gpu: Option<Box<GpuGrid>>,
    crop: [u32; 4],
    origin: crate::engine::Point<f64>,
    units: f64,
    search_span: u32,
}
struct GpuGrid {
    source: SourceImage,
    flags: Arc<Buffer>,
    pyramid: Option<GpuPyramid>,
}
struct ResidentLevel {
    size: [u32; 2],
    rgb: Arc<Buffer>,
    flags: Arc<Buffer>,
    radii: Arc<Buffer>,
    offsets: Arc<Buffer>,
}
struct GpuPyramid {
    levels: Vec<ResidentLevel>,
    meta: Arc<Buffer>,
    metadata: Vec<u32>,
}
impl GpuSmartEraseInput {
    fn dimensions(&self) -> [u32; 2] {
        self.gpu
            .as_ref()
            .map_or([self.pixels.width(), self.pixels.height()], |g| {
                g.source.dimensions()
            })
    }
    /// Rasterize a completed mask on this input's native source grid. The source
    /// must already be cropped to the intended, bounded donor neighborhood.
    pub fn from_filter(
        source: &SourceImage,
        filter: &crate::engine::FilterDisplayItem,
        cancel: &CanvasCancellation,
    ) -> Result<Self, CanvasError> {
        cancel.check()?;
        if filter.filter.filter_type != crate::engine::DisplayFilterType::SmartErase
            || filter.filter.render_phase != 0
        {
            return Err(crate::engine::ErrorCode::InvalidArgument.into());
        }
        crate::smart_erase_service::mask_bounds(filter)?;
        let [w, h] = source.dimensions();
        if w as usize * h as usize > MAX_PIXELS {
            return Err(CanvasError::ResourceLimit);
        }
        let mask = crate::smart_erase_service::reconstruction_mask(
            filter,
            source.origin(),
            source.units_per_pixel(),
            w,
            h,
        )?;
        Self::new(source, &mask, cancel)
    }
    /// Preserve the pre-crop search scale when a host supplies a smaller ROI.
    pub fn with_search_span(mut self, span: u32) -> Result<Self, CanvasError> {
        if span == 0 || span > MAX_PIXELS as u32 {
            return Err(crate::engine::ErrorCode::InvalidArgument.into());
        }
        self.search_span = span;
        Ok(self)
    }
    pub(crate) fn prepared(
        pixels: Arc<RgbaImage>,
        flags: Vec<u32>,
        crop: [u32; 4],
        origin: crate::engine::Point<f64>,
        units: f64,
        search_span: u32,
    ) -> Self {
        Self {
            pixels,
            flags,
            gpu: None,
            crop,
            origin,
            units,
            search_span,
        }
    }
    pub fn new(
        source: &SourceImage,
        mask: &[u8],
        cancel: &CanvasCancellation,
    ) -> Result<Self, CanvasError> {
        cancel.check()?;
        let [w, h] = source.dimensions();
        if w as usize * h as usize > MAX_PIXELS {
            return Err(CanvasError::ResourceLimit);
        }
        if mask.len() != w as usize * h as usize {
            return Err(crate::engine::ErrorCode::InvalidArgument.into());
        }
        let pixels = Arc::new(source.eligible_region([0, 0, w, h], cancel)?);
        let flags = crate::smart_erase::gpu_domain(&pixels, mask, cancel)?;
        let mut bounds = [w, h, 0, 0];
        for y in 0..h {
            cancel.check()?;
            for x in 0..w {
                if mask[(y * w + x) as usize] != 0 {
                    bounds[0] = bounds[0].min(x);
                    bounds[1] = bounds[1].min(y);
                    bounds[2] = bounds[2].max(x + 1);
                    bounds[3] = bounds[3].max(y + 1);
                }
            }
        }
        let left = bounds[0].saturating_sub(2);
        let top = bounds[1].saturating_sub(2);
        let right = bounds[2].saturating_add(2).min(w);
        let bottom = bounds[3].saturating_add(2).min(h);
        Ok(Self {
            pixels,
            flags,
            gpu: None,
            crop: [left, top, right - left, bottom - top],
            origin: source.origin(),
            units: source.units_per_pixel(),
            search_span: w.max(h),
        })
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct GpuSmartEraseProfile {
    pub solver: Option<crate::SmartEraseSolver>,
    pub levels: usize,
    pub submitted_dispatches: usize,
    pub poll_count: usize,
    pub uploaded_bytes: usize,
    pub source_upload_bytes: usize,
    pub uniform_upload_bytes: usize,
    pub control_readback_bytes: usize,
    pub retained_bytes: usize,
    pub peak_device_bytes: usize,
    pub gpu_ms: Option<f64>,
}
pub struct GpuSmartEraseOutput {
    pub original: SourceImage,
    pub reconstructed: SourceImage,
    pub profile: GpuSmartEraseProfile,
}

pub(crate) struct Buffer {
    resource: ID3D11Buffer,
    view: ID3D11ShaderResourceView,
    output: ID3D11UnorderedAccessView,
    allocation: Arc<Allocation>,
    bytes: usize,
}
impl Buffer {
    fn new(
        device: &ID3D11Device,
        count: usize,
        stride: usize,
        memory: &Arc<GpuMemory>,
    ) -> Result<Self> {
        let bytes = count
            .checked_mul(stride)
            .context("Compute buffer extent overflow")?;
        ensure!(
            bytes > 0 && bytes <= u32::MAX as usize,
            "Invalid compute buffer extent"
        );
        let allocation = memory.reserve(bytes)?;
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: bytes as u32,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_UNORDERED_ACCESS).0 as u32,
            MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
            StructureByteStride: stride as u32,
            ..Default::default()
        };
        let mut resource = None;
        let mut view = None;
        let mut output = None;
        unsafe {
            device.CreateBuffer(&desc, None, Some(&mut resource))?;
            let buffer = resource.as_ref().context("Compute buffer missing")?;
            device.CreateShaderResourceView(buffer, None, Some(&mut view))?;
            device.CreateUnorderedAccessView(buffer, None, Some(&mut output))?;
        }
        Ok(Self {
            resource: resource.unwrap(),
            view: view.context("Compute SRV missing")?,
            output: output.context("Compute UAV missing")?,
            allocation,
            bytes,
        })
    }
    fn upload(&self, context: &ID3D11DeviceContext, bytes: &[u8]) -> Result<()> {
        ensure!(bytes.len() == self.bytes, "Compute upload size mismatch");
        self.allocation.mark_gpu_use();
        unsafe {
            context.UpdateSubresource(&self.resource, 0, None, bytes.as_ptr().cast(), 0, 0);
        }
        Ok(())
    }
}

struct ComputeImage {
    resource: ID3D11Texture2D,
    output: ID3D11UnorderedAccessView,
    allocation: Arc<Allocation>,
}
impl ComputeImage {
    fn new(device: &ID3D11Device, size: [u32; 2], memory: &Arc<GpuMemory>) -> Result<Self> {
        ensure!(
            size.iter().all(|n| *n > 0 && *n <= 16384),
            "Invalid compute image extent"
        );
        let allocation = memory.reserve(size[0] as usize * size[1] as usize * 4)?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: size[0],
            Height: size[1],
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_UNORDERED_ACCESS).0 as u32,
            ..Default::default()
        };
        let mut resource = None;
        let mut output = None;
        unsafe {
            device.CreateTexture2D(&desc, None, Some(&mut resource))?;
            device.CreateUnorderedAccessView(
                resource.as_ref().unwrap(),
                None,
                Some(&mut output),
            )?;
        }
        Ok(Self {
            resource: resource.unwrap(),
            output: output.context("Compute image UAV missing")?,
            allocation,
        })
    }
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Parameters {
    extent: [u32; 4],
    previous: [u32; 4],
    options: [u32; 4],
    crop: [u32; 4],
    scales: [f32; 4],
}
fn bytes<T: Copy>(values: &[T]) -> &[u8] {
    // Only initialized numeric arrays and padding-free Parameters are passed.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

fn read_staging(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Buffer,
    words: usize,
) -> Result<Option<Vec<u32>>> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    let result = unsafe {
        context.Map(
            staging,
            0,
            D3D11_MAP_READ,
            D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
            Some(&mut mapped),
        )
    };
    match result {
        Err(error) if error.code() == DXGI_ERROR_WAS_STILL_DRAWING => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(()) => {}
    }
    // The staging extent is checked at allocation; Map supplies aligned storage
    // that remains readable until the matching Unmap on this same owner thread.
    let data = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u32>(), words).to_vec() };
    unsafe {
        context.Unmap(staging, 0);
    }
    Ok(Some(data))
}
macro_rules! kernels {
    ($($kind:ident => $entry:literal),+ $(,)?) => {
        #[derive(Clone,Copy)] enum Kernel {$($kind),+}
        fn shaders(device:&ID3D11Device)->Result<Vec<ID3D11ComputeShader>> {
            let mut result=Vec::new();
            $(let mut shader=None;
            unsafe {device.CreateComputeShader(include_bytes!(concat!(env!("OUT_DIR"),"/",$entry,".cso")),None,Some(&mut shader))?;}
            result.push(shader.context("Compute shader missing")?);)+
            Ok(result)
        }
    }
}
kernels! {Load=>"load_source", Downsample=>"downsample", Erode=>"erode_rows", Prefix=>"prefix_rows",
Compact=>"compact_rows", NearestRows=>"nearest_rows", NearestColumns=>"nearest_columns",
Lab=>"lab_colors", Horizontal=>"moments_horizontal", Vertical=>"moments_vertical", Guides=>"guide_lines",
Initialize=>"initialize_matches", ClearReuse=>"clear_reuse", CountReuse=>"count_reuse",
Match=>"match_patches", Vote=>"vote_patches", Finish=>"finish_pixels", Original=>"present_original",
SourceFlags=>"source_flags", DomainInit=>"domain_initialize", DomainPixels=>"domain_pixels",
DomainAdvance=>"domain_advance", DomainMargin=>"domain_margin", DetectPeriodic=>"detect_periodic", PeriodicLines=>"periodic_lines"}

struct Pipeline {
    shaders: Vec<ID3D11ComputeShader>,
    uniform: ID3D11Buffer,
    allocation: Arc<Allocation>,
}
impl Pipeline {
    fn new(device: &ID3D11Device, memory: &Arc<GpuMemory>) -> Result<Self> {
        let allocation = memory.reserve(std::mem::size_of::<Parameters>())?;
        let mut uniform = None;
        unsafe {
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: std::mem::size_of::<Parameters>() as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut uniform),
            )?;
        }
        Ok(Self {
            shaders: shaders(device)?,
            uniform: uniform.context("Compute constants missing")?,
            allocation,
        })
    }
}

#[derive(Clone)]
struct Step {
    kernel: Kernel,
    parameters: Parameters,
    threads: u32,
    inputs: [Option<usize>; 11],
    outputs: [Option<usize>; 8],
    image: Option<usize>,
}
impl Step {
    fn new(kernel: Kernel, level: &Level) -> Self {
        Self {
            kernel,
            parameters: Parameters {
                extent: [level.size[0], level.size[1], level.index as u32, 0],
                scales: [level.search_span as f32, 0.0, 0.0, 0.0],
                ..Default::default()
            },
            threads: level.size[0] * level.size[1],
            inputs: [None; 11],
            outputs: [None; 8],
            image: None,
        }
    }
    fn inputs(mut self, items: &[(usize, usize)]) -> Self {
        for &(slot, id) in items {
            self.inputs[slot] = Some(id);
        }
        self
    }
    fn outputs(mut self, items: &[(usize, usize)]) -> Self {
        for &(slot, id) in items {
            self.outputs[slot] = Some(id);
        }
        self
    }
}
struct Level {
    index: usize,
    size: [u32; 2],
    search_span: u32,
    rgb: usize,
    flags: usize,
    donor_max: usize,
    offsets: usize,
    result: usize,
    matches: usize,
}
fn submit_compute(
    context: &ID3D11DeviceContext,
    pipeline: &Pipeline,
    buffers: &[Arc<Buffer>],
    image: Option<&ComputeImage>,
    meta: Option<usize>,
    step: &Step,
) -> Result<()> {
    let inputs = step.inputs.map(|id| {
        id.map(|i| {
            buffers[i].allocation.mark_gpu_use();
            buffers[i].view.clone()
        })
    });
    let mut outputs = step.outputs.map(|id| {
        id.map(|i| {
            buffers[i].allocation.mark_gpu_use();
            buffers[i].output.clone()
        })
    });
    if let Some(meta) = meta {
        buffers[meta].allocation.mark_gpu_use();
        outputs[7] = Some(buffers[meta].output.clone());
    }
    if let Some(image) = image {
        image.allocation.mark_gpu_use();
        outputs[5] = Some(image.output.clone());
    }
    pipeline.allocation.mark_gpu_use();
    unsafe {
        context.UpdateSubresource(
            &pipeline.uniform,
            0,
            None,
            bytes(&[step.parameters]).as_ptr().cast(),
            0,
            0,
        );
        context.CSSetConstantBuffers(0, Some(&[Some(pipeline.uniform.clone())]));
        context.CSSetShaderResources(0, Some(&inputs));
        context.CSSetUnorderedAccessViews(0, 8, Some(outputs.as_ptr()), None);
        context.CSSetShader(&pipeline.shaders[step.kernel as usize], None);
        context.Dispatch(step.threads.div_ceil(64), 1, 1);
        context.CSSetShaderResources(0, Some(&[const { None }; 11]));
        context.CSSetUnorderedAccessViews(0, 8, Some([const { None }; 8].as_ptr()), None);
    }
    Ok(())
}
struct Scratch {
    lab: usize,
    mean: usize,
    guide: usize,
    moments: usize,
    weight: usize,
    guide_cost: usize,
    synth: [usize; 2],
    candidates: usize,
    donors: usize,
    holes: usize,
    nearest: [usize; 2],
    match_next: usize,
    cost: usize,
    reuse: usize,
    rows: usize,
}

/// Reusable shader/device state. Multiple jobs share GPUI's existing device and
/// allocation budget; no second windowed graphics device is created.
pub struct GpuSmartErase {
    service: Arc<dyn GpuService>,
    memory: Arc<GpuMemory>,
    pipeline: Arc<Pipeline>,
    profiling: bool,
    epoch: u64,
    wake: Notifier,
}
impl GpuSmartErase {
    pub fn new(service: Arc<dyn GpuService>) -> Result<Self, CanvasError> {
        ensure_gpu(&service)?;
        let memory =
            GpuMemory::shared(&service, None, DEFAULT_GPU_MEMORY_BUDGET).map_err(gpu_error)?;
        let native = native(&service).map_err(gpu_error)?;
        let pipeline = Arc::new(
            native
                .with_context(|gpu| Pipeline::new(gpu.device, &memory))
                .map_err(gpu_error)?,
        );
        let wake = Notifier::new(service.clone()).map_err(gpu_error)?;
        Ok(Self {
            epoch: service.epoch(),
            service,
            memory,
            pipeline,
            profiling: false,
            wake,
        })
    }
    pub fn set_profiling(&mut self, enabled: bool) {
        self.profiling = enabled;
    }
    /// Wake the calling task after already submitted GPU work, without polling a
    /// timer. `None` selects timer polling on devices without native fence support.
    /// A wake is only a scheduling hint: poll the job on its owner thread again
    /// to validate completion, cancellation and device state before publication.
    pub fn completion_wake(&self) -> Result<Option<GpuCompletionWake>, CanvasError> {
        self.wake.wake().map_err(gpu_error)
    }
    pub(crate) fn supports_service(&self, service: &Arc<dyn GpuService>) -> bool {
        self.service.service_id() == service.service_id() && self.epoch == service.epoch()
    }
    pub fn start(
        &self,
        input: GpuSmartEraseInput,
        cancel: CanvasCancellation,
    ) -> Result<GpuSmartEraseJob, CanvasError> {
        cancel.check()?;
        if !self.supports_service(&self.service) {
            return Err(CanvasError::Gpu(
                "Reconstruction device epoch changed".into(),
            ));
        }
        if let Some(grid) = &input.gpu {
            validate_source_device(&grid.source, &self.service)?;
        }
        if input.crop[2] == 0
            || input.crop[3] == 0
            || input.crop[2] > 16384
            || input.crop[3] > 16384
        {
            return Err(CanvasError::ResourceLimit);
        }
        native(&self.service)
            .map_err(gpu_error)?
            .with_context(|gpu| {
                GpuSmartEraseJob::new(self, input, cancel, gpu.device, gpu.immediate_context)
            })
            .map_err(gpu_error)
    }
}
fn ensure_gpu(service: &Arc<dyn GpuService>) -> Result<(), CanvasError> {
    if !service.capabilities().compute_shaders || !service.as_any().is::<DirectXGpuService>() {
        return Err(CanvasError::Unsupported("D3D11 compute reconstruction"));
    }
    Ok(())
}
fn native(service: &Arc<dyn GpuService>) -> Result<&DirectXGpuService> {
    service
        .as_any()
        .downcast_ref::<DirectXGpuService>()
        .context("D3D11 reconstruction device missing")
}
fn gpu_error(error: anyhow::Error) -> CanvasError {
    CanvasError::Gpu(format!("{error:#}"))
}
fn validate_source_device(
    source: &SourceImage,
    service: &Arc<dyn GpuService>,
) -> Result<(), CanvasError> {
    let Some(texture) = source.gpu_texture() else {
        return Err(CanvasError::Unsupported(
            "GPU preparation without a GPU source",
        ));
    };
    if source.gpu_straight.is_none() {
        return Err(CanvasError::Unsupported(
            "GPU preparation without exact straight RGBA",
        ));
    }
    if texture.service_id() != service.service_id() || texture.epoch() != service.epoch() {
        return Err(CanvasError::Gpu(
            "Source belongs to a retired or foreign device".into(),
        ));
    }
    Ok(())
}

enum Phase {
    Build,
    Metadata,
    Fill,
    Complete,
    Finished,
}
/// Poll on the GPUI device owner. Each poll submits at most eight dispatches and
/// yields after two milliseconds of command encoding. It never waits for the GPU.
pub struct GpuSmartEraseJob {
    service: Arc<dyn GpuService>,
    memory: Arc<GpuMemory>,
    pipeline: Arc<Pipeline>,
    epoch: u64,
    input: GpuSmartEraseInput,
    cancel: CanvasCancellation,
    buffers: Vec<Arc<Buffer>>,
    images: Vec<ComputeImage>,
    levels: Vec<Level>,
    scratch: Scratch,
    raw: usize,
    meta: usize,
    raw_output: [usize; 2],
    steps: VecDeque<Step>,
    phase: Phase,
    pending: Option<Box<dyn GpuCompletion>>,
    staging: Option<ID3D11Buffer>,
    _staging_allocation: Option<Arc<Allocation>>,
    profile: GpuSmartEraseProfile,
    profiling: bool,
    timings: Vec<GpuTiming>,
}
impl GpuSmartEraseJob {
    fn new(
        runtime: &GpuSmartErase,
        mut input: GpuSmartEraseInput,
        cancel: CanvasCancellation,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
    ) -> Result<Self> {
        let mut buffers = if let Some(grid) = &input.gpu {
            vec![
                grid.source
                    .gpu_straight
                    .as_ref()
                    .context("GPU pixels missing")?
                    .clone(),
                grid.flags.clone(),
            ]
        } else {
            Vec::<Arc<Buffer>>::new()
        };
        let prebuilt = input.gpu.as_mut().and_then(|g| g.pyramid.take());
        let mut resident = Vec::new();
        let prebuilt_meta = prebuilt.map(|pyramid| {
            let meta = buffers.len();
            buffers.push(pyramid.meta);
            for level in pyramid.levels {
                let start = buffers.len();
                buffers.extend([level.rgb, level.flags, level.radii, level.offsets]);
                resident.push((level.size, [start, start + 1, start + 2, start + 3]));
            }
            (meta, pyramid.metadata)
        });
        let mut add = |n: usize, stride: usize| -> Result<usize> {
            let index = buffers.len();
            buffers.push(Arc::new(Buffer::new(device, n, stride, &runtime.memory)?));
            Ok(index)
        };
        let [w, h] = input.dimensions();
        let n = w as usize * h as usize;
        let raw = if input.gpu.is_some() { 0 } else { add(n, 4)? };
        let flags = if input.gpu.is_some() { 1 } else { add(n, 4)? };
        let meta = if let Some((meta, _)) = &prebuilt_meta {
            *meta
        } else {
            add(META_WORDS, 4)?
        };
        let mut levels = Vec::new();
        let mut size = [w, h];
        let mut span = input.search_span;
        if !resident.is_empty() {
            for (size, ids) in resident {
                let pixels = size[0] as usize * size[1] as usize;
                levels.push(Level {
                    index: levels.len(),
                    size,
                    search_span: span,
                    rgb: ids[0],
                    flags: ids[1],
                    donor_max: ids[2],
                    offsets: ids[3],
                    result: add(pixels, 16)?,
                    matches: add(pixels, 4)?,
                });
                span = span.div_ceil(2);
            }
        } else {
            loop {
                let pixels = size[0] as usize * size[1] as usize;
                levels.push(Level {
                    index: levels.len(),
                    size,
                    search_span: span,
                    rgb: add(pixels, 16)?,
                    flags: add(pixels, 4)?,
                    donor_max: add(pixels, 4)?,
                    offsets: add(size[1] as usize * 2, 4)?,
                    result: add(pixels, 16)?,
                    matches: add(pixels, 4)?,
                });
                if levels.len() == 6 || size[0].min(size[1]) < 40 {
                    break;
                }
                size = size.map(|value| value.div_ceil(2));
                span = span.div_ceil(2);
            }
        }
        let scratch = Scratch {
            lab: add(n, 16)?,
            mean: add(n, 16)?,
            guide: add(n, 16)?,
            moments: add(n, 16)?,
            weight: add(n, 4)?,
            guide_cost: add(n, 4)?,
            synth: [add(n, 16)?, add(n, 16)?],
            candidates: add(n, 4)?,
            donors: add(n, 4)?,
            holes: add(n, 4)?,
            nearest: [add(n, 4)?, add(n, 4)?],
            match_next: add(n, 4)?,
            cost: add(n, 4)?,
            reuse: add(n, 4)?,
            rows: add(h as usize * 8, 4)?,
        };
        let cropped = input.crop[2] as usize * input.crop[3] as usize;
        let raw_output = [add(cropped, 4)?, add(cropped, 4)?];
        let mut staging = None;
        let staging_allocation = if prebuilt_meta.is_none() {
            Some(runtime.memory.reserve(META_WORDS * 4)?)
        } else {
            None
        };
        if prebuilt_meta.is_none() {
            unsafe {
                device.CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        ByteWidth: (META_WORDS * 4) as u32,
                        Usage: D3D11_USAGE_STAGING,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut staging),
                )?;
            }
        }
        let images = vec![
            ComputeImage::new(device, [input.crop[2], input.crop[3]], &runtime.memory)?,
            ComputeImage::new(device, [input.crop[2], input.crop[3]], &runtime.memory)?,
        ];
        if input.gpu.is_none() {
            buffers[raw].upload(context, input.pixels.as_raw())?;
            buffers[flags].upload(context, bytes(&input.flags))?;
        }
        if prebuilt_meta.is_none() {
            buffers[meta].allocation.mark_gpu_use();
            unsafe {
                context.ClearUnorderedAccessViewUint(&buffers[meta].output, &[0; 4]);
            }
        }
        let mut steps = VecDeque::new();
        if prebuilt_meta.is_none() {
            steps.push_back(
                Step::new(Kernel::Load, &levels[0])
                    .inputs(&[(4, raw), (5, flags)])
                    .outputs(&[(0, levels[0].rgb), (2, levels[0].flags)]),
            );
            for (i, level) in levels.iter().enumerate() {
                if i > 0 {
                    let prev = &levels[i - 1];
                    let mut step = Step::new(Kernel::Downsample, level)
                        .inputs(&[(0, prev.rgb), (4, prev.flags)])
                        .outputs(&[(0, level.rgb), (2, level.flags)]);
                    step.parameters.previous[2..].copy_from_slice(&prev.size);
                    steps.push_back(step);
                }
                let mut erosion = Step::new(Kernel::Erode, level)
                    .inputs(&[(4, level.flags)])
                    .outputs(&[(2, level.donor_max), (3, scratch.rows)]);
                erosion.threads = level.size[1];
                steps.push_back(erosion);
                let mut prefix = Step::new(Kernel::Prefix, level)
                    .inputs(&[(4, scratch.rows)])
                    .outputs(&[(2, level.offsets)]);
                prefix.threads = 1;
                steps.push_back(prefix);
            }
        }
        let uploaded = if input.gpu.is_some() { 0 } else { n * 8 };
        let mut job = Self {
            service: runtime.service.clone(),
            memory: runtime.memory.clone(),
            pipeline: runtime.pipeline.clone(),
            epoch: runtime.service.epoch(),
            input,
            cancel,
            buffers,
            images,
            levels,
            scratch,
            raw,
            meta,
            raw_output,
            steps,
            phase: if prebuilt_meta.is_some() {
                Phase::Fill
            } else {
                Phase::Build
            },
            pending: None,
            staging,
            _staging_allocation: staging_allocation,
            profile: GpuSmartEraseProfile {
                solver: Some(crate::SmartEraseSolver::Multiscale),
                uploaded_bytes: uploaded,
                source_upload_bytes: uploaded,
                ..Default::default()
            },
            profiling: runtime.profiling,
            timings: Vec::new(),
        };
        if let Some((_, words)) = prebuilt_meta {
            job.plan_fill(&words)?;
        }
        Ok(job)
    }

    /// Ready command batches should resume after an event-loop yield. Delay only
    /// when waiting for GPU completion; timer granularity must not throttle work
    /// which can already be submitted on the owner thread.
    pub fn has_pending_submissions(&self) -> bool {
        matches!(self.phase, Phase::Build | Phase::Fill) && !self.steps.is_empty()
    }

    pub fn poll(&mut self) -> Poll<Result<GpuSmartEraseOutput, CanvasError>> {
        if matches!(self.phase, Phase::Finished) {
            return Poll::Ready(Err(crate::engine::ErrorCode::InvalidState.into()));
        }
        if let Err(error) = self.cancel.check() {
            self.phase = Phase::Finished;
            return Poll::Ready(Err(error));
        }
        match self.poll_inner() {
            Ok(Some(output)) => Poll::Ready(Ok(output)),
            Ok(None) => Poll::Pending,
            Err(error) => {
                self.phase = Phase::Finished;
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
            "Reconstruction device epoch changed"
        );
        self.profile.poll_count += 1;
        let service = self.service.clone();
        let native = native(&service)?;
        if matches!(self.phase, Phase::Metadata | Phase::Complete) {
            if !self
                .pending
                .as_mut()
                .context("Reconstruction completion missing")?
                .poll()?
            {
                return Ok(None);
            }
            if matches!(self.phase, Phase::Complete) {
                self.pending = None;
                return self.finish(native).map(Some);
            }
            let Some(words) = native.with_context(|gpu| {
                read_staging(
                    gpu.immediate_context,
                    self.staging.as_ref().context("Metadata staging missing")?,
                    META_WORDS,
                )
            })?
            else {
                return Ok(None);
            };
            self.pending = None;
            self.profile.control_readback_bytes = META_WORDS * 4;
            ensure!(
                words[1] == 0 && words[0] > 0,
                "GPU reconstruction has no complete donor patches"
            );
            self.plan_fill(&words)?;
            self.phase = Phase::Fill;
        }
        native.with_context(|gpu| {
            let timing = if self.profiling {
                Some(GpuTiming::start(gpu.device, gpu.immediate_context)?)
            } else {
                None
            };
            let start = Instant::now();
            for _ in 0..8 {
                self.cancel.check().map_err(anyhow::Error::from)?;
                let Some(step) = self.steps.pop_front() else {
                    break;
                };
                self.submit(gpu.immediate_context, &step)?;
                if start.elapsed().as_millis() >= 2 {
                    break;
                }
            }
            if let Some(timing) = timing {
                timing.end(gpu.immediate_context);
                self.timings.push(timing);
            }
            if self.steps.is_empty() && matches!(self.phase, Phase::Build) {
                self._staging_allocation
                    .as_ref()
                    .context("Metadata allocation missing")?
                    .mark_gpu_use();
                unsafe {
                    gpu.immediate_context.CopyResource(
                        self.staging.as_ref().context("Metadata staging missing")?,
                        &self.buffers[self.meta].resource,
                    );
                }
            }
            Ok(())
        })?;
        if self.steps.is_empty() {
            self.pending = Some(native.completion()?);
            self.phase = if matches!(self.phase, Phase::Build) {
                Phase::Metadata
            } else {
                Phase::Complete
            };
        }
        Ok(None)
    }
    fn submit(&mut self, context: &ID3D11DeviceContext, step: &Step) -> Result<()> {
        submit_compute(
            context,
            &self.pipeline,
            &self.buffers,
            step.image.map(|i| &self.images[i]),
            Some(self.meta),
            step,
        )?;
        self.profile.submitted_dispatches += 1;
        self.profile.uniform_upload_bytes += std::mem::size_of::<Parameters>();
        self.profile.uploaded_bytes += std::mem::size_of::<Parameters>();
        Ok(())
    }

    fn plan_fill(&mut self, words: &[u32]) -> Result<()> {
        let active = words[0] as usize;
        ensure!(
            active <= self.levels.len(),
            "Invalid reconstruction level count"
        );
        self.profile.levels = active;
        let s = &self.scratch;
        for i in (0..active).rev() {
            let l = &self.levels[i];
            let holes = words[META_BASE + i * 12 + 1];
            let mut compact = Step::new(Kernel::Compact, l)
                .inputs(&[(4, l.donor_max), (5, l.offsets), (6, l.flags)])
                .outputs(&[(2, s.candidates), (3, s.donors), (4, s.holes)]);
            compact.threads = l.size[1];
            self.steps.push_back(compact);
            let mut near = Step::new(Kernel::NearestRows, l)
                .inputs(&[(4, s.donors)])
                .outputs(&[(2, s.nearest[0])]);
            near.threads = l.size[1];
            self.steps.push_back(near);
            let mut near = Step::new(Kernel::NearestColumns, l)
                .inputs(&[(4, s.nearest[0])])
                .outputs(&[(2, s.nearest[1])]);
            near.threads = l.size[0];
            self.steps.push_back(near);
            self.steps.push_back(
                Step::new(Kernel::Lab, l)
                    .inputs(&[(0, l.rgb)])
                    .outputs(&[(0, s.lab)]),
            );
            self.steps.push_back(
                Step::new(Kernel::Horizontal, l)
                    .inputs(&[(0, s.lab), (4, l.flags)])
                    .outputs(&[(0, s.moments), (2, s.weight)]),
            );
            self.steps.push_back(
                Step::new(Kernel::Vertical, l)
                    .inputs(&[(0, s.moments), (4, s.weight)])
                    .outputs(&[(0, s.mean), (1, s.guide), (2, s.guide_cost)]),
            );
            for direction in 0..4 {
                let mut step = Step::new(Kernel::Guides, l)
                    .inputs(&[(0, s.mean), (4, l.flags)])
                    .outputs(&[(0, s.guide), (2, s.guide_cost)]);
                step.parameters.options[3] = direction;
                step.threads = l.size[0] + l.size[1] - 1;
                self.steps.push_back(step);
            }
            let mut init = Step::new(Kernel::Initialize, l)
                .inputs(&[
                    (0, s.lab),
                    (3, s.guide),
                    (4, l.flags),
                    (5, s.nearest[1]),
                    (6, s.candidates),
                    (8, s.donors),
                ])
                .outputs(&[
                    (0, s.synth[0]),
                    (1, s.synth[1]),
                    (2, l.matches),
                    (3, s.match_next),
                ]);
            if i + 1 < active {
                let previous = &self.levels[i + 1];
                init.parameters.previous[..2].copy_from_slice(&previous.size);
                init.inputs[2] = Some(previous.result);
                init.inputs[7] = Some(previous.matches);
            }
            self.steps.push_back(init);
            let passes = if i + 1 == active {
                5
            } else if i == 0 {
                2
            } else {
                3
            };
            let mut image = 0;
            for pass in 0..passes {
                self.steps
                    .push_back(Step::new(Kernel::ClearReuse, l).outputs(&[(2, s.reuse)]));
                for base in (0..holes).step_by(TARGET_BATCH as usize) {
                    let mut step = Step::new(Kernel::CountReuse, l)
                        .inputs(&[(4, s.holes), (5, l.matches)])
                        .outputs(&[(2, s.reuse)]);
                    step.parameters.options = [base, (holes - base).min(TARGET_BATCH), pass, 0];
                    step.threads = step.parameters.options[1];
                    self.steps.push_back(step);
                }
                let mut matches = l.matches;
                for propagation in 0..PROPAGATIONS {
                    let next = if matches == l.matches {
                        s.match_next
                    } else {
                        l.matches
                    };
                    for base in (0..holes).step_by(TARGET_BATCH as usize) {
                        let mut step = Step::new(Kernel::Match, l)
                            .inputs(&[
                                (0, s.lab),
                                (1, s.synth[image]),
                                (2, s.guide),
                                (4, l.flags),
                                (5, matches),
                                (6, s.candidates),
                                (7, s.nearest[1]),
                                (8, s.reuse),
                                (9, s.holes),
                                (10, s.donors),
                            ])
                            .outputs(&[(2, next), (3, s.cost)]);
                        step.parameters.options =
                            [base, (holes - base).min(TARGET_BATCH), pass, propagation];
                        step.threads = step.parameters.options[1];
                        step.parameters.scales[1] =
                            0.15 + 0.20 * pass as f32 / (passes - 1).max(1) as f32;
                        self.steps.push_back(step);
                    }
                    matches = next;
                }
                debug_assert_eq!(matches, l.matches);
                for base in (0..holes).step_by(TARGET_BATCH as usize) {
                    let mut step = Step::new(Kernel::Vote, l)
                        .inputs(&[
                            (0, s.lab),
                            (1, l.rgb),
                            (2, s.guide),
                            (3, s.synth[image]),
                            (4, l.flags),
                            (5, l.matches),
                            (6, s.cost),
                            (7, s.holes),
                        ])
                        .outputs(&[(0, s.synth[1 - image]), (1, l.result)]);
                    step.parameters.options = [base, (holes - base).min(TARGET_BATCH), pass, 0];
                    step.threads = step.parameters.options[1];
                    self.steps.push_back(step);
                }
                image = 1 - image;
            }
        }
        let l = &self.levels[0];
        let mut finish = Step::new(Kernel::Finish, l)
            .inputs(&[(0, l.result), (4, self.raw), (5, l.flags)])
            .outputs(&[(2, self.raw_output[0]), (3, self.raw_output[1])]);
        finish.parameters.crop = self.input.crop;
        finish.threads = self.input.crop[2] * self.input.crop[3];
        finish.image = Some(1);
        self.steps.push_back(finish);
        let mut original = Step::new(Kernel::Original, l).inputs(&[(4, self.raw_output[0])]);
        original.parameters.extent[..2].copy_from_slice(&self.input.crop[2..]);
        original.threads = self.input.crop[2] * self.input.crop[3];
        original.image = Some(0);
        self.steps.push_back(original);
        Ok(())
    }

    fn finish(&mut self, native: &DirectXGpuService) -> Result<GpuSmartEraseOutput> {
        self.cancel.check()?;
        let origin = crate::engine::Point::new(
            self.input.origin.x + self.input.crop[0] as f64 * self.input.units,
            self.input.origin.y + self.input.crop[1] as f64 * self.input.units,
        );
        let mut images = Vec::new();
        for (i, image) in self.images.iter().enumerate() {
            let wrapped = Arc::new(ResultTexture {
                texture: native.wrap_texture(image.resource.clone())?,
                _allocation: image.allocation.clone(),
            });
            let mut source =
                SourceImage::from_gpu_texture(wrapped)?.with_placement(origin, self.input.units)?;
            source.gpu_straight = Some(self.buffers[self.raw_output[i]].clone());
            images.push(source);
        }
        if self.profiling {
            self.profile.gpu_ms = native.with_context(|gpu| {
                let mut sum = 0.0;
                for timing in &self.timings {
                    match timing.poll(gpu.immediate_context)? {
                        Some(Some(ms)) => sum += ms,
                        _ => return Ok(None),
                    }
                }
                Ok(Some(sum))
            })?;
        }
        self.profile.retained_bytes =
            self.input.crop[2] as usize * self.input.crop[3] as usize * 16;
        self.profile.peak_device_bytes = self.memory.peak();
        self.phase = Phase::Finished;
        for buffer in &mut self.buffers {
            if let Some(buffer) = Arc::get_mut(buffer)
                && let Some(allocation) = Arc::get_mut(&mut buffer.allocation)
            {
                allocation.mark_gpu_complete();
            }
        }
        self.buffers.clear();
        self.images.clear();
        Ok(GpuSmartEraseOutput {
            original: images.remove(0),
            reconstructed: images.remove(0),
            profile: self.profile.clone(),
        })
    }
}
/// Explicit persistence readback of exact straight RGBA, independent of the
/// premultiplied display texture. The staging copy stays budgeted on cancellation.
pub(crate) struct StraightReadback {
    service: Arc<dyn GpuService>,
    source: SourceImage,
    cancel: CanvasCancellation,
    staging: ID3D11Buffer,
    allocation: Arc<Allocation>,
    pending: Box<dyn GpuCompletion>,
    finished: bool,
}
impl StraightReadback {
    pub(crate) fn new(
        service: Arc<dyn GpuService>,
        source: SourceImage,
        cancel: CanvasCancellation,
    ) -> Result<Self, CanvasError> {
        cancel.check()?;
        let texture = source.gpu_texture().ok_or(CanvasError::Unsupported(
            "straight GPU source readback without a texture",
        ))?;
        if texture.service_id() != service.service_id() || texture.epoch() != service.epoch() {
            return Err(CanvasError::Gpu(
                "Source belongs to a retired or foreign device".into(),
            ));
        }
        let buffer = source
            .gpu_straight
            .as_ref()
            .ok_or(CanvasError::Unsupported(
                "straight GPU source readback without exact pixels",
            ))?;
        let memory =
            GpuMemory::shared(&service, None, DEFAULT_GPU_MEMORY_BUDGET).map_err(gpu_error)?;
        let allocation = memory.reserve(buffer.bytes).map_err(gpu_error)?;
        let device = native(&service).map_err(gpu_error)?;
        let staging = device
            .with_context(|gpu| {
                let mut staging = None;
                unsafe {
                    gpu.device.CreateBuffer(
                        &D3D11_BUFFER_DESC {
                            ByteWidth: buffer.bytes as u32,
                            Usage: D3D11_USAGE_STAGING,
                            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                            ..Default::default()
                        },
                        None,
                        Some(&mut staging),
                    )?;
                    let staging = staging.context("Straight RGBA staging missing")?;
                    allocation.mark_gpu_use();
                    buffer.allocation.mark_gpu_use();
                    gpu.immediate_context
                        .CopyResource(&staging, &buffer.resource);
                    Ok(staging)
                }
            })
            .map_err(gpu_error)?;
        let pending = device.completion().map_err(gpu_error)?;
        Ok(Self {
            service,
            source,
            cancel,
            staging,
            allocation,
            pending,
            finished: false,
        })
    }

    pub(crate) fn poll(&mut self) -> Poll<Result<RgbaImage, CanvasError>> {
        if self.finished {
            return Poll::Ready(Err(crate::engine::ErrorCode::InvalidState.into()));
        }
        if let Err(error) = self.cancel.check() {
            self.finished = true;
            return Poll::Ready(Err(error));
        }
        let result = (|| -> Result<Option<RgbaImage>> {
            let texture = self
                .source
                .gpu_texture()
                .context("Readback source texture missing")?;
            ensure!(
                texture.service_id() == self.service.service_id()
                    && texture.epoch() == self.service.epoch(),
                "Readback device epoch changed"
            );
            if !self.pending.poll()? {
                return Ok(None);
            }
            let data = native(&self.service)?.with_context(|gpu| {
                read_staging(
                    gpu.immediate_context,
                    &self.staging,
                    self.source.width() as usize * self.source.height() as usize,
                )
            })?;
            let Some(data) = data else { return Ok(None) };
            if let Some(allocation) = Arc::get_mut(&mut self.allocation) {
                allocation.mark_gpu_complete();
            }
            let pixels = data.into_iter().flat_map(u32::to_le_bytes).collect();
            Ok(Some(
                RgbaImage::from_raw(self.source.width(), self.source.height(), pixels)
                    .context("Straight RGBA dimensions differ")?,
            ))
        })();
        match result {
            Ok(None) => Poll::Pending,
            Ok(Some(image)) => {
                self.finished = true;
                Poll::Ready(Ok(image))
            }
            Err(error) => {
                self.finished = true;
                Poll::Ready(Err(gpu_error(error)))
            }
        }
    }
}

struct ResultTexture {
    texture: Arc<dyn GpuTexture>,
    _allocation: Arc<Allocation>,
}
impl std::fmt::Debug for ResultTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.texture.fmt(f)
    }
}
impl GpuTexture for ResultTexture {
    fn service_id(&self) -> u64 {
        self.texture.service_id()
    }
    fn epoch(&self) -> u64 {
        self.texture.epoch()
    }
    fn resource_id(&self) -> u64 {
        self.texture.resource_id()
    }
    fn size(&self) -> [u32; 2] {
        self.texture.size()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self.texture.as_any()
    }
}

#[cfg(test)]
#[path = "gpu_smart_erase_tests.rs"]
mod tests;
