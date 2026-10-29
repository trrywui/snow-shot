use crate::{CanvasDocument, CanvasError, SourceImage};
use image::{Rgba, RgbaImage};

/// Reproducible demonstration fixture: immutable image source plus editable engine annotations.
pub fn showcase_document() -> Result<CanvasDocument, CanvasError> {
    let mut document = CanvasDocument::default();
    let background = RgbaImage::from_fn(960, 640, |x, y| {
        if (490..582).contains(&y)
            && [48, 280, 512, 744]
                .iter()
                .any(|left| x >= *left && x < *left + 168)
        {
            let local = (x - 48) % 232;
            let stripe = ((local / 10 + (y - 490) / 10) % 2) as u8;
            Rgba([
                80 + (local * 130 / 168) as u8,
                120 + stripe * 70,
                180 - ((y - 490) * 90 / 92) as u8,
                255,
            ])
        } else {
            Rgba([255, 255, 255, 255])
        }
    });
    document.set_background(Some(SourceImage::new(background)?));
    document.command(|engine| {
        engine.apply_annotation_json(include_bytes!("../samples/showcase.annotations.json"))
    })?;
    Ok(document)
}
