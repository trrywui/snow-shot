//! Owner-thread GPU work and bounded worker-thread encoding for immutable exports.
use std::{
    collections::VecDeque,
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    task::Poll,
};

use gpui::{GpuReadback, GpuRect, GpuService};
use image::RgbaImage;

use crate::{
    CanvasCancellation, CanvasError, CanvasSnapshot, ExportTile, GpuCanvas, GpuCanvasTile,
    SmartEraseSnapshot, engine::*,
};

const MAX_READBACKS: usize = 2;
const MAX_BAND_BYTES: u64 = 8 * 1024 * 1024;
const SEGMENT_WIDTH: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuExportFormat {
    Png,
    Svg,
}

struct Readback {
    operation: Box<dyn GpuReadback>,
    allocation: Arc<crate::gpu_memory::Allocation>,
    destination: [u32; 2],
    size: [u32; 2],
}

#[derive(Clone, Copy)]
struct Segment {
    source: [u32; 4],
    destination: [u32; 2],
}

/// Poll only on the thread owning the supplied GPUI service. Each poll submits
/// at most one bounded render segment, and never waits for GPU or encoder work.
/// Dropping the job cancels its worker and releases pending staging resources.
pub struct GpuExportJob<T> {
    gpu: GpuCanvas,
    snapshot: CanvasSnapshot,
    smart: SmartEraseSnapshot,
    scale: f32,
    size: [u32; 2],
    tiles: Option<Vec<ExportTile>>,
    cancel: CanvasCancellation,
    bands: Option<SyncSender<Vec<u8>>>,
    result: Receiver<Result<T, CanvasError>>,
    band: Option<RgbaImage>,
    next_row: u32,
    segments: VecDeque<Segment>,
    textures: VecDeque<(GpuCanvasTile, [u32; 2])>,
    readbacks: VecDeque<Readback>,
    spare_readbacks: Vec<Readback>,
    finished: bool,
}

impl<T: Send + 'static> GpuExportJob<T> {
    fn start(
        service: Arc<dyn GpuService>,
        snapshot: CanvasSnapshot,
        scale: f32,
        smart: SmartEraseSnapshot,
        cancel: CanvasCancellation,
        packed: Option<([u32; 2], Vec<ExportTile>)>,
        worker: impl FnOnce(Receiver<Vec<u8>>, [u32; 2], CanvasCancellation) -> Result<T, CanvasError>
        + Send
        + 'static,
    ) -> Result<Self, CanvasError> {
        cancel.check()?;
        let source_size = crate::export::dimensions(&snapshot, scale)?;
        let (size, tiles) = match packed {
            Some((size, tiles)) => {
                validate_tiles(source_size, size, &tiles)?;
                (size, Some(tiles))
            }
            None => (source_size, None),
        };
        let smart = smart.for_device(&service);
        validate_smart_erase(&snapshot, &smart)?;
        let gpu = GpuCanvas::new(service).map_err(gpu_error)?;
        let (bands, input) = mpsc::sync_channel(2);
        let (output, result) = mpsc::sync_channel(1);
        let token = cancel.clone();
        std::thread::Builder::new()
            .name("canvas-export-encoder".into())
            .spawn(move || {
                let _ = output.send(worker(input, size, token));
            })
            .map_err(io_error)?;
        Ok(Self {
            gpu,
            snapshot,
            smart,
            scale,
            size,
            tiles,
            cancel,
            bands: Some(bands),
            result,
            band: None,
            next_row: 0,
            segments: VecDeque::new(),
            textures: VecDeque::new(),
            readbacks: VecDeque::new(),
            spare_readbacks: Vec::new(),
            finished: false,
        })
    }

    pub fn poll(&mut self) -> Poll<Result<T, CanvasError>> {
        if self.finished {
            return Poll::Ready(Err(ErrorCode::InvalidState.into()));
        }
        match self.advance() {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(result)) => {
                self.finished = true;
                if result.is_err() {
                    self.cancel.cancel();
                }
                self.bands.take();
                self.readbacks.clear();
                self.spare_readbacks.clear();
                self.textures.clear();
                self.gpu.clear();
                Poll::Ready(result)
            }
            Err(error) => {
                self.finished = true;
                self.cancel.cancel();
                self.bands.take();
                self.readbacks.clear();
                self.spare_readbacks.clear();
                self.textures.clear();
                self.gpu.clear();
                Poll::Ready(Err(error))
            }
        }
    }

    fn advance(&mut self) -> Result<Poll<Result<T, CanvasError>>, CanvasError> {
        self.cancel.check()?;
        match self.result.try_recv() {
            Ok(result) => return Ok(Poll::Ready(result)),
            Err(TryRecvError::Disconnected) => {
                return Err(CanvasError::InvalidFile("Export encoder stopped".into()));
            }
            Err(TryRecvError::Empty) => {}
        }
        // Preserve submission order, including overlapping packed destinations.
        if let Some(pending) = self.readbacks.front_mut()
            && let Some(mut image) = pending.operation.poll().map_err(gpu_error)?
        {
            if [image.width, image.height] != pending.size
                || image.pixels.len() != image.width as usize * image.height as usize * 4
            {
                return Err(CanvasError::Gpu("Invalid GPU readback dimensions".into()));
            }
            unpremultiply(&mut image.pixels);
            let band = self.band.as_mut().expect("readback has a band");
            let stride = band.width() as usize * 4;
            let row_bytes = image.width as usize * 4;
            for row in 0..image.height as usize {
                let destination = (pending.destination[1] as usize + row) * stride
                    + pending.destination[0] as usize * 4;
                band.as_mut()[destination..destination + row_bytes]
                    .copy_from_slice(&image.pixels[row * row_bytes..(row + 1) * row_bytes]);
            }
            let mut completed = self.readbacks.pop_front().expect("completed readback");
            Arc::get_mut(&mut completed.allocation)
                .expect("staging lease is uniquely owned")
                .mark_gpu_complete();
            self.spare_readbacks.push(completed);
        }
        while self.readbacks.len() < MAX_READBACKS {
            let Some((tile, destination)) = self.textures.pop_front() else {
                break;
            };
            let region = GpuRect {
                x: 0,
                y: 0,
                width: tile.region.width,
                height: tile.region.height,
            };
            let size = [region.width, region.height];
            if let Some(index) = self
                .spare_readbacks
                .iter()
                .position(|spare| spare.size == size)
            {
                let mut spare = self.spare_readbacks.swap_remove(index);
                spare.allocation.mark_gpu_use();
                if spare
                    .operation
                    .restart(&tile.texture, region)
                    .map_err(gpu_error)?
                {
                    spare.destination = destination;
                    self.readbacks.push_back(spare);
                    continue;
                }
                Arc::get_mut(&mut spare.allocation)
                    .expect("staging lease is uniquely owned")
                    .mark_gpu_complete();
            }
            // Cached handles keep both their native staging allocation and its
            // budget lease. Evict incompatible completed slots before growing.
            while self.readbacks.len() + self.spare_readbacks.len() >= MAX_READBACKS {
                self.spare_readbacks.pop();
            }
            let allocation = self
                .gpu
                .reserve_readback(region.width as usize * region.height as usize * 4)
                .map_err(gpu_error)?;
            let operation = self
                .gpu
                .service()
                .readback(&tile.texture, region)
                .map_err(gpu_error)?;
            allocation.mark_gpu_use();
            self.readbacks.push_back(Readback {
                operation,
                allocation,
                destination,
                size,
            });
        }
        if !self.readbacks.is_empty() || !self.textures.is_empty() {
            return Ok(Poll::Pending);
        }
        if !self.segments.is_empty() && !self.gpu.is_ready().map_err(gpu_error)? {
            return Ok(Poll::Pending);
        }
        if let Some(segment) = self.segments.pop_front() {
            let frame = region_frame(self.snapshot.viewport, self.scale, segment.source);
            let scene = self.snapshot.scene.with_frame(frame);
            let frame = self
                .gpu
                .prepare(
                    &scene,
                    self.snapshot.background.as_ref(),
                    self.snapshot.resources,
                    1.0,
                    &self.snapshot.presentation,
                    &self.smart,
                    &self.cancel,
                )
                .map_err(gpu_error)?;
            self.textures
                .extend(frame.tiles.iter().cloned().map(|tile| {
                    let destination = [
                        segment.destination[0] + tile.region.x,
                        segment.destination[1] + tile.region.y,
                    ];
                    (tile, destination)
                }));
            return Ok(Poll::Pending);
        }
        if let Some(band) = self.band.take() {
            let height = band.height();
            match self
                .bands
                .as_ref()
                .expect("active encoder")
                .try_send(band.into_raw())
            {
                Ok(()) => self.next_row += height,
                Err(TrySendError::Full(bytes)) => {
                    self.band = RgbaImage::from_raw(self.size[0], height, bytes);
                    return Ok(Poll::Pending);
                }
                Err(TrySendError::Disconnected(_)) => return Ok(Poll::Pending),
            }
        }
        if self.next_row == self.size[1] {
            self.bands.take();
            return Ok(Poll::Pending);
        }
        let rows = (MAX_BAND_BYTES / u64::from(self.size[0]) / 4).clamp(1, 128) as u32;
        let height = rows.min(self.size[1] - self.next_row);
        self.band = Some(RgbaImage::new(self.size[0], height));
        self.segments = band_segments(self.size, self.next_row, height, self.tiles.as_deref());
        Ok(Poll::Pending)
    }
}

impl<T> Drop for GpuExportJob<T> {
    fn drop(&mut self) {
        if !self.finished {
            self.cancel.cancel();
        }
        self.bands.take();
    }
}

impl GpuExportJob<Vec<u8>> {
    pub fn bytes(
        service: Arc<dyn GpuService>,
        snapshot: CanvasSnapshot,
        scale: f32,
        smart: SmartEraseSnapshot,
        format: GpuExportFormat,
        cancel: CanvasCancellation,
    ) -> Result<Self, CanvasError> {
        let vector = vector_snapshot(&snapshot, format);
        let vector_only = vector.is_some();
        let mut job = Self::start(
            service,
            snapshot,
            scale,
            smart,
            cancel,
            None,
            move |bands, size, cancel| {
                if let Some(snapshot) = vector {
                    return vector_svg(&snapshot, scale, &cancel).map(String::into_bytes);
                }
                let mut output = crate::export::BoundedOutput::default();
                encode(bands, size, format, &mut output, &cancel)?;
                Ok(output.bytes)
            },
        )?;
        if vector_only {
            job.bands.take();
            job.next_row = job.size[1];
        }
        Ok(job)
    }

    pub fn packed_png(
        service: Arc<dyn GpuService>,
        snapshot: CanvasSnapshot,
        scale: f32,
        size: [u32; 2],
        tiles: Vec<ExportTile>,
        smart: SmartEraseSnapshot,
        cancel: CanvasCancellation,
    ) -> Result<Self, CanvasError> {
        Self::start(
            service,
            snapshot,
            scale,
            smart,
            cancel,
            Some((size, tiles)),
            move |bands, size, cancel| {
                let mut output = crate::export::BoundedOutput::default();
                encode(bands, size, GpuExportFormat::Png, &mut output, &cancel)?;
                Ok(output.bytes)
            },
        )
    }
}

impl GpuExportJob<()> {
    #[allow(clippy::too_many_arguments)]
    pub fn packed_to_writer(
        service: Arc<dyn GpuService>,
        snapshot: CanvasSnapshot,
        scale: f32,
        size: [u32; 2],
        tiles: Vec<ExportTile>,
        smart: SmartEraseSnapshot,
        mut writer: impl Write + Send + 'static,
        cancel: CanvasCancellation,
    ) -> Result<Self, CanvasError> {
        Self::start(
            service,
            snapshot,
            scale,
            smart,
            cancel,
            Some((size, tiles)),
            move |bands, size, cancel| {
                encode(bands, size, GpuExportFormat::Png, &mut writer, &cancel)
            },
        )
    }

    pub fn to_path(
        service: Arc<dyn GpuService>,
        snapshot: CanvasSnapshot,
        scale: f32,
        smart: SmartEraseSnapshot,
        format: GpuExportFormat,
        path: PathBuf,
        cancel: CanvasCancellation,
    ) -> Result<Self, CanvasError> {
        let vector = vector_snapshot(&snapshot, format);
        let vector_only = vector.is_some();
        let mut job = Self::start(
            service,
            snapshot,
            scale,
            smart,
            cancel,
            None,
            move |bands, size, cancel| {
                crate::write_atomic_with(&path, |writer| {
                    if let Some(snapshot) = vector {
                        let svg = vector_svg(&snapshot, scale, &cancel)?;
                        writer.write_all(svg.as_bytes()).map_err(io_error)
                    } else {
                        encode(bands, size, format, writer, &cancel)
                    }
                })
            },
        )?;
        if vector_only {
            job.bands.take();
            job.next_row = job.size[1];
        }
        Ok(job)
    }

    pub fn to_writer(
        service: Arc<dyn GpuService>,
        snapshot: CanvasSnapshot,
        scale: f32,
        smart: SmartEraseSnapshot,
        format: GpuExportFormat,
        mut writer: impl Write + Send + 'static,
        cancel: CanvasCancellation,
    ) -> Result<Self, CanvasError> {
        let vector = vector_snapshot(&snapshot, format);
        let vector_only = vector.is_some();
        let mut job = Self::start(
            service,
            snapshot,
            scale,
            smart,
            cancel,
            None,
            move |bands, size, cancel| {
                if let Some(snapshot) = vector {
                    let svg = vector_svg(&snapshot, scale, &cancel)?;
                    writer.write_all(svg.as_bytes()).map_err(io_error)
                } else {
                    encode(bands, size, format, &mut writer, &cancel)
                }
            },
        )?;
        if vector_only {
            job.bands.take();
            job.next_row = job.size[1];
        }
        Ok(job)
    }
}

impl GpuExportJob<RgbaImage> {
    pub fn rgba(
        service: Arc<dyn GpuService>,
        snapshot: CanvasSnapshot,
        smart: SmartEraseSnapshot,
        cancel: CanvasCancellation,
    ) -> Result<Self, CanvasError> {
        Self::start(
            service,
            snapshot,
            1.0,
            smart,
            cancel,
            None,
            move |bands, size, cancel| {
                // Full CPU storage is explicit for source serialization/image consumers.
                let bytes = u64::from(size[0]) * u64::from(size[1]) * 4;
                if bytes > 512 * 1024 * 1024 {
                    return Err(CanvasError::ResourceLimit);
                }
                let mut output = Vec::with_capacity(bytes as usize);
                for band in bands {
                    cancel.check()?;
                    output.extend(band);
                }
                cancel.check()?;
                RgbaImage::from_raw(size[0], size[1], output).ok_or(CanvasError::Cancelled)
            },
        )
    }
}

fn region_frame(frame: FrameView, scale: f32, [x, y, width, height]: [u32; 4]) -> FrameView {
    let zoom = frame.camera.zoom * f64::from(scale);
    let origin = view_to_canvas(
        Point::new(
            f64::from(x) / f64::from(scale),
            f64::from(y) / f64::from(scale),
        ),
        &frame.camera,
        frame.surface,
    );
    FrameView {
        camera: Camera {
            center: Point::new(
                origin.x + f64::from(width) / (2.0 * zoom),
                origin.y + f64::from(height) / (2.0 * zoom),
            ),
            zoom,
        },
        surface: SurfaceSize { width, height },
        ..frame
    }
}

fn band_segments(
    size: [u32; 2],
    y: u32,
    height: u32,
    tiles: Option<&[ExportTile]>,
) -> VecDeque<Segment> {
    let mut segments = VecDeque::new();
    let full = [ExportTile {
        source: [0, 0, size[0], size[1]],
        destination: [0, 0],
    }];
    for tile in tiles.unwrap_or(&full) {
        let top = y.max(tile.destination[1]);
        let bottom = (y + height).min(tile.destination[1] + tile.source[3]);
        if top >= bottom {
            continue;
        }
        let mut x = 0;
        while x < tile.source[2] {
            let width = SEGMENT_WIDTH.min(tile.source[2] - x);
            segments.push_back(Segment {
                source: [
                    tile.source[0] + x,
                    tile.source[1] + top - tile.destination[1],
                    width,
                    bottom - top,
                ],
                destination: [tile.destination[0] + x, top - y],
            });
            x += width;
        }
    }
    segments
}

fn validate_tiles(
    source: [u32; 2],
    size: [u32; 2],
    tiles: &[ExportTile],
) -> Result<(), CanvasError> {
    crate::document::validate_raster(size[0], size[1])?;
    if size[0] > 2 * 1024 * 1024 || tiles.len() > 16384 {
        return Err(CanvasError::ResourceLimit);
    }
    for tile in tiles {
        let [x, y, width, height] = tile.source;
        let [dx, dy] = tile.destination;
        if width == 0
            || height == 0
            || x.checked_add(width).is_none_or(|v| v > source[0])
            || y.checked_add(height).is_none_or(|v| v > source[1])
            || dx.checked_add(width).is_none_or(|v| v > size[0])
            || dy.checked_add(height).is_none_or(|v| v > size[1])
        {
            return Err(ErrorCode::InvalidArgument.into());
        }
    }
    Ok(())
}

fn validate_smart_erase(
    snapshot: &CanvasSnapshot,
    smart: &SmartEraseSnapshot,
) -> Result<(), CanvasError> {
    let Some(source) = &snapshot.background else {
        return Ok(());
    };
    let origin = source.origin();
    let right = origin.x + f64::from(source.width()) * source.units_per_pixel();
    let bottom = origin.y + f64::from(source.height()) * source.units_per_pixel();
    for (_, node) in snapshot.scene.nodes() {
        if let SceneDisplayItem::Filter(filter) = node.item.as_ref()
            && filter.filter.filter_type == DisplayFilterType::SmartErase
            && node.bounds[0] < right
            && node.bounds[2] > origin.x
            && node.bounds[1] < bottom
            && node.bounds[3] > origin.y
            && smart.patch(filter, source).is_none()
        {
            return Err(CanvasError::Unsupported(
                "Smart Erase geometry missing from the frozen result snapshot",
            ));
        }
    }
    Ok(())
}

fn vector_snapshot(snapshot: &CanvasSnapshot, format: GpuExportFormat) -> Option<CanvasSnapshot> {
    (format == GpuExportFormat::Svg
        && crate::vector_export::supports(snapshot, &mut crate::vector::VectorContext::default()))
    .then(|| snapshot.clone())
}

fn vector_svg(
    snapshot: &CanvasSnapshot,
    scale: f32,
    cancel: &CanvasCancellation,
) -> Result<String, CanvasError> {
    crate::vector_export::svg(snapshot, scale, cancel)
}

fn encode(
    bands: Receiver<Vec<u8>>,
    [width, height]: [u32; 2],
    format: GpuExportFormat,
    mut writer: impl Write,
    cancel: &CanvasCancellation,
) -> Result<(), CanvasError> {
    cancel.check()?;
    if format == GpuExportFormat::Svg {
        write!(writer, r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><image width="100%" height="100%" href="data:image/png;base64,"#).map_err(io_error)?;
        let mut encoded = base64::write::EncoderWriter::new(
            &mut writer,
            &base64::engine::general_purpose::STANDARD,
        );
        encode_png(bands, [width, height], &mut encoded, cancel)?;
        encoded.finish().map_err(io_error)?;
        drop(encoded);
        writer.write_all(b"\"/></svg>").map_err(io_error)?;
    } else {
        encode_png(bands, [width, height], &mut writer, cancel)?;
    }
    cancel.check()
}

fn encode_png(
    bands: Receiver<Vec<u8>>,
    [width, height]: [u32; 2],
    writer: impl Write,
    cancel: &CanvasCancellation,
) -> Result<(), CanvasError> {
    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png = encoder.write_header().map_err(io_error)?;
    let mut stream = png.stream_writer_with_size(64 * 1024).map_err(io_error)?;
    let mut bytes = 0_u64;
    for band in bands {
        cancel.check()?;
        bytes += band.len() as u64;
        stream.write_all(&band).map_err(io_error)?;
    }
    cancel.check()?;
    if bytes != u64::from(width) * u64::from(height) * 4 {
        return Err(CanvasError::Cancelled);
    }
    stream.finish().map_err(io_error)?;
    png.finish().map_err(io_error)?;
    Ok(())
}

fn unpremultiply(bytes: &mut [u8]) {
    for pixel in bytes.as_chunks_mut::<4>().0.iter_mut() {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = (u32::from(*channel) * 255 + alpha / 2)
                .checked_div(alpha)
                .unwrap_or(0)
                .min(255) as u8;
        }
    }
}
fn gpu_error(error: impl std::fmt::Display) -> CanvasError {
    CanvasError::Gpu(error.to_string())
}
fn io_error(error: impl std::fmt::Display) -> CanvasError {
    CanvasError::InvalidFile(error.to_string())
}

/// Asynchronously freezes both schema-1 source composites on the app's GPU.
/// The completed snapshot can then be encoded entirely on a CPU worker.
pub struct GpuSavePreparation {
    snapshot: Option<crate::DrawingSnapshot>,
    service: Arc<dyn GpuService>,
    cancel: CanvasCancellation,
    image: Option<SaveReadback>,
    visual: Option<crate::SourceImage>,
}

enum SaveReadback {
    Rendered(Box<GpuExportJob<RgbaImage>>),
    Straight(crate::gpu_smart_erase::StraightReadback),
}
impl SaveReadback {
    fn new(
        service: Arc<dyn GpuService>,
        source: crate::SourceImage,
        cancel: CanvasCancellation,
    ) -> Result<Self, CanvasError> {
        if source.gpu_straight.is_some() {
            return crate::gpu_smart_erase::StraightReadback::new(service, source, cancel)
                .map(Self::Straight);
        }
        GpuExportJob::rgba(
            service,
            CanvasSnapshot::source_only(source),
            Default::default(),
            cancel,
        )
        .map(|job| Self::Rendered(Box::new(job)))
    }

    fn poll(&mut self) -> Poll<Result<RgbaImage, CanvasError>> {
        match self {
            Self::Rendered(job) => job.poll(),
            Self::Straight(job) => job.poll(),
        }
    }
}

impl GpuSavePreparation {
    pub fn new(
        service: Arc<dyn GpuService>,
        snapshot: crate::DrawingSnapshot,
        cancel: CanvasCancellation,
    ) -> Result<Self, CanvasError> {
        cancel.check()?;
        let image = if snapshot.requires_materialization() {
            Some(SaveReadback::new(
                service.clone(),
                snapshot.source().expect("GPU source").clone(),
                cancel.clone(),
            )?)
        } else {
            None
        };
        Ok(Self {
            snapshot: Some(snapshot),
            service,
            cancel,
            image,
            visual: None,
        })
    }

    pub fn poll(&mut self) -> Poll<Result<crate::DrawingSnapshot, CanvasError>> {
        let Some(snapshot) = &self.snapshot else {
            return Poll::Ready(Err(ErrorCode::InvalidState.into()));
        };
        if let Err(error) = self.cancel.check() {
            return Poll::Ready(Err(error));
        }
        let Some(job) = &mut self.image else {
            return Poll::Ready(Ok(self.snapshot.take().expect("active save")));
        };
        let image = match job.poll() {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(image)) => match crate::SourceImage::new(image) {
                Ok(source) => source,
                Err(error) => return Poll::Ready(Err(error)),
            },
        };
        self.image = None;
        if self.visual.is_none() {
            self.visual = Some(image);
            let source = snapshot
                .source()
                .expect("layered source")
                .eligible_for_rendering();
            self.image = match SaveReadback::new(self.service.clone(), source, self.cancel.clone())
            {
                Ok(job) => Some(job),
                Err(error) => return Poll::Ready(Err(error)),
            };
            Poll::Pending
        } else {
            let visual = self.visual.take().expect("visual completed");
            Poll::Ready(
                self.snapshot
                    .take()
                    .expect("active save")
                    .materialize_for_save(|_, eligible| {
                        Ok(if eligible {
                            image.clone()
                        } else {
                            visual.clone()
                        })
                    }),
            )
        }
    }
}

#[cfg(test)]
#[path = "gpu_export_tests.rs"]
mod tests;
