// SPDX-License-Identifier: Apache-2.0
//! Scope culling must keep the global stamp lattice and bounded GPU work.
use super::*;
fn stamped(width: u32, height: u32, text: &str, revision: u64) -> Result<RetainedScene> {
    let mut delta = reset(view(width, height), vec![]);
    delta.revision = revision;
    delta.decoration_revision = revision;
    let watermark = &mut delta.decoration.as_mut().unwrap().watermark;
    watermark.text[..text.len()].copy_from_slice(text.as_bytes());
    watermark.text_len = text.len().try_into()?;
    watermark.font_size = 16.;
    watermark.gap = 56.;
    watermark.angle = 30.;
    watermark.opacity = 0.16;
    watermark.color = ColorRgba8 {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    let mut scene = RetainedScene::default();
    scene.apply_delta(&delta)?;
    Ok(scene)
}
#[test]
fn watermark_area_culling_preserves_global_lattice_at_fractional_scales() -> Result<()> {
    let source = SourceImage::new(RgbaImage::from_pixel(320, 192, Rgba([200, 120, 80, 255])))?;
    let scene = stamped(320, 192, "Private 2026-10-25_10-26-00", 1)?;
    let area = CanvasRect::new(37.5, 22.5, 125., 98.)?;
    let mut device = Device::new()?;
    for scale in [1., 1.5, 2.] {
        let (_, full) = device.render(
            &scene,
            Some(&source),
            scale,
            &CanvasPresentation {
                // Same intermediate compositing path: `None` instead paints onto
                // the multisampled scene and is a different coverage comparison.
                watermark_area: Some(CanvasRect::new(0., 0., 320., 192.)?),
                ..Default::default()
            },
        )?;
        let (_, clipped) = device.render(
            &scene,
            Some(&source),
            scale,
            &CanvasPresentation {
                watermark_area: Some(area),
                ..Default::default()
            },
        )?;
        let left = (area.x * scale as f64).ceil() as u32 + 1;
        let top = (area.y * scale as f64).ceil() as u32 + 1;
        let right = ((area.x + area.width) * scale as f64).floor() as u32 - 1;
        let bottom = ((area.y + area.height) * scale as f64).floor() as u32 - 1;
        let mut changed = 0;
        for y in top..bottom {
            for x in left..right {
                ensure!(
                    full.get_pixel(x, y) == clipped.get_pixel(x, y),
                    "scope changed watermark phase/coverage at scale={scale}, ({x},{y})"
                );
                changed += usize::from(clipped.get_pixel(x, y).0 != [200, 120, 80, 255]);
            }
        }
        ensure!(changed > 10, "clipped watermark fixture is empty");
        ensure!(
            clipped.get_pixel(0, 0).0 == [200, 120, 80, 255],
            "watermark escaped clip"
        );
    }
    Ok(())
}
#[test]
fn watermark_area_repeated_timestamp_changes_on_4k_stay_within_default_budget() -> Result<()> {
    let source = SourceImage::new(RgbaImage::from_pixel(3840, 2160, Rgba([200, 120, 80, 255])))?;
    let mut device = Device::new()?;
    let presentation = CanvasPresentation {
        watermark_area: Some(CanvasRect::new(350., 300., 400., 300.)?),
        ..Default::default()
    };
    let mut previous: Option<RgbaImage> = None;
    for seconds in 0..6 {
        let scene = stamped(
            3840,
            2160,
            &format!("Private 2026-10-25_10-26-{seconds:02}"),
            seconds + 1,
        )?;
        let (_, image) = device.render(&scene, Some(&source), 1., &presentation)?;
        ensure!(
            image.dimensions() == (3840, 2160),
            "4k frame dimensions changed"
        );
        ensure!(
            image.get_pixel(100, 100).0 == [200, 120, 80, 255],
            "off-selection stamp escaped scope"
        );
        ensure!(
            (300..600).any(|y| (350..750).any(|x| image.get_pixel(x, y).0 != [200, 120, 80, 255])),
            "timestamped watermark did not render"
        );
        if let Some(previous) = previous.as_ref() {
            ensure!(
                previous != &image,
                "changed timestamp reused stale watermark pixels"
            );
        }
        previous = Some(image);
    }
    Ok(())
}
#[test]
fn watermark_area_whole_4k_timestamp_changes_stay_within_default_budget() -> Result<()> {
    let source = SourceImage::new(RgbaImage::from_pixel(3840, 2160, Rgba([200, 120, 80, 255])))?;
    let mut device = Device::new()?;
    let presentation = CanvasPresentation {
        watermark_area: Some(CanvasRect::new(0., 0., 3840., 2160.)?),
        ..Default::default()
    };
    let mut previous: Option<RgbaImage> = None;
    for seconds in 0..3 {
        let scene = stamped(
            3840,
            2160,
            &format!("Private 2026-10-25_10-26-{seconds:02}"),
            seconds + 10,
        )?;
        let (_, image) = device.render(&scene, Some(&source), 1., &presentation)?;
        ensure!(
            image.pixels().any(|pixel| pixel.0 != [200, 120, 80, 255]),
            "whole-frame watermark is empty"
        );
        if let Some(previous) = previous.as_ref() {
            ensure!(previous != &image, "whole-frame timestamp did not update");
        }
        previous = Some(image);
    }
    Ok(())
}
