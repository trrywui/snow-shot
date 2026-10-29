use super::*;

#[test]
fn host_coverage_mask_restores_backdrop_and_tracks_presentation_changes() -> Result<()> {
    let source = SourceImage::new(RgbaImage::from_pixel(64, 48, Rgba([20, 80, 180, 255])))?;
    let mask = SourceImage::new(RgbaImage::from_fn(64, 48, |x, y| {
        Rgba([
            255,
            255,
            255,
            if x < 16 || ((40..44).contains(&x) && (20..24).contains(&y)) {
                0
            } else if x < 32 {
                128
            } else {
                255
            },
        ])
    }))?;
    let scene = scene(
        view(64, 48),
        vec![node(
            0,
            1,
            rect(
                [0., 0., 64., 48.],
                ColorRgba8 {
                    r: 220,
                    g: 40,
                    b: 60,
                    a: 255,
                },
            ),
            [0., 0., 64., 48.],
        )],
    );
    let mut device = Device::new()?;
    for scale in [1., 1.5, 2.] {
        let presentation = CanvasPresentation {
            content_mask: Some(mask.clone()),
            ..Default::default()
        };
        let (_, image) = device.render(&scene, Some(&source), scale, &presentation)?;
        let at = |x: f32, y: f32| image.get_pixel((x * scale) as u32, (y * scale) as u32).0;
        close(at(8., 8.), [20, 80, 180, 255], 0);
        close(at(24., 8.), [120, 60, 120, 255], 1);
        close(at(48., 8.), [220, 40, 60, 255], 0);
        close(at(42., 22.), [20, 80, 180, 255], 0);
        let presentation = CanvasPresentation {
            background_tint: Some(ColorRgba8 {
                r: 0,
                g: 0,
                b: 0,
                a: 128,
            }),
            ..presentation
        };
        let (_, image) = device.render(&scene, Some(&source), scale, &presentation)?;
        close(
            image.get_pixel((8. * scale) as u32, (8. * scale) as u32).0,
            [10, 40, 90, 255],
            1,
        );
        let (_, image) = device.render(&scene, Some(&source), scale, &Default::default())?;
        close(
            image.get_pixel((8. * scale) as u32, (8. * scale) as u32).0,
            [220, 40, 60, 255],
            0,
        );
    }
    Ok(())
}

#[test]
fn invisible_text_lines_do_not_submit_coverage_passes() -> Result<()> {
    for scale in [1.0, 1.5, 2.0] {
        for stroke_width in [0.0, 1.5] {
            let mut device = Device::new()?;
            let item = TextDisplayItem {
                center_x: 64.0,
                center_y: 2500.0,
                width: 120.0,
                height: 5000.0,
                // Both variants extend beyond the viewport, keeping the
                // intermediate surface bounds and sampling grid identical.
                text: ["Label"; 8].join("\n"),
                font_size: 20.0,
                color: ColorRgba8 {
                    r: 60,
                    g: 120,
                    b: 200,
                    a: 180,
                },
                stroke: ColorRgba8 {
                    r: 200,
                    g: 60,
                    b: 30,
                    a: 160,
                },
                stroke_width,
                opacity: 0.6,
                vertical_align: DisplayTextVerticalAlign::Top,
                ..Default::default()
            };
            let make_scene = |item| {
                scene(
                    view(128, 80),
                    vec![node(
                        0,
                        1,
                        SceneDisplayItem::Text(item),
                        [0.0, 0.0, 128.0, 5000.0],
                    )],
                )
            };
            let (reference, expected) =
                device.render(&make_scene(item.clone()), None, scale, &Default::default())?;
            let long = TextDisplayItem {
                text: vec!["Label"; 128].join("\n"),
                ..item
            };
            let (frame, actual) =
                device.render(&make_scene(long), None, scale, &Default::default())?;
            assert!(expected.pixels().any(|pixel| pixel[3] != 0));
            assert!(
                actual.pixels().zip(expected.pixels()).all(|(a, b)| a
                    .0
                    .into_iter()
                    .zip(b.0)
                    .all(|(a, b)| a.abs_diff(b) <= 1)),
                "invisible lines changed visible text at scale {scale}, stroke {stroke_width}"
            );
            assert_eq!(
                frame.profile.uniform_upload_bytes, reference.profile.uniform_upload_bytes,
                "offscreen lines submitted extra coverage passes at scale {scale}, stroke {stroke_width}"
            );
        }
    }
    Ok(())
}

#[test]
fn camera_translation_reuses_local_geometry_and_matches_full_rendering() -> Result<()> {
    let mut device = Device::new()?;
    let original = scene(
        view(128, 96),
        vec![node(
            0,
            1,
            rect(
                [20.0, 20.0, 80.0, 60.0],
                ColorRgba8 {
                    r: 40,
                    g: 120,
                    b: 200,
                    a: 255,
                },
            ),
            [20.0, 20.0, 80.0, 60.0],
        )],
    );
    device.render(&original, None, 1.25, &Default::default())?;
    let mut frame = original.frame();
    frame.camera.center.x += 7.25;
    frame.camera.center.y -= 3.5;
    let moved = original.with_frame(frame);
    let (retained, image) = device.render(&moved, None, 1.25, &Default::default())?;
    assert_eq!(retained.profile.rebuilt_geometry, 0);
    assert_eq!(retained.profile.mesh_upload_bytes, 0);
    let mut full = GpuCanvas::new(device.service.clone())?;
    let expected = full.prepare(
        &moved,
        None,
        1,
        1.25,
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )?;
    assert_eq!(image, read_frame(&device.service, &expected)?);
    Ok(())
}

#[test]
fn restore_replaces_transparent_channels_and_color_filters_keep_alpha() -> Result<()> {
    let mut device = Device::new()?;
    let source = SourceImage::new(RgbaImage::from_fn(96, 64, |x, _| {
        if x < 48 {
            Rgba([240, 80, 20, 0])
        } else {
            Rgba([180, 60, 30, 128])
        }
    }))?;
    for (kind, strength, expected) in [
        (DisplayFilterType::RestoreBackground, 1.0, [90, 30, 15, 128]),
        (DisplayFilterType::Brightness, 0.15, [27, 9, 5, 128]),
        (DisplayFilterType::Brightness, 0.75, [109, 79, 71, 128]),
        (DisplayFilterType::Emboss, 0.5, [64, 64, 64, 128]),
    ] {
        let mut nodes = Vec::new();
        if kind == DisplayFilterType::RestoreBackground {
            nodes.push(node(
                0,
                1,
                rect(
                    [0.0, 0.0, 96.0, 64.0],
                    ColorRgba8 {
                        r: 0,
                        g: 240,
                        b: 0,
                        a: 255,
                    },
                ),
                [0.0, 0.0, 96.0, 64.0],
            ));
        }
        nodes.push(node(
            1,
            1,
            SceneDisplayItem::Filter(FilterDisplayItem {
                center_x: 48.0,
                center_y: 32.0,
                width: 96.0,
                height: 64.0,
                opacity: 1.0,
                filter: FilterRenderSpec::resolve(kind, strength),
                ..Default::default()
            }),
            [0.0, 0.0, 96.0, 64.0],
        ));
        let scene = scene(
            FrameView {
                clear_color: ColorRgba8 {
                    r: 30,
                    g: 60,
                    b: 90,
                    a: 255,
                },
                ..view(96, 64)
            },
            nodes,
        );
        let presentation = CanvasPresentation {
            clear_background: false,
            ..Default::default()
        };
        let (_, image) = device.render(&scene, Some(&source), 1.25, &presentation)?;
        close(image.get_pixel(30, 40).0, [0, 0, 0, 0], 0);
        close(image.get_pixel(90, 40).0, expected, 2);
        device.canvas.clear();
    }
    Ok(())
}

#[test]
fn spotlight_watermark_and_host_layers_obey_order_and_clips() -> Result<()> {
    let mut device = Device::new()?;
    let source = SourceImage::new(RgbaImage::from_pixel(320, 192, Rgba([200, 120, 80, 255])))?;
    let mut delta = reset(view(320, 192), vec![]);
    delta.decoration.as_mut().unwrap().spotlight = DisplaySpotlightConfig {
        active: true,
        color: ColorRgba8 {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        },
        opacity: 0.5,
    };
    delta.spotlights = Some(
        vec![DisplaySpotlightCutout {
            center_x: 80.0,
            center_y: 96.0,
            width: 64.0,
            height: 64.0,
            shape: DisplayRectangleShape::Ellipse,
            ..Default::default()
        }]
        .into(),
    );
    let mut scene = RetainedScene::default();
    scene.apply_delta(&delta)?;
    let presentation = CanvasPresentation {
        spotlight_area: Some(CanvasRect::new(0.0, 0.0, 240.0, 192.0)?),
        ..Default::default()
    };
    let (_, baseline) = device.render(&scene, Some(&source), 1.25, &presentation)?;
    close(baseline.get_pixel(100, 120).0, [200, 120, 80, 255], 0);
    close(baseline.get_pixel(200, 120).0, [100, 60, 40, 255], 1);
    close(baseline.get_pixel(350, 120).0, [200, 120, 80, 255], 0);
    let watermark = &mut delta.decoration.as_mut().unwrap().watermark;
    watermark.text[..4].copy_from_slice(b"SNOW");
    watermark.text_len = 4;
    watermark.font_size = 18.0;
    watermark.gap = 10.0;
    watermark.angle = 27.0;
    watermark.color = ColorRgba8 {
        r: 240,
        g: 240,
        b: 240,
        a: 255,
    };
    watermark.opacity = 0.75;
    delta.revision = 2;
    delta.decoration_revision = 2;
    scene.apply_delta(&delta)?;
    let presentation = CanvasPresentation {
        watermark_area: Some(CanvasRect::new(96.0, 32.0, 128.0, 128.0)?),
        ..presentation
    };
    let (_, marked) = device.render(&scene, Some(&source), 1.25, &presentation)?;
    let mut changed = 0;
    for (x, y, pixel) in marked.enumerate_pixels() {
        if pixel != baseline.get_pixel(x, y) {
            assert!(
                (120..280).contains(&x) && (40..200).contains(&y),
                "watermark escaped its clip at {x},{y}"
            );
            changed += 1;
        }
    }
    assert!(changed > 100, "watermark glyphs did not render");
    let green = SourceImage::new(RgbaImage::from_pixel(24, 24, Rgba([0, 240, 0, 255])))?
        .with_placement(Point::new(120.0, 60.0), 1.0)?;
    let presentation = CanvasPresentation {
        after: Some(green),
        ..presentation
    };
    let (_, image) = device.render(&scene, Some(&source), 1.25, &presentation)?;
    close(image.get_pixel(160, 90).0, [0, 240, 0, 255], 0);
    let mut full = GpuCanvas::new(device.service.clone())?;
    let frame = full.prepare(
        &scene,
        Some(&source),
        1,
        1.25,
        &presentation,
        &Default::default(),
        &Default::default(),
    )?;
    assert_eq!(image, read_frame(&device.service, &frame)?);
    Ok(())
}

#[test]
fn shape_patterns_and_seeded_rough_styles_are_repeatable_on_the_device() -> Result<()> {
    let mut nodes = Vec::new();
    for (row, style) in [
        StrokeRenderingStyle::Architect,
        StrokeRenderingStyle::Artist,
        StrokeRenderingStyle::Cartoonist,
    ]
    .into_iter()
    .enumerate()
    {
        for (column, shape) in [
            DisplayRectangleShape::Rectangle,
            DisplayRectangleShape::Ellipse,
            DisplayRectangleShape::Diamond,
        ]
        .into_iter()
        .enumerate()
        {
            let left = column as f64 * 96.0 + 16.0;
            let top = row as f64 * 96.0 + 16.0;
            let item = RectangleDisplayItem {
                center_x: left + 32.0,
                center_y: top + 32.0,
                width: 64.0,
                height: 64.0,
                shape,
                rotation: 0.13,
                fill: ColorRgba8 {
                    r: 40,
                    g: 140,
                    b: 220,
                    a: 255,
                },
                fill_style: [
                    DisplayFillStyle::Solid,
                    DisplayFillStyle::Line,
                    DisplayFillStyle::CrossLine,
                ][column],
                stroke: ColorRgba8 {
                    r: 180,
                    g: 40,
                    b: 60,
                    a: 255,
                },
                stroke_width: 3.0,
                stroke_rendering_style: style,
                stroke_rendering_seed: 37,
                opacity: 0.7,
                corner_radii: CornerRadii {
                    top_left: 4.0,
                    top_right: 8.0,
                    bottom_left: 12.0,
                    bottom_right: 16.0,
                },
                ..Default::default()
            };
            nodes.push(node(
                (row * 3 + column) as u32,
                1,
                SceneDisplayItem::Rectangle(item),
                [left - 12.0, top - 12.0, left + 76.0, top + 76.0],
            ));
        }
    }
    let scene = scene(view(288, 288), nodes);
    let mut device = Device::new()?;
    let (_, first) = device.render(&scene, None, 1.5, &Default::default())?;
    for row in 0..3 {
        for column in 0..3 {
            let covered = first
                .enumerate_pixels()
                .filter(|(x, y, p)| x / 144 == column && y / 144 == row && p[3] > 0)
                .count();
            assert!(covered > 200, "missing shape/style family {row},{column}");
        }
    }
    device.canvas.clear();
    let (_, second) = device.render(&scene, None, 1.5, &Default::default())?;
    assert_eq!(
        first, second,
        "seeded rough styles changed after cache invalidation"
    );
    Ok(())
}
