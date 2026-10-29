use super::*;

fn resident_source(service: &Arc<dyn GpuService>, pixels: &RgbaImage) -> Result<SourceImage> {
    let mut bytes = pixels.as_raw().clone();
    for pixel in bytes.as_chunks_mut::<4>().0 {
        for channel in 0..3 {
            pixel[channel] = ((u32::from(pixel[channel]) * u32::from(pixel[3]) + 127) / 255) as u8;
        }
    }
    let mut texture = service.create_rgba8_texture([pixels.width(), pixels.height()])?;
    service.upload_rgba8_region(
        &mut texture,
        GpuRect {
            x: 0,
            y: 0,
            width: pixels.width(),
            height: pixels.height(),
        },
        &bytes,
        pixels.width() * 4,
    )?;
    Ok(SourceImage::from_gpu_texture(texture)?)
}

#[test]
fn gpu_resident_sources_match_raster_placement_alpha_and_lods_without_display_transfers()
-> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let pixels = RgbaImage::from_fn(129, 97, |x, y| {
        Rgba(if x % 3 == 0 {
            [255, 0, 0, 0]
        } else {
            [20, 100 + ((x / 4 + y / 4) % 2 * 80) as u8, 200, 128]
        })
    });
    let origin = Point::new(7.25, 8.5);
    let cpu = SourceImage::new(pixels.clone())?.with_placement(origin, 0.75)?;
    let gpu = resident_source(&service, &pixels)?.with_placement(origin, 0.75)?;
    assert!(gpu.raster_pixels().is_none());
    assert!(gpu.gpu_texture().is_some());
    let mut cpu_canvas = GpuCanvas::new(service.clone())?;
    let mut gpu_canvas = GpuCanvas::new(service.clone())?;
    for zoom in [0.125, 0.5, 1.0, 2.0] {
        for scale in [1.0, 1.5] {
            let mut frame = view(160, 128);
            frame.camera.zoom = zoom;
            frame.camera.center = Point::new(40.0, 35.0);
            let scene = scene(frame, Vec::new());
            wait_ready(&mut cpu_canvas)?;
            let cpu_frame = cpu_canvas.prepare(
                &scene,
                Some(&cpu),
                cpu.pixels_identity(),
                scale,
                &Default::default(),
                &Default::default(),
                &Default::default(),
            )?;
            let expected = read_frame(&service, &cpu_frame)?;
            let before = service.diagnostics();
            wait_ready(&mut gpu_canvas)?;
            let gpu_frame = gpu_canvas.prepare(
                &scene,
                Some(&gpu),
                gpu.pixels_identity(),
                scale,
                &Default::default(),
                &Default::default(),
                &Default::default(),
            )?;
            assert_eq!(gpu_frame.profile.source_upload_bytes, 0);
            assert_eq!(service.diagnostics().readback_bytes, before.readback_bytes);
            let actual = read_frame(&service, &gpu_frame)?;
            for (a, b) in actual.pixels().zip(expected.pixels()) {
                close(a.0, b.0, 1);
            }
        }
    }
    Ok(())
}

#[test]
fn gpu_resident_sources_stay_pinned_until_completion_and_reject_foreign_devices() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let texture = service.create_rgba8_texture([16, 16])?;
    let source = SourceImage::from_gpu_texture(texture.clone())?;
    let mut canvas = GpuCanvas::new(service.clone())?;
    let scene = scene(view(32, 32), Vec::new());
    let frame = canvas.prepare(
        &scene,
        Some(&source),
        1,
        1.0,
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )?;
    drop(source);
    assert!(
        Arc::strong_count(&texture) > 1,
        "submitted source was released before completion"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while Arc::strong_count(&texture) != 1 {
        canvas.is_ready()?;
        ensure!(Instant::now() < deadline, "source retirement timed out");
        std::thread::yield_now();
    }
    let other_host = OffscreenGpuHost::new()?;
    let foreign = resident_source(&other_host.service(), &RgbaImage::new(16, 16))?;
    let error = canvas
        .prepare(
            &scene,
            Some(&foreign),
            2,
            1.0,
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )
        .err()
        .context("foreign device should fail")?;
    assert!(error.to_string().contains("another device"));
    assert_eq!(frame.tiles[0].texture.service_id(), service.service_id());
    Ok(())
}

#[test]
fn gpu_resident_sources_materialize_for_save_and_reject_silent_cpu_donor_sampling() -> Result<()> {
    use base64::Engine as _;
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let source = resident_source(
        &service,
        &RgbaImage::from_pixel(24, 16, Rgba([20, 60, 100, 255])),
    )?;
    assert!(matches!(
        source.eligible_region([0, 0, 24, 16], &Default::default()),
        Err(CanvasError::Unsupported(_))
    ));
    let layered = SourceImage::from_layers(&[ImageSourceLayer::new(source.clone())])?.unwrap();
    assert!(matches!(
        layered.eligible_region([0, 0, 24, 16], &Default::default()),
        Err(CanvasError::Unsupported(_))
    ));
    let mut export = GpuExportJob::bytes(
        service.clone(),
        CanvasSnapshot::source_only(source.clone()),
        1.0,
        Default::default(),
        GpuExportFormat::Svg,
        Default::default(),
    )?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let svg = loop {
        if let std::task::Poll::Ready(result) = export.poll() {
            break String::from_utf8(result?)?;
        }
        ensure!(Instant::now() < deadline, "GPU source SVG timed out");
        std::thread::yield_now();
    };
    let data = svg
        .split("data:image/png;base64,")
        .nth(1)
        .context("SVG embedded image")?;
    let png = base64::engine::general_purpose::STANDARD.decode(data.split('"').next().unwrap())?;
    let embedded = image::load_from_memory(&png)?.to_rgba8();
    assert_eq!(embedded.dimensions(), (24, 16));
    assert_eq!(embedded.get_pixel(12, 8).0, [20, 60, 100, 255]);
    let mut document = CanvasDocument::default();
    document.set_background(Some(source));
    let snapshot = document.persistence_snapshot()?;
    assert!(snapshot.requires_materialization());
    assert!(snapshot.clone().encode().is_err());
    let mut job = GpuSavePreparation::new(service, snapshot, Default::default())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let saved = loop {
        if let std::task::Poll::Ready(result) = job.poll() {
            break result?;
        }
        ensure!(Instant::now() < deadline, "GPU source save timed out");
        std::thread::yield_now();
    };
    assert!(!saved.requires_materialization());
    let restored = DrawingSnapshot::decode(&saved.encode()?)?.into_document()?;
    assert_eq!(
        restored
            .background()
            .unwrap()
            .raster_pixels()
            .unwrap()
            .get_pixel(12, 8)
            .0,
        [20, 60, 100, 255]
    );
    Ok(())
}

#[test]
fn gpu_resident_sources_preserve_anisotropic_layer_placement_and_coverage() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let pixels = RgbaImage::from_fn(65, 49, |x, y| Rgba([20 + x as u8, 60 + y as u8, 100, 255]));
    let cpu = SourceImage::new(pixels.clone())?;
    let gpu = resident_source(&service, &pixels)?;
    let layer = |image| ImageSourceLayer {
        image,
        canvas_rect: Some(CanvasRect::new(5.25, 8.5, 120.5, 55.5).unwrap()),
        coverage: Some(CanvasRect::new(12.75, 13.25, 101.5, 40.25).unwrap()),
        smart_erase_source: true,
    };
    let cpu = SourceImage::from_layers(&[layer(cpu)])?.unwrap();
    let gpu = SourceImage::from_layers(&[layer(gpu)])?.unwrap();
    let mut cpu_canvas = GpuCanvas::new(service.clone())?;
    let mut gpu_canvas = GpuCanvas::new(service.clone())?;
    for zoom in [0.25, 1.0, 2.0] {
        let mut view = view(160, 128);
        view.camera.zoom = zoom;
        view.camera.center = Point::new(64.0, 48.0);
        let scene = scene(view, Vec::new());
        wait_ready(&mut cpu_canvas)?;
        let expected = cpu_canvas.prepare(
            &scene,
            Some(&cpu),
            cpu.pixels_identity(),
            1.5,
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )?;
        let expected = read_frame(&service, &expected)?;
        wait_ready(&mut gpu_canvas)?;
        let actual = gpu_canvas.prepare(
            &scene,
            Some(&gpu),
            gpu.pixels_identity(),
            1.5,
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )?;
        assert_eq!(actual.profile.source_upload_bytes, 0);
        let actual = read_frame(&service, &actual)?;
        for (a, b) in actual.pixels().zip(expected.pixels()) {
            close(a.0, b.0, 1);
        }
    }
    Ok(())
}
