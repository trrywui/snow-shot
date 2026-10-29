use super::*;

#[test]
fn fractional_host_and_source_coverage_clips_exclude_outside_pixel_centers() -> Result<()> {
    let mut device = Device::new()?;
    let area = CanvasRect::new(8.6, 8.6, 47.6, 47.6)?;
    let green = ColorRgba8 {
        r: 0,
        g: 220,
        b: 0,
        a: 255,
    };
    for kind in 0..4 {
        let mut delta = reset(view(96, 96), Vec::new());
        let mut presentation = CanvasPresentation::default();
        let source =
            match kind {
                0 => {
                    delta = reset(
                        view(96, 96),
                        vec![node(
                            0,
                            1,
                            rect([0.0, 0.0, 96.0, 96.0], green),
                            [0.0, 0.0, 96.0, 96.0],
                        )],
                    );
                    presentation.content_clip = Some(area);
                    None
                }
                1 => {
                    presentation.spotlight_area = Some(area);
                    delta.decoration.as_mut().unwrap().spotlight = DisplaySpotlightConfig {
                        active: true,
                        color: green,
                        opacity: 1.0,
                    };
                    None
                }
                2 => {
                    presentation.watermark_area = Some(area);
                    let watermark = &mut delta.decoration.as_mut().unwrap().watermark;
                    watermark.text[0] = b'W';
                    watermark.text_len = 1;
                    watermark.font_size = 12.0;
                    watermark.gap = 0.0;
                    watermark.angle = 23.0;
                    watermark.color = green;
                    watermark.opacity = 1.0;
                    None
                }
                _ => {
                    let mut layer = ImageSourceLayer::new(SourceImage::new(
                        RgbaImage::from_pixel(96, 96, Rgba([0, 220, 0, 255])),
                    )?);
                    layer.coverage = Some(area);
                    SourceImage::from_layers(&[layer])?
                }
            };
        let mut scene = RetainedScene::default();
        scene.apply_delta(&delta)?;
        let (_, image) = device.render(&scene, source.as_ref(), 1.25, &presentation)?;
        let mut painted = 0;
        for (x, y, pixel) in image.enumerate_pixels() {
            if !(11..70).contains(&x) || !(11..70).contains(&y) {
                assert_eq!(pixel.0, [0; 4], "clip kind {kind} escaped at {x},{y}");
            } else if pixel[3] > 0 {
                painted += 1;
                if kind != 2 {
                    close(pixel.0, [0, 220, 0, 255], 0);
                }
            }
        }
        assert!(painted > 20, "clip kind {kind} painted nothing");
        device.canvas.clear();
    }
    Ok(())
}

#[test]
fn rotated_magnifier_bridge_and_source_outline_preserve_pristine_sampling() -> Result<()> {
    let mut device = Device::new()?;
    let source = SourceImage::new(RgbaImage::from_pixel(512, 160, Rgba([220, 20, 10, 255])))?;
    let annotation = ColorRgba8 {
        r: 0,
        g: 20,
        b: 220,
        a: 255,
    };
    let mut lens = MagnifierDisplayItem {
        lens: RectangleDisplayItem {
            center_x: 300.0,
            center_y: 70.0,
            width: 80.0,
            height: 48.0,
            rotation: std::f64::consts::FRAC_PI_4,
            opacity: 1.0,
            stroke_width: 3.0,
            stroke: ColorRgba8 {
                r: 20,
                g: 220,
                b: 40,
                a: 255,
            },
            ..Default::default()
        },
        source_center: Point::new(60.0, 70.0),
        source_width: 40.0,
        source_height: 24.0,
        factor: 2.0,
        connection_type: snow_draw_engine_core::MagnifierConnectionType::None,
        source_outline: false,
        leader: None,
    };
    let mut scene = scene(
        view(512, 160),
        vec![
            node(
                0,
                1,
                rect([0.0, 0.0, 512.0, 160.0], annotation),
                [0.0, 0.0, 512.0, 160.0],
            ),
            node(
                1,
                1,
                SceneDisplayItem::Magnifier(lens.clone()),
                [20.0, 10.0, 370.0, 135.0],
            ),
        ],
    );
    let scale = 1.25;
    let (_, plain) = device.render(&scene, Some(&source), scale, &Default::default())?;
    lens.connection_type = snow_draw_engine_core::MagnifierConnectionType::Gradient;
    lens.source_outline = true;
    scene.apply_delta(&RetainedSceneDelta {
        base_revision: 1,
        revision: 2,
        scene_revision: 2,
        reset: false,
        upserts: vec![node(
            1,
            2,
            SceneDisplayItem::Magnifier(lens.clone()),
            [20.0, 10.0, 370.0, 135.0],
        )],
        order: None,
        overlays: None,
        decoration: None,
        spotlights: None,
        ..reset(view(512, 160), Vec::new())
    })?;
    let (_, decorated) = device.render(&scene, Some(&source), scale, &Default::default())?;
    let at =
        |image: &RgbaImage, x: f64, y: f64| image.get_pixel((x * 1.25) as u32, (y * 1.25) as u32).0;
    close(at(&decorated, 300.0, 70.0), [220, 20, 10, 255], 1);
    close(at(&decorated, 321.0, 91.0), [220, 20, 10, 255], 1);
    close(at(&decorated, 335.0, 35.0), [0, 20, 220, 255], 1);
    close(at(&decorated, 60.0, 70.0), [0, 20, 220, 255], 1);
    assert!(at(&decorated, 170.0, 70.0)[1] > at(&plain, 170.0, 70.0)[1] + 10);
    let outlined = (45..110)
        .flat_map(|y| (35..115).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            decorated.get_pixel(x, y)[1] > plain.get_pixel(x, y)[1].saturating_add(40)
        })
        .count();
    assert!(outlined > 30, "rotated source outline is absent");
    let mut fresh = GpuCanvas::new(device.service.clone())?;
    let expected = fresh.prepare(
        &scene,
        Some(&source),
        1,
        scale,
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )?;
    assert_eq!(decorated, read_frame(&device.service, &expected)?);
    Ok(())
}

#[test]
fn chained_source_pass_halos_match_shifted_tile_grid_and_incremental_damage() -> Result<()> {
    let mut device = Device::new()?;
    let red = ColorRgba8 {
        r: 255,
        g: 0,
        b: 0,
        a: 255,
    };
    for scale in [1.0, 1.25] {
        let frame = view(512, 128);
        let blur = |strength| {
            SceneDisplayItem::Filter(FilterDisplayItem {
                center_x: 256.0,
                center_y: 64.0,
                width: 128.0,
                height: 128.0,
                opacity: 1.0,
                filter: FilterRenderSpec::resolve(DisplayFilterType::GaussianBlur, strength),
                ..Default::default()
            })
        };
        let mut delta = reset(
            frame,
            vec![
                node(
                    0,
                    1,
                    rect([234.0, 0.0, 250.0, 128.0], red),
                    [234.0, 0.0, 250.0, 128.0],
                ),
                node(1, 1, blur(0.5), [192.0, 0.0, 320.0, 128.0]),
                node(
                    2,
                    1,
                    rect([240.0, 60.0, 241.0, 61.0], ColorRgba8::default()),
                    [240.0, 60.0, 241.0, 61.0],
                ),
                node(3, 1, blur(0.6), [192.0, 0.0, 320.0, 128.0]),
            ],
        );
        delta.order.as_mut().unwrap().effect_runs = [1, 3]
            .map(|index| SceneRenderRun {
                source_pass: DisplayItemId {
                    index,
                    generation: 1,
                },
                effect_run: DisplayItemId {
                    index,
                    generation: 1,
                },
                start: index,
                count: 1,
            })
            .into();
        let mut scene = RetainedScene::default();
        scene.apply_delta(&delta)?;
        let (_, before) = device.render(&scene, None, scale, &Default::default())?;
        scene.apply_delta(&RetainedSceneDelta {
            base_revision: 1,
            revision: 2,
            scene_revision: 2,
            reset: false,
            upserts: vec![node(
                0,
                2,
                rect([238.0, 0.0, 254.0, 128.0], red),
                [238.0, 0.0, 254.0, 128.0],
            )],
            order: None,
            overlays: None,
            decoration: None,
            spotlights: None,
            ..reset(frame, Vec::new())
        })?;
        let (changed, actual) = device.render(&scene, None, scale, &Default::default())?;
        assert_eq!(changed.profile.rendered_tiles, 2);
        let sample = ((260.0 * scale) as u32, (64.0 * scale) as u32);
        assert!(actual.get_pixel(sample.0, sample.1)[3] > before.get_pixel(sample.0, sample.1)[3]);
        let mut fresh = GpuCanvas::new(device.service.clone())?;
        let expected = fresh.prepare(
            &scene,
            None,
            1,
            scale,
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )?;
        assert_eq!(actual, read_frame(&device.service, &expected)?);

        // Shift the tile origin by 128 canvas units. The full-view tile seam now
        // sits inside the reference tile, exposing incomplete chained halos.
        let shifted = scene.with_frame(FrameView {
            surface: SurfaceSize {
                width: 256,
                height: 128,
            },
            ..frame
        });
        wait_ready(&mut fresh)?;
        let expected = fresh.prepare(
            &shifted,
            None,
            1,
            scale,
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )?;
        let expected = read_frame(&device.service, &expected)?;
        let offset = (128.0 * scale) as u32;
        for y in (40.0 * scale) as u32..(88.0 * scale) as u32 {
            for x in (196.0 * scale) as u32..(316.0 * scale) as u32 {
                close(
                    actual.get_pixel(x, y).0,
                    expected.get_pixel(x - offset, y).0,
                    1,
                );
            }
        }
        drop(fresh);
        device.canvas.clear();
    }
    Ok(())
}
