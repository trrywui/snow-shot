use super::*;
use gpui_windows::gpu_extensions::OffscreenGpuHost;
use image::Rgba;
use std::time::Duration;

pub(super) fn read_buffer(service: &Arc<dyn GpuService>, buffer: &Buffer) -> Result<Vec<u32>> {
    let native = native(service)?;
    let staging = native.with_context(|gpu| {
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
            let staging = staging.context("test staging")?;
            gpu.immediate_context
                .CopyResource(&staging, &buffer.resource);
            Ok(staging)
        }
    })?;
    let mut done = native.completion()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done.poll()? {
        ensure!(Instant::now() < deadline, "test GPU copy timed out");
        std::thread::yield_now();
    }
    loop {
        if let Some(data) = native
            .with_context(|gpu| read_staging(gpu.immediate_context, &staging, buffer.bytes / 4))?
        {
            return Ok(data);
        }
        ensure!(Instant::now() < deadline, "test GPU map timed out");
        std::thread::yield_now();
    }
}

fn solve(
    runtime: &GpuSmartErase,
    source: &SourceImage,
    mask: &[u8],
) -> Result<(GpuSmartEraseOutput, RgbaImage)> {
    let cancel = CanvasCancellation::default();
    let input = GpuSmartEraseInput::new(source, mask, &cancel)?;
    let mut job = runtime.start(input, cancel)?;
    let raw = job.buffers[job.raw_output[1]].clone();
    let temporary_matches = job.buffers[job.scratch.match_next].clone();
    native(&runtime.service)?.with_context(|gpu| {
        temporary_matches.allocation.mark_gpu_use();
        unsafe {
            gpu.immediate_context
                .ClearUnorderedAccessViewUint(&temporary_matches.output, &[1234; 4]);
        }
        Ok(())
    })?;
    let before = runtime.service.diagnostics().readback_bytes;
    let deadline = Instant::now() + Duration::from_secs(30);
    let output = loop {
        if let Poll::Ready(result) = job.poll() {
            break result?;
        }
        ensure!(Instant::now() < deadline, "GPU reconstruction timed out");
        std::thread::yield_now();
    };
    assert_eq!(
        runtime.service.diagnostics().readback_bytes,
        before,
        "solver read back a reconstruction image"
    );
    let temporary = read_buffer(&runtime.service, &temporary_matches)?;
    for (flags, matched) in job.input.flags.iter().zip(temporary) {
        if flags & 2 == 0 {
            assert_eq!(
                matched,
                u32::MAX,
                "propagation read undefined unmasked matches"
            );
        }
    }
    let data = read_buffer(&runtime.service, &raw)?;
    let pixels: Vec<u8> = data.into_iter().flat_map(u32::to_le_bytes).collect();
    let [w, h] = output.reconstructed.dimensions();
    Ok((
        output,
        RgbaImage::from_raw(w, h, pixels).context("raw result dimensions")?,
    ))
}

#[test]
fn gpu_compute_nearest_matches_cpu_bfs_with_ties_and_odd_extents() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let runtime = GpuSmartErase::new(service.clone())?;
    let source = SourceImage::new(RgbaImage::from_pixel(37, 29, Rgba([70, 120, 180, 255])))?;
    let mut mask = vec![0; 37 * 29];
    mask[14 * 37 + 18] = 255;
    let mut job = runtime.start(
        GpuSmartEraseInput::new(&source, &mask, &Default::default())?,
        Default::default(),
    )?;
    let seeds: Vec<bool> = (0..37 * 29)
        .map(|i| i % 97 == 0 || i == 37 * 29 - 1)
        .collect();
    let flags: Vec<u32> = seeds.iter().copied().map(u32::from).collect();
    let rows = {
        let mut step = Step::new(Kernel::NearestRows, &job.levels[0])
            .inputs(&[(4, job.scratch.donors)])
            .outputs(&[(2, job.scratch.nearest[0])]);
        step.threads = 29;
        step
    };
    let columns = {
        let mut step = Step::new(Kernel::NearestColumns, &job.levels[0])
            .inputs(&[(4, job.scratch.nearest[0])])
            .outputs(&[(2, job.scratch.nearest[1])]);
        step.threads = 37;
        step
    };
    native(&service)?.with_context(|gpu| {
        job.buffers[job.scratch.donors].upload(gpu.immediate_context, bytes(&flags))?;
        job.submit(gpu.immediate_context, &rows)?;
        job.submit(gpu.immediate_context, &columns)?;
        Ok(())
    })?;
    let actual = read_buffer(&service, &job.buffers[job.scratch.nearest[1]])?;
    let expected = crate::smart_erase::nearest(37, 29, &seeds, &Default::default())?;
    assert_eq!(
        actual,
        expected.into_iter().map(|i| i as u32).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn gpu_compute_pyramid_never_promotes_transparency_or_masked_colors() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let runtime = GpuSmartErase::new(service.clone())?;
    let source = SourceImage::new(RgbaImage::from_fn(161, 165, |x, y| {
        if (78..84).contains(&x) {
            Rgba([255, 0, 255, 0])
        } else if (50..115).contains(&x) && (30..135).contains(&y) {
            Rgba([255, 0, 255, 255])
        } else {
            Rgba([80 + (x % 7) as u8, 120, 160, 255])
        }
    }))?;
    let mask: Vec<u8> = (0..161 * 165)
        .map(|i| u8::from((50..115).contains(&(i % 161)) && (30..135).contains(&(i / 161))))
        .collect();
    let mut job = runtime.start(
        GpuSmartEraseInput::new(&source, &mask, &Default::default())?,
        Default::default(),
    )?;
    native(&service)?.with_context(|gpu| {
        while let Some(step) = job.steps.pop_front() {
            job.submit(gpu.immediate_context, &step)?;
        }
        Ok(())
    })?;
    let mut previous = job.input.flags.clone();
    let mut previous_size = [161_u32, 165_u32];
    for (n, level) in job.levels.iter().enumerate().skip(1) {
        let flags = read_buffer(&service, &job.buffers[level.flags])?;
        let colors = read_buffer(&service, &job.buffers[level.rgb])?;
        if n == 1 {
            assert!(
                flags.iter().any(|f| f & 1 != 0),
                "first coarse grid was not constructed"
            );
        }
        for y in 0..level.size[1] {
            for x in 0..level.size[0] {
                let i = (y * level.size[0] + x) as usize;
                let f = flags[i];
                if f & 1 != 0 {
                    for dy in -2..=2 {
                        for dx in -2..=2 {
                            let xx = (x * 2) as i32 + dx;
                            let yy = (y * 2) as i32 + dy;
                            assert!(
                                xx >= 0
                                    && yy >= 0
                                    && xx < previous_size[0] as i32
                                    && yy < previous_size[1] as i32
                            );
                            assert_ne!(
                                previous[yy as usize * previous_size[0] as usize + xx as usize] & 1,
                                0,
                                "coverage gap entered GPU pyramid"
                            );
                        }
                    }
                    if f & 2 == 0 {
                        assert_eq!(f32::from_bits(colors[i * 4 + 1]), 120.0);
                        assert_eq!(f32::from_bits(colors[i * 4 + 2]), 160.0);
                    }
                }
                if f & 2 != 0 {
                    assert_eq!(colors[i * 4], 0);
                    assert_eq!(colors[i * 4 + 1], 0);
                    assert_eq!(colors[i * 4 + 2], 0);
                }
                if f & 4 != 0 {
                    assert_eq!(f & 3, 1, "unsafe coarse donor");
                }
            }
        }
        previous = flags;
        previous_size = level.size;
    }
    Ok(())
}

#[test]
fn gpu_compute_multiscale_preserves_bands_alpha_and_masked_color_independence() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let mut runtime = GpuSmartErase::new(host.service())?;
    runtime.set_profiling(true);
    let clean = RgbaImage::from_fn(144, 128, |x, y| {
        let n = (((x * 17 + y * 31) ^ (x * y)) % 17) as i32 - 8;
        let base = if x < 72 { [55, 80, 110] } else { [205, 65, 90] };
        Rgba([
            (base[0] + n) as u8,
            (base[1] + n) as u8,
            (base[2] + n) as u8,
            if y == 63 {
                0
            } else if y % 7 == 0 {
                128
            } else {
                255
            },
        ])
    });
    let mask: Vec<u8> = (0..144 * 128)
        .map(|i| {
            if (48..96).contains(&(i % 144)) && (36..92).contains(&(i / 144)) {
                255
            } else {
                0
            }
        })
        .collect();
    let mut previous = None;
    for hidden in [[255, 0, 255], [0, 255, 0]] {
        let mut pixels = clean.clone();
        for (p, m) in pixels.pixels_mut().zip(&mask) {
            if *m != 0 && p[3] != 0 {
                p.0[..3].copy_from_slice(&hidden);
            }
        }
        let original = pixels.clone();
        let (result, raw) = solve(&runtime, &SourceImage::new(pixels)?, &mask)?;
        assert!(
            result.profile.levels >= 2,
            "pyramid was not exercised: {:?}",
            result.profile
        );
        assert!(result.profile.submitted_dispatches > 10);
        assert_eq!(result.profile.control_readback_bytes, META_WORDS * 4);
        assert!(
            result.original.gpu_texture().is_some() && result.reconstructed.gpu_texture().is_some()
        );
        let origin = result.reconstructed.origin();
        let mut wrong = 0;
        let mut total = 0;
        for (x, y, pixel) in raw.enumerate_pixels() {
            let xx = x + origin.x as u32;
            let yy = y + origin.y as u32;
            let before = original.get_pixel(xx, yy);
            assert_eq!(pixel[3], before[3]);
            if mask[(yy * 144 + xx) as usize] == 0 || before[3] == 0 {
                assert_eq!(pixel, before);
            } else if xx.abs_diff(72) > 4 {
                wrong += usize::from(clean.get_pixel(xx, yy)[0].abs_diff(pixel[0]) > 40);
                total += 1;
            }
        }
        assert!(
            wrong * 100 < total * 2,
            "wrong texture band {wrong}/{total}"
        );
        if let Some(previous) = previous {
            assert_eq!(raw, previous, "masked colors entered GPU reconstruction");
        }
        previous = Some(raw);
    }
    Ok(())
}

#[test]
fn gpu_compute_preserves_diagonal_edges_and_texture_variation() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let runtime = GpuSmartErase::new(host.service())?;
    for diagonal in [true, false] {
        let source = RgbaImage::from_fn(144, 144, |x, y| {
            let n = (((x * 17 + y * 31) ^ (x * y)) % 65) as i32 - 32;
            let base = if !diagonal {
                [120, 150, 90]
            } else if x + y < 144 {
                [65, 100, 170]
            } else {
                [170, 150, 50]
            };
            Rgba([
                (base[0] + n) as u8,
                (base[1] + n) as u8,
                (base[2] + n) as u8,
                255,
            ])
        });
        let mask: Vec<u8> = (0..144 * 144)
            .map(|i| {
                if (42..102).contains(&(i % 144)) && (40..104).contains(&(i / 144)) {
                    255
                } else {
                    0
                }
            })
            .collect();
        let (_, raw) = solve(&runtime, &SourceImage::new(source.clone())?, &mask)?;
        let mut values = Vec::new();
        let mut wrong = 0;
        let mut count = 0;
        for (x, y, p) in raw.enumerate_pixels() {
            let xx = x + 40;
            let yy = y + 38;
            if mask[(yy * 144 + xx) as usize] == 0 {
                continue;
            }
            values.push(f64::from(p[0]));
            if (xx + yy).abs_diff(144) > 8 {
                wrong += usize::from(source.get_pixel(xx, yy)[0].abs_diff(p[0]) > 65);
                count += 1;
            }
        }
        if diagonal {
            assert!(wrong * 100 < count * 2, "wrong diagonal {wrong}/{count}");
        } else {
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            let variance =
                values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
            assert!(variance > 40.0, "lost texture variation: {variance}");
        }
    }
    Ok(())
}

#[test]
fn gpu_patch_persistence_preserves_exact_straight_rgba_and_readback_cancellation() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let runtime = GpuSmartErase::new(service.clone())?;
    let pixels = RgbaImage::from_fn(96, 80, |x, y| {
        Rgba([
            17 + (x % 29) as u8,
            81 + (y % 31) as u8,
            193,
            [0, 1, 7, 128, 255][x as usize % 5],
        ])
    });
    let mask: Vec<u8> = (0..96 * 80)
        .map(|i| u8::from((32..64).contains(&(i % 96)) && (24..56).contains(&(i / 96))))
        .collect();
    let (result, expected) = solve(&runtime, &SourceImage::new(pixels)?, &mask)?;
    let mut document = crate::CanvasDocument::default();
    document.set_background(Some(result.reconstructed.clone()));
    let mut save = crate::GpuSavePreparation::new(
        service.clone(),
        document.persistence_snapshot()?,
        Default::default(),
    )?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let saved = loop {
        if let Poll::Ready(saved) = save.poll() {
            break saved?;
        }
        ensure!(Instant::now() < deadline, "straight patch save timed out");
        std::thread::yield_now();
    };
    let restored = crate::DrawingSnapshot::decode(&saved.encode()?)?.into_document()?;
    assert_eq!(
        restored
            .background()
            .unwrap()
            .raster_pixels()
            .unwrap()
            .as_ref(),
        &expected
    );
    assert_eq!(
        restored.background().unwrap().origin(),
        result.reconstructed.origin()
    );
    assert_eq!(
        restored.background().unwrap().smart_erase_pixels().as_ref(),
        &expected
    );

    let foreign = OffscreenGpuHost::new()?;
    assert!(
        StraightReadback::new(
            foreign.service(),
            result.reconstructed.clone(),
            Default::default()
        )
        .is_err()
    );
    let cancel = CanvasCancellation::default();
    let baseline = runtime.memory.used();
    let mut readback =
        StraightReadback::new(service.clone(), result.reconstructed, cancel.clone())?;
    assert!(runtime.memory.used() > baseline);
    cancel.cancel();
    assert!(matches!(
        readback.poll(),
        Poll::Ready(Err(CanvasError::Cancelled))
    ));
    drop(readback);
    while service.poll_deferred()? {
        ensure!(
            Instant::now() < deadline,
            "cancelled straight readback did not retire"
        );
        std::thread::yield_now();
    }
    assert_eq!(runtime.memory.used(), baseline);
    Ok(())
}

#[test]
fn gpu_compute_cancel_and_allocation_failure_retire_the_shared_budget() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let _canvas = crate::GpuCanvas::with_memory_budget(service.clone(), 16 * 1024 * 1024)?;
    let runtime = GpuSmartErase::new(service.clone())?;
    let source = SourceImage::new(RgbaImage::from_pixel(96, 80, Rgba([70, 120, 180, 255])))?;
    let mask: Vec<u8> = (0..96 * 80)
        .map(|i| u8::from((32..64).contains(&(i % 96)) && (24..56).contains(&(i / 96))))
        .collect();
    let before = runtime.memory.used();
    let cancel = CanvasCancellation::default();
    let mut job = runtime.start(
        GpuSmartEraseInput::new(&source, &mask, &cancel)?,
        cancel.clone(),
    )?;
    assert!(matches!(job.poll(), Poll::Pending));
    cancel.cancel();
    assert!(matches!(
        job.poll(),
        Poll::Ready(Err(CanvasError::Cancelled))
    ));
    drop(job);
    let deadline = Instant::now() + Duration::from_secs(10);
    while runtime.memory.used() > before {
        service.poll_deferred()?;
        ensure!(
            Instant::now() < deadline,
            "cancelled reconstruction allocations did not retire"
        );
        std::thread::yield_now();
    }
    let large = SourceImage::new(RgbaImage::from_pixel(512, 384, Rgba([70, 120, 180, 255])))?;
    let mask: Vec<u8> = (0..512 * 384)
        .map(|i| u8::from((192..320).contains(&(i % 512)) && (144..240).contains(&(i / 512))))
        .collect();
    let input = GpuSmartEraseInput::new(&large, &mask, &Default::default())?;
    assert!(runtime.start(input, Default::default()).is_err());
    assert_eq!(
        runtime.memory.used(),
        before,
        "failed allocation leaked a partial job"
    );
    Ok(())
}

#[test]
fn gpu_compute_prepared_patch_is_resident_cached_and_strictly_device_qualified() -> Result<()> {
    use crate::{
        CanvasSnapshot, GpuCanvas, GpuExportFormat, GpuExportJob, SmartEraseJob, SmartEraseService,
        SmartEraseSnapshot, engine::*,
    };
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let runtime = GpuSmartErase::new(service.clone())?;
    let source = SourceImage::new(RgbaImage::from_fn(192, 160, |x, y| {
        let n = (((x * 17 + y * 31) ^ (x * y)) % 49) as u8;
        Rgba([90 + n, 120 + n, 60 + n, 255])
    }))?;
    let filter = FilterDisplayItem {
        center_x: 96.0,
        center_y: 80.0,
        width: 48.0,
        height: 48.0,
        opacity: 1.0,
        filter: FilterRenderSpec::resolve(DisplayFilterType::SmartErase, 0.5),
        ..Default::default()
    };
    let prepared = SmartEraseJob::new(
        9,
        Some(source.clone()),
        vec![filter.clone()],
        Default::default(),
    )
    .prepare_gpu(&Default::default())?;
    assert!(prepared.warnings.is_empty());
    assert_eq!(prepared.snapshot.result_count(), 0);
    assert_eq!(prepared.requests.len(), 1);
    let request = prepared.requests.into_iter().next().unwrap();
    let crate::smart_erase_service::GpuPatchInput::Prepared(input) = request.input else {
        anyhow::bail!("raster fixture unexpectedly requested donor assembly");
    };
    let mut job = runtime.start(input, Default::default())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let result = loop {
        if let Poll::Ready(result) = job.poll() {
            break result?;
        }
        ensure!(Instant::now() < deadline, "GPU patch timed out");
        std::thread::yield_now();
    };
    let patch = request.key.complete(result);
    let snapshot = SmartEraseSnapshot {
        entries: VecDeque::from([patch.clone()]),
    };
    let mut cache = SmartEraseService::default();
    cache.restore_frozen(&snapshot);
    let reused = cache
        .solve(&filter, &source, &Default::default())?
        .context("frozen GPU patch")?;
    assert!(Arc::ptr_eq(&patch, &reused));
    assert!(reused.reconstructed().gpu_texture().is_some());
    let scene = crate::gpu_tests::scene(
        crate::gpu_tests::view(192, 160),
        vec![crate::gpu_tests::node(
            0,
            1,
            SceneDisplayItem::Filter(filter),
            [72.0, 56.0, 120.0, 104.0],
        )],
    );
    let mut canvas = GpuCanvas::new(service.clone())?;
    let warm = canvas.prepare(
        &scene,
        Some(&source),
        1,
        1.0,
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )?;
    crate::gpu_tests::read_frame(&service, &warm)?;
    let before = service.diagnostics().readback_bytes;
    let frame = canvas.prepare(
        &scene,
        Some(&source),
        1,
        1.0,
        &Default::default(),
        &snapshot,
        &Default::default(),
    )?;
    assert_eq!(
        frame.profile.source_upload_bytes, 0,
        "GPU patches were downloaded/uploaded through SourceImage"
    );
    assert_eq!(service.diagnostics().readback_bytes, before);
    let expected = crate::gpu_tests::read_frame(&service, &frame)?;
    let mut export = CanvasSnapshot::source_only(source);
    export.scene = scene;
    let other = OffscreenGpuHost::new()?;
    assert!(matches!(
        GpuExportJob::bytes(
            other.service(),
            export.clone(),
            1.0,
            snapshot.clone(),
            GpuExportFormat::Png,
            Default::default()
        ),
        Err(CanvasError::Unsupported(_))
    ));
    let mut export = GpuExportJob::bytes(
        service.clone(),
        export,
        1.0,
        snapshot.clone(),
        GpuExportFormat::Png,
        Default::default(),
    )?;
    let bytes = loop {
        if let Poll::Ready(result) = export.poll() {
            break result?;
        }
        ensure!(Instant::now() < deadline, "GPU patch export timed out");
        std::thread::yield_now();
    };
    assert_eq!(image::load_from_memory(&bytes)?.to_rgba8(), expected);
    drop(host);
    assert_eq!(
        snapshot.for_device(&service).result_count(),
        0,
        "retired GPU patch survived epoch validation"
    );
    Ok(())
}
