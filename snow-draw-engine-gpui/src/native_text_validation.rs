//! Focused real-window text interaction and GPU readback validation.
use super::*;
use anyhow::{Result, ensure};

async fn settle(view: &Entity<CanvasView>, cx: &mut AsyncApp) -> Result<()> {
    let start = Instant::now();
    loop {
        cx.background_executor()
            .timer(Duration::from_millis(30))
            .await;
        if view.read_with(cx, |view, _| !view.is_rendering()) {
            return Ok(());
        }
        ensure!(
            start.elapsed() < Duration::from_secs(30),
            "Text canvas did not settle"
        );
    }
}

fn click(window: &mut Window, cx: &mut App, position: Point<Pixels>) {
    for input in [
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            ..Default::default()
        }),
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            click_count: 1,
            ..Default::default()
        }),
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            click_count: 1,
            ..Default::default()
        }),
    ] {
        window.dispatch_event(input, cx);
    }
}

fn key(window: &mut Window, cx: &mut App, key: &'static str) {
    window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
}

fn capture(
    window: AnyWindowHandle,
    directory: &std::path::Path,
    name: &str,
    cx: &mut AsyncApp,
) -> Result<image::RgbaImage> {
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        let image = window.render_to_image()?;
        image.save(directory.join(name))?;
        Ok::<_, anyhow::Error>(image)
    })?
}

fn pixel_region(
    bounds: Bounds<Pixels>,
    scale: f32,
    image: &image::RgbaImage,
    padding: f32,
) -> (u32, u32, u32, u32) {
    (
        ((f32::from(bounds.left()) - padding) * scale)
            .floor()
            .max(0.0) as u32,
        ((f32::from(bounds.top()) - padding) * scale)
            .floor()
            .max(0.0) as u32,
        (((f32::from(bounds.right()) + padding) * scale)
            .ceil()
            .max(0.0) as u32)
            .min(image.width()),
        (((f32::from(bounds.bottom()) + padding) * scale)
            .ceil()
            .max(0.0) as u32)
            .min(image.height()),
    )
}

pub(super) async fn run(
    window: AnyWindowHandle,
    demo: Entity<Demo>,
    directory: PathBuf,
    cx: &mut AsyncApp,
) -> Result<()> {
    std::fs::create_dir_all(&directory)?;
    let view = demo.read_with(cx, |demo, _| demo.canvas.clone());
    cx.update_window(window, |_, window, cx| {
        demo.update(cx, |demo, cx| demo.reset(false, window, cx));
        view.update(cx, |view, cx| {
            view.set_tool(draw::ActiveTool::Text, window, cx);
            let mut style = view
                .document()
                .engine()
                .viewport_style_toolbar_state(view.session().viewport())
                .unwrap()
                .text_style;
            style.font_size = 36.0;
            style.color.r = 35;
            style.color.g = 45;
            style.color.b = 55;
            style.color.a = 255;
            style.opacity = 1.0;
            view.set_active_text_style(style, window, cx).unwrap();
        });
        window.activate_window();
    })?;
    settle(&view, cx).await?;
    let bounds = view.read_with(cx, |view, _| view.surface_bounds());
    let creation = bounds.origin + point(px(350.0), px(210.0));
    let blank = bounds.origin + point(px(720.0), px(420.0));
    let insertion = cx.update_window(window, |_, window, cx| {
        click(window, cx, creation);
        view.update(cx, |view, cx| {
            ensure!(view.has_text_draft(), "Text creation did not start editing");
            view.replace_text_in_range(None, "Native text editing", window, cx);
            Ok::<_, anyhow::Error>(
                view.bounds_for_range(3..3, bounds, window, cx)
                    .unwrap()
                    .center(),
            )
        })
    })??;
    settle(&view, cx).await?;
    capture(window, &directory, "text-created.png", cx)?;
    cx.update_window(window, |_, window, cx| click(window, cx, blank))?;
    ensure!(
        !view.read_with(cx, |view, _| view.has_text_draft()),
        "The blank finishing press created another editor"
    );
    settle(&view, cx).await?;
    let id = view.read_with(cx, |view, _| {
        ensure!(
            view.document()
                .engine()
                .viewport_active_tool(view.session().viewport())
                .unwrap()
                == draw::ActiveTool::Text,
            "Finishing text changed the active text tool"
        );
        let ids: Vec<_> = view
            .session()
            .scene()
            .ordered_nodes()
            .map(|node| node.item.as_ref())
            .filter_map(|item| {
                if let draw::SceneDisplayItem::Text(text) = item {
                    Some(draw::ElementId {
                        index: text.id.index,
                        generation: text.id.generation,
                    })
                } else {
                    None
                }
            })
            .collect();
        ensure!(
            ids.len() == 1,
            "Finishing text did not create exactly one committed element"
        );
        Ok::<_, anyhow::Error>(ids[0])
    })?;
    cx.update_window(window, |_, window, cx| {
        click(window, cx, insertion);
        view.update(cx, |view, cx| {
            ensure!(
                view.has_text_draft(),
                "Clicking existing text did not open editing"
            );
            ensure!(
                view.selected_text_range(false, window, cx).unwrap().range == (3..3),
                "Existing text did not place the caret at the clicked position"
            );
            view.replace_text_in_range(None, "✓", window, cx);
            Ok::<_, anyhow::Error>(())
        })?;
        key(window, cx, "ctrl-enter");
        Ok::<_, anyhow::Error>(())
    })??;
    ensure!(
        view.read_with(cx, |view, _| view
            .document()
            .engine()
            .text_element_info(id)
            .unwrap()
            .text)
            == "Nat✓ive text editing",
        "Typing did not insert at the clicked caret"
    );
    settle(&view, cx).await?;
    cx.update_window(window, |_, window, cx| click(window, cx, insertion))?;
    settle(&view, cx).await?;
    let unselected = capture(window, &directory, "text-unselected.png", cx)?;
    cx.update_window(window, |_, window, cx| key(window, cx, "ctrl-a"))?;
    settle(&view, cx).await?;
    let selection_bounds = cx.update_window(window, |_, window, cx| {
        view.update(cx, |view, cx| {
            let range = view.selected_text_range(false, window, cx).unwrap().range;
            ensure!(
                range == (0..20),
                "Select all did not include the inserted Unicode text"
            );
            Ok::<_, anyhow::Error>(view.bounds_for_range(range, bounds, window, cx).unwrap())
        })
    })??;
    let selected = capture(window, &directory, "text-selected.png", cx)?;
    let scale = cx.update_window(window, |_, window, _| window.scale_factor())?;
    let (left, top, right, bottom) = pixel_region(selection_bounds, scale, &selected, 0.0);
    let mut blue_pixels = 0;
    let mut white_glyph_pixels = 0;
    for y in top..bottom {
        for x in left..right {
            let before = unselected.get_pixel(x, y).0;
            let after = selected.get_pixel(x, y).0;
            blue_pixels += usize::from(after[2] > after[0].saturating_add(30) && after[2] > 170);
            white_glyph_pixels += usize::from(
                before[..3].iter().all(|channel| *channel < 140)
                    && after[..3].iter().all(|channel| *channel > 230),
            );
        }
    }
    ensure!(
        blue_pixels > 100,
        "Selection highlight is absent: {blue_pixels} blue pixels"
    );
    ensure!(
        white_glyph_pixels > 40,
        "Selected glyphs are not white: {white_glyph_pixels} pixels"
    );

    cx.update_window(window, |_, window, cx| key(window, cx, "right"))?;
    let caret_bounds = cx.update_window(window, |_, window, cx| {
        view.update(cx, |view, cx| {
            let range = view.selected_text_range(false, window, cx).unwrap().range;
            ensure!(range.is_empty(), "Collapsing selection kept selected text");
            Ok::<_, anyhow::Error>(view.bounds_for_range(range, bounds, window, cx).unwrap())
        })
    })??;
    let caret_visible = capture(window, &directory, "text-caret-visible.png", cx)?;
    cx.background_executor()
        .timer(Duration::from_millis(550))
        .await;
    let caret_hidden = capture(window, &directory, "text-caret-hidden.png", cx)?;
    let (left, top, right, bottom) = pixel_region(caret_bounds, scale, &caret_visible, 3.0);
    let blink_pixels = (top..bottom)
        .flat_map(|y| (left..right).map(move |x| (x, y)))
        .filter(|&(x, y)| caret_visible.get_pixel(x, y) != caret_hidden.get_pixel(x, y))
        .count();
    ensure!(
        blink_pixels > 5,
        "The caret did not blink: {blink_pixels} changed pixels"
    );
    cx.update_window(window, |_, window, cx| key(window, cx, "ctrl-enter"))?;
    ensure!(
        !view.read_with(cx, |view, _| view.has_text_draft()),
        "Finishing the text left its editor active"
    );
    let report = serde_json::json!({
        "result": "passed",
        "input_method": "GPUI native Window dispatch and EntityInputHandler",
        "checks": [
            "blank finishing press commits without creating another draft",
            "text-tool existing-text click enters editing at the clicked glyph",
            "typing inserts at the clicked caret",
            "selected text has blue background and white glyphs",
            "caret visibly blinks and selection collapses before caret drawing"
        ],
        "visual_assertions": {
            "blue_selection_pixels": blue_pixels,
            "white_selected_glyph_pixels": white_glyph_pixels,
            "blink_changed_pixels": blink_pixels
        },
        "scale": scale
    });
    std::fs::write(
        directory.join("text-validation.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}
