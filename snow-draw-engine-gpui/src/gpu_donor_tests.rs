use super::*;
use gpui_windows::gpu_extensions::OffscreenGpuHost;
use image::{Rgba, RgbaImage};
use std::time::Duration;

fn finish(assembler: &GpuDonorAssembler, input: GpuDonorInput) -> Result<GpuDonorOutput> {
    let mut job = assembler.start(input, Default::default())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let before = assembler.service.diagnostics().readback_bytes;
    loop {
        if let Poll::Ready(result) = job.poll() {
            assert_eq!(assembler.service.diagnostics().readback_bytes, before);
            return result.map_err(Into::into);
        }
        ensure!(Instant::now() < deadline, "GPU donor assembly timed out");
        std::thread::yield_now();
    }
}
fn read(service: Arc<dyn GpuService>, source: SourceImage) -> Result<RgbaImage> {
    let mut readback = StraightReadback::new(service, source, Default::default())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Poll::Ready(result) = readback.poll() {
            return result.map_err(Into::into);
        }
        ensure!(Instant::now() < deadline, "GPU donor readback timed out");
        std::thread::yield_now();
    }
}
fn pattern(w: u32, h: u32, phase: u32) -> Result<SourceImage> {
    Ok(SourceImage::new(RgbaImage::from_fn(w, h, |x, y| {
        Rgba([
            (x.wrapping_mul(71) + y * 13 + phase) as u8,
            (x * 31 + y * 17 + phase * 2) as u8,
            (x * 7 + y * 97 + phase * 3) as u8,
            [0, 1, 7, 128, 254, 255][((x + y * 3 + phase) % 6) as usize],
        ])
    }))?)
}

#[test]
fn gpu_donor_pages_preserve_leaf_bytes_and_reuse_overlapping_regions() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let assembler = GpuDonorAssembler::new(service.clone())?;
    let source = pattern(769, 577, 5)?;
    for (index, region) in [
        [240, 120, 272, 320],
        [240, 120, 272, 320],
        [256, 128, 272, 320],
    ]
    .into_iter()
    .enumerate()
    {
        let expected = source.eligible_region(region, &Default::default())?;
        let output = finish(
            &assembler,
            GpuDonorInput::new(&source, region, &Default::default())?,
        )?;
        assert_eq!(
            output.source.origin(),
            crate::engine::Point::new(region[0] as f64, region[1] as f64)
        );
        if index == 0 {
            assert!(output.profile.page_upload_bytes > 0);
        } else if index == 1 {
            assert_eq!(output.profile.page_upload_bytes, 0);
            assert!(output.profile.page_hits > 0);
        } else {
            assert!(output.profile.page_hits > 0);
        }
        assert_eq!(read(service.clone(), output.source)?, expected);
    }
    Ok(())
}

#[test]
fn gpu_donor_sampling_matches_fractional_layers_coverage_and_large_coordinates() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let assembler = GpuDonorAssembler::new(service.clone())?;
    let background = pattern(321, 257, 0)?;
    let overlay = pattern(129, 97, 9)?;
    let visual = SourceImage::new(RgbaImage::from_pixel(32, 32, Rgba([255, 0, 255, 255])))?;
    let source = SourceImage::from_layers(&[
        crate::ImageSourceLayer::new(background),
        crate::ImageSourceLayer {
            image: overlay,
            canvas_rect: Some(crate::CanvasRect::new(7.25, 11.5, 287.25, 205.75)?),
            coverage: Some(crate::CanvasRect::new(13.75, 19.25, 271.5, 188.25)?),
            smart_erase_source: true,
        },
        crate::ImageSourceLayer {
            image: visual,
            canvas_rect: Some(crate::CanvasRect::new(0.0, 0.0, 321.0, 257.0)?),
            coverage: None,
            smart_erase_source: false,
        },
    ])?
    .context("layered fixture")?
    .with_placement(
        crate::engine::Point::new(100_000_000.25, -200_000_000.5),
        0.75,
    )?;
    let region = [3, 5, 303, 239];
    for selected in 0..2 {
        let layers: Vec<_> = source
            .layers()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, l)| crate::ImageSourceLayer {
                smart_erase_source: i == selected,
                ..l.clone()
            })
            .collect();
        let separate = SourceImage::from_layers(&layers)?.context("separate donor fixture")?;
        let expected = separate.eligible_region(region, &Default::default())?;
        let output = finish(
            &assembler,
            GpuDonorInput::new(&separate, region, &Default::default())?,
        )?;
        let actual = read(service.clone(), output.source)?;
        let different: Vec<_> = actual
            .enumerate_pixels()
            .filter_map(|(x, y, p)| {
                let q = expected.get_pixel(x, y);
                (p != q).then_some((x, y, p.0, q.0))
            })
            .collect();
        assert!(
            different.is_empty(),
            "separate layer {selected}: {} differences, first {:?}",
            different.len(),
            different.first()
        );
    }
    let expected = source.eligible_region(region, &Default::default())?;
    let output = finish(
        &assembler,
        GpuDonorInput::new(&source, region, &Default::default())?,
    )?;
    assert_eq!(
        output.profile.layers, 2,
        "visual-only source entered donors"
    );
    let actual = read(service, output.source)?;
    let different: Vec<_> = actual
        .enumerate_pixels()
        .filter_map(|(x, y, p)| {
            let q = expected.get_pixel(x, y);
            (p != q).then_some((x, y, p.0, q.0))
        })
        .collect();
    assert!(
        different.is_empty(),
        "{} differing donor pixels, first {:?}",
        different.len(),
        different.first()
    );
    Ok(())
}

#[test]
fn gpu_donor_blending_matches_nonzero_background_byte_truncation() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let assembler = GpuDonorAssembler::new(service.clone())?;
    let layer = |phase: u32| {
        SourceImage::new(RgbaImage::from_fn(256, 256, |x, y| {
            Rgba([
                (x * 71 + y * 13 + phase) as u8,
                (x * 31 + y * 17 + phase * 2) as u8,
                (x * 7 + y * 97 + phase * 3) as u8,
                if phase == 0 { x as u8 } else { y as u8 },
            ])
        }))
    };
    let background = layer(0)?;
    let single = SourceImage::from_layers(&[crate::ImageSourceLayer::new(background.clone())])?
        .context("single blend fixture")?;
    let expected_single = single.eligible_region([0, 0, 256, 256], &Default::default())?;
    let output = finish(
        &assembler,
        GpuDonorInput::new(&single, [0, 0, 256, 256], &Default::default())?,
    )?;
    assert_eq!(
        read(service.clone(), output.source)?,
        expected_single,
        "single-layer blend differs"
    );
    let source = SourceImage::from_layers(&[
        crate::ImageSourceLayer::new(background),
        crate::ImageSourceLayer::new(layer(9)?),
    ])?
    .context("blend fixture")?;
    let expected = source.eligible_region([0, 0, 256, 256], &Default::default())?;
    let output = finish(
        &assembler,
        GpuDonorInput::new(&source, [0, 0, 256, 256], &Default::default())?,
    )?;
    let actual = read(service, output.source)?;
    let different: Vec<_> = actual
        .enumerate_pixels()
        .filter_map(|(x, y, p)| {
            let q = expected.get_pixel(x, y);
            (p != q).then_some((x, y, p.0, q.0))
        })
        .collect();
    assert!(
        different.is_empty(),
        "{} differing blend pixels, first {:?}",
        different.len(),
        different.first()
    );
    Ok(())
}

#[test]
fn gpu_donor_filtered_page_boundaries_and_nested_clips_match_cpu() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let assembler = GpuDonorAssembler::new(service.clone())?;
    let inner = SourceImage::from_layers(&[crate::ImageSourceLayer {
        image: pattern(513, 257, 11)?,
        canvas_rect: Some(crate::CanvasRect::new(17.25, 9.5, 720.25, 325.5)?),
        coverage: Some(crate::CanvasRect::new(23.75, 15.25, 696.5, 307.25)?),
        smart_erase_source: true,
    }])?
    .context("inner source")?;
    let source = SourceImage::from_layers(&[
        crate::ImageSourceLayer::new(pattern(1024, 400, 3)?),
        crate::ImageSourceLayer {
            image: inner,
            canvas_rect: Some(crate::CanvasRect::new(13.5, 7.25, 760.5, 345.25)?),
            coverage: Some(crate::CanvasRect::new(27.75, 17.5, 723.75, 309.25)?),
            smart_erase_source: true,
        },
    ])?
    .context("nested source")?;
    // These regions straddle the overlay's source-page boundary at texel 256.
    for region in [
        [300, 30, 256, 257],
        [300, 30, 127, 257],
        [427, 30, 129, 257],
    ] {
        let expected = source.eligible_region(region, &Default::default())?;
        let output = finish(
            &assembler,
            GpuDonorInput::new(&source, region, &Default::default())?,
        )?;
        assert_eq!(read(service.clone(), output.source)?, expected);
    }
    Ok(())
}

#[test]
fn gpu_donors_reuse_exact_gpu_sources_and_reject_foreign_devices() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let assembler = GpuDonorAssembler::new(service.clone())?;
    let original = pattern(96, 80, 3)?;
    let output = finish(
        &assembler,
        GpuDonorInput::new(&original, [0, 0, 96, 80], &Default::default())?,
    )?;
    let input = GpuDonorInput::new(&output.source, [8, 9, 40, 36], &Default::default())?;
    let result = finish(&assembler, input.clone())?;
    assert_eq!(result.profile.page_upload_bytes, 0);
    assert_eq!(
        read(service, result.source)?,
        original.eligible_region([8, 9, 40, 36], &Default::default())?
    );
    let layer = |image| crate::ImageSourceLayer {
        image,
        canvas_rect: Some(crate::CanvasRect::new(2.25, 1.75, 91.5, 75.25).unwrap()),
        coverage: Some(crate::CanvasRect::new(5.5, 3.25, 81.5, 69.75).unwrap()),
        smart_erase_source: true,
    };
    let cpu = SourceImage::from_layers(&[
        crate::ImageSourceLayer::new(original.clone()),
        layer(original.clone()),
    ])?
    .unwrap();
    let gpu = SourceImage::from_layers(&[
        crate::ImageSourceLayer::new(original.clone()),
        layer(output.source.clone()),
    ])?
    .unwrap();
    let [w, h] = cpu.dimensions();
    let expected = cpu.eligible_region([0, 0, w, h], &Default::default())?;
    let actual = finish(
        &assembler,
        GpuDonorInput::new(&gpu, [0, 0, w, h], &Default::default())?,
    )?;
    assert_eq!(actual.profile.page_upload_bytes, 0);
    assert_eq!(read(assembler.service.clone(), actual.source)?, expected);
    let external = SourceImage::from_gpu_texture(output.source.gpu_texture().unwrap().clone())?;
    assert!(GpuDonorInput::new(&external, [0, 0, 96, 80], &Default::default()).is_err());
    let visual = SourceImage::from_layers(&[crate::ImageSourceLayer {
        image: external,
        smart_erase_source: false,
        canvas_rect: None,
        coverage: None,
    }])?
    .unwrap();
    let empty = finish(
        &assembler,
        GpuDonorInput::new(&visual, [0, 0, 96, 80], &Default::default())?,
    )?;
    assert_eq!(empty.profile.layers, 0);
    assert_eq!(empty.profile.page_upload_bytes, 0);
    assert!(
        read(assembler.service.clone(), empty.source)?
            .as_raw()
            .iter()
            .all(|v| *v == 0)
    );
    let foreign = OffscreenGpuHost::new()?;
    let foreign = GpuDonorAssembler::new(foreign.service())?;
    let used = foreign.memory.used();
    assert!(foreign.start(input, Default::default()).is_err());
    assert_eq!(foreign.memory.used(), used);
    drop(host);
    assert!(
        assembler
            .start(
                GpuDonorInput::new(&original, [0, 0, 96, 80], &Default::default())?,
                Default::default()
            )
            .is_err(),
        "retired donor device accepted another job"
    );
    Ok(())
}

#[test]
fn gpu_donor_leaf_copies_work_without_optional_filtered_sampling_shaders() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let mut assembler = GpuDonorAssembler::new(service.clone())?;
    // Simulate the optional shader being unavailable; the plain-copy shader has
    // no double-precision instructions or dependence on filtered-layer support.
    Arc::get_mut(&mut assembler.pipeline).unwrap().assemble = None;
    assert!(!assembler.supports_filtered_layers());
    let source = pattern(96, 80, 7)?;
    let input = GpuDonorInput::new(&source, [5, 7, 40, 36], &Default::default())?;
    let actual = finish(&assembler, input)?;
    assert_eq!(
        read(service, actual.source)?,
        source.eligible_region([5, 7, 40, 36], &Default::default())?
    );
    let layered = SourceImage::from_layers(&[crate::ImageSourceLayer::new(source)])?.unwrap();
    let input = GpuDonorInput::new(&layered, [5, 7, 40, 36], &Default::default())?;
    let allocations = assembler.memory.allocations();
    assert!(matches!(
        assembler.start(input, Default::default()),
        Err(CanvasError::Unsupported(_))
    ));
    assert_eq!(assembler.memory.allocations(), allocations);
    Ok(())
}

#[test]
fn gpu_donor_cancel_and_partial_allocation_failure_retire_budget() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let _canvas = crate::GpuCanvas::with_memory_budget(service.clone(), 16 * 1024 * 1024)?;
    let mut assembler = GpuDonorAssembler::new(service.clone())?;
    let baseline = assembler.memory.used();
    let source = pattern(2048, 1024, 2)?;
    assert!(
        assembler
            .start(
                GpuDonorInput::new(&source, [0, 0, 2048, 1024], &Default::default())?,
                Default::default()
            )
            .is_err()
    );
    assert_eq!(
        assembler.memory.used(),
        baseline,
        "partial allocation leaked"
    );
    let cancel = CanvasCancellation::default();
    let mut job = assembler.start(
        GpuDonorInput::new(&source, [5, 7, 120, 110], &cancel)?,
        cancel.clone(),
    )?;
    assert!(matches!(job.poll(), Poll::Pending));
    cancel.cancel();
    assert!(matches!(
        job.poll(),
        Poll::Ready(Err(CanvasError::Cancelled))
    ));
    drop(job);
    assembler.clear_cache();
    let deadline = Instant::now() + Duration::from_secs(10);
    while service.poll_deferred()? {
        ensure!(
            Instant::now() < deadline,
            "GPU donor cancellation did not retire"
        );
        std::thread::yield_now();
    }
    assert_eq!(assembler.memory.used(), baseline);
    let input = GpuDonorInput::new(&source, [5, 7, 120, 110], &Default::default())?;
    let cancel = CanvasCancellation::after_checks(2);
    let mut job = assembler.start(input, cancel)?;
    assert!(
        matches!(job.poll(), Poll::Ready(Err(CanvasError::Cancelled))),
        "cancellation during owner submission became a GPU error"
    );
    drop(job);
    while service.poll_deferred()? {
        ensure!(
            Instant::now() < deadline,
            "mid-submission cancellation did not retire"
        );
        std::thread::yield_now();
    }
    assert_eq!(assembler.memory.used(), baseline);
    Ok(())
}
