//! Public-API smoke test; does not depend on the GPUI mock window backend.
#![cfg(target_os = "windows")]
use anyhow::{Context, Result, ensure};
use gpui::GpuRect;
use gpui_windows::gpu_extensions::OffscreenGpuHost;
use snow_draw_engine_gpui::{
    CanvasCancellation, CanvasDocument, CanvasPresentation, CanvasSession, GpuCanvas,
    SmartEraseSnapshot,
    engine::{Camera, Point},
};
use std::time::{Duration, Instant};

#[test]
fn production_device_compiles_shaders_and_renders_without_cpu_rasterization() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let mut renderer = GpuCanvas::new(service.clone())?;
    let mut document = CanvasDocument::default();
    let mut session = CanvasSession::new(&mut document)?;
    session.set_surface(&mut document, 64, 64)?;
    session.set_camera(
        &mut document,
        Camera {
            center: Point::new(32.0, 32.0),
            zoom: 1.0,
        },
    )?;
    document.command(|editor|editor.apply_annotation_json(br#"{"version":1,"operations":[{"type":"rectangle","bounds":[16,16,32,32],"style":{"fill":[220,40,20,255],"stroke_width":0}}]}"#))?;
    session.refresh(&document)?;
    let frame = renderer.prepare(
        session.scene(),
        None,
        1,
        1.0,
        &CanvasPresentation {
            clear_background: false,
            ..Default::default()
        },
        &SmartEraseSnapshot::default(),
        &CanvasCancellation::default(),
    )?;
    let tile = frame.tiles.first().context("rendered tile")?;
    assert_eq!(tile.texture.service_id(), service.service_id());
    let mut readback = service.readback(
        &tile.texture,
        GpuRect {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        },
    )?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let image = loop {
        if let Some(image) = readback.poll()? {
            break image;
        }
        ensure!(Instant::now() < deadline, "GPU readback timed out");
        std::thread::yield_now();
    };
    assert_eq!(
        &image.pixels[(32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 4],
        &[220, 40, 20, 255]
    );
    assert_eq!(&image.pixels[0..4], &[0, 0, 0, 0]);
    Ok(())
}
