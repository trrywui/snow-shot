//! Device-backed, offscreen checks of the production GPU graph and readback.
#[path = "gpu_feature_tests.rs"]
mod features;
#[path = "gpu_semantics_tests.rs"]
mod semantics;
#[path = "gpu_source_tests.rs"]
mod source;
#[path = "gpu_watermark_area_tests.rs"]
mod watermark_area;
use crate::engine::*;
use crate::*;
use anyhow::{Context, Result, ensure};
use gpui::{GpuRect, GpuService};
use gpui_windows::gpu_extensions::OffscreenGpuHost;
use image::{Rgba, RgbaImage};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

struct Device {
    _host: OffscreenGpuHost,
    service: Arc<dyn GpuService>,
    canvas: GpuCanvas,
}
impl Device {
    fn new() -> Result<Self> {
        let host = OffscreenGpuHost::new()?;
        let service = host.service();
        let canvas = GpuCanvas::new(service.clone())?;
        Ok(Self {
            _host: host,
            service,
            canvas,
        })
    }
    fn render(
        &mut self,
        scene: &RetainedScene,
        source: Option<&SourceImage>,
        scale: f32,
        presentation: &CanvasPresentation,
    ) -> Result<(GpuCanvasFrame, RgbaImage)> {
        wait_ready(&mut self.canvas)?;
        let frame = self.canvas.prepare(
            scene,
            source,
            1,
            scale,
            presentation,
            &SmartEraseSnapshot::default(),
            &CanvasCancellation::default(),
        )?;
        let image = read_frame(&self.service, &frame)?;
        Ok((frame, image))
    }
}
fn wait_ready(canvas: &mut GpuCanvas) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !canvas.is_ready()? {
        ensure!(Instant::now() < deadline, "GPU frame retirement timed out");
        std::thread::yield_now();
    }
    Ok(())
}
pub(crate) fn read_frame(
    service: &Arc<dyn GpuService>,
    frame: &GpuCanvasFrame,
) -> Result<RgbaImage> {
    let mut output = RgbaImage::new(frame.size[0], frame.size[1]);
    for tile in frame.tiles.iter() {
        let mut readback = service.readback(
            &tile.texture,
            GpuRect {
                x: 0,
                y: 0,
                width: tile.region.width,
                height: tile.region.height,
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
        let image = RgbaImage::from_raw(image.width, image.height, image.pixels)
            .context("readback dimensions")?;
        image::imageops::replace(
            &mut output,
            &image,
            i64::from(tile.region.x),
            i64::from(tile.region.y),
        );
    }
    Ok(output)
}
pub(crate) fn view(width: u32, height: u32) -> FrameView {
    FrameView {
        surface: SurfaceSize { width, height },
        camera: Camera {
            center: Point::new(f64::from(width) / 2.0, f64::from(height) / 2.0),
            zoom: 1.0,
        },
        clear_color: ColorRgba8 {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        },
    }
}
pub(crate) fn node(
    index: u32,
    revision: u64,
    item: SceneDisplayItem,
    bounds: [f64; 4],
) -> Arc<RetainedSceneNode> {
    let item = Arc::new(item);
    Arc::new(RetainedSceneNode {
        id: RetainedNodeId {
            element: DisplayItemId {
                index,
                generation: 1,
            },
            role: RetainedNodeRole::Content,
        },
        geometry_revision: revision,
        material_revision: revision,
        pose_revision: revision,
        pose: ScenePose::default(),
        local_item: item.clone(),
        item,
        bounds,
    })
}
fn reset(frame: FrameView, nodes: Vec<Arc<RetainedSceneNode>>) -> RetainedSceneDelta {
    RetainedSceneDelta {
        base_revision: 0,
        revision: 1,
        scene_revision: 1,
        reset: true,
        frame,
        order: Some(RetainedSceneOrder {
            nodes: nodes.iter().map(|node| node.id).collect::<Vec<_>>().into(),
            effect_runs: Arc::default(),
        }),
        upserts: nodes,
        removals: Vec::new(),
        overlay_revision: 1,
        overlays: Some(Arc::default()),
        decoration_revision: 1,
        decoration: Some(DecorationView::default()),
        spotlights: Some(Arc::default()),
    }
}
pub(crate) fn scene(frame: FrameView, nodes: Vec<Arc<RetainedSceneNode>>) -> RetainedScene {
    let mut scene = RetainedScene::default();
    scene.apply_delta(&reset(frame, nodes)).unwrap();
    scene
}
fn rect(bounds: [f64; 4], color: ColorRgba8) -> SceneDisplayItem {
    SceneDisplayItem::Rectangle(RectangleDisplayItem {
        center_x: (bounds[0] + bounds[2]) / 2.0,
        center_y: (bounds[1] + bounds[3]) / 2.0,
        width: bounds[2] - bounds[0],
        height: bounds[3] - bounds[1],
        fill: color,
        fill_style: DisplayFillStyle::Solid,
        stroke: ColorRgba8 {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        },
        opacity: 1.0,
        ..Default::default()
    })
}
fn close(actual: [u8; 4], expected: [u8; 4], tolerance: u8) {
    assert!(
        actual
            .into_iter()
            .zip(expected)
            .all(|(a, b)| a.abs_diff(b) <= tolerance),
        "actual {actual:?}, expected {expected:?}"
    );
}

#[test]
fn gpu_smoke_compiles_shaders_and_preserves_premultiplied_group_alpha() -> Result<()> {
    let mut device = Device::new()?;
    let mut item = rect(
        [16.0, 16.0, 48.0, 48.0],
        ColorRgba8 {
            r: 255,
            g: 40,
            b: 20,
            a: 128,
        },
    );
    if let SceneDisplayItem::Rectangle(rect) = &mut item {
        rect.opacity = 0.5;
    }
    let scene = scene(
        view(64, 64),
        vec![node(0, 1, item, [16.0, 16.0, 48.0, 48.0])],
    );
    let (frame, image) = device.render(&scene, None, 1.0, &CanvasPresentation::default())?;
    ensure!(frame.tiles.len() == 1, "expected one tile");
    close(image.get_pixel(32, 32).0, [64, 10, 5, 64], 1);
    assert_eq!(image.get_pixel(0, 0).0, [0; 4]);
    Ok(())
}

#[test]
fn gpu_local_edit_reuses_unaffected_tiles_and_matches_a_fresh_frame() -> Result<()> {
    let mut device = Device::new()?;
    let frame = view(768, 192);
    let red = ColorRgba8 {
        r: 200,
        g: 0,
        b: 0,
        a: 180,
    };
    let blue = ColorRgba8 {
        r: 0,
        g: 0,
        b: 200,
        a: 128,
    };
    let left = node(
        0,
        1,
        rect([20.0, 20.0, 100.0, 100.0], red),
        [20.0, 20.0, 100.0, 100.0],
    );
    let right = node(
        1,
        1,
        rect([600.0, 20.0, 700.0, 100.0], blue),
        [600.0, 20.0, 700.0, 100.0],
    );
    let mut scene = scene(frame, vec![left, right]);
    let (before, _) = device.render(&scene, None, 1.0, &CanvasPresentation::default())?;
    let changed = node(
        0,
        2,
        rect([30.0, 20.0, 110.0, 100.0], red),
        [30.0, 20.0, 110.0, 100.0],
    );
    scene.apply_delta(&RetainedSceneDelta {
        base_revision: 1,
        revision: 2,
        scene_revision: 2,
        reset: false,
        upserts: vec![changed],
        order: None,
        overlays: None,
        decoration: None,
        spotlights: None,
        ..reset(frame, Vec::new())
    })?;
    let (after, incremental) = device.render(&scene, None, 1.0, &CanvasPresentation::default())?;
    assert_eq!(after.profile.rendered_tiles, 1);
    assert_eq!(
        before.tiles[2].texture.resource_id(),
        after.tiles[2].texture.resource_id()
    );
    let mut fresh = GpuCanvas::new(device.service.clone())?;
    let expected = fresh.prepare(
        &scene,
        None,
        1,
        1.0,
        &CanvasPresentation::default(),
        &SmartEraseSnapshot::default(),
        &CanvasCancellation::default(),
    )?;
    assert_eq!(incremental, read_frame(&device.service, &expected)?);
    Ok(())
}

#[test]
fn gpu_layered_sources_and_content_clipping_preserve_alpha_at_fractional_dpi() -> Result<()> {
    let red = SourceImage::new(RgbaImage::from_pixel(64, 64, Rgba([200, 0, 0, 128])))?;
    let blue = SourceImage::new(RgbaImage::from_pixel(64, 64, Rgba([0, 0, 200, 128])))?
        .with_placement(Point::new(32.0, 0.0), 1.0)?;
    let source =
        SourceImage::from_layers(&[ImageSourceLayer::new(red), ImageSourceLayer::new(blue)])?
            .context("layered source")?;
    let green = ColorRgba8 {
        r: 0,
        g: 220,
        b: 0,
        a: 255,
    };
    let scene = scene(
        view(96, 64),
        vec![node(
            0,
            1,
            rect([0.0, 0.0, 96.0, 64.0], green),
            [0.0, 0.0, 96.0, 64.0],
        )],
    );
    let presentation = CanvasPresentation {
        content_clip: Some(CanvasRect::new(40.0, 0.0, 20.0, 64.0)?),
        ..Default::default()
    };
    let mut device = Device::new()?;
    let (_, image) = device.render(&scene, Some(&source), 1.25, &presentation)?;
    assert_eq!(image.dimensions(), (120, 80));
    close(image.get_pixel(12, 20).0, [100, 0, 0, 128], 1);
    close(image.get_pixel(45, 20).0, [50, 0, 100, 192], 2);
    close(image.get_pixel(62, 20).0, [0, 220, 0, 255], 1);
    close(image.get_pixel(110, 20).0, [0, 0, 100, 128], 1);
    Ok(())
}

#[test]
fn gpu_effects_use_the_captured_input_and_preserve_transparent_pixels() -> Result<()> {
    let mut device = Device::new()?;
    let source = SourceImage::new(RgbaImage::from_pixel(96, 64, Rgba([180, 60, 30, 128])))?;
    for (kind, expected) in [
        (DisplayFilterType::Inversion, [38, 98, 113, 128]),
        (DisplayFilterType::Grayscale, [42, 42, 42, 128]),
        (DisplayFilterType::GaussianBlur, [90, 30, 15, 128]),
        (DisplayFilterType::Mosaic, [90, 30, 15, 128]),
    ] {
        let filter = FilterDisplayItem {
            center_x: 48.0,
            center_y: 32.0,
            width: 48.0,
            height: 48.0,
            opacity: 1.0,
            filter: FilterRenderSpec::resolve(kind, 0.15),
            ..Default::default()
        };
        let scene = scene(
            view(96, 64),
            vec![node(
                0,
                1,
                SceneDisplayItem::Filter(filter),
                [24.0, 8.0, 72.0, 56.0],
            )],
        );
        let (_, image) =
            device.render(&scene, Some(&source), 1.0, &CanvasPresentation::default())?;
        close(image.get_pixel(48, 32).0, expected, 3);
        close(image.get_pixel(8, 32).0, [90, 30, 15, 128], 1);
        // Distinct scenes with the same revision cannot reuse another scene's output.
        device.canvas.clear();
    }
    Ok(())
}

#[test]
fn gpu_magnifier_samples_pristine_source_above_annotations() -> Result<()> {
    let mut device = Device::new()?;
    let source = SourceImage::new(RgbaImage::from_pixel(96, 64, Rgba([220, 20, 10, 255])))?;
    let lens = MagnifierDisplayItem {
        lens: RectangleDisplayItem {
            center_x: 70.0,
            center_y: 32.0,
            width: 32.0,
            height: 32.0,
            opacity: 1.0,
            stroke_width: 0.0,
            stroke: ColorRgba8 {
                r: 0,
                g: 0,
                b: 0,
                a: 0,
            },
            ..Default::default()
        },
        source_center: Point::new(24.0, 32.0),
        source_width: 16.0,
        source_height: 16.0,
        factor: 2.0,
        connection_type: snow_draw_engine_core::MagnifierConnectionType::None,
        source_outline: false,
        leader: None,
    };
    let scene = scene(
        view(96, 64),
        vec![
            node(
                0,
                1,
                rect(
                    [0.0, 0.0, 96.0, 64.0],
                    ColorRgba8 {
                        r: 0,
                        g: 20,
                        b: 220,
                        a: 255,
                    },
                ),
                [0.0, 0.0, 96.0, 64.0],
            ),
            node(
                1,
                1,
                SceneDisplayItem::Magnifier(lens),
                [54.0, 16.0, 86.0, 48.0],
            ),
        ],
    );
    let (_, image) = device.render(&scene, Some(&source), 1.0, &CanvasPresentation::default())?;
    close(image.get_pixel(70, 32).0, [220, 20, 10, 255], 1);
    close(image.get_pixel(24, 32).0, [0, 20, 220, 255], 1);
    Ok(())
}

#[test]
fn gpu_text_outline_and_chunk_unions_apply_color_once() -> Result<()> {
    let mut device = Device::new()?;
    let mut geometry = crate::gpu_geometry::GpuGeometryCache::default();
    let text = SceneDisplayItem::Text(TextDisplayItem {
        center_x: 96.0,
        center_y: 48.0,
        width: 180.0,
        height: 60.0,
        font_size: 36.0,
        text: "office אבג".into(),
        color: ColorRgba8 {
            r: 255,
            g: 255,
            b: 255,
            a: 200,
        },
        stroke: ColorRgba8 {
            r: 255,
            g: 255,
            b: 255,
            a: 180,
        },
        stroke_width: 2.0,
        opacity: 0.4,
        ..Default::default()
    });
    let prepared = geometry.item(&text, 1.25, ColorRgba8::default())?;
    let frame = device
        .canvas
        .prepare_overlay(&prepared, view(192, 96), 1.25)?;
    let image = read_frame(&device.service, &frame)?;
    let maximum = image.pixels().map(|pixel| pixel[3]).max().unwrap();
    assert!(
        (75..=97).contains(&maximum),
        "group opacity over glyph fill/stroke: {maximum}"
    );
    let contour = |x: f64| {
        vec![
            PathCommand::MoveTo { point: [x, 20.0] },
            PathCommand::LineTo {
                point: [x + 40.0, 20.0],
            },
            PathCommand::LineTo {
                point: [x + 40.0, 60.0],
            },
            PathCommand::LineTo { point: [x, 60.0] },
        ]
    };
    let pressure = snow_draw_engine_core::PathGeometry::replace_contour_tail(
        None,
        1,
        0,
        vec![(contour(20.0), 0.0, 40.0), (contour(40.0), 40.0, 80.0)],
    );
    let item = SceneDisplayItem::Arrow(ArrowDisplayItem {
        pressure_geometry: Some(Arc::new(pressure)),
        stroke: ColorRgba8 {
            r: 255,
            g: 0,
            b: 0,
            a: 128,
        },
        opacity: 0.5,
        ..Default::default()
    });
    let prepared = geometry.item(&item, 1.0, ColorRgba8::default())?;
    let frame = device
        .canvas
        .prepare_overlay(&prepared, view(96, 80), 1.0)?;
    let image = read_frame(&device.service, &frame)?;
    close(image.get_pixel(30, 40).0, [64, 0, 0, 64], 1);
    close(image.get_pixel(50, 40).0, [64, 0, 0, 64], 1);
    close(image.get_pixel(70, 40).0, [64, 0, 0, 64], 1);
    Ok(())
}

#[test]
fn gpu_large_canvas_coordinates_remain_camera_relative() -> Result<()> {
    let mut device = Device::new()?;
    let mut geometry = crate::gpu_geometry::GpuGeometryCache::default();
    let color = ColorRgba8 {
        r: 80,
        g: 130,
        b: 220,
        a: 200,
    };
    let origin = geometry.item(
        &rect([15.25, 18.5, 45.25, 40.5], color),
        1.5,
        ColorRgba8::default(),
    )?;
    let first = device.canvas.prepare_overlay(&origin, view(64, 64), 1.5)?;
    let expected = read_frame(&device.service, &first)?;
    let offset = 1.0e9;
    let mut frame = view(64, 64);
    frame.camera.center.x += offset;
    frame.camera.center.y += offset;
    let translated = geometry.item(
        &rect(
            [offset + 15.25, offset + 18.5, offset + 45.25, offset + 40.5],
            color,
        ),
        1.5,
        ColorRgba8::default(),
    )?;
    let second = device.canvas.prepare_overlay(&translated, frame, 1.5)?;
    assert_eq!(expected, read_frame(&device.service, &second)?);
    Ok(())
}

#[test]
fn gpu_sibling_effect_runs_sample_one_immutable_source_pass() -> Result<()> {
    let mut device = Device::new()?;
    let source = SourceImage::new(RgbaImage::from_pixel(96, 64, Rgba([100, 150, 200, 255])))?;
    let filter = |kind, x| {
        SceneDisplayItem::Filter(FilterDisplayItem {
            center_x: x,
            center_y: 32.0,
            width: 64.0,
            height: 64.0,
            opacity: 1.0,
            filter: FilterRenderSpec::resolve(kind, 1.0),
            ..Default::default()
        })
    };
    let mut delta = reset(
        view(96, 64),
        vec![
            node(
                0,
                1,
                filter(DisplayFilterType::Grayscale, 32.0),
                [0.0, 0.0, 64.0, 64.0],
            ),
            node(
                1,
                1,
                filter(DisplayFilterType::Inversion, 64.0),
                [32.0, 0.0, 96.0, 64.0],
            ),
        ],
    );
    delta.order.as_mut().unwrap().effect_runs = vec![
        SceneRenderRun {
            source_pass: DisplayItemId {
                index: 10,
                generation: 1,
            },
            effect_run: DisplayItemId {
                index: 20,
                generation: 1,
            },
            start: 0,
            count: 1,
        },
        SceneRenderRun {
            source_pass: DisplayItemId {
                index: 10,
                generation: 1,
            },
            effect_run: DisplayItemId {
                index: 21,
                generation: 1,
            },
            start: 1,
            count: 1,
        },
    ]
    .into();
    let mut scene = RetainedScene::default();
    scene.apply_delta(&delta)?;
    let (_, image) = device.render(&scene, Some(&source), 1.0, &CanvasPresentation::default())?;
    close(image.get_pixel(16, 32).0, [143, 143, 143, 255], 1);
    close(image.get_pixel(48, 32).0, [155, 105, 55, 255], 1);
    Ok(())
}

#[test]
fn gpu_incremental_blur_halo_crosses_tile_boundaries_without_seams() -> Result<()> {
    let mut device = Device::new()?;
    let frame = view(512, 128);
    let red = ColorRgba8 {
        r: 255,
        g: 0,
        b: 0,
        a: 255,
    };
    let filter = SceneDisplayItem::Filter(FilterDisplayItem {
        center_x: 256.0,
        center_y: 64.0,
        width: 100.0,
        height: 100.0,
        opacity: 1.0,
        filter: FilterRenderSpec::resolve(DisplayFilterType::GaussianBlur, 0.2),
        ..Default::default()
    });
    let mut scene = scene(
        frame,
        vec![
            node(
                0,
                1,
                rect([244.0, 40.0, 252.0, 88.0], red),
                [244.0, 40.0, 252.0, 88.0],
            ),
            node(1, 1, filter, [206.0, 14.0, 306.0, 114.0]),
        ],
    );
    let (_, before) = device.render(&scene, None, 1.0, &CanvasPresentation::default())?;
    scene.apply_delta(&RetainedSceneDelta {
        base_revision: 1,
        revision: 2,
        scene_revision: 2,
        reset: false,
        upserts: vec![node(
            0,
            2,
            rect([248.0, 40.0, 256.0, 88.0], red),
            [248.0, 40.0, 256.0, 88.0],
        )],
        order: None,
        overlays: None,
        decoration: None,
        spotlights: None,
        ..reset(frame, Vec::new())
    })?;
    let (changed, actual) = device.render(&scene, None, 1.0, &CanvasPresentation::default())?;
    assert_eq!(changed.profile.rendered_tiles, 2);
    assert!(actual.get_pixel(257, 64)[3] > before.get_pixel(257, 64)[3]);
    let mut fresh = GpuCanvas::new(device.service.clone())?;
    let expected = fresh.prepare(
        &scene,
        None,
        1,
        1.0,
        &CanvasPresentation::default(),
        &SmartEraseSnapshot::default(),
        &CanvasCancellation::default(),
    )?;
    assert_eq!(actual, read_frame(&device.service, &expected)?);
    Ok(())
}

#[test]
fn gpu_cpu_smart_erase_patch_is_uploaded_without_scene_readback() -> Result<()> {
    let mut device = Device::new()?;
    let source = SourceImage::new(RgbaImage::from_pixel(96, 64, Rgba([80, 120, 160, 255])))?;
    let filter = FilterDisplayItem {
        center_x: 48.0,
        center_y: 32.0,
        width: 12.0,
        height: 12.0,
        opacity: 1.0,
        filter: FilterRenderSpec::resolve(DisplayFilterType::SmartErase, 0.5),
        ..Default::default()
    };
    let mut service = SmartEraseService::default();
    service
        .solve(&filter, &source, &CanvasCancellation::default())?
        .context("reconstruction patch")?;
    let scene = scene(
        view(96, 64),
        vec![node(
            0,
            1,
            SceneDisplayItem::Filter(filter),
            [42.0, 26.0, 54.0, 38.0],
        )],
    );
    let frame = device.canvas.prepare(
        &scene,
        Some(&source),
        1,
        1.0,
        &CanvasPresentation::default(),
        &service.snapshot(),
        &CanvasCancellation::default(),
    )?;
    assert_eq!(device.service.diagnostics().pending_readback_bytes, 0);
    let image = read_frame(&device.service, &frame)?;
    close(image.get_pixel(48, 32).0, [80, 120, 160, 255], 1);
    Ok(())
}

#[test]
fn gpu_large_sources_use_bounded_premultiplied_lod_pages_and_reuse_uploads() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let budget = 16 * 1024 * 1024;
    let mut canvas = GpuCanvas::with_memory_budget(service.clone(), budget)?;
    // 32 MiB of immutable CPU input must fit a 16 MiB GPU budget when reduced.
    // Hidden red in transparent samples must never bleed into filtered blue.
    let source = SourceImage::new(RgbaImage::from_fn(8192, 1024, |x, _| {
        if x % 2 == 0 {
            Rgba([240, 0, 0, 0])
        } else {
            Rgba([0, 0, 240, 255])
        }
    }))?;
    let mut frame = view(768, 192);
    frame.camera = Camera {
        center: Point::new(4096.0, 512.0),
        zoom: 0.09,
    };
    let mut scene = scene(frame, Vec::new());
    let first = canvas.prepare(
        &scene,
        Some(&source),
        1,
        1.0,
        &CanvasPresentation::default(),
        &SmartEraseSnapshot::default(),
        &CanvasCancellation::default(),
    )?;
    let image = read_frame(&service, &first)?;
    assert!(first.profile.peak_bytes <= budget);
    assert!(first.profile.source_upload_bytes >= 8192 * 1024 * 4);
    for x in [80, 127, 255, 367, 383, 384, 385, 511, 680] {
        close(image.get_pixel(x, 96).0, [0, 0, 120, 128], 1);
    }
    drop(first);
    wait_ready(&mut canvas)?;
    let second = canvas.prepare(
        &scene,
        Some(&source),
        1,
        1.25,
        &CanvasPresentation::default(),
        &SmartEraseSnapshot::default(),
        &CanvasCancellation::default(),
    )?;
    assert_eq!(
        second.profile.source_upload_bytes, 0,
        "DPI change within one LOD reuploaded source"
    );
    let image = read_frame(&service, &second)?;
    for x in [100, 255, 256, 479, 480, 481, 767, 850] {
        close(image.get_pixel(x, 120).0, [0, 0, 120, 128], 1);
    }
    drop(second);
    frame.camera.zoom = 0.0001;
    scene = scene.with_frame(frame);
    wait_ready(&mut canvas)?;
    let tiny = canvas.prepare(
        &scene,
        Some(&source),
        1,
        1.0,
        &CanvasPresentation::default(),
        &SmartEraseSnapshot::default(),
        &CanvasCancellation::default(),
    )?;
    let image = read_frame(&service, &tiny)?;
    assert!(
        image.pixels().all(|pixel| pixel[0] == 0),
        "transparent source RGB contaminated mip filtering"
    );
    let maximum = image.pixels().map(|pixel| pixel[3]).max().unwrap();
    assert!(
        (2..=4).contains(&maximum),
        "fractional image area coverage: {maximum}"
    );
    assert!(tiny.profile.peak_bytes <= budget);
    Ok(())
}

#[test]
fn gpu_budget_failure_preserves_published_frame_and_retries_after_release() -> Result<()> {
    let host = OffscreenGpuHost::new()?;
    let service = host.service();
    let budget = 16 * 1024 * 1024;
    let mut canvas = GpuCanvas::with_memory_budget(service.clone(), budget)?;
    let mut retained = Vec::new();
    let mut pending = None;
    for revision in 1..=8 {
        let mut frame = view(1024, 1024);
        frame.clear_color = ColorRgba8 {
            r: revision * 24,
            g: 40,
            b: 80,
            a: 255,
        };
        let scene = scene(frame, Vec::new());
        wait_ready(&mut canvas)?;
        match canvas.prepare(
            &scene,
            None,
            1,
            1.0,
            &CanvasPresentation::default(),
            &SmartEraseSnapshot::default(),
            &CanvasCancellation::default(),
        ) {
            Ok(frame) => {
                let image = read_frame(&service, &frame)?;
                close(image.get_pixel(512, 512).0, [revision * 24, 40, 80, 255], 0);
                retained.push(frame);
            }
            Err(error) => {
                ensure!(
                    error.to_string().contains("budget"),
                    "unexpected failure: {error:#}"
                );
                pending = Some((scene, revision));
                break;
            }
        }
    }
    let (pending, revision) =
        pending.context("retained frames did not exercise budget pressure")?;
    let published = retained.last().context("initial GPU frame")?;
    let image = read_frame(&service, published)?;
    close(
        image.get_pixel(512, 512).0,
        [(revision - 1) * 24, 40, 80, 255],
        0,
    );
    retained.clear();
    wait_ready(&mut canvas)?;
    let retry = canvas.prepare(
        &pending,
        None,
        1,
        1.0,
        &CanvasPresentation::default(),
        &SmartEraseSnapshot::default(),
        &CanvasCancellation::default(),
    )?;
    let image = read_frame(&service, &retry)?;
    close(image.get_pixel(512, 512).0, [revision * 24, 40, 80, 255], 0);
    assert_eq!(retry.profile.rendered_tiles, 16);
    assert!(retry.profile.peak_bytes <= budget);
    Ok(())
}

#[test]
fn gpu_stroke_tail_reuses_prefix_meshes_and_repaints_only_local_tiles() -> Result<()> {
    let mut device = Device::new()?;
    let frame = view(4096, 256);
    let commands: Vec<_> = (0..2048)
        .map(|index| {
            let point = [10.0 + f64::from(index) * 1.95, 64.0];
            if index == 0 {
                PathCommand::MoveTo { point }
            } else {
                PathCommand::LineTo { point }
            }
        })
        .collect();
    let mut stroke = ArrowDisplayItem {
        geometry: Arc::new(snow_draw_engine_core::PathGeometry::from_commands(
            1,
            commands.clone(),
            false,
        )),
        stroke: ColorRgba8 {
            r: 240,
            g: 50,
            b: 80,
            a: 128,
        },
        stroke_width: 6.0,
        is_free_draw: true,
        opacity: 0.7,
        fill: ColorRgba8::default(),
        fill_style: DisplayFillStyle::Border,
        ..Default::default()
    };
    let mut scene = scene(
        frame,
        vec![node(
            0,
            1,
            SceneDisplayItem::Arrow(stroke.clone()),
            [6.0, 60.0, 4006.0, 68.0],
        )],
    );
    let (before, _) = device.render(&scene, None, 1.0, &CanvasPresentation::default())?;
    let mut tail = commands[1984..].to_vec();
    *tail.last_mut().unwrap() = PathCommand::LineTo {
        point: [4001.65, 96.0],
    };
    stroke.geometry = Arc::new(
        stroke
            .geometry
            .replace_tail(2, 1984, [3876.85, 64.0], tail, false),
    );
    let mut changed = node(
        0,
        2,
        SceneDisplayItem::Arrow(stroke),
        [6.0, 60.0, 4006.0, 100.0],
    );
    Arc::make_mut(&mut changed).material_revision = 1;
    Arc::make_mut(&mut changed).pose_revision = 1;
    scene.apply_delta(&RetainedSceneDelta {
        base_revision: 1,
        revision: 2,
        scene_revision: 2,
        reset: false,
        upserts: vec![changed],
        order: None,
        overlays: None,
        decoration: None,
        spotlights: None,
        ..reset(frame, Vec::new())
    })?;
    let (after, actual) = device.render(&scene, None, 1.0, &CanvasPresentation::default())?;
    assert_eq!(after.profile.rendered_tiles, 1);
    assert_eq!(after.profile.rebuilt_geometry, 1);
    assert!(
        after.profile.submitted_draws <= 3,
        "unchanged distant chunks were submitted: {}",
        after.profile.submitted_draws
    );
    assert_eq!(
        before.tiles[0].texture.resource_id(),
        after.tiles[0].texture.resource_id()
    );
    let mut fresh = GpuCanvas::new(device.service.clone())?;
    let expected = fresh.prepare(
        &scene,
        None,
        1,
        1.0,
        &CanvasPresentation::default(),
        &SmartEraseSnapshot::default(),
        &CanvasCancellation::default(),
    )?;
    assert_eq!(actual, read_frame(&device.service, &expected)?);
    Ok(())
}

#[test]
fn gpu_zoom_out_reduces_resident_source_pages_without_reupload() -> Result<()> {
    let mut device = Device::new()?;
    let source = SourceImage::new(RgbaImage::from_fn(1027, 1027, |x, y| {
        if (x + y) % 2 == 0 {
            Rgba([200, 30, 90, 0])
        } else {
            Rgba([20, 140, 240, 255])
        }
    }))?;
    let mut frame = view(1100, 1100);
    frame.camera.center = Point::new(513.5, 513.5);
    let mut scene = scene(frame, Vec::new());
    let (base, _) = device.render(&scene, Some(&source), 1.0, &CanvasPresentation::default())?;
    assert!(base.profile.source_upload_bytes >= 1027 * 1027 * 4);
    drop(base);
    frame.camera.zoom = 0.25;
    scene = scene.with_frame(frame);
    let (reduced, actual) =
        device.render(&scene, Some(&source), 1.25, &CanvasPresentation::default())?;
    assert_eq!(
        reduced.profile.source_upload_bytes, 0,
        "resident source pages were uploaded again for a new LOD"
    );
    close(actual.get_pixel(687, 687).0, [10, 70, 120, 128], 1);
    let mut fresh = GpuCanvas::new(device.service.clone())?;
    let expected = fresh.prepare(
        &scene,
        Some(&source),
        1,
        1.25,
        &CanvasPresentation::default(),
        &SmartEraseSnapshot::default(),
        &CanvasCancellation::default(),
    )?;
    assert_eq!(actual, read_frame(&device.service, &expected)?);
    Ok(())
}

#[test]
fn gpu_color_emoji_keeps_native_palette_alpha_and_pose() -> Result<()> {
    let mut device = Device::new()?;
    let item = TextDisplayItem {
        center_x: 128.0,
        center_y: 128.0,
        width: 150.0,
        height: 150.0,
        text: "😀".into(),
        font_family: Some("Segoe UI Emoji".into()),
        font_size: 96.0,
        color: ColorRgba8 {
            r: 220,
            g: 20,
            b: 30,
            a: 255,
        },
        opacity: 0.5,
        horizontal_align: DisplayTextHorizontalAlign::Center,
        vertical_align: DisplayTextVerticalAlign::Center,
        ..Default::default()
    };
    let mut scene = scene(
        view(256, 256),
        vec![node(
            0,
            1,
            SceneDisplayItem::Text(item.clone()),
            [40.0, 40.0, 216.0, 216.0],
        )],
    );
    let (initial, before) = device.render(&scene, None, 1.0, &CanvasPresentation::default())?;
    assert!(
        before
            .pixels()
            .any(|pixel| pixel[0] > 80 && pixel[1] > 50 && pixel[2] < 40),
        "native emoji yellow palette missing"
    );
    assert!(before.pixels().filter(|pixel| pixel[3] > 0).count() > 1000);
    assert!(
        before.pixels().all(|pixel| pixel[3] <= 129),
        "color layer opacity was applied repeatedly"
    );
    let moved = TextDisplayItem {
        center_x: 134.0,
        center_y: 125.0,
        rotation: 0.21,
        opacity: 0.8,
        ..item
    };
    scene.apply_delta(&RetainedSceneDelta {
        base_revision: 1,
        revision: 2,
        scene_revision: 2,
        reset: false,
        upserts: vec![node(
            0,
            2,
            SceneDisplayItem::Text(moved),
            [20.0, 20.0, 236.0, 236.0],
        )],
        order: None,
        overlays: None,
        decoration: None,
        spotlights: None,
        ..reset(view(256, 256), Vec::new())
    })?;
    let (moved_frame, actual) = device.render(&scene, None, 1.0, &CanvasPresentation::default())?;
    assert!(initial.profile.color_glyph_raster_pages > 0);
    assert_eq!(
        moved_frame.profile.color_glyph_raster_pages, 0,
        "pose/opacity edit rerasterized the glyph"
    );
    assert_ne!(actual, before);
    let mut fresh = GpuCanvas::new(device.service.clone())?;
    let expected = fresh.prepare(
        &scene,
        None,
        1,
        1.0,
        &CanvasPresentation::default(),
        &SmartEraseSnapshot::default(),
        &CanvasCancellation::default(),
    )?;
    assert_eq!(actual, read_frame(&device.service, &expected)?);
    Ok(())
}

#[test]
fn gpu_oversized_color_glyph_rasterizes_only_visible_pages_without_holes() -> Result<()> {
    let mut device = Device::new()?;
    let item = TextDisplayItem {
        center_x: 128.0,
        center_y: 128.0,
        width: 150.0,
        height: 150.0,
        text: "😀".into(),
        font_family: Some("Segoe UI Emoji".into()),
        font_size: 96.0,
        color: ColorRgba8 {
            r: 20,
            g: 30,
            b: 40,
            a: 255,
        },
        opacity: 1.0,
        horizontal_align: DisplayTextHorizontalAlign::Center,
        vertical_align: DisplayTextVerticalAlign::Center,
        ..Default::default()
    };
    let small = scene(
        view(256, 256),
        vec![node(
            0,
            1,
            SceneDisplayItem::Text(item.clone()),
            [30.0, 30.0, 226.0, 226.0],
        )],
    );
    let (_, image) = device.render(&small, None, 1.0, &CanvasPresentation::default())?;
    let (x, y, color) = image
        .enumerate_pixels()
        .filter(|(_, _, pixel)| {
            pixel[3] == 255 && pixel[0] > 180 && pixel[1] > 100 && pixel[2] < 80
        })
        .min_by_key(|(x, y, _)| x.abs_diff(128) + y.abs_diff(128))
        .context("opaque palette sample")?;
    let factor = 20_000.0 / 96.0;
    let mut frame = view(256, 256);
    frame.camera.center = Point::new(
        128.0 + (f64::from(x) + 0.5 - 128.0) * factor,
        128.0 + (f64::from(y) + 0.5 - 128.0) * factor,
    );
    let large = scene(
        frame,
        vec![node(
            0,
            1,
            SceneDisplayItem::Text(TextDisplayItem {
                font_size: 20_000.0,
                width: item.width * factor,
                height: item.height * factor,
                ..item
            }),
            [-20_000.0, -20_000.0, 20_000.0, 20_000.0],
        )],
    );
    let (rendered, image) = device.render(&large, None, 1.0, &CanvasPresentation::default())?;
    assert!(
        rendered.profile.color_glyph_raster_pages > 0
            && rendered.profile.color_glyph_raster_pages <= 9,
        "oversized glyph rasterized {} pages",
        rendered.profile.color_glyph_raster_pages
    );
    assert!(
        image.pixels().all(|pixel| pixel[3] >= 250),
        "paged native glyph has coverage holes"
    );
    close(image.get_pixel(128, 128).0, color.0, 18);
    assert!(rendered.profile.peak_bytes < 64 * 1024 * 1024);
    Ok(())
}
