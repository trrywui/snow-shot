use super::*;
use crate::{CanvasRect, ImageSourceLayer};
#[test]
fn decode_preflights_rgba_budget_before_native_decode_or_conversion() {
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, 16, 16);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[90; 16 * 16 * 3])
        .unwrap();
    // Native RGB fits, converted RGBA does not. A header-only input also rejects
    // on dimensions, before the deliberately missing image payload is decoded.
    for bytes in [&png[..], &png[..41]] {
        assert!(matches!(
            SourceImage::decode_with_limit(bytes, 800),
            Err(CanvasError::ResourceLimit)
        ));
    }
    assert_eq!(
        SourceImage::decode_with_limit(&png, 2048)
            .unwrap()
            .raster_pixels()
            .unwrap()
            .dimensions(),
        (16, 16)
    );
}
#[cfg(target_os = "windows")]
#[test]
fn visual_overlays_preserve_the_eligible_composite_and_persistence() {
    let base = SourceImage::new(RgbaImage::from_fn(80, 60, |x, y| {
        image::Rgba([40 + (x % 10) as u8, 90 + (y % 10) as u8, 150, 255])
    }))
    .unwrap();
    let overlay = SourceImage::new(RgbaImage::from_pixel(
        80,
        60,
        image::Rgba([240, 20, 30, 128]),
    ))
    .unwrap();
    let layered = SourceImage::from_layers(&[
        ImageSourceLayer::new(base.clone()),
        ImageSourceLayer {
            image: overlay,
            smart_erase_source: false,
            canvas_rect: None,
            coverage: None,
        },
    ])
    .unwrap()
    .unwrap();
    assert!(layered.raster_pixels().is_none());
    assert_eq!(
        layered
            .eligible_region([0, 0, 80, 60], &Default::default())
            .unwrap(),
        **base.raster_pixels().unwrap()
    );
    let mut document = CanvasDocument::default();
    document.set_background(Some(layered));
    assert!(document.serialize().is_err());
    let host = gpui_windows::gpu_extensions::OffscreenGpuHost::new().unwrap();
    let mut save = crate::GpuSavePreparation::new(
        host.service(),
        document.persistence_snapshot().unwrap(),
        Default::default(),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let saved = loop {
        if let std::task::Poll::Ready(result) = save.poll() {
            break result.unwrap().encode().unwrap();
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    };
    let restored = CanvasDocument::deserialize(&saved).unwrap();
    assert_eq!(
        restored.background().unwrap().smart_erase_pixels(),
        base.raster_pixels().unwrap()
    );
}

#[test]
fn retained_layers_allocate_no_composite_and_donor_roi_keeps_visual_layers_out() {
    let base = SourceImage::new(RgbaImage::from_pixel(
        1024,
        768,
        image::Rgba([20, 80, 140, 255]),
    ))
    .unwrap();
    let overlay = SourceImage::new(RgbaImage::from_pixel(
        1024,
        768,
        image::Rgba([240, 10, 30, 255]),
    ))
    .unwrap();
    let source = SourceImage::from_layers(&[
        ImageSourceLayer::new(base.clone()),
        ImageSourceLayer {
            image: overlay.clone(),
            canvas_rect: None,
            coverage: Some(CanvasRect::new(500.0, 300.0, 100.0, 100.0).unwrap()),
            smart_erase_source: false,
        },
    ])
    .unwrap()
    .unwrap();
    assert_eq!(source.dimensions(), [1024, 768]);
    assert!(source.raster_pixels().is_none());
    let layers = source.layers().unwrap();
    assert!(Arc::ptr_eq(
        layers[0].image.raster_pixels().unwrap(),
        base.raster_pixels().unwrap()
    ));
    assert!(Arc::ptr_eq(
        layers[1].image.raster_pixels().unwrap(),
        overlay.raster_pixels().unwrap()
    ));
    let roi = source
        .eligible_region([490, 290, 120, 120], &Default::default())
        .unwrap();
    assert_eq!(roi.dimensions(), (120, 120));
    assert!(roi.pixels().all(|pixel| pixel.0 == [20, 80, 140, 255]));
}

#[test]
fn smart_erase_layered_worker_uses_only_donor_roi_without_materializing_source() {
    let base = SourceImage::new(RgbaImage::from_fn(96, 80, |x, y| {
        image::Rgba(if (36..60).contains(&x) && (30..50).contains(&y) {
            [240, 5, 9, 255]
        } else {
            [70, 120, 180, 255]
        })
    }))
    .unwrap();
    let overlay =
        SourceImage::new(RgbaImage::from_pixel(96, 80, image::Rgba([0, 255, 0, 255]))).unwrap();
    let source = SourceImage::from_layers(&[
        ImageSourceLayer::new(base),
        ImageSourceLayer {
            image: overlay,
            smart_erase_source: false,
            canvas_rect: None,
            coverage: None,
        },
    ])
    .unwrap()
    .unwrap();
    let filter = crate::engine::FilterDisplayItem {
        center_x: 48.0,
        center_y: 40.0,
        width: 24.0,
        height: 20.0,
        opacity: 1.0,
        filter: crate::engine::FilterRenderSpec::resolve(
            crate::engine::DisplayFilterType::SmartErase,
            0.5,
        ),
        ..Default::default()
    };
    let source_for_worker = source.clone();
    let patch = std::thread::spawn(move || {
        crate::SmartEraseService::default()
            .solve(&filter, &source_for_worker, &Default::default())
            .unwrap()
            .unwrap()
    })
    .join()
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
    assert_eq!(patch.original().dimensions(), [28, 24]);
}

#[test]
fn nested_layer_metadata_maps_coverage_and_eligibility_without_pixel_copies() {
    let leaf = SourceImage::new(RgbaImage::from_pixel(
        10,
        10,
        image::Rgba([90, 120, 150, 255]),
    ))
    .unwrap();
    let inner = SourceImage::from_layers(&[ImageSourceLayer {
        image: leaf.clone(),
        canvas_rect: Some(CanvasRect::new(10.0, 20.0, 20.0, 30.0).unwrap()),
        coverage: None,
        smart_erase_source: true,
    }])
    .unwrap()
    .unwrap();
    let outer = SourceImage::from_layers(&[ImageSourceLayer {
        image: inner.clone(),
        canvas_rect: Some(CanvasRect::new(100.0, 200.0, 40.0, 60.0).unwrap()),
        coverage: Some(CanvasRect::new(110.0, 210.0, 20.0, 30.0).unwrap()),
        smart_erase_source: false,
    }])
    .unwrap()
    .unwrap();
    let layer = &outer.layers().unwrap()[0];
    assert!(layer.image.layers().is_none());
    assert!(Arc::ptr_eq(
        layer.image.raster_pixels().unwrap(),
        leaf.raster_pixels().unwrap()
    ));
    assert!(!layer.smart_erase_source);
    assert_eq!(
        layer.placement(),
        CanvasRect::new(100.0, 200.0, 40.0, 60.0).unwrap()
    );
    assert_eq!(
        layer.coverage,
        Some(CanvasRect::new(110.0, 210.0, 20.0, 30.0).unwrap())
    );
    let mapped = outer.with_placement(Point::new(-20.0, -40.0), 1.0).unwrap();
    assert_eq!(mapped.origin(), Point::new(-20.0, -40.0));
}

#[test]
fn layered_save_requires_preparation_and_freezes_native_grid_composites() {
    let base = SourceImage::new(RgbaImage::from_pixel(
        16,
        12,
        image::Rgba([10, 20, 30, 255]),
    ))
    .unwrap();
    let source = SourceImage::from_layers(&[ImageSourceLayer::new(base)])
        .unwrap()
        .unwrap();
    let mut document = CanvasDocument::default();
    document.set_background(Some(source.clone()));
    let snapshot = document.persistence_snapshot().unwrap();
    assert!(snapshot.requires_materialization());
    assert!(matches!(
        snapshot.clone().encode(),
        Err(CanvasError::Unsupported(_))
    ));
    let mut calls = Vec::new();
    let prepared = snapshot
        .materialize_for_save(|source, eligible| {
            calls.push(eligible);
            let color = if eligible {
                [20, 40, 60, 255]
            } else {
                [120, 140, 160, 255]
            };
            SourceImage::new(RgbaImage::from_pixel(
                source.width(),
                source.height(),
                image::Rgba(color),
            ))
        })
        .unwrap();
    assert_eq!(calls, [false, true]);
    assert!(!prepared.requires_materialization());
    let decoded = crate::DrawingSnapshot::decode(&prepared.encode().unwrap())
        .unwrap()
        .into_document()
        .unwrap();
    assert_eq!(
        decoded
            .background()
            .unwrap()
            .raster_pixels()
            .unwrap()
            .get_pixel(0, 0)
            .0,
        [120, 140, 160, 255]
    );
    assert_eq!(
        decoded
            .background()
            .unwrap()
            .smart_erase_pixels()
            .get_pixel(0, 0)
            .0,
        [20, 40, 60, 255]
    );
    assert!(
        document
            .persistence_snapshot()
            .unwrap()
            .materialize_for_save(|_, _| SourceImage::new(RgbaImage::new(1, 1)))
            .is_err()
    );
}
#[test]
fn effective_coverage_and_anisotropic_placement_bound_source_extent() {
    let image = SourceImage::new(RgbaImage::from_pixel(
        100,
        100,
        image::Rgba([10, 20, 30, 255]),
    ))
    .unwrap();
    let layers = [
        ImageSourceLayer {
            image: image.clone(),
            canvas_rect: None,
            coverage: Some(CanvasRect::new(10.0, 10.0, 1.0, 1.0).unwrap()),
            smart_erase_source: true,
        },
        ImageSourceLayer {
            image: image.clone(),
            canvas_rect: Some(CanvasRect::new(10.0, 10.0, 1.0, 2.0).unwrap()),
            coverage: None,
            smart_erase_source: true,
        },
    ];
    let result = SourceImage::from_layers(&layers).unwrap().unwrap();
    assert_eq!(result.dimensions(), [100, 200]);
    assert!(result.raster_pixels().is_none());
    assert_eq!(result.origin(), Point::new(10.0, 10.0));
    assert_ne!(
        SourceImage::new((**image.raster_pixels().unwrap()).clone())
            .unwrap()
            .pixels_identity(),
        image.pixels_identity()
    );
    assert_eq!(image.clone().pixels_identity(), image.pixels_identity());
    let cancel = crate::CanvasCancellation::default();
    cancel.cancel();
    assert!(matches!(
        SourceImage::from_layers_cancellable(&layers, &cancel),
        Err(CanvasError::Cancelled)
    ));
}
