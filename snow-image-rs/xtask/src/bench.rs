use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use snow_image_rs::*;

#[derive(Serialize, Deserialize)]
pub struct Report {
    schema: u32,
    platform: String,
    iterations: usize,
    settings: String,
    records: Vec<Record>,
}
#[derive(Serialize, Deserialize)]
struct Record {
    name: String,
    width: u32,
    height: u32,
    median_ms: f64,
    samples_ms: Vec<f64>,
    output_bytes: u64,
    psnr_db: Option<f64>,
    #[serde(default)]
    preview_reference: Option<PreviewComparison>,
    peak_accounted_bytes: u64,
    peak_reserved_disk_bytes: u64,
    sampled_worker_footprint_bytes: u64,
    post_job_accounted_bytes: u64,
    post_job_disk_bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct PreviewComparison {
    rgba_psnr_db: Option<f64>,
    maximum_channel_error: u8,
    changed_samples: u64,
    samples: u64,
}
fn compare_preview(image: &Image, path: &Path) -> Result<PreviewComparison> {
    let reference = std::fs::read(path)?;
    if reference.len() != image.bytes().len() {
        return Err(Error::InvalidInput(
            "preview reference dimensions differ".into(),
        ));
    }
    let mut squared_error = 0.0;
    let mut maximum_channel_error = 0;
    let mut changed_samples = 0;
    for (&actual, expected) in image.bytes().iter().zip(reference) {
        let error = actual.abs_diff(expected);
        maximum_channel_error = maximum_channel_error.max(error);
        changed_samples += u64::from(error != 0);
        squared_error += f64::from(error).powi(2);
    }
    let samples = image.bytes().len() as u64;
    Ok(PreviewComparison {
        rgba_psnr_db: (squared_error != 0.0)
            .then(|| 10.0 * (255.0 * 255.0 * samples as f64 / squared_error).log10()),
        maximum_channel_error,
        changed_samples,
        samples,
    })
}

fn fixture(service: &ImageService, width: u32, height: u32, alpha: bool) -> Result<Image> {
    let descriptor =
        ImageDescriptor::packed(width, height, PixelFormat::RGBA8, ColorEncoding::Srgb)?;
    let mut image = MutableImage::new(descriptor, Metadata::default(), service.budget())?;
    for (index, pixel) in image.bytes_mut().chunks_exact_mut(4).enumerate() {
        pixel.copy_from_slice(&pattern(index as u32 % width, index as u32 / width, alpha));
    }
    Ok(image.seal())
}
fn pattern(x: u32, y: u32, alpha: bool) -> [u8; 4] {
    // Flat UI panels, sharp one-pixel colored glyph-like strokes, gradients and
    // translucent edges. Entirely generated; no user screenshots enter the corpus.
    let stroke = y % 24 < 13 && x % 11 < 2;
    let color = if stroke {
        [16, 83, 219]
    } else if y % 180 < 36 {
        [31, 41, 55]
    } else {
        [232 + (x % 23) as u8, 230 + (y % 25) as u8, 240]
    };
    [
        color[0],
        color[1],
        color[2],
        if alpha {
            ((x * 3 + y) % 256) as u8
        } else {
            255
        },
    ]
}
struct Rows {
    descriptor: ImageDescriptor,
    tile: Image,
    started: Option<Arc<AtomicBool>>,
}
impl RasterSource for Rows {
    fn descriptor(&self) -> &ImageDescriptor {
        &self.descriptor
    }
    fn access(&self) -> RasterAccess {
        RasterAccess::RandomRows
    }
    fn read_rows(&self, first: u32, count: u32, destination: &mut [u8]) -> Result<()> {
        if first
            .checked_add(count)
            .is_none_or(|end| end > self.descriptor.height)
            || destination.len() != count as usize * self.descriptor.row_bytes()?
        {
            return Err(Error::InvalidInput("generated row range".into()));
        }
        for (index, row) in destination
            .chunks_exact_mut(self.descriptor.row_bytes()?)
            .enumerate()
        {
            self.tile.read_rows(
                (first + index as u32) % self.tile.descriptor().height,
                1,
                row,
            )?;
        }
        if let Some(started) = &self.started {
            started.store(true, Ordering::Release);
        }
        Ok(())
    }
}
fn measured<T>(
    iterations: usize,
    mut operation: impl FnMut() -> Result<T>,
) -> Result<(T, Vec<f64>)> {
    drop(operation()?); // warm startup and immutable source paths
    let mut samples = Vec::with_capacity(iterations);
    let mut last = None;
    for _ in 0..iterations {
        drop(last.take());
        let start = Instant::now();
        let result = operation()?;
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        last = Some(result);
    }
    Ok((last.expect("nonzero iterations"), samples))
}
fn record(
    service: &ImageService,
    name: String,
    dimensions: (u32, u32),
    mut samples: Vec<f64>,
    bytes: u64,
    psnr: Option<f64>,
) -> Record {
    samples.sort_by(f64::total_cmp);
    let middle = samples.len() / 2;
    let median = if samples.len().is_multiple_of(2) {
        (samples[middle - 1] + samples[middle]) / 2.0
    } else {
        samples[middle]
    };
    println!("{name}: {median:.3} ms; {bytes} bytes");
    Record {
        name,
        width: dimensions.0,
        height: dimensions.1,
        median_ms: median,
        samples_ms: samples,
        output_bytes: bytes,
        psnr_db: psnr,
        preview_reference: None,
        peak_accounted_bytes: service.budget().peak_total_used(),
        peak_reserved_disk_bytes: service.budget().peak_used(ResourceKind::TempDisk),
        sampled_worker_footprint_bytes: service.worker_statistics().sampled_peak_footprint_bytes,
        post_job_accounted_bytes: service.budget().total_used(),
        post_job_disk_bytes: service.budget().used(ResourceKind::TempDisk),
    }
}
fn quality(reference: &Image, decoded: &Image) -> Result<Option<f64>> {
    if reference.descriptor().width != decoded.descriptor().width
        || reference.descriptor().height != decoded.descriptor().height
        || reference.bytes().len() != decoded.bytes().len()
    {
        return Err(Error::Codec("benchmark roundtrip layout mismatch".into()));
    }
    let mse = reference
        .bytes()
        .iter()
        .zip(decoded.bytes())
        .map(|(a, b)| {
            let d = *a as f64 - *b as f64;
            d * d
        })
        .sum::<f64>()
        / reference.bytes().len() as f64;
    Ok(if mse == 0.0 {
        None
    } else {
        Some(10.0 * (255.0 * 255.0 / mse).log10())
    })
}

pub fn run(
    worker: &Path,
    output: &Path,
    iterations: usize,
    quick: bool,
    corpus: Option<&Path>,
    preview_reference: Option<&Path>,
) -> Result<()> {
    if cfg!(debug_assertions) {
        return Err(Error::InvalidInput(
            "benchmarks require --profile performance or --release".into(),
        ));
    }
    if iterations == 0 || iterations > 1000 {
        return Err(Error::InvalidInput("iterations must be in 1..=1000".into()));
    }
    let mut report = Report { schema: 1, platform: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS), iterations,
        settings: "v4; generated-ui; tall rows from precomputed 1800-row tile; PNG opaque+alpha adaptive balanced; JPEG95/444/sequential; fit2048 linear premultiplied Lanczos3; service latency includes worker, transport and publication; PSNR includes RGBA; null PSNR means exact or not applicable".into(), records: Vec::new() };
    let dimensions = if quick {
        vec![(1920, 1080)]
    } else {
        vec![(1920, 1080), (3840, 2160), (7680, 4320)]
    };
    for (width, height) in dimensions {
        for (name, options, alpha) in [
            ("png", EncodeOptions::Png(PngOptions::default()), false),
            ("png-alpha", EncodeOptions::Png(PngOptions::default()), true),
            ("jpeg", EncodeOptions::Jpeg(JpegOptions::default()), false),
        ] {
            let service = ImageService::new(ServiceConfig::new(worker))?;
            let image = fixture(&service, width, height, alpha)?;
            let source: Arc<dyn RasterSource> = Arc::new(image.clone());
            let (artifact, samples) = measured(iterations, || {
                service.encode(source.clone(), options.clone()).wait()
            })?;
            let bytes = artifact.receipt().bytes;
            if let Some(corpus) = corpus {
                std::fs::create_dir_all(corpus)?;
                artifact.save(
                    corpus.join(format!(
                        "ui-{width}x{height}-{name}.{}",
                        if name == "jpeg" { "jpg" } else { "png" }
                    )),
                    WriteMode::Replace,
                )?;
            }
            let encoded = artifact.read_bytes(256 << 20)?;
            let input = service.input_bytes(&encoded)?;
            let decoded = service
                .decode(input.clone(), DecodeOptions::default())
                .wait()?;
            let psnr = quality(&image, &decoded)?;
            if name.starts_with("png") && psnr.is_some() {
                return Err(Error::Codec("lossless benchmark mismatch".into()));
            }
            drop(decoded);
            drop(encoded);
            drop(artifact);
            drop(source);
            drop(image);
            if name == "jpeg" {
                // Input is retained for preview. Encode post-job counters are recorded
                // after removing it, so the cleanup columns have a zero baseline.
                let (preview, preview_samples) =
                    measured(iterations, || service.preview(input.clone(), 2048).wait())?;
                let preview_bytes = preview.bytes().len() as u64;
                let comparison = preview_reference
                    .map(|directory| {
                        compare_preview(
                            &preview,
                            &directory.join(format!("legacy-preview-{width}x{height}.rgba")),
                        )
                    })
                    .transpose()?;
                if let Some(corpus) = corpus {
                    std::fs::write(
                        corpus.join(format!("preview-{width}x{height}.rgba")),
                        preview.bytes(),
                    )?;
                }
                drop(preview);
                drop(input);
                let mut preview_record = record(
                    &service,
                    format!("preview-{width}x{height}"),
                    (width, height),
                    preview_samples,
                    preview_bytes,
                    None,
                );
                preview_record.preview_reference = comparison;
                report.records.push(preview_record);
            } else {
                drop(input);
            }
            report.records.push(record(
                &service,
                format!("{name}-{width}x{height}"),
                (width, height),
                samples,
                bytes,
                psnr,
            ));
            save(&report, output)?;
        }
    }
    if !quick {
        let service = ImageService::new(ServiceConfig::new(worker))?;
        // The pattern repeats every 1800 rows. Generate its bounded tile outside
        // timing, matching the legacy source without materializing the tall raster.
        let source: Arc<dyn RasterSource> = Arc::new(Rows {
            descriptor: ImageDescriptor::packed(
                256,
                131_072,
                PixelFormat::RGBA8,
                ColorEncoding::Srgb,
            )?,
            tile: fixture(&service, 256, 1800, false)?,
            started: None,
        });
        let (artifact, samples) = measured(iterations, || {
            service
                .encode(source.clone(), EncodeOptions::Png(PngOptions::default()))
                .wait()
        })?;
        let bytes = artifact.receipt().bytes;
        {
            let encoded = artifact.read_bytes(256 << 20)?;
            let mut decoder = png::Decoder::new(std::io::Cursor::new(encoded.as_ref()))
                .read_info()
                .map_err(|error| Error::Codec(error.to_string().into()))?;
            if decoder.info().width != 256 || decoder.info().height != 131_072 {
                return Err(Error::Codec("tall PNG dimensions differ".into()));
            }
            let mut expected = vec![0; 256 * 4];
            for row in 0..131_072 {
                source.read_rows(row, 1, &mut expected)?;
                if decoder
                    .next_row()
                    .map_err(|error| Error::Codec(error.to_string().into()))?
                    .is_none_or(|row| row.data() != expected)
                {
                    return Err(Error::Codec("tall PNG samples differ".into()));
                }
            }
            decoder
                .finish()
                .map_err(|error| Error::Codec(error.to_string().into()))?;
        }
        drop(artifact);
        drop(source);
        report.records.push(record(
            &service,
            "tall-png-256x131072".into(),
            (256, 131_072),
            samples,
            bytes,
            None,
        ));
        // Start a real streaming export, wait until its producer has supplied
        // rows, then measure cancellation through result delivery and release.
        // No artificial codec or producer delay is included in this workload.
        let started = Arc::new(AtomicBool::new(false));
        let source: Arc<dyn RasterSource> = Arc::new(Rows {
            descriptor: ImageDescriptor::packed(
                256,
                131_072,
                PixelFormat::RGBA8,
                ColorEncoding::Srgb,
            )?,
            tile: fixture(&service, 256, 1800, false)?,
            started: Some(started.clone()),
        });
        let baseline = service.budget().total_used();
        let mut cancellation_samples = Vec::with_capacity(iterations);
        for iteration in 0..=iterations {
            started.store(false, Ordering::Release);
            let job = service.encode(source.clone(), EncodeOptions::default());
            let deadline = Instant::now() + Duration::from_secs(5);
            while !started.load(Ordering::Acquire) {
                if Instant::now() >= deadline {
                    job.cancel();
                    return Err(Error::Worker("cancellation benchmark did not start".into()));
                }
                std::thread::sleep(Duration::from_micros(100));
            }
            let begin = Instant::now();
            job.cancel();
            match job.wait() {
                Err(Error::Cancelled) => (),
                Err(error) => return Err(error),
                Ok(_) => return Err(Error::Worker("export finished before cancellation".into())),
            }
            while service.budget().total_used() != baseline
                || service.budget().used(ResourceKind::TempDisk) != 0
            {
                if begin.elapsed() >= Duration::from_secs(5) {
                    return Err(Error::Worker("cancellation retained job resources".into()));
                }
                std::thread::sleep(Duration::from_micros(100));
            }
            if iteration != 0 {
                cancellation_samples.push(begin.elapsed().as_secs_f64() * 1000.0);
            }
        }
        drop(source);
        report.records.push(record(
            &service,
            "cancel-active-tall-png".into(),
            (256, 131_072),
            cancellation_samples,
            0,
            None,
        ));
        let mut descriptor = ImageDescriptor::packed(
            1920,
            1080,
            PixelFormat {
                channels: ChannelOrder::Rgba,
                sample: SampleType::F32,
                alpha: AlphaMode::Straight,
                significant_bits: 32,
            },
            ColorEncoding::ScRgb {
                reference_white_nits: 80.0,
            },
        )?;
        descriptor.orientation = Orientation::Normal;
        let mut image = MutableImage::new(descriptor, Metadata::default(), service.budget())?;
        for (index, pixel) in image.bytes_mut().chunks_exact_mut(16).enumerate() {
            let light = (index % 1920) as f32 / 240.0;
            for (channel, value) in pixel
                .chunks_exact_mut(4)
                .zip([light, light * 0.5, 0.5, 1.0])
            {
                channel.copy_from_slice(&value.to_ne_bytes());
            }
        }
        let source: Arc<dyn RasterSource> = Arc::new(image.seal());
        let (prepared, samples) = measured(iterations, || {
            service
                .prepare_transform(
                    source.clone(),
                    TransformOptions {
                        dynamic_range: DynamicRangePolicy::ScreenshotSdr,
                        ..Default::default()
                    },
                )
                .wait()
        })?;
        let bytes = prepared.descriptor().required_bytes()? as u64;
        drop(prepared);
        drop(source);
        report.records.push(record(
            &service,
            "hdr-screenshot-sdr-1920x1080".into(),
            (1920, 1080),
            samples,
            bytes,
            None,
        ));
        let mut gif = Vec::new();
        {
            let mut encoder = gif::Encoder::new(&mut gif, 320, 180, &[255, 0, 0, 0, 255, 0])
                .map_err(|e| Error::Codec(e.to_string().into()))?;
            for frame in 0..8 {
                encoder
                    .write_frame(&gif::Frame {
                        width: 320,
                        height: 180,
                        delay: 4,
                        buffer: std::borrow::Cow::Owned(vec![frame % 2; 320 * 180]),
                        ..Default::default()
                    })
                    .map_err(|e| Error::Codec(e.to_string().into()))?;
            }
        }
        if let Some(corpus) = corpus {
            std::fs::create_dir_all(corpus)?;
            std::fs::write(corpus.join("four-animations.gif"), &gif)?;
        }
        let input = service.input_bytes(&gif)?;
        let sessions = (0..4)
            .map(|_| {
                service
                    .open_animation(input.clone(), DecodeOptions::default())
                    .wait()
            })
            .collect::<Result<Vec<_>>>()?;
        // Verify the generated animation outside the timing window as well.
        for session in &sessions {
            for frame in 0..8 {
                let snapshot = session
                    .next_frame()
                    .wait()?
                    .ok_or_else(|| Error::Codec("animation ended early".into()))?;
                let expected = if frame % 2 == 0 {
                    [255, 0, 0, 255]
                } else {
                    [0, 255, 0, 255]
                };
                if snapshot
                    .image
                    .bytes()
                    .chunks_exact(4)
                    .any(|pixel| pixel != expected)
                {
                    return Err(Error::Codec("animation samples differ".into()));
                }
            }
        }
        let (_, samples) = measured(iterations, || {
            for session in &sessions {
                session.reset().wait()?;
            }
            for _ in 0..8 {
                for session in &sessions {
                    drop(session.next_frame().wait()?);
                }
            }
            Ok(())
        })?;
        drop(sessions);
        drop(input);
        let cleanup = Instant::now();
        while service.budget().total_used() != 0
            || service.budget().used(ResourceKind::TempDisk) != 0
        {
            if cleanup.elapsed().as_secs() >= 5 {
                return Err(Error::Worker(
                    "animation cleanup did not return reservations".into(),
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        report.records.push(record(
            &service,
            "four-animations-32-frames".into(),
            (320, 180),
            samples,
            0,
            None,
        ));
    }
    save(&report, output)
}

fn save(report: &Report, output: &Path) -> Result<()> {
    if report
        .records
        .iter()
        .any(|record| record.post_job_accounted_bytes != 0 || record.post_job_disk_bytes != 0)
    {
        return Err(Error::Worker("benchmark retained job resources".into()));
    }
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        output,
        serde_json::to_vec_pretty(report).map_err(|e| Error::InvalidInput(e.to_string().into()))?,
    )?;
    Ok(())
}

pub fn compare(baseline: &Path, candidate: &Path) -> Result<()> {
    let read = |path| -> Result<Report> {
        serde_json::from_slice(&std::fs::read(path)?)
            .map_err(|e| Error::InvalidInput(e.to_string().into()))
    };
    let baseline = read(baseline)?;
    let candidate = read(candidate)?;
    if baseline.schema != candidate.schema
        || baseline.platform != candidate.platform
        || baseline.settings != candidate.settings
    {
        return Err(Error::InvalidInput(
            "benchmark platform/schema/settings must match".into(),
        ));
    }
    let mut failures = Vec::new();
    for old in &baseline.records {
        let new = candidate
            .records
            .iter()
            .find(|record| record.name == old.name)
            .ok_or_else(|| Error::InvalidInput(format!("missing benchmark {}", old.name).into()))?;
        let ratio = new.median_ms / old.median_ms;
        println!("{}: {:.1}%", old.name, ratio * 100.0);
        if ratio > 1.10 {
            failures.push(old.name.clone());
        }
    }
    if !failures.is_empty() {
        return Err(Error::InvalidInput(
            format!("median regression exceeds 10%: {}", failures.join(", ")).into(),
        ));
    }
    Ok(())
}
