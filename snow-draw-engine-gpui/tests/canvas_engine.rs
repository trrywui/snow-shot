#![cfg(target_os = "windows")]
use gpui::{GpuRect, GpuService};
use gpui_windows::gpu_extensions::OffscreenGpuHost;
use image::{GenericImageView, Rgba, RgbaImage};
use snow_draw_engine_gpui::{
    CanvasCancellation, CanvasDocument, CanvasError, CanvasPresentation, CanvasSession,
    CanvasSnapshot, ExportTile, GpuCanvas, GpuExportFormat, GpuExportJob, GpuSavePreparation,
    RetainedScene, SourceImage, engine::*, prepare_text_metrics,
};
use std::{
    sync::{Arc, Mutex},
    task::Poll,
    time::{Duration, Instant},
};

fn capture(
    document: &mut CanvasDocument,
    session: &mut CanvasSession,
) -> Result<CanvasSnapshot, CanvasError> {
    prepare_text_metrics(document, session)?;
    CanvasSnapshot::capture(document, session)
}

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct GpuTest {
    _host: OffscreenGpuHost,
    service: Arc<dyn GpuService>,
    canvas: GpuCanvas,
    last_profile: snow_draw_engine_gpui::GpuRenderProfile,
    presentation: CanvasPresentation,
}
impl GpuTest {
    fn new() -> Self {
        let host = OffscreenGpuHost::new().unwrap();
        let service = host.service();
        Self {
            canvas: GpuCanvas::new(service.clone()).unwrap(),
            service,
            _host: host,
            last_profile: Default::default(),
            presentation: Default::default(),
        }
    }
    fn finish<T: Send + 'static>(&self, mut job: GpuExportJob<T>) -> Result<T, CanvasError> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let result = job.poll();
            assert!(self.service.diagnostics().pending_readback_bytes <= 2 * 256 * 128 * 4);
            if let Poll::Ready(result) = result {
                return result;
            }
            assert!(Instant::now() < deadline, "GPU export timed out");
            std::thread::yield_now();
        }
    }
    fn png(&mut self, snapshot: &CanvasSnapshot, scale: f32) -> Result<Vec<u8>, CanvasError> {
        self.finish(GpuExportJob::bytes(
            self.service.clone(),
            snapshot.clone(),
            scale,
            Default::default(),
            GpuExportFormat::Png,
            Default::default(),
        )?)
    }
    fn svg(&mut self, snapshot: &CanvasSnapshot, scale: f32) -> Result<String, CanvasError> {
        String::from_utf8(self.finish(GpuExportJob::bytes(
            self.service.clone(),
            snapshot.clone(),
            scale,
            Default::default(),
            GpuExportFormat::Svg,
            Default::default(),
        )?)?)
        .map_err(|error| CanvasError::InvalidFile(error.to_string()))
    }
    fn packed(
        &mut self,
        snapshot: &CanvasSnapshot,
        scale: f32,
        size: [u32; 2],
        tiles: &[ExportTile],
    ) -> Result<Vec<u8>, CanvasError> {
        self.finish(GpuExportJob::packed_png(
            self.service.clone(),
            snapshot.clone(),
            scale,
            size,
            tiles.to_vec(),
            Default::default(),
            Default::default(),
        )?)
    }
    fn write(
        &mut self,
        snapshot: &CanvasSnapshot,
        scale: f32,
        format: GpuExportFormat,
        output: &mut Vec<u8>,
        cancel: &CanvasCancellation,
    ) -> Result<(), CanvasError> {
        let sink = Output::default();
        self.finish(GpuExportJob::to_writer(
            self.service.clone(),
            snapshot.clone(),
            scale,
            Default::default(),
            format,
            sink.clone(),
            cancel.clone(),
        )?)?;
        output.extend_from_slice(&sink.0.lock().unwrap());
        Ok(())
    }
    fn save(&mut self, document: &CanvasDocument) -> Vec<u8> {
        let mut job = GpuSavePreparation::new(
            self.service.clone(),
            document.persistence_snapshot().unwrap(),
            Default::default(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Poll::Ready(result) = job.poll() {
                return result.unwrap().encode().unwrap();
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    fn draw(
        &mut self,
        scene: &RetainedScene,
        source: Option<&SourceImage>,
        resources: u64,
        scale: f32,
    ) -> Result<Arc<RgbaImage>, CanvasError> {
        let gpu_error = |error: anyhow::Error| CanvasError::Gpu(error.to_string());
        let deadline = Instant::now() + Duration::from_secs(30);
        while !self.canvas.is_ready().map_err(gpu_error)? {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let frame = self
            .canvas
            .prepare(
                scene,
                source,
                resources,
                scale,
                &self.presentation,
                &Default::default(),
                &Default::default(),
            )
            .map_err(gpu_error)?;
        self.last_profile = frame.profile.clone();
        let mut output = RgbaImage::new(frame.size[0], frame.size[1]);
        for tile in frame.tiles.iter() {
            let mut readback = self
                .service
                .readback(
                    &tile.texture,
                    GpuRect {
                        x: 0,
                        y: 0,
                        width: tile.region.width,
                        height: tile.region.height,
                    },
                )
                .map_err(gpu_error)?;
            let mut image = loop {
                if let Some(image) = readback.poll().map_err(gpu_error)? {
                    break image;
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            };
            for pixel in image.pixels.as_chunks_mut::<4>().0 {
                let alpha = u32::from(pixel[3]);
                for channel in &mut pixel[..3] {
                    *channel = (u32::from(*channel) * 255 + alpha / 2)
                        .checked_div(alpha)
                        .unwrap_or(0)
                        .min(255) as u8;
                }
            }
            let image = RgbaImage::from_raw(image.width, image.height, image.pixels).unwrap();
            image::imageops::replace(
                &mut output,
                &image,
                i64::from(tile.region.x),
                i64::from(tile.region.y),
            );
        }
        Ok(Arc::new(output))
    }
}

fn setup() -> (CanvasDocument, CanvasSession) {
    let mut document = CanvasDocument::default();
    let mut session = CanvasSession::new(&mut document).unwrap();
    session.set_surface(&mut document, 200, 160).unwrap();
    session
        .set_camera(
            &mut document,
            Camera {
                center: Point::new(100.0, 80.0),
                zoom: 1.0,
            },
        )
        .unwrap();
    (document, session)
}

#[test]
fn persistence_codec_moves_to_worker_and_atomic_cancel_preserves_target() {
    use snow_draw_engine_gpui::{CanvasError, DrawingSnapshot, write_atomic_with};
    use std::io::Write;
    let (mut document, mut session) = setup();
    document.set_background(Some(
        SourceImage::new(RgbaImage::from_pixel(16, 16, Rgba([40, 80, 120, 180]))).unwrap(),
    ));
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([
            {"type":"text","bounds":[10,10,100,40],"text":"Worker 你好 😀"}
        ]),
    );
    let checkpoint = document.engine().document_checkpoint().unwrap();
    let payload = document.persistence_snapshot().unwrap();
    assert!(payload.clone().encode_with_limit(800).is_err());
    let encoded = payload.clone().encode_with_limit(2048).unwrap();
    assert!(DrawingSnapshot::decode_with_limit(&encoded, 800).is_err());
    let explicit = DrawingSnapshot::decode_with_limit(&encoded, 2048)
        .unwrap()
        .into_document()
        .unwrap();
    assert_eq!(
        explicit.background().unwrap().raster_pixels().unwrap(),
        document.background().unwrap().raster_pixels().unwrap()
    );
    let loaded =
        std::thread::spawn(move || DrawingSnapshot::decode(&payload.encode().unwrap()).unwrap())
            .join()
            .unwrap();
    let restored = loaded.into_document().unwrap();
    assert_eq!(restored.engine().document_checkpoint().unwrap(), checkpoint);
    assert_eq!(
        restored.background().unwrap().raster_pixels().unwrap(),
        document.background().unwrap().raster_pixels().unwrap()
    );
    let directory = std::env::temp_dir().join(format!("snow-canvas-atomic-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("drawing.snowdraw");
    std::fs::write(&path, b"old complete document").unwrap();
    let result = write_atomic_with(&path, |writer| {
        writer.write_all(b"partial replacement").unwrap();
        Err(CanvasError::Cancelled)
    });
    assert!(matches!(result, Err(CanvasError::Cancelled)));
    assert_eq!(std::fs::read(&path).unwrap(), b"old complete document");
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&directory).unwrap();
}

#[test]
fn fractional_scale_dimensions_agree_in_full_stream_svg_and_packed_tiles() {
    let (mut document, mut session) = setup();
    let mut gpu = GpuTest::new();
    let snapshot = capture(&mut document, &mut session)
        .unwrap()
        .with_region(
            snow_draw_engine_gpui::CanvasRect::new(0.0, 0.0, 1000.0, 100.0).unwrap(),
            [1000, 100],
        )
        .unwrap();
    let full = image::load_from_memory(&gpu.png(&snapshot, 1.2).unwrap())
        .unwrap()
        .into_rgba8();
    let mut png = Vec::new();
    gpu.write(
        &snapshot,
        1.2,
        GpuExportFormat::Png,
        &mut png,
        &Default::default(),
    )
    .unwrap();
    let streamed = image::load_from_memory(&png).unwrap().into_rgba8();
    assert_eq!(streamed, full);
    let mut svg = Vec::new();
    gpu.write(
        &snapshot,
        1.2,
        GpuExportFormat::Svg,
        &mut svg,
        &Default::default(),
    )
    .unwrap();
    let svg = String::from_utf8(svg).unwrap();
    assert!(svg.contains(&format!(
        "width=\"{}\" height=\"{}\"",
        full.width(),
        full.height()
    )));
    let tree = usvg::Tree::from_str(&svg, &Default::default()).unwrap();
    assert_eq!(
        (tree.size().width() as u32, tree.size().height() as u32),
        full.dimensions()
    );
    let packed = gpu
        .packed(
            &snapshot,
            1.2,
            [1, 1],
            &[snow_draw_engine_gpui::ExportTile {
                source: [full.width() - 1, full.height() - 1, 1, 1],
                destination: [0, 0],
            }],
        )
        .unwrap();
    assert_eq!(
        image::load_from_memory(&packed)
            .unwrap()
            .into_rgba8()
            .get_pixel(0, 0),
        full.get_pixel(full.width() - 1, full.height() - 1)
    );
}

#[test]
fn fractional_svg_dimensions_and_gpu_effect_payload_preserve_the_png_grid() {
    fn check(svg: &str, pixels: &RgbaImage) {
        let tree = usvg::Tree::from_str(svg, &Default::default()).unwrap();
        assert_eq!(
            (tree.size().width() as u32, tree.size().height() as u32),
            pixels.dimensions()
        );
        if let Some((_, payload)) = svg.split_once("data:image/png;base64,") {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(payload.split('"').next().unwrap())
                .unwrap();
            assert_eq!(
                &image::load_from_memory(&bytes).unwrap().into_rgba8(),
                pixels
            );
        } else {
            assert!(svg.contains("<path"));
        }
    }
    let (mut document, mut session) = setup();
    session.set_surface(&mut document, 101, 67).unwrap();
    session
        .set_camera(
            &mut document,
            Camera {
                center: Point::new(50.5, 33.5),
                zoom: 1.0,
            },
        )
        .unwrap();
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([
            {"type":"rectangle","bounds":[0,0,101,67],"style":{"fill":[230,240,250,255],"stroke_width":1}},
            {"type":"ellipse","bounds":[15,10,30,30],"style":{"fill":[30,80,150,255]}}
        ]),
    );
    let mut gpu = GpuTest::new();
    for effects in [false, true] {
        if effects {
            annotations(
                &mut document,
                &mut session,
                serde_json::json!([
                    {"type":"rectangle_filter","bounds":[40,10,30,40],"style":{"filter":"gaussian_blur","strength":0.1}}
                ]),
            );
        }
        let direct = gpu
            .draw(session.scene(), None, document.resource_revision(), 1.2)
            .unwrap();
        let png = image::load_from_memory(
            &gpu.png(&capture(&mut document, &mut session).unwrap(), 1.2)
                .unwrap(),
        )
        .unwrap()
        .into_rgba8();
        assert_eq!(direct.as_ref(), &png);
        check(
            &gpu.svg(&capture(&mut document, &mut session).unwrap(), 1.2)
                .unwrap(),
            &png,
        );
        let snapshot = capture(&mut document, &mut session).unwrap();
        let png = image::load_from_memory(&gpu.png(&snapshot, 1.2).unwrap())
            .unwrap()
            .into_rgba8();
        check(&gpu.svg(&snapshot, 1.2).unwrap(), &png);
    }
}

#[test]
fn streamed_gpu_export_preserves_ordered_effect_support() {
    let (mut document, mut session) = setup();
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([
            {"type":"rectangle_filter","bounds":[0,0,1000,1000],"style":{"filter":"gaussian_blur","strength":1}},
            {"type":"rectangle","bounds":[1,1,2,2]},
            {"type":"rectangle_filter","bounds":[0,0,1000,1000],"style":{"filter":"gaussian_blur","strength":1}},
            {"type":"rectangle","bounds":[2,2,2,2]},
            {"type":"rectangle_filter","bounds":[0,0,1000,1000],"style":{"filter":"gaussian_blur","strength":1}}
        ]),
    );
    let mut gpu = GpuTest::new();
    let snapshot = capture(&mut document, &mut session)
        .unwrap()
        .with_region(
            snow_draw_engine_gpui::CanvasRect::new(0.0, 0.0, 1000.0, 1000.0).unwrap(),
            [100, 100],
        )
        .unwrap();
    let mut png = Vec::new();
    gpu.write(
        &snapshot,
        1.0,
        GpuExportFormat::Png,
        &mut png,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        image::load_from_memory(&png).unwrap().dimensions(),
        (100, 100)
    );
}

#[test]
fn maximum_display_sized_export_keeps_staging_bounded() {
    let (mut document, mut session) = setup();
    let mut gpu = GpuTest::new();
    let snapshot = capture(&mut document, &mut session)
        .unwrap()
        .with_region(
            snow_draw_engine_gpui::CanvasRect::new(0.0, 0.0, 4096.0, 4096.0).unwrap(),
            [4096, 4096],
        )
        .unwrap();
    // Export staging stays bounded even when output reaches the display limit.
    let png = gpu.png(&snapshot, 1.0).unwrap();
    assert_eq!(
        image::ImageReader::new(std::io::Cursor::new(&png))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap(),
        (4096, 4096)
    );
    assert_eq!(gpu.service.diagnostics().pending_readback_bytes, 0);
}

fn annotations(
    document: &mut CanvasDocument,
    session: &mut CanvasSession,
    operations: serde_json::Value,
) {
    let batch = serde_json::to_vec(
        &serde_json::json!({"version":1,"label":"fixture","operations":operations}),
    )
    .unwrap();
    document
        .command(|engine| engine.apply_annotation_json(&batch))
        .unwrap();
    session.refresh(document).unwrap();
}

#[test]
fn session_and_background_roundtrip_preserve_engine_history() {
    let (mut document, mut session) = setup();
    document.set_background(Some(
        SourceImage::new(RgbaImage::from_pixel(16, 12, Rgba([17, 42, 99, 120]))).unwrap(),
    ));
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([
            {"type":"rectangle","bounds":[10,20,70,40]},
            {"type":"text","bounds":[80,20,100,50],"text":"Canvas 你好 😀"}
        ]),
    );
    let before = document.engine().document_checkpoint().unwrap();
    let restored = CanvasDocument::deserialize(&document.serialize().unwrap()).unwrap();
    assert_eq!(restored.engine().document_checkpoint().unwrap(), before);
    assert_eq!(
        restored
            .background()
            .unwrap()
            .raster_pixels()
            .unwrap()
            .get_pixel(0, 0)
            .0,
        [17, 42, 99, 120]
    );
    assert!(restored.engine().history_state().can_undo);
    assert!(CanvasDocument::deserialize(b"{\"schema\":2}").is_err());
    assert_eq!(document.engine().document_checkpoint().unwrap(), before);
}

#[test]
fn sessions_reject_other_documents_and_viewport_roundtrip_is_precise() {
    let (mut document, mut session) = setup();
    let (mut other, _) = setup();
    assert!(session.input(&mut other, InputEvent::FocusLost).is_err());
    assert!(session.refresh(&other).is_err());
    let camera = Camera {
        center: Point::new(-19.25, 803.125),
        zoom: 2.75,
    };
    session.set_camera(&mut document, camera).unwrap();
    let frame = session.scene().frame();
    for world in [Point::new(0.0, 0.0), Point::new(-50.75, 1900.0)] {
        let view = canvas_to_view(world, &camera, frame.surface);
        let roundtrip = view_to_canvas(view, &camera, frame.surface);
        assert!((world.x - roundtrip.x).abs() < 1e-10);
        assert!((world.y - roundtrip.y).abs() < 1e-10);
    }
}

fn pointer(kind: PointerEventType, x: f64, y: f64) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        pointer_id: 1,
        event_type: kind,
        device: PointerDevice::Mouse,
        position: Point::new(x, y),
        button: Some(PointerButton::Primary),
        buttons: PointerButtons(if kind == PointerEventType::Up {
            0
        } else {
            PointerButtons::PRIMARY
        }),
        modifiers: Modifiers::default(),
        pressure: None,
        timestamp_ms: 1,
    })
}

#[test]
fn interrupted_gestures_release_capture_and_repeated_actions_have_history() {
    let (mut document, mut session) = setup();
    session.set_tool(&mut document, ActiveTool::Shape).unwrap();
    session
        .input(&mut document, pointer(PointerEventType::Down, 10.0, 20.0))
        .unwrap();
    session
        .input(&mut document, pointer(PointerEventType::Move, 80.0, 80.0))
        .unwrap();
    assert!(document.engine().has_pointer_capture());
    session.input(&mut document, InputEvent::FocusLost).unwrap();
    assert!(!document.engine().has_pointer_capture());
    assert!(!document.engine().has_document_content());
    for _ in 0..3 {
        session
            .input(&mut document, pointer(PointerEventType::Down, 20.0, 20.0))
            .unwrap();
        session
            .input(&mut document, pointer(PointerEventType::Move, 90.0, 90.0))
            .unwrap();
        session
            .input(&mut document, pointer(PointerEventType::Up, 90.0, 90.0))
            .unwrap();
    }
    let checkpoint = document.engine().document_checkpoint().unwrap();
    document.command(|engine| engine.undo()).unwrap();
    document.command(|engine| engine.redo()).unwrap();
    assert_eq!(document.engine().document_checkpoint().unwrap(), checkpoint);
    session.close(&mut document).unwrap();
    assert!(!document.engine().has_pointer_capture());
}

#[test]
fn retained_delta_scene_matches_full_reset_after_long_freehand_tail_updates() {
    let (mut document, mut session) = setup();
    session
        .set_tool(&mut document, ActiveTool::FreeDraw)
        .unwrap();
    session
        .input(&mut document, pointer(PointerEventType::Down, 10.0, 60.0))
        .unwrap();
    for index in 0..200 {
        session
            .input(
                &mut document,
                pointer(
                    PointerEventType::Move,
                    10.0 + f64::from(index) * 0.8,
                    60.0 + (f64::from(index) * 0.1).sin() * 20.0,
                ),
            )
            .unwrap();
        let reset = document
            .engine()
            .acquire_retained_scene_delta(session.viewport(), None)
            .unwrap();
        assert!(reset.reset);
        let mut expected = RetainedScene::default();
        expected.apply_delta(&reset).unwrap();
        assert_eq!(session.scene().order_ids(), expected.order_ids());
        assert!(session.scene().ordered_nodes().eq(expected.ordered_nodes()));
        assert_eq!(session.scene().overlays(), expected.overlays());
    }
    session
        .input(&mut document, pointer(PointerEventType::Up, 170.0, 80.0))
        .unwrap();
}

#[test]
fn gpu_retains_idle_tiles_and_exports_png_svg() {
    let (mut document, mut session) = setup();
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([
            {"type":"rectangle","bounds":[20,20,60,50],"style":{"fill":[230,30,40,255],"stroke_width":0}},
            {"type":"ellipse","bounds":[100,20,60,50],"style":{"fill":[30,70,230,255],"stroke_width":0}}
        ]),
    );
    let mut gpu = GpuTest::new();
    let first = gpu.draw(session.scene(), None, 0, 1.0).unwrap();
    assert_eq!(first.get_pixel(40, 40).0, [230, 30, 40, 255]);
    assert_eq!(first.get_pixel(130, 40).0, [30, 70, 230, 255]);
    let second = gpu.draw(session.scene(), None, 0, 1.0).unwrap();
    assert_eq!(first, second);
    assert_eq!(gpu.last_profile.rendered_tiles, 0);
    assert_eq!(gpu.last_profile.uploaded_bytes, 0);
    let png = gpu
        .png(&capture(&mut document, &mut session).unwrap(), 1.5)
        .unwrap();
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (300, 240));
    assert!(
        gpu.svg(&capture(&mut document, &mut session).unwrap(), 1.0)
            .unwrap()
            .contains("<path")
    );
}

#[test]
fn overlapping_filters_read_one_immutable_backdrop_and_restore_replaces_alpha() {
    let (mut document, mut session) = setup();
    let mut source = RgbaImage::from_pixel(200, 160, Rgba([20, 50, 100, 255]));
    source.put_pixel(50, 50, Rgba([0, 0, 0, 0]));
    document.set_background(Some(SourceImage::new(source).unwrap()));
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([
            {"type":"rectangle_filter","bounds":[10,10,100,100],"style":{"filter":"inversion"}},
            {"type":"rectangle_filter","bounds":[30,30,100,100],"style":{"filter":"grayscale"}},
            {"type":"rectangle_filter","bounds":[40,40,100,100],"style":{"filter":"inversion"}}
        ]),
    );
    let mut gpu = GpuTest::new();
    gpu.presentation.clear_background = false;
    let result = gpu
        .draw(
            session.scene(),
            document.background(),
            document.resource_revision(),
            1.0,
        )
        .unwrap();
    assert_eq!(result.get_pixel(70, 70).0, [235, 205, 155, 255]);
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([
            {"type":"rectangle","bounds":[30,30,70,70],"style":{"fill":[200,10,10,255],"stroke_width":0}}
        ]),
    );
    session
        .set_tool(&mut document, ActiveTool::RectangleEraser)
        .unwrap();
    session
        .input(&mut document, pointer(PointerEventType::Down, 40.0, 40.0))
        .unwrap();
    session
        .input(&mut document, pointer(PointerEventType::Move, 70.0, 70.0))
        .unwrap();
    session
        .input(&mut document, pointer(PointerEventType::Up, 70.0, 70.0))
        .unwrap();
    let result = gpu
        .draw(
            session.scene(),
            document.background(),
            document.resource_revision(),
            1.0,
        )
        .unwrap();
    assert_eq!(result.get_pixel(50, 50).0, [0, 0, 0, 0]);
    assert_eq!(result.get_pixel(55, 55).0, [20, 50, 100, 255]);
}

#[test]
fn closing_an_idle_view_preserves_another_views_gesture() {
    let (mut document, mut live) = setup();
    let idle = CanvasSession::new(&mut document).unwrap();
    live.set_tool(&mut document, ActiveTool::Shape).unwrap();
    live.input(&mut document, pointer(PointerEventType::Down, 20.0, 20.0))
        .unwrap();
    live.input(&mut document, pointer(PointerEventType::Move, 90.0, 80.0))
        .unwrap();
    assert!(idle.close(&mut document).unwrap().is_none());
    assert!(document.engine().has_pointer_capture());
    live.input(&mut document, pointer(PointerEventType::Up, 90.0, 80.0))
        .unwrap();
    assert!(document.engine().has_document_content());
    let mut other = CanvasSession::new(&mut document).unwrap();
    live.input(&mut document, pointer(PointerEventType::Down, 30.0, 30.0))
        .unwrap();
    let update = live.close(&mut document).unwrap().unwrap();
    assert_eq!(update.interaction.capture, PointerCaptureCommand::Release);
    assert!(!document.engine().has_pointer_capture());
    other.set_tool(&mut document, ActiveTool::Shape).unwrap();
    other
        .input(&mut document, pointer(PointerEventType::Down, 5.0, 5.0))
        .unwrap();
    other
        .input(&mut document, pointer(PointerEventType::Up, 60.0, 60.0))
        .unwrap();
    assert!(!document.engine().has_pointer_capture());
}
#[test]
fn plain_svg_outlines_text_and_effect_svg_preserves_pixels() {
    let (mut document, mut session) = setup();
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([{"type":"rectangle","bounds":[10,10,40,30],"style":{"fill":[32,64,128,255]}},{"type":"text","bounds":[60,20,120,50],"text":"Portable"}]),
    );
    let mut gpu = GpuTest::new();
    prepare_text_metrics(&mut document, &mut session).unwrap();
    let svg = gpu
        .svg(&capture(&mut document, &mut session).unwrap(), 1.5)
        .unwrap();
    assert!(svg.contains("<path"));
    assert!(!svg.contains("<text"));
    assert!(!svg.contains("data:image/png"));
    let tree = usvg::Tree::from_str(&svg, &Default::default()).unwrap();
    assert_eq!((tree.size().width(), tree.size().height()), (300.0, 240.0));
    assert!(!tree.root().children().is_empty());
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([{"type":"rectangle_filter","bounds":[20,20,80,60],"style":{"filter":"gaussian_blur","strength":0.2}}]),
    );
    let expected = gpu
        .draw(session.scene(), None, document.resource_revision(), 1.5)
        .unwrap();
    let svg = gpu
        .svg(&capture(&mut document, &mut session).unwrap(), 1.5)
        .unwrap();
    let encoded = svg
        .split("data:image/png;base64,")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    assert_eq!(
        *expected,
        image::load_from_memory(&png).unwrap().into_rgba8()
    );
}

#[test]
fn export_snapshot_keeps_effects_invariant_under_viewport_crop() {
    let mut document = CanvasDocument::default();
    let mut small = CanvasSession::new(&mut document).unwrap();
    small.set_surface(&mut document, 120, 90).unwrap();
    small
        .set_camera(
            &mut document,
            Camera {
                center: Point::new(120.0, 90.0),
                zoom: 1.0,
            },
        )
        .unwrap();
    document.set_background(Some(
        SourceImage::new(RgbaImage::from_fn(240, 180, |x, y| {
            Rgba([
                ((x * 13 + y * 17) % 256) as u8,
                (x % 256) as u8,
                (y % 256) as u8,
                255,
            ])
        }))
        .unwrap(),
    ));
    annotations(
        &mut document,
        &mut small,
        serde_json::json!([
        {"type":"rectangle_filter","bounds":[0,0,240,180],"style":{"filter":"gaussian_blur","strength":0.2}},
        {"type":"rectangle","bounds":[1,1,2,2],"style":{"opacity":0.0}},
        {"type":"rectangle_filter","bounds":[0,0,240,180],"style":{"filter":"gaussian_blur","strength":0.2}},
        {"type":"rectangle","bounds":[235,175,2,2],"style":{"opacity":0.0}},
        {"type":"rectangle_filter","bounds":[0,0,240,180],"style":{"filter":"gaussian_blur","strength":0.2}}
        ]),
    );
    let mut gpu = GpuTest::new();
    let snapshot = capture(&mut document, &mut small).unwrap();
    let cropped = image::load_from_memory(&gpu.png(&snapshot, 1.0).unwrap())
        .unwrap()
        .into_rgba8();
    let mut large = CanvasSession::new(&mut document).unwrap();
    large.set_surface(&mut document, 240, 180).unwrap();
    large
        .set_camera(
            &mut document,
            Camera {
                center: Point::new(120.0, 90.0),
                zoom: 1.0,
            },
        )
        .unwrap();
    let snapshot = capture(&mut document, &mut large).unwrap();
    let full = image::load_from_memory(&gpu.png(&snapshot, 1.0).unwrap())
        .unwrap()
        .into_rgba8();
    let expected = image::imageops::crop_imm(&full, 60, 45, 120, 90).to_image();
    assert!(
        cropped
            .as_raw()
            .iter()
            .zip(expected.as_raw())
            .all(|(a, b)| a.abs_diff(*b) <= 1)
    );
}

#[test]
fn immutable_gpu_snapshots_preserve_older_document_content() {
    let (mut document, mut session) = setup();
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([{"type":"rectangle","bounds":[20,20,80,60],"style":{"fill":[220,30,40,255]}}]),
    );
    let mut gpu = GpuTest::new();
    let first = capture(&mut document, &mut session).unwrap();
    let before = gpu.png(&first, 1.0).unwrap();
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([{"type":"rectangle","bounds":[20,20,80,60],"style":{"fill":[20,80,230,255]}}]),
    );
    let second = capture(&mut document, &mut session).unwrap();
    let after = gpu.png(&second, 1.0).unwrap();
    assert_ne!(before, after);
    assert_eq!(
        image::load_from_memory(&after)
            .unwrap()
            .into_rgba8()
            .get_pixel(50, 50)
            .0,
        [20, 80, 230, 255]
    );
    assert_eq!(
        image::load_from_memory(&gpu.png(&first, 1.0).unwrap())
            .unwrap()
            .into_rgba8()
            .get_pixel(50, 50)
            .0,
        [220, 30, 40, 255]
    );
}

#[test]
fn fractional_snapshot_exports_preserve_the_original_pixel_grid() {
    let (mut document, mut session) = setup();
    session.set_surface(&mut document, 101, 83).unwrap();
    session
        .set_camera(
            &mut document,
            Camera {
                center: Point::new(60.125, 55.25),
                zoom: 1.3,
            },
        )
        .unwrap();
    document.set_background(Some(
        SourceImage::new(RgbaImage::from_fn(160, 140, |x, y| {
            if (x + y) % 2 == 0 {
                Rgba([30, 90, 210, 255])
            } else {
                Rgba([220, 150, 40, 255])
            }
        }))
        .unwrap(),
    ));
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([{"type":"line","points":[[38,15],[38,95]],"style":{"stroke":[0,0,0,255],"stroke_width":1}}]),
    );
    let mut gpu = GpuTest::new();
    for scale in [1.25, 1.5] {
        let snapshot = capture(&mut document, &mut session).unwrap();
        let expected = gpu
            .draw(
                session.scene(),
                document.background(),
                document.resource_revision(),
                scale,
            )
            .unwrap();
        let actual = image::load_from_memory(&gpu.png(&snapshot, scale).unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(actual.dimensions(), expected.dimensions());
        assert!(
            actual
                .as_raw()
                .iter()
                .zip(expected.as_raw())
                .all(|(a, b)| a.abs_diff(*b) <= 1),
            "fractional grid {scale}"
        );
    }
}

#[test]
fn host_layers_coverage_roundtrip_presentation_clips_and_disjoint_exports() {
    use snow_draw_engine_gpui::{CanvasPresentation, CanvasRect, ExportTile, ImageSourceLayer};
    let (mut document, mut session) = setup();
    let base = SourceImage::new(RgbaImage::from_pixel(200, 160, Rgba([20, 60, 100, 255]))).unwrap();
    let overlay = SourceImage::new(RgbaImage::from_pixel(60, 60, Rgba([200, 50, 20, 255])))
        .unwrap()
        .with_placement(Point::new(20.0, 20.0), 1.0)
        .unwrap();
    let source = SourceImage::from_layers(&[
        ImageSourceLayer {
            image: base,
            canvas_rect: None,
            coverage: None,
            smart_erase_source: true,
        },
        ImageSourceLayer {
            image: overlay,
            canvas_rect: None,
            coverage: Some(CanvasRect::new(30.0, 30.0, 20.0, 20.0).unwrap()),
            smart_erase_source: false,
        },
    ])
    .unwrap()
    .unwrap();
    assert!(source.raster_pixels().is_none());
    document.set_background(Some(source));
    let mut gpu = GpuTest::new();
    let visual = image::load_from_memory(
        &gpu.png(&capture(&mut document, &mut session).unwrap(), 1.0)
            .unwrap(),
    )
    .unwrap()
    .into_rgba8();
    assert_eq!(visual.get_pixel(25, 25).0, [20, 60, 100, 255]);
    assert_eq!(visual.get_pixel(35, 35).0, [200, 50, 20, 255]);
    let saved = gpu.save(&document);
    let restored = CanvasDocument::deserialize(&saved).unwrap();
    assert_eq!(
        &visual,
        restored
            .background()
            .unwrap()
            .raster_pixels()
            .unwrap()
            .as_ref()
    );
    assert_eq!(saved, restored.serialize().unwrap());
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([
            {"type":"rectangle","bounds":[10,10,100,80],"style":{"fill":[220,20,30,255],"stroke_width":1}},
            {"type":"rectangle_filter","bounds":[20,20,80,60],"style":{"filter":"gaussian_blur","strength":0.2}}
        ]),
    );
    let snapshot = capture(&mut document, &mut session).unwrap();
    let backdrop =
        SourceImage::new(RgbaImage::from_pixel(200, 160, Rgba([70, 180, 90, 255]))).unwrap();
    let after = SourceImage::new(RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 255]))).unwrap();
    let presentation = CanvasPresentation {
        background_override: Some(backdrop),
        after: Some(after),
        content_clip: Some(CanvasRect::new(25.0, 25.0, 20.0, 20.0).unwrap()),
        ..Default::default()
    };
    let clipped = snapshot
        .clone()
        .with_presentation(presentation.clone())
        .unwrap();
    let full = image::load_from_memory(&gpu.png(&clipped, 1.0).unwrap())
        .unwrap()
        .into_rgba8();
    assert_eq!(full.get_pixel(5, 5).0, [0, 0, 0, 255]);
    assert_eq!(full.get_pixel(15, 15).0, [70, 180, 90, 255]);
    assert_ne!(full.get_pixel(35, 35).0, [70, 180, 90, 255]);
    let tiles = [
        ExportTile {
            source: [20, 20, 30, 30],
            destination: [0, 0],
        },
        ExportTile {
            source: [70, 40, 20, 30],
            destination: [30, 0],
        },
    ];
    let packed = image::load_from_memory(&gpu.packed(&clipped, 1.0, [50, 30], &tiles).unwrap())
        .unwrap()
        .into_rgba8();
    for tile in tiles {
        for y in 0..tile.source[3] {
            for x in 0..tile.source[2] {
                assert_eq!(
                    packed.get_pixel(tile.destination[0] + x, y),
                    full.get_pixel(tile.source[0] + x, tile.source[1] + y)
                );
            }
        }
    }
    let hidden = snapshot
        .with_presentation(CanvasPresentation {
            content_visible: false,
            ..presentation
        })
        .unwrap();
    let hidden = image::load_from_memory(&gpu.png(&hidden, 1.0).unwrap())
        .unwrap()
        .into_rgba8();
    assert_eq!(hidden.get_pixel(35, 35).0, [70, 180, 90, 255]);
}

#[test]
fn strict_export_rejects_live_smart_erase_preview_without_stealing_capture() {
    let (mut document, mut session) = setup();
    document.set_background(Some(
        SourceImage::new(RgbaImage::from_pixel(200, 160, Rgba([50, 100, 150, 255]))).unwrap(),
    ));
    let mut style = document
        .engine()
        .viewport_style_toolbar_state(session.viewport())
        .unwrap()
        .filter_style;
    style.filter_type = CanvasFilterType::SmartErase;
    document
        .command(|e| {
            e.set_viewport_filter_style(session.viewport(), style, FILTER_STYLE_PROPERTY_ALL)
        })
        .unwrap();
    session
        .set_tool(&mut document, ActiveTool::RectangleFilter)
        .unwrap();
    session
        .input(&mut document, pointer(PointerEventType::Down, 40.0, 40.0))
        .unwrap();
    session
        .input(&mut document, pointer(PointerEventType::Move, 80.0, 80.0))
        .unwrap();
    let mut gpu = GpuTest::new();
    let snapshot = capture(&mut document, &mut session).unwrap();
    assert!(gpu.png(&snapshot, 1.0).is_err());
    assert!(document.engine().has_pointer_capture());
    session.input(&mut document, InputEvent::FocusLost).unwrap();
    assert!(!document.engine().has_pointer_capture());
    assert!(!document.engine().history_state().can_undo);
}

#[test]
fn source_replacement_undo_is_ordered_with_document_commands() {
    let (mut document, mut session) = setup();
    let red = SourceImage::new(RgbaImage::from_pixel(20, 20, Rgba([200, 30, 40, 255]))).unwrap();
    let blue = SourceImage::new(RgbaImage::from_pixel(20, 20, Rgba([30, 40, 200, 255]))).unwrap();
    document.set_background(Some(red.clone()));
    document
        .command(|e| e.commit_host_history_state(999))
        .unwrap();
    for state in 1000..1020 {
        document
            .command(|e| e.commit_host_history_state(state))
            .unwrap();
    }
    document.replace_background(Some(blue.clone())).unwrap();
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([{"type":"rectangle","bounds":[10,10,40,30]}]),
    );
    document.command(|e| e.undo()).unwrap();
    assert_eq!(
        document.background().unwrap().raster_pixels().unwrap(),
        blue.raster_pixels().unwrap()
    );
    document.command(|e| e.undo()).unwrap();
    assert_eq!(
        document.background().unwrap().raster_pixels().unwrap(),
        red.raster_pixels().unwrap()
    );
    document.command(|e| e.redo()).unwrap();
    assert_eq!(
        document.background().unwrap().raster_pixels().unwrap(),
        blue.raster_pixels().unwrap()
    );
    document.command(|e| e.redo()).unwrap();
    assert!(document.engine().has_document_content());
    let restored = CanvasDocument::deserialize(&document.serialize().unwrap()).unwrap();
    assert_eq!(
        restored.background().unwrap().raster_pixels().unwrap(),
        blue.raster_pixels().unwrap()
    );
}

#[test]
fn selection_rotation_multiselect_templates_and_history_use_engine_geometry() {
    let (mut document, mut session) = setup();
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([
            {"type":"rectangle","bounds":[30,35,50,40]}, {"type":"ellipse","bounds":[110,80,45,35]}
        ]),
    );
    session.set_tool(&mut document, ActiveTool::Select).unwrap();
    for kind in [PointerEventType::Down, PointerEventType::Up] {
        session
            .input(&mut document, pointer(kind, 30.0, 55.0))
            .unwrap();
    }
    let before = document.engine().document_checkpoint().unwrap();
    let handle = session
        .scene()
        .overlays()
        .iter()
        .find_map(|overlay| {
            if let OverlayDisplayItem::Rectangle(r) = overlay {
                (r.kind == UiShapeKind::SelectionRotationHandle).then_some(*r)
            } else {
                None
            }
        })
        .unwrap();
    let center = Point::new(55.0, 55.0);
    let dx = handle.center_x - center.x;
    let dy = handle.center_y - center.y;
    session
        .input(
            &mut document,
            pointer(PointerEventType::Down, handle.center_x, handle.center_y),
        )
        .unwrap();
    session
        .input(
            &mut document,
            pointer(PointerEventType::Move, center.x - dy, center.y + dx),
        )
        .unwrap();
    session
        .input(
            &mut document,
            pointer(PointerEventType::Up, center.x - dy, center.y + dx),
        )
        .unwrap();
    assert_ne!(document.engine().document_checkpoint().unwrap(), before);
    document.command(|e| e.undo()).unwrap();
    assert_eq!(document.engine().document_checkpoint().unwrap(), before);
    session.refresh(&document).unwrap();
    for kind in [PointerEventType::Down, PointerEventType::Up] {
        let InputEvent::Pointer(mut event) = pointer(kind, 110.0, 97.5) else {
            unreachable!()
        };
        event.modifiers.shift = true;
        session
            .input(&mut document, InputEvent::Pointer(event))
            .unwrap();
    }
    assert_eq!(document.engine().selected_ids().len(), 2);
    let template = document
        .engine()
        .serialize_selected_draw_template()
        .unwrap();
    session
        .mutate(&mut document, |e, v| {
            e.insert_draw_template_with_viewport_changes(v, &template, Point::new(100.0, 100.0))
        })
        .unwrap();
    assert_eq!(document.engine().selected_ids().len(), 2);
    let after = document.engine().document_checkpoint().unwrap();
    document.command(|e| e.undo()).unwrap();
    assert_eq!(document.engine().document_checkpoint().unwrap(), before);
    document.command(|e| e.redo()).unwrap();
    assert_eq!(document.engine().document_checkpoint().unwrap(), after);
}

#[test]
fn streaming_matches_full_pixels_across_tile_and_strip_boundaries() {
    let (mut document, mut session) = setup();
    document.set_background(Some(
        SourceImage::new(RgbaImage::from_fn(1500, 260, |x, y| {
            Rgba([
                ((x * 7 + y * 19) % 256) as u8,
                (x % 255) as u8,
                (y % 255) as u8,
                if x > 600 { 180 } else { 255 },
            ])
        }))
        .unwrap(),
    ));
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([
            {"type":"rectangle_filter","bounds":[650,50,600,180],"style":{"filter":"gaussian_blur","strength":0.3}},
            {"type":"rectangle","bounds":[1,1,2,2],"style":{"opacity":0}},
            {"type":"rectangle_filter","bounds":[650,50,600,180],"style":{"filter":"mosaic","strength":0.6}},
            {"type":"arrow","points":[[10,110],[1480,110]],"style":{"stroke_width":3}}
        ]),
    );
    let mut gpu = GpuTest::new();
    let snapshot = capture(&mut document, &mut session)
        .unwrap()
        .with_region(
            snow_draw_engine_gpui::CanvasRect::new(0.0, 0.0, 1500.0, 260.0).unwrap(),
            [1500, 260],
        )
        .unwrap();
    let frame = snapshot.viewport();
    session
        .set_surface(&mut document, frame.surface.width, frame.surface.height)
        .unwrap();
    session.set_camera(&mut document, frame.camera).unwrap();
    session.flush(&document).unwrap();
    let full = gpu
        .draw(
            session.scene(),
            document.background(),
            document.resource_revision(),
            1.25,
        )
        .unwrap();
    let mut png = Vec::new();
    gpu.write(
        &snapshot,
        1.25,
        GpuExportFormat::Png,
        &mut png,
        &Default::default(),
    )
    .unwrap();
    let streamed = image::load_from_memory(&png).unwrap().into_rgba8();
    let differences = full
        .as_raw()
        .iter()
        .zip(streamed.as_raw())
        .filter(|(a, b)| a.abs_diff(**b) > 1)
        .count();
    assert_eq!(differences, 0, "strip/tile halo pixels differ");
    assert_eq!(gpu.service.diagnostics().pending_readback_bytes, 0);
    let cancel = snow_draw_engine_gpui::CanvasCancellation::default();
    cancel.cancel();
    assert!(
        gpu.write(
            &snapshot,
            1.0,
            GpuExportFormat::Png,
            &mut Vec::new(),
            &cancel
        )
        .is_err()
    );
}

#[test]
fn tall_export_and_disjoint_tiles_do_not_allocate_the_full_source_frame() {
    let (mut document, mut session) = setup();
    annotations(
        &mut document,
        &mut session,
        serde_json::json!([
            {"type":"rectangle","bounds":[0,0,32,120],"style":{"fill":[30,100,200,255]}},
            {"type":"rectangle","bounds":[0,19880,32,120],"style":{"fill":[200,100,30,255]}}
        ]),
    );
    let mut gpu = GpuTest::new();
    let snapshot = capture(&mut document, &mut session)
        .unwrap()
        .with_region(
            snow_draw_engine_gpui::CanvasRect::new(0.0, 0.0, 32.0, 20000.0).unwrap(),
            [32, 20000],
        )
        .unwrap();
    let mut png = Vec::new();
    gpu.write(
        &snapshot,
        1.0,
        GpuExportFormat::Png,
        &mut png,
        &Default::default(),
    )
    .unwrap();
    let image = image::load_from_memory(&png).unwrap().into_rgba8();
    assert_eq!(image.dimensions(), (32, 20000));
    assert_eq!(image.get_pixel(16, 50).0, [30, 100, 200, 255]);
    assert_eq!(image.get_pixel(16, 19950).0, [200, 100, 30, 255]);
    let tiles = [
        snow_draw_engine_gpui::ExportTile {
            source: [0, 19880, 32, 120],
            destination: [0, 0],
        },
        snow_draw_engine_gpui::ExportTile {
            source: [0, 0, 32, 120],
            destination: [32, 0],
        },
    ];
    let compact = image::load_from_memory(&gpu.packed(&snapshot, 1.0, [64, 120], &tiles).unwrap())
        .unwrap()
        .into_rgba8();
    assert_eq!(compact.get_pixel(16, 50).0, [200, 100, 30, 255]);
    assert_eq!(compact.get_pixel(48, 50).0, [30, 100, 200, 255]);
    assert_eq!(gpu.service.diagnostics().pending_readback_bytes, 0);
}
