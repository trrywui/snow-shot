use super::*;
use image::Rgba;
use snow_draw_engine::FilterRenderSpec;

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
fn automatic_tiny_raster_routing_bounds_donor_work_and_preserves_cpu_bytes() {
    for (w, h, gap, expect_cpu) in [
        (8, 8, 0, true),
        (16, 16, 0, true),
        (32, 32, 0, false),
        (4, 64, 0, false),
        (128, 2, 0, false),
        (16, 16, 1, false),
        (16, 16, 2, true),
    ] {
        let source = SourceImage::new(RgbaImage::from_fn(512, 384, |x, y| {
            if (192..192 + w).contains(&x) && (144..144 + h).contains(&y) {
                return Rgba([240, 5, 9, 255]);
            }
            let n = (((x * 17 + y * 31) ^ (x * y)) % 49) as u8;
            let alpha = if (gap == 1 && x == 185 && y == 140) || (gap == 2 && x == 0 && y == 0) {
                0
            } else {
                255
            };
            Rgba([90 + n, 120 + n, 60 + n, alpha])
        }))
        .unwrap();
        let filter = FilterDisplayItem {
            center_x: 192.0 + w as f64 / 2.0,
            center_y: 144.0 + h as f64 / 2.0,
            width: w as f64,
            height: h as f64,
            opacity: 1.0,
            filter: FilterRenderSpec::resolve(DisplayFilterType::SmartErase, 0.5),
            ..Default::default()
        };
        let job = || {
            SmartEraseJob::new(
                7,
                Some(source.clone()),
                vec![filter.clone()],
                Default::default(),
            )
        };
        let automatic = job().prepare_automatic(&Default::default()).unwrap();
        assert!(automatic.warnings.is_empty(), "{w}x{h}, gap={gap}");
        assert_eq!(automatic.snapshot.result_count(), usize::from(expect_cpu));
        assert_eq!(automatic.requests.len(), usize::from(!expect_cpu));
        let gpu = job().prepare_gpu(&Default::default()).unwrap();
        assert_eq!(
            gpu.requests.len(),
            1,
            "GPU preference lost texture reconstruction"
        );
        if expect_cpu {
            let actual = automatic.snapshot.patch(&filter, &source).unwrap();
            let reference = SmartEraseService::default()
                .solve(&filter, &source, &Default::default())
                .unwrap()
                .unwrap();
            assert_eq!(actual.original.pixels, reference.original.pixels);
            assert_eq!(actual.filled.pixels, reference.filled.pixels);
        }
    }
    // A small native mask with a larger donor region must retain GPU preference.
    let source = SourceImage::new(RgbaImage::from_fn(512, 384, |x, y| {
        let n = (((x * 17 + y * 31) ^ (x * y)) % 49) as u8;
        Rgba([90 + n, 120 + n, 60 + n, 255])
    }))
    .unwrap()
    .with_placement(Point::new(0.0, 0.0), 0.03125)
    .unwrap();
    let filter = FilterDisplayItem {
        center_x: 200.0 * 0.03125,
        center_y: 152.0 * 0.03125,
        width: 0.5,
        height: 0.5,
        opacity: 1.0,
        filter: FilterRenderSpec::resolve(DisplayFilterType::SmartErase, 0.5),
        ..Default::default()
    };
    let automatic = SmartEraseJob::new(1, Some(source), vec![filter], Default::default())
        .prepare_automatic(&Default::default())
        .unwrap();
    assert_eq!(automatic.requests.len(), 1);
    // A small mask in a virtual layered source retains resident GPU assembly.
    let image = SourceImage::new(RgbaImage::from_fn(96, 80, |x, y| {
        let n = (((x * 17 + y * 31) ^ (x * y)) % 49) as u8;
        Rgba([90 + n, 120 + n, 60 + n, 255])
    }))
    .unwrap();
    let source = SourceImage::from_layers(&[crate::ImageSourceLayer::new(image)])
        .unwrap()
        .unwrap();
    let mut filter = super::tests::filter();
    filter.width = 8.0;
    filter.height = 8.0;
    let automatic = SmartEraseJob::new(1, Some(source), vec![filter], Default::default())
        .prepare_automatic(&Default::default())
        .unwrap();
    assert_eq!(automatic.requests.len(), 1);
    assert!(matches!(
        automatic.requests[0].input,
        GpuPatchInput::Source(_)
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn gpu_worker_keeps_layered_fast_models_and_routes_texture_to_resident_assembly() {
    for kind in ["affine", "periodic", "texture"] {
        let pixels = RgbaImage::from_fn(96, 80, |x, y| {
            let mut p = match kind {
                "affine" => Rgba([70, 120, 180, if y % 7 == 0 { 128 } else { 255 }]),
                "periodic" => {
                    let n = ((x / 8 + y / 8) % 2 * 24) as u8;
                    Rgba([140 + n, 160 + n, 180 + n, 255])
                }
                _ => {
                    let n = (((x * 17 + y * 31) ^ (x * y)) % 49) as u8;
                    Rgba([90 + n, 120 + n, 60 + n, 255])
                }
            };
            if (36..60).contains(&x) && (30..50).contains(&y) {
                p.0[..3].copy_from_slice(&[240, 5, 9]);
            }
            p
        });
        let source = SourceImage::from_layers(&[
            crate::ImageSourceLayer::new(SourceImage::new(pixels).unwrap()),
            crate::ImageSourceLayer {
                image: SourceImage::new(RgbaImage::from_pixel(16, 16, Rgba([0, 255, 0, 255])))
                    .unwrap(),
                canvas_rect: Some(crate::CanvasRect::new(0.0, 0.0, 96.0, 80.0).unwrap()),
                coverage: None,
                smart_erase_source: false,
            },
        ])
        .unwrap()
        .unwrap();
        let filter = filter();
        let prepared = SmartEraseJob::new(
            7,
            Some(source.clone()),
            vec![filter.clone()],
            Default::default(),
        )
        .prepare_gpu(&Default::default())
        .unwrap();
        assert!(prepared.warnings.is_empty());
        if kind == "texture" {
            assert_eq!(prepared.snapshot.result_count(), 0);
            assert_eq!(prepared.requests.len(), 1);
            assert!(matches!(
                prepared.requests[0].input,
                GpuPatchInput::Source(_)
            ));
        } else {
            assert!(
                prepared.requests.is_empty(),
                "{kind}: cheap model left the worker"
            );
            let actual = prepared.snapshot.patch(&filter, &source).unwrap();
            let expected = SmartEraseService::default()
                .solve(&filter, &source, &Default::default())
                .unwrap()
                .unwrap();
            assert_eq!(actual.original.origin(), expected.original.origin());
            assert_eq!(
                actual.original.pixels, expected.original.pixels,
                "{kind}: underpaint changed"
            );
            assert_eq!(
                actual.filled.pixels, expected.filled.pixels,
                "{kind}: reconstruction changed"
            );
        }
    }
}

#[cfg(target_os = "windows")]
#[test]
fn gpu_worker_falls_back_when_a_valid_cpu_patch_exceeds_native_texture_width() {
    let pixels = RgbaImage::from_fn(20_000, 16, |x, y| {
        if (1000..19_000).contains(&x) && (4..12).contains(&y) {
            Rgba([240, 5, 9, 255])
        } else {
            Rgba([70, 120, 180, 255])
        }
    });
    let source = SourceImage::from_layers(&[crate::ImageSourceLayer::new(
        SourceImage::new(pixels).unwrap(),
    )])
    .unwrap()
    .unwrap();
    let filter = FilterDisplayItem {
        center_x: 10_000.0,
        center_y: 8.0,
        width: 18_000.0,
        height: 8.0,
        opacity: 1.0,
        filter: FilterRenderSpec::resolve(DisplayFilterType::SmartErase, 0.5),
        ..Default::default()
    };
    let result = SmartEraseJob::new(
        9,
        Some(source.clone()),
        vec![filter.clone()],
        Default::default(),
    )
    .prepare_gpu(&Default::default())
    .unwrap();
    assert!(result.warnings.is_empty());
    assert!(result.requests.is_empty());
    let patch = result.snapshot.patch(&filter, &source).unwrap();
    assert!(patch.filled.width() > 16_384);
    assert!(patch.filled.raster_pixels().is_some());
    assert_eq!(
        patch.filled.pixels.get_pixel(9000, 4).0,
        [70, 120, 180, 255]
    );
    assert_eq!(patch.original.pixels.get_pixel(9000, 4).0, [240, 5, 9, 255]);
}

fn source() -> SourceImage {
    SourceImage::new(RgbaImage::from_fn(96, 80, |x, y| {
        if (36..60).contains(&x) && (30..50).contains(&y) {
            Rgba([240, 5, 9, 255])
        } else {
            Rgba([70, 120, 180, 255])
        }
    }))
    .unwrap()
}

fn filter() -> FilterDisplayItem {
    FilterDisplayItem {
        center_x: 48.0,
        center_y: 40.0,
        width: 24.0,
        height: 20.0,
        opacity: 1.0,
        filter: FilterRenderSpec::resolve(DisplayFilterType::SmartErase, 0.5),
        ..Default::default()
    }
}

#[test]
fn service_retains_only_cropped_source_grid_patches_and_reuses_opacity() {
    let source = source();
    let filter = filter();
    let mut service = SmartEraseService::default();
    let patch = service
        .solve(&filter, &source, &Default::default())
        .unwrap()
        .unwrap();
    assert_eq!(
        patch.original().raster_pixels().unwrap().dimensions(),
        (28, 24)
    );
    assert_eq!(patch.original().origin(), Point::new(34.0, 28.0));
    assert_eq!(
        patch
            .original()
            .raster_pixels()
            .unwrap()
            .get_pixel(14, 12)
            .0,
        [240, 5, 9, 255]
    );
    assert_eq!(
        patch
            .reconstructed()
            .raster_pixels()
            .unwrap()
            .get_pixel(14, 12)
            .0,
        [70, 120, 180, 255]
    );
    assert_eq!(patch.retained_bytes(), 28 * 24 * 8);
    let mut translucent = filter.clone();
    translucent.opacity = 0.2;
    let reused = service
        .solve(&translucent, &source, &Default::default())
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&patch, &reused));
    assert!(Arc::ptr_eq(
        &patch,
        &service.snapshot().patch(&translucent, &source).unwrap()
    ));
    assert_eq!(service.snapshot().result_count(), 1);
    translucent.filter.render_phase = 1;
    assert!(service.snapshot().patch(&translucent, &source).is_none());
    assert!(
        service
            .solve(&translucent, &source, &Default::default())
            .is_err()
    );
}

#[test]
fn result_identity_rejects_replaced_source_placement_and_mask() {
    let source = source();
    let filter = filter();
    let mut service = SmartEraseService::default();
    service
        .solve(&filter, &source, &Default::default())
        .unwrap();
    let snapshot = service.snapshot();
    let replacement = SourceImage::new((**source.raster_pixels().unwrap()).clone()).unwrap();
    assert!(snapshot.patch(&filter, &replacement).is_none());
    let moved = source
        .clone()
        .with_placement(Point::new(1.0, 0.0), 1.0)
        .unwrap();
    assert!(snapshot.patch(&filter, &moved).is_none());
    let mut moved_mask = filter.clone();
    moved_mask.center_x += 1.0;
    assert!(snapshot.patch(&moved_mask, &source).is_none());
    let mut frozen = SmartEraseService::default();
    frozen.restore_frozen(&snapshot);
    assert!(frozen.solve(&filter, &source, &Default::default()).is_ok());
    assert!(
        frozen
            .solve(&moved_mask, &source, &Default::default())
            .is_err()
    );
    frozen.unfreeze();
    assert!(
        frozen
            .solve(&moved_mask, &source, &Default::default())
            .is_ok()
    );
    assert_eq!(frozen.snapshot().result_count(), 2);
}

#[test]
fn eligible_pixels_supply_both_reconstruction_and_underpaint() {
    let eligible = source();
    let mut visual =
        SourceImage::new(RgbaImage::from_pixel(96, 80, Rgba([0, 255, 0, 255]))).unwrap();
    visual.smart_erase_pixels = Some(eligible.raster_pixels().unwrap().clone());
    let patch = SmartEraseService::default()
        .solve(&filter(), &visual, &Default::default())
        .unwrap()
        .unwrap();
    assert_eq!(
        patch
            .original()
            .raster_pixels()
            .unwrap()
            .get_pixel(14, 12)
            .0,
        [240, 5, 9, 255]
    );
    assert_eq!(
        patch
            .reconstructed()
            .raster_pixels()
            .unwrap()
            .get_pixel(14, 12)
            .0,
        [70, 120, 180, 255]
    );
    assert_eq!(
        visual.raster_pixels().unwrap().get_pixel(48, 40).0,
        [0, 255, 0, 255]
    );
}

#[test]
fn background_job_reports_revision_and_failures_without_rendering_other_items() {
    let source = source();
    let mut unfinished = filter();
    unfinished.filter.render_phase = 1;
    let mut unrelated = filter();
    unrelated.filter = FilterRenderSpec::resolve(DisplayFilterType::GaussianBlur, 0.5);
    let mut no_donors = filter();
    no_donors.width = 400.0;
    no_donors.height = 400.0;
    let result = SmartEraseJob::new(
        72,
        Some(source.clone()),
        vec![unfinished, unrelated, filter(), no_donors],
        Default::default(),
    )
    .run(&Default::default())
    .unwrap();
    assert_eq!(result.revision, 72);
    assert_eq!(result.snapshot.result_count(), 1);
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].contains("without unmasked source donors"));
    assert!(result.snapshot.patch(&filter(), &source).is_some());
}

#[test]
fn cancellation_never_publishes_partial_cache_or_job() {
    let source = source();
    let mut service = SmartEraseService::default();
    let cancel = CanvasCancellation::after_checks(4);
    assert!(matches!(
        service.solve(&filter(), &source, &cancel),
        Err(CanvasError::Cancelled)
    ));
    assert_eq!(service.snapshot().result_count(), 0);
    let job = SmartEraseJob::new(9, Some(source), vec![filter()], Default::default());
    assert!(matches!(job.run(&cancel), Err(CanvasError::Cancelled)));
}

#[test]
fn local_masks_are_precise_at_large_coordinates_and_support_brushes() {
    let origin = Point::new(999_999_000.0, -999_999_000.0);
    let units = 1.25;
    let mut translated = filter();
    translated.center_x = origin.x + 48.0 * units;
    translated.center_y = origin.y + 40.0 * units;
    translated.width *= units;
    translated.height *= units;
    let mask = reconstruction_mask(&translated, origin, units, 96, 80).unwrap();
    let baseline = reconstruction_mask(&filter(), Point::new(0.0, 0.0), 1.0, 96, 80).unwrap();
    assert_eq!(mask, baseline);
    let mut brush = filter();
    brush.is_pen_filter = true;
    brush.stroke_width = 10.0;
    brush.points = vec![[20.0, 20.0]].into();
    let circle = reconstruction_mask(&brush, Point::new(0.0, 0.0), 1.0, 64, 64).unwrap();
    assert_eq!(circle[20 * 64 + 20], 255);
    assert_eq!(circle[20 * 64 + 30], 0);
    brush.points = vec![[20.0, 20.0], [40.0, 20.0]].into();
    let stroke = reconstruction_mask(&brush, Point::new(0.0, 0.0), 1.0, 64, 64).unwrap();
    assert_eq!(stroke[20 * 64 + 30], 255);
    assert_eq!(stroke[30 * 64 + 30], 0);
}

#[test]
fn service_rejects_oversized_roi_and_invalid_masks_before_reconstruction() {
    let source = SourceImage::new(RgbaImage::from_pixel(1500, 1500, Rgba([5, 6, 7, 255]))).unwrap();
    let mut mask = filter();
    mask.center_x = 750.0;
    mask.center_y = 750.0;
    mask.width = 1400.0;
    mask.height = 1400.0;
    let mut service = SmartEraseService::default();
    assert!(matches!(
        service.solve(&mask, &source, &Default::default()),
        Err(CanvasError::ResourceLimit)
    ));
    assert_eq!(service.snapshot().retained_bytes(), 0);
    mask.width = f64::NAN;
    assert!(service.solve(&mask, &source, &Default::default()).is_err());
    mask = filter();
    mask.is_pen_filter = true;
    assert!(service.solve(&mask, &source, &Default::default()).is_err());
    mask = filter();
    mask.center_x = -10_000.0;
    assert!(
        service
            .solve(&mask, &source, &Default::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn profiling_preserves_pixels_identity_and_reports_cache_hits_without_solving() {
    let source = source();
    let filter = filter();
    let expected = SmartEraseService::default()
        .solve(&filter, &source, &Default::default())
        .unwrap()
        .unwrap();
    let mut service = SmartEraseService::default();
    let mut profile = SmartEraseProfile::default();
    let actual = service
        .solve_profiled(&filter, &source, &Default::default(), &mut profile)
        .unwrap()
        .unwrap();
    assert_eq!(actual.filled.pixels, expected.filled.pixels);
    assert_eq!(actual.original.pixels, expected.original.pixels);
    assert_eq!(profile.solver, Some(crate::SmartEraseSolver::Affine));
    assert!(!profile.cache_hit);
    assert_eq!(profile.source_region, [0, 0, 96, 80]);
    assert_eq!(profile.masked_pixels, 24 * 20);
    assert_eq!(profile.donor_pixels, 96 * 80 - 24 * 20);
    assert_eq!(profile.estimated_scratch_bytes, 96 * 80 * 256);
    assert!(profile.levels.is_empty());

    let cached = service
        .solve_profiled(&filter, &source, &Default::default(), &mut profile)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&actual, &cached));
    assert!(profile.cache_hit);
    assert_eq!(profile.solver, None);
    assert_eq!(profile.masked_pixels, 0);
    assert_eq!(profile.estimated_scratch_bytes, 0);
    assert_eq!(profile.stages.affine_ms, 0.0);
    assert!(profile.levels.is_empty());

    let cancel = CanvasCancellation::default();
    cancel.cancel();
    assert!(matches!(
        service.solve_profiled(&filter, &source, &cancel, &mut profile),
        Err(CanvasError::Cancelled)
    ));
    assert!(!profile.cache_hit);
    assert_eq!(profile.solver, None);
    assert_eq!(service.snapshot().result_count(), 1);
}

#[test]
fn adaptive_regions_bound_small_solves_and_allow_large_flat_masks() {
    let source =
        SourceImage::new(RgbaImage::from_pixel(2048, 2048, Rgba([70, 120, 180, 255]))).unwrap();
    for side in [32.0, 512.0] {
        let mut filter = filter();
        filter.center_x = 1024.0;
        filter.center_y = 1024.0;
        filter.width = side;
        filter.height = side;
        let mut profile = SmartEraseProfile::default();
        let patch = SmartEraseService::default()
            .solve_profiled(&filter, &source, &Default::default(), &mut profile)
            .unwrap()
            .unwrap();
        assert_eq!(profile.solver, Some(crate::SmartEraseSolver::Affine));
        assert_eq!(profile.attempts, 1);
        assert!(profile.working_pixels < 800_000);
        if side == 32.0 {
            assert!(profile.working_pixels < 40_000);
        }
        assert_eq!(patch.original.pixels, patch.filled.pixels);
        assert_eq!(patch.filled.width(), side as u32 + 4);
    }
}

#[test]
fn adaptive_regions_expand_for_remote_donors_and_cancel_without_publishing() {
    let source = SourceImage::new(RgbaImage::from_fn(512, 384, |x, y| {
        if (244..268).contains(&x) && (182..202).contains(&y) {
            Rgba([240, 5, 9, 255])
        } else if x >= 380 {
            Rgba([70, 120, 180, 255])
        } else {
            Rgba([255, 0, 255, 0])
        }
    }))
    .unwrap();
    let mut filter = filter();
    filter.center_x = 256.0;
    filter.center_y = 192.0;
    let mut profile = SmartEraseProfile::default();
    let patch = SmartEraseService::default()
        .solve_profiled(&filter, &source, &Default::default(), &mut profile)
        .unwrap()
        .unwrap();
    assert!(profile.attempts > 1);
    assert!(profile.donor_halo >= 128);
    assert_eq!(patch.filled.pixels.get_pixel(14, 12).0, [70, 120, 180, 255]);
    assert_eq!(patch.filled.pixels.get_pixel(0, 0).0, [255, 0, 255, 0]);

    let mut service = SmartEraseService::default();
    assert!(matches!(
        service.solve_profiled(
            &filter,
            &source,
            &CanvasCancellation::after_checks(350),
            &mut profile,
        ),
        Err(CanvasError::Cancelled)
    ));
    assert!(profile.attempts > 1);
    assert_eq!(service.snapshot().result_count(), 0);
}

#[test]
fn adaptive_texture_regions_preserve_bands_alpha_and_masked_color_independence() {
    let clean = RgbaImage::from_fn(512, 384, |x, y| {
        let n = (((x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663)) >> 3) % 25) as i32 - 12;
        let base = if x < 256 {
            [55, 80, 110]
        } else {
            [205, 65, 90]
        };
        Rgba([
            (base[0] + n) as u8,
            (base[1] + n) as u8,
            (base[2] + n) as u8,
            if y == 185 { 0 } else { 255 },
        ])
    });
    let mut filter = filter();
    filter.center_x = 256.0;
    filter.center_y = 192.0;
    filter.width = 64.0;
    filter.height = 80.0;
    let mut previous = None;
    for hidden in [[255, 0, 255], [0, 255, 0]] {
        let mut pixels = clean.clone();
        for y in 152..232 {
            for x in 224..288 {
                if pixels.get_pixel(x, y)[3] != 0 {
                    pixels.get_pixel_mut(x, y).0[..3].copy_from_slice(&hidden);
                }
            }
        }
        let source = SourceImage::new(pixels).unwrap();
        let mut profile = SmartEraseProfile::default();
        let patch = SmartEraseService::default()
            .solve_profiled(&filter, &source, &Default::default(), &mut profile)
            .unwrap()
            .unwrap();
        assert_eq!(profile.solver, Some(crate::SmartEraseSolver::Multiscale));
        assert!(profile.working_pixels < 512 * 384);
        let mut wrong = 0;
        let mut count = 0;
        for y in 0..patch.filled.height() {
            for x in 0..patch.filled.width() {
                let original = patch.original.pixels.get_pixel(x, y);
                let filled = patch.filled.pixels.get_pixel(x, y);
                assert_eq!(original[3], filled[3]);
                if !(2..66).contains(&x) || !(2..82).contains(&y) || original[3] == 0 {
                    assert_eq!(original, filled);
                } else if x.abs_diff(34) >= 4 {
                    let expected = clean.get_pixel(x + 222, y + 150);
                    wrong += usize::from(expected[0].abs_diff(filled[0]) > 40);
                    count += 1;
                }
            }
        }
        assert!(wrong * 100 < count * 2, "wrong band {wrong}/{count}");
        if let Some(previous) = previous {
            assert_eq!(patch.filled.pixels, previous);
        }
        previous = Some(patch.filled.pixels.clone());
    }
}

#[test]
fn adaptive_brush_crop_preserves_full_context_search() {
    let pixels = RgbaImage::from_fn(512, 384, |x, y| {
        let n = ((x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663)) % 49) as u8;
        Rgba([90 + n, 120 + n, 60 + n, 255])
    });
    let mut filter = filter();
    filter.is_pen_filter = true;
    filter.stroke_width = 18.0;
    filter.points = (0..32)
        .map(|i| [192.0 + f64::from(i) * 4.0, 176.0 + f64::from(i % 5) * 4.0])
        .collect::<Vec<_>>()
        .into();
    let mask = reconstruction_mask(&filter, Point::new(0.0, 0.0), 1.0, 512, 384).unwrap();
    let expected = crate::smart_erase::reconstruct(&pixels, &mask, &Default::default()).unwrap();
    let source = SourceImage::new(pixels).unwrap();
    let mut profile = SmartEraseProfile::default();
    let patch = SmartEraseService::default()
        .solve_profiled(&filter, &source, &Default::default(), &mut profile)
        .unwrap()
        .unwrap();
    assert!(profile.working_pixels < 512 * 384);
    let expected = image::imageops::crop_imm(
        &expected,
        patch.filled.origin().x as u32,
        patch.filled.origin().y as u32,
        patch.filled.width(),
        patch.filled.height(),
    )
    .to_image();
    assert_eq!(*patch.filled.pixels, expected);
}
