use super::*;
use gpui_windows::gpu_extensions::OffscreenGpuHost;
use image::Rgba;
use std::time::Duration;

fn drain<T>(mut poll: impl FnMut() -> Poll<Result<T, CanvasError>>) -> Result<T> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Poll::Ready(result) = poll() {
            return result.map_err(Into::into);
        }
        ensure!(
            Instant::now() < deadline,
            "GPU source preparation timed out"
        );
        std::thread::yield_now();
    }
}
fn resident(service: Arc<dyn GpuService>, pixels: RgbaImage) -> Result<SourceImage> {
    let source = SourceImage::new(pixels)?;
    let size = source.dimensions();
    let assembler = crate::GpuDonorAssembler::new(service)?;
    let input = crate::GpuDonorInput::new(&source, [0, 0, size[0], size[1]], &Default::default())?;
    let mut job = assembler.start(input, Default::default())?;
    Ok(drain(|| job.poll())?.source)
}
fn rgba(service: Arc<dyn GpuService>, source: SourceImage) -> Result<RgbaImage> {
    let mut readback = StraightReadback::new(service, source, Default::default())?;
    drain(|| readback.poll())
}

#[test]
fn gpu_source_domains_match_cpu_with_odd_extents_gaps_and_escalation() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let runtime = GpuSmartErase::new(service.clone())?;
    let (w, h) = (161, 165);
    let mask: Vec<u8> = (0..w * h)
        .map(|i| u8::from((62..99).contains(&(i % w)) && (61..104).contains(&(i / w))))
        .collect();
    for sparse in [false, true] {
        let pixels = RgbaImage::from_fn(w, h, |x, y| {
            Rgba([
                17 + (x % 29) as u8,
                81 + (y % 31) as u8,
                193,
                if sparse {
                    if x < 12 || mask[(y * w + x) as usize] != 0 {
                        255
                    } else {
                        0
                    }
                } else {
                    if y % 19 == 0 || x == 79 { 0 } else { 128 }
                },
            ])
        });
        let expected = crate::smart_erase::gpu_domain(&pixels, &mask, &Default::default())?;
        let source = resident(service.clone(), pixels)?;
        let input = GpuSourceMask::new(&mask, [w, h], &Default::default())?;
        let mut job = runtime.prepare_source(source, input, Default::default())?;
        let result = drain(|| job.poll())?;
        assert_eq!(
            super::super::tests::read_buffer(&service, &result.flags)?,
            expected
        );
        assert_eq!(result.profile.control_readback_bytes, META_WORDS * 4);
        assert_eq!(
            result.profile.mask_upload_bytes,
            w as usize * h as usize * 4
        );
        if sparse {
            assert!(result.domain_reach >= 64);
        }
    }
    Ok(())
}

#[test]
fn gpu_source_periodic_solver_is_exact_and_ignores_masked_colors() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let mut runtime = GpuSmartErase::new(service.clone())?;
    runtime.set_profiling(true);
    let (w, h) = (192, 144);
    let mask: Vec<u8> = (0..w * h)
        .map(|i| u8::from((64..128).contains(&(i % w)) && (48..96).contains(&(i / w))))
        .collect();
    let clean = RgbaImage::from_fn(w, h, |x, y| {
        let n = ((x / 8 + y / 8) % 2 * 24) as u8;
        Rgba([
            140 + n,
            160 + n,
            180 + n,
            if y == 71 {
                0
            } else if y % 7 == 0 {
                128
            } else {
                255
            },
        ])
    });
    let mut previous = None;
    for hidden in [[255, 0, 255], [0, 255, 0]] {
        let mut pixels = clean.clone();
        for (p, m) in pixels.pixels_mut().zip(&mask) {
            if *m != 0 && p[3] != 0 {
                p.0[..3].copy_from_slice(&hidden);
            }
        }
        let source = resident(service.clone(), pixels.clone())?;
        let before = service.diagnostics().readback_bytes;
        let mut preparation = runtime.prepare_source(
            source,
            GpuSourceMask::new(&mask, [w, h], &Default::default())?,
            Default::default(),
        )?;
        let prepared = drain(|| preparation.poll())?;
        assert_eq!(prepared.solver(), crate::SmartEraseSolver::Periodic);
        assert_eq!(prepared.periodic, Some(14));
        let mut job = runtime.start_prepared(prepared, Default::default())?;
        let result = drain(|| job.poll())?;
        assert_eq!(
            result.profile.solver,
            Some(crate::SmartEraseSolver::Periodic)
        );
        assert_eq!(result.profile.source_upload_bytes, 0);
        assert_eq!(
            service.diagnostics().readback_bytes,
            before,
            "resident solve read back an image"
        );
        let crop = [62, 46, 68, 52];
        let expected_original =
            image::imageops::crop_imm(&pixels, crop[0], crop[1], crop[2], crop[3]).to_image();
        assert_eq!(rgba(service.clone(), result.original)?, expected_original);
        let actual = rgba(service.clone(), result.reconstructed)?;
        for (x, y, p) in actual.enumerate_pixels() {
            let xx = x + crop[0];
            let yy = y + crop[1];
            let i = (yy * w + xx) as usize;
            let expected = if mask[i] != 0 && pixels.get_pixel(xx, yy)[3] != 0 {
                clean.get_pixel(xx, yy)
            } else {
                pixels.get_pixel(xx, yy)
            };
            assert_eq!(p, expected, "periodic pixel {xx},{yy}");
        }
        if let Some(previous) = previous {
            assert_eq!(actual, previous);
        }
        previous = Some(actual);
    }
    Ok(())
}

#[test]
fn gpu_source_constant_and_uncovered_masks_preserve_exact_straight_bytes() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let runtime = GpuSmartErase::new(service.clone())?;
    let (w, h) = (96, 80);
    let mask: Vec<u8> = (0..w * h)
        .map(|i| u8::from((32..64).contains(&(i % w)) && (24..56).contains(&(i / w))))
        .collect();
    for uncovered in [false, true] {
        let pixels = RgbaImage::from_fn(w, h, |x, y| {
            let hole = mask[(y * w + x) as usize] != 0;
            if hole {
                Rgba([255, 3, 17, if uncovered { 0 } else { 7 }])
            } else {
                Rgba([20, 60, 150, 128])
            }
        });
        let source = resident(service.clone(), pixels.clone())?;
        let mut prepare = runtime.prepare_source(
            source,
            GpuSourceMask::new(&mask, [w, h], &Default::default())?,
            Default::default(),
        )?;
        let prepared = drain(|| prepare.poll())?;
        assert_eq!(
            prepared.solver(),
            if uncovered {
                crate::SmartEraseSolver::Unchanged
            } else {
                crate::SmartEraseSolver::Affine
            }
        );
        let mut job = runtime.start_prepared(prepared, Default::default())?;
        let result = drain(|| job.poll())?;
        assert_eq!(result.profile.source_upload_bytes, 0);
        assert!(result.profile.submitted_dispatches <= 3);
        let actual = rgba(service.clone(), result.reconstructed)?;
        for (x, y, p) in actual.enumerate_pixels() {
            let xx = x + 30;
            let yy = y + 22;
            let before = pixels.get_pixel(xx, yy);
            if mask[(yy * w + xx) as usize] != 0 && !uncovered {
                assert_eq!(p.0, [20, 60, 150, before[3]]);
            } else {
                assert_eq!(p, before);
            }
        }
    }
    Ok(())
}

#[test]
fn gpu_source_multiscale_consumes_resident_pixels_and_flags_without_upload() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let runtime = GpuSmartErase::new(service.clone())?;
    let (w, h) = (144, 128);
    let mask: Vec<u8> = (0..w * h)
        .map(|i| u8::from((48..96).contains(&(i % w)) && (36..92).contains(&(i / w))))
        .collect();
    let pixels = RgbaImage::from_fn(w, h, |x, y| {
        let n = (((x * 17 + y * 31) ^ (x * y)) % 49) as u8;
        if mask[(y * w + x) as usize] != 0 {
            Rgba([255, 0, 255, if y == 63 { 0 } else { 128 }])
        } else {
            Rgba([90 + n, 120 + n, 60 + n, if y == 63 { 0 } else { 255 }])
        }
    });
    let source = resident(service.clone(), pixels.clone())?;
    let mut prepare = runtime.prepare_source(
        source,
        GpuSourceMask::new(&mask, [w, h], &Default::default())?,
        Default::default(),
    )?;
    let prepared = drain(|| prepare.poll())?;
    assert_eq!(prepared.solver(), crate::SmartEraseSolver::Multiscale);
    let prepared_levels = prepared.profile.prepared_levels;
    let mut solve = runtime.start_prepared(prepared, Default::default())?;
    let result = drain(|| solve.poll())?;
    assert_eq!(result.profile.source_upload_bytes, 0);
    assert!(result.profile.levels >= 2);
    assert_eq!(
        result.profile.control_readback_bytes, 0,
        "resident solve introduced another preparation wait"
    );
    assert_eq!(result.profile.levels, prepared_levels);
    let actual = rgba(service, result.reconstructed)?;
    for (x, y, p) in actual.enumerate_pixels() {
        let xx = x + 46;
        let yy = y + 34;
        let before = pixels.get_pixel(xx, yy);
        assert_eq!(p[3], before[3]);
        if mask[(yy * w + xx) as usize] == 0 || before[3] == 0 {
            assert_eq!(p, before);
        } else {
            assert!(
                (90..=138).contains(&p[0]) && (60..=108).contains(&p[2]),
                "masked color leaked"
            );
        }
    }
    Ok(())
}

#[test]
fn gpu_source_adaptive_regions_expand_before_publishing_a_layered_solve() -> Result<()> {
    use crate::smart_erase_service::{GpuLayeredInput, GpuPatchInput};
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let runtime = GpuSmartErase::new(service.clone())?;
    let assembler = crate::GpuDonorAssembler::new(service.clone())?;
    let filter = crate::engine::FilterDisplayItem {
        center_x: 512.0,
        center_y: 256.0,
        width: 24.0,
        height: 32.0,
        opacity: 1.0,
        filter: crate::engine::FilterRenderSpec::resolve(
            crate::engine::DisplayFilterType::SmartErase,
            0.5,
        ),
        ..Default::default()
    };
    let reference_mask = crate::smart_erase_service::reconstruction_mask(
        &filter,
        crate::engine::Point::new(0.0, 0.0),
        1.0,
        1024,
        512,
    )?;
    let pixels = RgbaImage::from_fn(1024, 512, |x, y| {
        if reference_mask[(y * 1024 + x) as usize] != 0 {
            Rgba([255, 0, 255, 255])
        } else if (180..220).contains(&x) && (220..300).contains(&y) {
            Rgba([70, 120, 180, 255])
        } else {
            Rgba([249, 3, 17, 0])
        }
    });
    let source =
        SourceImage::from_layers(&[crate::ImageSourceLayer::new(SourceImage::new(pixels)?)])?
            .unwrap();
    let prepared = crate::SmartEraseJob::new(
        1,
        Some(source.clone()),
        vec![filter.clone()],
        Default::default(),
    )
    .prepare_gpu(&Default::default())?;
    assert!(prepared.warnings.is_empty());
    assert_eq!(prepared.requests.len(), 1);
    let GpuPatchInput::Source(plan) = prepared.requests.into_iter().next().unwrap().input else {
        anyhow::bail!("layered request used CPU donor assembly")
    };
    let mut plan = *plan;
    let mut attempts = 0;
    loop {
        attempts += 1;
        let GpuLayeredInput {
            donors,
            mask,
            context,
            halo,
        } = plan;
        let mut donors = assembler.start(donors, Default::default())?;
        while donors.has_pending_submissions() {
            assert!(matches!(donors.poll(), Poll::Pending));
        }
        // Queue domain construction without observing the assembly completion.
        let mut job = runtime.prepare_assembled(&mut donors, mask, Default::default())?;
        job.set_donor_context(context)?;
        let prepared = drain(|| job.poll())?;
        if context.needs_margin(
            prepared.hole_bounds.map(|v| v as usize),
            (prepared.domain_reach as usize + 16).min(512),
        ) {
            assert!(prepared.needs_more);
            assert_eq!(
                prepared.profile.prepared_levels, 0,
                "truncated ROI built a pyramid"
            );
            assert_eq!(
                prepared.known_pixels, 0,
                "unexpected donor entered the cropped ROI"
            );
            plan = GpuLayeredInput::new(
                &filter,
                &source,
                Some((halo * 2).min(512)),
                &Default::default(),
            )?
            .unwrap();
            continue;
        }
        assert_eq!(halo, 512);
        assert_eq!(attempts, 4);
        assert_eq!(prepared.solver(), crate::SmartEraseSolver::Affine);
        let mut solve = runtime.start_prepared(
            prepared.with_search_span(context.search_span as u32)?,
            Default::default(),
        )?;
        let result = drain(|| solve.poll())?;
        let origin = result.reconstructed.origin();
        let actual = rgba(service.clone(), result.reconstructed)?;
        let mut expected = source.eligible_region(
            [
                origin.x as u32,
                origin.y as u32,
                actual.width(),
                actual.height(),
            ],
            &Default::default(),
        )?;
        for (x, y, p) in expected.enumerate_pixels_mut() {
            let xx = x + origin.x as u32;
            let yy = y + origin.y as u32;
            if reference_mask[(yy * 1024 + xx) as usize] != 0 {
                *p = Rgba([70, 120, 180, 255]);
            }
        }
        assert_eq!(actual, expected);
        break;
    }
    Ok(())
}

#[test]
fn gpu_source_prepare_cancellation_and_missing_donors_are_explicit() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let _canvas = crate::GpuCanvas::with_memory_budget(service.clone(), 16 * 1024 * 1024)?;
    let runtime = GpuSmartErase::new(service.clone())?;
    let (w, h) = (96, 80);
    let mask: Vec<u8> = (0..w * h)
        .map(|i| u8::from((32..64).contains(&(i % w)) && (24..56).contains(&(i / w))))
        .collect();
    let pixels = RgbaImage::from_fn(w, h, |x, y| {
        Rgba([
            70,
            120,
            180,
            if mask[(y * w + x) as usize] != 0 {
                255
            } else {
                0
            },
        ])
    });
    let source = resident(service.clone(), pixels)?;
    let cancel = CanvasCancellation::default();
    let baseline = runtime.memory.used();
    let mut job = runtime.prepare_source(
        source.clone(),
        GpuSourceMask::new(&mask, [w, h], &cancel)?,
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
    while service.poll_deferred()? {
        ensure!(
            Instant::now() < deadline,
            "cancelled source preparation did not retire"
        );
        std::thread::yield_now();
    }
    assert!(runtime.memory.used() <= baseline);
    let mut job = runtime.prepare_source(
        source,
        GpuSourceMask::new(&mask, [w, h], &Default::default())?,
        Default::default(),
    )?;
    let prepared = drain(|| job.poll())?;
    assert_eq!(prepared.domain_reach, 512);
    assert_eq!(prepared.known_pixels, 0);
    let allocations = runtime.memory.allocations();
    assert!(matches!(
        runtime.start_prepared(prepared, Default::default()),
        Err(CanvasError::Unsupported(_))
    ));
    assert_eq!(
        runtime.memory.allocations(),
        allocations,
        "missing donors allocated solver storage"
    );
    Ok(())
}
