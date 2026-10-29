//! Real-process tests: SNOW_IMAGE_WORKER=... cargo test -p snow-image-rs --test service -- --ignored
use snow_image_rs::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

fn service() -> ImageService {
    let worker = std::env::var_os("SNOW_IMAGE_WORKER")
        .expect("set SNOW_IMAGE_WORKER to the built worker binary");
    ImageService::new(ServiceConfig::new(worker)).unwrap()
}
fn fixture(budget: &Budget, alpha: bool) -> Image {
    let descriptor =
        ImageDescriptor::packed(31, 19, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
    let mut image = MutableImage::new(descriptor, Metadata::default(), budget).unwrap();
    for (i, pixel) in image.bytes_mut().chunks_exact_mut(4).enumerate() {
        pixel.copy_from_slice(&[
            (i % 31 * 7) as u8,
            (i / 31 * 11) as u8,
            71,
            if alpha && i % 5 == 0 { 0 } else { 255 },
        ]);
    }
    image.seal()
}

fn process_handle_count() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        // Each enumeration includes its own directory descriptor on both sides.
        Some(std::fs::read_dir("/dev/fd").unwrap().count() as u32)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
        let mut count = 0;
        // SAFETY: the current-process pseudo handle is valid and count is writable.
        assert_ne!(
            unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) },
            0
        );
        Some(count)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires the separately built native worker"]
fn jpeg_shared_pixels_retain_page_charge_without_disk_after_service_close() {
    let service = service();
    let budget = service.budget().clone();
    let source = fixture(&budget, false);
    let artifact = service
        .encode(
            Arc::new(source),
            EncodeOptions::Jpeg(JpegOptions::default()),
        )
        .wait()
        .unwrap();
    let input = service
        .input_bytes(&artifact.read_bytes(1 << 20).unwrap())
        .unwrap();
    drop(artifact);
    let image = service
        .decode(input, DecodeOptions::default())
        .wait()
        .unwrap();
    assert_eq!(
        (image.descriptor().width, image.descriptor().height),
        (31, 19)
    );
    let expected = image.bytes().to_vec();
    let allocated = snow_image_transport::allocation_len(expected.len()).unwrap() as u64;
    assert_eq!(budget.used(ResourceKind::Pixels), allocated);
    assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    let retained = image.clone();
    drop(image);
    let monitor = service.worker_monitor();
    drop(service);
    let deadline = Instant::now() + Duration::from_secs(5);
    while monitor.statistics().live != 0 || monitor.statistics().spawning != 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(retained.bytes(), expected);
    assert_eq!(budget.used(ResourceKind::Pixels), allocated);
    drop(retained);
    assert_eq!(budget.total_used(), 0);
    assert_eq!(budget.used(ResourceKind::TempDisk), 0);
}

#[test]
#[ignore = "requires the separately built native worker"]
fn static_formats_roundtrip_and_release() {
    let service = service();
    let budget = service.budget().clone();
    let source = fixture(&budget, true);
    for options in [
        EncodeOptions::Png(PngOptions::default()),
        EncodeOptions::Bmp(BmpOptions::default()),
        EncodeOptions::WebP(WebPOptions::default()),
        EncodeOptions::Jxl(JxlOptions::default()),
        EncodeOptions::Avif(AvifOptions {
            quality: 100,
            bit_depth: 8,
            speed: 10,
            ..Default::default()
        }),
    ] {
        let artifact = service
            .encode(Arc::new(source.clone()), options.clone())
            .wait()
            .unwrap();
        let bytes = artifact.read_bytes(1 << 20).unwrap();
        let input = service.input_bytes(&bytes).unwrap();
        let info = service
            .inspect(input.clone(), DecodeOptions::default())
            .wait()
            .unwrap();
        assert_eq!((info.width, info.height), (31, 19));
        let image = service
            .decode(input, DecodeOptions::default())
            .wait()
            .unwrap();
        assert_eq!(
            (image.descriptor().width, image.descriptor().height),
            (31, 19)
        );
        if artifact.receipt().fidelity == PixelFidelity::Exact {
            assert_eq!(
                image.bytes(),
                source.bytes(),
                "claimed exact roundtrip for {options:?}"
            );
        }
    }
    drop(source);
    assert_eq!(budget.total_used(), 0);
    assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    assert_eq!(budget.used(ResourceKind::CodecOperations), 0);
}

#[test]
#[ignore = "requires the separately built native worker"]
fn jpeg_preview_transform_and_atomic_artifact() {
    let service = service();
    let image = fixture(service.budget(), false);
    let encoded = service
        .encode(
            Arc::new(image.clone()),
            EncodeOptions::Jpeg(JpegOptions::default()),
        )
        .wait()
        .unwrap();
    let bytes = encoded.read_bytes(1 << 20).unwrap();
    let input = service.input_bytes(&bytes).unwrap();
    let preview = service.preview(input, 8).wait().unwrap();
    assert_eq!(preview.descriptor().width, 8);
    assert!(preview.descriptor().height <= 8);
    let prepared = service
        .prepare_transform(
            Arc::new(image),
            TransformOptions {
                max_dimensions: Some((7, 7)),
                ..Default::default()
            },
        )
        .wait()
        .unwrap();
    assert_eq!(prepared.descriptor().width, 7);
    let output = service
        .encode(prepared.source(), EncodeOptions::default())
        .wait()
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("result.png");
    output.save(&path, WriteMode::CreateNew).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(encoded.save(&path, WriteMode::CreateNew).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    encoded.save(&path, WriteMode::Replace).unwrap();
    assert_eq!(&std::fs::read(&path).unwrap()[..2], &[255, 216]);
}

#[test]
#[ignore = "requires the separately built native worker"]
fn prepared_pixel_cache_is_shared_and_full_cache_keeps_file_backing_usable() {
    let worker = std::env::var_os("SNOW_IMAGE_WORKER").unwrap();
    let mut config = ServiceConfig::new(worker);
    let length = 31 * 19 * 4;
    config.policy.max_prepared_bytes = length;
    let service = ImageService::new(config).unwrap();
    let source = fixture(service.budget(), false);
    let first = service
        .prepare_transform(Arc::new(source.clone()), TransformOptions::default())
        .wait()
        .unwrap();
    assert_eq!(first.access(), RasterAccess::Regions);
    assert_eq!(service.budget().used(ResourceKind::PreparedPixels), length);
    let retained = first.clone();
    drop(first);
    let fallback = service
        .prepare_transform(Arc::new(source.clone()), TransformOptions::default())
        .wait()
        .unwrap();
    assert_eq!(fallback.access(), RasterAccess::RandomRows);
    assert_eq!(service.budget().used(ResourceKind::PreparedPixels), length);
    for prepared in [&retained, &fallback] {
        let mut pixels = vec![0; length as usize];
        prepared.read_rows(0, 19, &mut pixels).unwrap();
        assert_eq!(pixels, source.bytes());
        let artifact = service
            .encode(prepared.source(), EncodeOptions::default())
            .wait()
            .unwrap();
        assert!(artifact.receipt().bytes > 0);
    }
    drop(retained);
    assert_eq!(service.budget().used(ResourceKind::PreparedPixels), 0);
    drop(fallback);
    drop(source);
    assert_eq!(service.budget().total_used(), 0);
    assert_eq!(service.budget().used(ResourceKind::TempDisk), 0);
}

struct GeneratedRows {
    descriptor: ImageDescriptor,
    started: Arc<AtomicBool>,
}
impl RasterSource for GeneratedRows {
    fn descriptor(&self) -> &ImageDescriptor {
        &self.descriptor
    }
    fn access(&self) -> RasterAccess {
        RasterAccess::SequentialRows
    }
    fn read_rows(&self, _first: u32, _rows: u32, dst: &mut [u8]) -> Result<()> {
        self.started.store(true, Ordering::Release);
        std::thread::sleep(Duration::from_millis(2));
        dst.fill(255);
        Ok(())
    }
}

#[test]
#[ignore = "requires the separately built native worker"]
fn cancellation_and_output_failure_release_process_resources() {
    let service = service();
    let started = Arc::new(AtomicBool::new(false));
    let source = GeneratedRows {
        descriptor: ImageDescriptor::packed(1024, 8192, PixelFormat::RGBA8, ColorEncoding::Srgb)
            .unwrap(),
        started: started.clone(),
    };
    let job = service.encode(Arc::new(source), EncodeOptions::default());
    let limit = Instant::now() + Duration::from_secs(10);
    while !started.load(Ordering::Acquire) {
        assert!(Instant::now() < limit, "worker never consumed rows");
        std::thread::sleep(Duration::from_millis(2));
    }
    job.cancel();
    assert!(matches!(job.wait(), Err(Error::Cancelled)));
    assert_eq!(service.budget().total_used(), 0);
    assert_eq!(service.budget().used(ResourceKind::TempDisk), 0);
    let mut config = service.config().clone();
    config.policy.max_artifact_bytes = 16;
    let limited = ImageService::new(config).unwrap();
    let image = fixture(limited.budget(), false);
    assert!(
        limited
            .encode(Arc::new(image), EncodeOptions::default())
            .wait()
            .is_err()
    );
    assert_eq!(limited.budget().total_used(), 0);
    assert_eq!(limited.budget().used(ResourceKind::TempDisk), 0);
}

#[test]
#[ignore = "requires the separately built native worker"]
fn imported_file_is_a_snapshot_and_bad_input_isolated() {
    let service = service();
    let image = fixture(service.budget(), false);
    let artifact = service
        .encode(Arc::new(image), EncodeOptions::default())
        .wait()
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("image.png");
    artifact.save(&path, WriteMode::CreateNew).unwrap();
    let input = service.input_file(&path).unwrap();
    std::fs::write(&path, b"replaced").unwrap();
    assert_eq!(
        service
            .decode(input, DecodeOptions::default())
            .wait()
            .unwrap()
            .descriptor()
            .width,
        31
    );
    let bad = service.input_bytes(b"not an image").unwrap();
    assert!(
        service
            .decode(bad, DecodeOptions::default())
            .wait()
            .is_err()
    );
    let bytes = artifact.read_bytes(1 << 20).unwrap();
    assert!(
        service
            .decode(
                service.input_bytes(&bytes).unwrap(),
                DecodeOptions::default()
            )
            .wait()
            .is_ok()
    );
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires the separately built native worker"]
fn concurrent_input_capabilities_do_not_share_a_consuming_file_cursor() {
    let service = service();
    let source = fixture(service.budget(), true);
    let artifact = service
        .encode(Arc::new(source.clone()), EncodeOptions::default())
        .wait()
        .unwrap();
    let input = service
        .input_bytes(&artifact.read_bytes(1 << 20).unwrap())
        .unwrap();
    drop(artifact);
    for _ in 0..8 {
        let decode = service.decode(input.clone(), DecodeOptions::default());
        let preview = service.preview(input.clone(), 2048);
        assert_eq!(decode.wait().unwrap().bytes(), source.bytes());
        assert_eq!(preview.wait().unwrap().bytes(), source.bytes());
    }
    let inspect = service.inspect(input.clone(), DecodeOptions::default());
    let decode = service.decode(input.clone(), DecodeOptions::default());
    assert_eq!(
        (
            inspect.wait().unwrap().width,
            decode.wait().unwrap().descriptor().height
        ),
        (31, 19)
    );
    drop(input);
    drop(source);
    assert_eq!(service.budget().total_used(), 0);
    assert_eq!(service.budget().used(ResourceKind::TempDisk), 0);
}

#[test]
#[ignore = "requires the separately built native worker"]
fn animated_gif_snapshots_survive_sessions_and_workers() {
    let service = service();
    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut bytes, 2, 1, &[255, 0, 0, 0, 255, 0]).unwrap();
        encoder.set_repeat(gif::Repeat::Infinite).unwrap();
        for index in [0, 1] {
            let frame = gif::Frame {
                width: 2,
                height: 1,
                delay: 3,
                buffer: std::borrow::Cow::Owned(vec![index; 2]),
                ..Default::default()
            };
            encoder.write_frame(&frame).unwrap();
        }
    }
    let input = service.input_bytes(&bytes).unwrap();
    let session = service
        .open_animation(input.clone(), DecodeOptions::default())
        .wait()
        .unwrap();
    let other = service
        .open_animation(input, DecodeOptions::default())
        .wait()
        .unwrap();
    let first = session.next_frame().wait().unwrap().unwrap();
    let second = session.next_frame().wait().unwrap().unwrap();
    assert_eq!(first.image.bytes(), &[255, 0, 0, 255, 255, 0, 0, 255]);
    assert_eq!(second.image.bytes(), &[0, 255, 0, 255, 0, 255, 0, 255]);
    assert_eq!(first.duration, Duration::from_millis(30));
    session.reset().wait().unwrap();
    assert_eq!(
        session.next_frame().wait().unwrap().unwrap().image.bytes(),
        first.image.bytes()
    );
    assert_eq!(
        other.next_frame().wait().unwrap().unwrap().image.bytes(),
        first.image.bytes()
    );
    drop(session);
    drop(other);
    drop(service);
    assert_eq!(first.image.bytes()[0], 255);
}

#[test]
#[ignore = "requires the separately built native worker"]
fn oriented_animation_dimensions_match_its_owned_snapshots() {
    let service = service();
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(1, 0).unwrap();
        let mut writer = encoder.write_header().unwrap();
        // Little-endian TIFF: one SHORT Orientation entry, value 6 (90 degrees).
        let exif = [
            0x49, 0x49, 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ];
        writer
            .write_chunk(png::chunk::ChunkType(*b"eXIf"), &exif)
            .unwrap();
        writer
            .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255])
            .unwrap();
        writer.finish().unwrap();
    }
    let input = service.input_bytes(&encoded).unwrap();
    for apply_orientation in [false, true] {
        let session = service
            .open_animation(
                input.clone(),
                DecodeOptions {
                    apply_orientation,
                    ..Default::default()
                },
            )
            .wait()
            .unwrap();
        let expected = if apply_orientation { (1, 2) } else { (2, 1) };
        assert_eq!((session.info().width, session.info().height), expected);
        let frame = session.next_frame().wait().unwrap().unwrap();
        assert_eq!(
            (
                frame.image.descriptor().width,
                frame.image.descriptor().height
            ),
            expected
        );
        assert_eq!(
            frame.image.descriptor().orientation,
            if apply_orientation {
                Orientation::Normal
            } else {
                Orientation::Rotate90
            }
        );
        assert_eq!(frame.image.bytes(), &[255, 0, 0, 255, 0, 255, 0, 255]);
        assert!(session.next_frame().wait().unwrap().is_none());
        session.reset().wait().unwrap();
        assert_eq!(
            session.next_frame().wait().unwrap().unwrap().image.bytes(),
            frame.image.bytes()
        );
        drop(session);
        assert_eq!(frame.image.bytes()[0], 255);
    }
}

#[test]
#[ignore = "requires the separately built native worker"]
fn repeated_workloads_reap_every_worker_and_release_every_reservation() {
    // Other tests deliberately drop services before their background reapers finish.
    // Isolate the OS handle baseline from those unrelated, asynchronous closures.
    const ISOLATED: &str = "SNOW_IMAGE_HANDLE_BASELINE_CHILD";
    if std::env::var_os(ISOLATED).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "repeated_workloads_reap_every_worker_and_release_every_reservation",
                "--ignored",
                "--test-threads=1",
            ])
            .env(ISOLATED, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated lifecycle check failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let handles_before = process_handle_count();
    let service = service();
    let budget = service.budget().clone();
    let monitor = service.worker_monitor();
    let image = fixture(&budget, false);
    let image_bytes = image.bytes().len() as u64;
    let mut snapshot = None;
    for _ in 0..24 {
        let artifact = service
            .encode(Arc::new(image.clone()), EncodeOptions::default())
            .wait()
            .unwrap();
        let encoded = artifact.read_bytes(1 << 20).unwrap();
        let decoded = service
            .decode(
                service.input_bytes(&encoded).unwrap(),
                DecodeOptions::default(),
            )
            .wait()
            .unwrap();
        assert_eq!(decoded.bytes(), image.bytes());
        snapshot = Some(decoded);
        let input = service.input_bytes(b"invalid").unwrap();
        assert!(
            service
                .decode(input, DecodeOptions::default())
                .wait()
                .is_err()
        );
        let cancelled = service.encode(Arc::new(image.clone()), EncodeOptions::default());
        cancelled.cancel();
        assert!(matches!(cancelled.wait(), Err(Error::Cancelled)));
        drop(encoded);
        drop(artifact);
        assert_eq!(budget.total_used(), image_bytes * 2);
        // The retained decoded snapshot owns its sealed mapped backing file.
        assert_eq!(budget.used(ResourceKind::TempDisk), image_bytes);
    }
    drop(service);
    let deadline = Instant::now() + Duration::from_secs(5);
    while monitor.statistics().live != 0 || monitor.statistics().spawning != 0 {
        assert!(Instant::now() < deadline, "worker cleanup did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    let counts = monitor.statistics();
    assert_eq!(counts.spawned, counts.reaped);
    assert_eq!(snapshot.as_ref().unwrap().bytes(), image.bytes());
    drop(snapshot);
    drop(image);
    assert_eq!(budget.total_used(), 0);
    assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_handle_count() != handles_before {
        assert!(
            Instant::now() < deadline,
            "OS handle count did not return to baseline: before={handles_before:?}, after={:?}",
            process_handle_count()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires the separately built native worker"]
fn explicit_hdr_preparation_preserves_pq_through_avif() {
    let service = service();
    let descriptor = ImageDescriptor::packed(
        3,
        1,
        PixelFormat {
            channels: ChannelOrder::Rgba,
            sample: SampleType::F32,
            alpha: AlphaMode::Straight,
            significant_bits: 32,
        },
        ColorEncoding::ScRgb {
            reference_white_nits: 80.0,
        },
    )
    .unwrap();
    let mut image = MutableImage::new(descriptor, Metadata::default(), service.budget()).unwrap();
    for (pixel, light) in image
        .bytes_mut()
        .chunks_exact_mut(16)
        .zip([0.0_f32, 1.0, 10.0])
    {
        for (channel, value) in pixel.chunks_exact_mut(4).zip([light, light, light, 1.0]) {
            channel.copy_from_slice(&value.to_ne_bytes());
        }
    }
    let prepared = service
        .prepare_transform(
            Arc::new(image.seal()),
            TransformOptions {
                dynamic_range: DynamicRangePolicy::PreserveDynamicRange,
                hdr_output: Some(HdrOutput::Bt2020Pq10),
                ..Default::default()
            },
        )
        .wait()
        .unwrap();
    assert_eq!(prepared.descriptor().color, ColorEncoding::Bt2020Pq);
    assert_eq!(prepared.descriptor().format.significant_bits, 10);
    let mut expected = vec![0; prepared.descriptor().row_bytes().unwrap()];
    prepared.read_rows(0, 1, &mut expected).unwrap();
    let artifact = service
        .encode(
            prepared.source(),
            EncodeOptions::Avif(AvifOptions {
                quality: 100,
                bit_depth: 10,
                speed: 10,
                ..Default::default()
            }),
        )
        .wait()
        .unwrap();
    let bytes = artifact.read_bytes(1 << 20).unwrap();
    let decoded = service
        .decode(
            service.input_bytes(&bytes).unwrap(),
            DecodeOptions::default(),
        )
        .wait()
        .unwrap();
    assert_eq!(decoded.descriptor().color, ColorEncoding::Bt2020Pq);
    assert_eq!(decoded.descriptor().format.significant_bits, 10);
    assert_eq!(decoded.bytes(), expected);
}
