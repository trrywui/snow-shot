//! One mask and placement implementation for preview, live compositors, and deferred replay.
use crate::surface::Surface;
use snow_recording_model::{WebcamOrientation, WebcamShape, WebcamStyle};
use snow_webcam::Frame;

pub struct WebcamImage<'a> {
    pub rect: (u32, u32, u32),
    pub pixels: &'a [u8],
    pub generation: u64,
}

#[derive(Default)]
pub struct WebcamOverlay {
    key: Option<(u64, u32, WebcamShape, WebcamOrientation)>,
    rect: (u32, u32, u32),
    mask: Vec<u8>,
    mask_key: Option<(u32, WebcamShape)>,
    pixels: Vec<u8>,
    generation: u64,
    sample_key: Option<(u32, u32)>,
    samples: Vec<usize>,
}

impl WebcamOverlay {
    pub fn prepare(
        &mut self,
        frame: Option<&Frame>,
        style: WebcamStyle,
        output: (u32, u32),
    ) -> bool {
        let rect = style.placement(output);
        let moved = self.rect != rect;
        self.rect = rect;
        let next = frame.map(|frame| (frame.revision, rect.2, style.shape, style.orientation));
        if next == self.key {
            return moved && self.visible();
        }
        self.key = next;
        self.generation = self.generation.wrapping_add(1);
        self.pixels.clear();
        let Some(frame) = frame else {
            return true;
        };
        let size = self.rect.2;
        if size == 0 {
            return true;
        }
        if self.mask_key != Some((size, style.shape)) {
            self.mask_key = Some((size, style.shape));
            self.mask.resize(size as usize * size as usize, 255);
            let radius = size as f32 / 2.0;
            for y in 0..size {
                for x in 0..size {
                    self.mask[(y * size + x) as usize] = if style.shape == WebcamShape::Circle {
                        let dx = x as f32 + 0.5 - radius;
                        let dy = y as f32 + 0.5 - radius;
                        ((radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0) * 255.0)
                            .round() as u8
                    } else {
                        255
                    };
                }
            }
        }
        self.pixels.resize(size as usize * size as usize * 4, 0);
        if self.sample_key != Some((frame.size, size)) {
            self.sample_key = Some((frame.size, size));
            self.samples.clear();
            self.samples.extend(
                (0..size)
                    .map(|i| (u64::from(i) * u64::from(frame.size) / u64::from(size)) as usize),
            );
        }
        // Dispatch once. Rotation/reflection shares the existing sampling and mask pass,
        // with no extra image, frame allocation, or branch per output pixel.
        match style.shape {
            WebcamShape::Square => self.orient::<false>(frame, size, style.orientation),
            WebcamShape::Circle => self.orient::<true>(frame, size, style.orientation),
        }
        true
    }

    fn orient<const MASKED: bool>(
        &mut self,
        frame: &Frame,
        size: u32,
        orientation: WebcamOrientation,
    ) {
        match orientation {
            WebcamOrientation::Identity => {
                self.rasterize::<false, false, false, MASKED>(frame, size)
            }
            WebcamOrientation::Clockwise90 => {
                self.rasterize::<true, false, true, MASKED>(frame, size)
            }
            WebcamOrientation::Clockwise180 => {
                self.rasterize::<false, true, true, MASKED>(frame, size)
            }
            WebcamOrientation::Clockwise270 => {
                self.rasterize::<true, true, false, MASKED>(frame, size)
            }
            WebcamOrientation::Mirrored => {
                self.rasterize::<false, true, false, MASKED>(frame, size)
            }
            WebcamOrientation::MirroredClockwise90 => {
                self.rasterize::<true, true, true, MASKED>(frame, size)
            }
            WebcamOrientation::MirroredClockwise180 => {
                self.rasterize::<false, false, true, MASKED>(frame, size)
            }
            WebcamOrientation::MirroredClockwise270 => {
                self.rasterize::<true, false, false, MASKED>(frame, size)
            }
        }
    }

    fn rasterize<const SWAP: bool, const FLIP_X: bool, const FLIP_Y: bool, const MASKED: bool>(
        &mut self,
        frame: &Frame,
        size: u32,
    ) {
        let size = size as usize;
        let samples = &self.samples[..size];
        let stride = frame.size as usize * 4;
        let reverse_rows = if SWAP { FLIP_X } else { FLIP_Y };
        let reverse_columns = if SWAP { FLIP_Y } else { FLIP_X };
        let row_step = if SWAP { 4 } else { stride };
        let column_step = if SWAP { stride } else { 4 };
        let rows = self
            .pixels
            .chunks_exact_mut(size * 4)
            .zip(self.mask.chunks_exact(size));
        for (y, (pixels, mask)) in rows.enumerate() {
            let base = samples[if reverse_rows { size - 1 - y } else { y }] * row_step;
            let columns = samples.iter().copied();
            if reverse_columns {
                Self::rasterize_row::<MASKED>(
                    pixels,
                    mask,
                    columns.rev(),
                    &frame.rgba,
                    base,
                    column_step,
                );
            } else {
                Self::rasterize_row::<MASKED>(
                    pixels,
                    mask,
                    columns,
                    &frame.rgba,
                    base,
                    column_step,
                );
            }
        }
    }

    #[inline]
    fn rasterize_row<const MASKED: bool>(
        pixels: &mut [u8],
        mask: &[u8],
        columns: impl Iterator<Item = usize>,
        source: &[u8],
        base: usize,
        step: usize,
    ) {
        for ((target, &alpha), column) in pixels.chunks_exact_mut(4).zip(mask).zip(columns) {
            // Bound each source pixel once; output chunks always contain four bytes.
            // Opaque square overlays also skip all mask multiplication.
            let from = base + column * step;
            let source = &source[from..from + 4];
            if MASKED {
                for (target, &channel) in target[..3].iter_mut().zip(&source[..3]) {
                    *target = ((u32::from(channel) * u32::from(alpha) + 127) / 255) as u8;
                }
                target[3] = alpha;
            } else {
                target.copy_from_slice(source);
                target[3] = 255;
            }
        }
    }

    /// Premultiplied RGBA in a compact square, shared by the native compositors.
    pub fn image(&self) -> Option<WebcamImage<'_>> {
        self.visible().then_some(WebcamImage {
            rect: self.rect,
            pixels: &self.pixels,
            generation: self.generation,
        })
    }

    pub fn visible(&self) -> bool {
        !self.pixels.is_empty()
    }
    pub fn draw_to<S: Surface>(&self, surface: &mut S) {
        if self.pixels.is_empty() {
            return;
        }
        let (x, y, size) = self.rect;
        for row in 0..size {
            surface.span(x, y + row, size, |target, offset| {
                let begin = (row as usize * size as usize + offset) * 4;
                for (to, from) in target
                    .chunks_exact_mut(4)
                    .zip(self.pixels[begin..].chunks_exact(4))
                {
                    let inverse = 255 - u32::from(from[3]);
                    for channel in 0..3 {
                        to[channel] = (u32::from(from[channel])
                            + (u32::from(to[channel]) * inverse + 127) / 255)
                            .min(255) as u8;
                    }
                    to[3] = if S::OPAQUE {
                        255
                    } else {
                        (u32::from(from[3]) + (u32::from(to[3]) * inverse + 127) / 255).min(255)
                            as u8
                    };
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::{RgbaSurface, TileSurface};
    use std::{sync::Arc, time::Instant};

    #[test]
    fn webcam_orientations_match_display_space_transforms_and_reuse_storage() {
        let mut rgba = Vec::new();
        for y in 0..5 {
            for x in 0..5 {
                rgba.extend_from_slice(&[x * 40 + y, y * 40 + x, x * 10 + y * 5, 255]);
            }
        }
        let frame = Frame {
            revision: 1,
            at: Instant::now(),
            size: 5,
            rgba: Arc::new(rgba),
        };
        for shape in [WebcamShape::Square, WebcamShape::Circle] {
            for side in [0, 1, 7, 50, 73] {
                let mut overlay = WebcamOverlay::default();
                let mut style = WebcamStyle {
                    size: 80,
                    shape,
                    ..Default::default()
                };
                overlay.prepare(Some(&frame), style, (side, side));
                let original = overlay.pixels.clone();
                // Independently anchor the identity image to the input coordinates,
                // so a shared resampling error cannot pass the rotation comparisons.
                for y in 0..side {
                    for x in 0..side {
                        let from = (((u64::from(y) * 5 / u64::from(side)) * 5
                            + u64::from(x) * 5 / u64::from(side))
                            * 4) as usize;
                        let to = ((y * side + x) * 4) as usize;
                        let alpha = u32::from(original[to + 3]);
                        for channel in 0..3 {
                            assert_eq!(
                                original[to + channel],
                                ((u32::from(frame.rgba[from + channel]) * alpha + 127) / 255) as u8
                            );
                        }
                    }
                }
                let pixels = overlay.pixels.as_ptr();
                let mask = overlay.mask.as_ptr();
                let samples = overlay.samples.as_ptr();
                for raw in 0..8 {
                    style.orientation = WebcamOrientation::from_raw(raw).unwrap();
                    let generation = overlay.generation;
                    assert_eq!(overlay.prepare(Some(&frame), style, (side, side)), raw != 0);
                    assert_eq!(overlay.generation, generation + u64::from(raw != 0));
                    assert!(!overlay.prepare(Some(&frame), style, (side, side)));
                    let mut expected = original.clone();
                    let side = side as usize;
                    if raw & 4 != 0 {
                        for row in expected.chunks_exact_mut(side.max(1) * 4) {
                            for x in 0..side / 2 {
                                for c in 0..4 {
                                    row.swap(x * 4 + c, (side - 1 - x) * 4 + c);
                                }
                            }
                        }
                    }
                    for _ in 0..raw & 3 {
                        let before = expected.clone();
                        for y in 0..side {
                            for x in 0..side {
                                let from = (y * side + x) * 4;
                                let to = (x * side + side - 1 - y) * 4;
                                expected[to..to + 4].copy_from_slice(&before[from..from + 4]);
                            }
                        }
                    }
                    assert_eq!(
                        overlay.pixels, expected,
                        "orientation {raw}, shape {shape:?}, side {side}"
                    );
                    assert_eq!(overlay.pixels.as_ptr(), pixels);
                    assert_eq!(overlay.mask.as_ptr(), mask);
                    assert_eq!(overlay.samples.as_ptr(), samples);
                }
            }
        }
    }
    #[test]
    fn webcam_movement_invalidates_placement_without_a_new_camera_frame() {
        let frame = Frame {
            revision: 1,
            at: Instant::now(),
            size: 50,
            rgba: Arc::new([220, 40, 10, 255].repeat(50 * 50)),
        };
        for shape in [WebcamShape::Square, WebcamShape::Circle] {
            let mut overlay = WebcamOverlay::default();
            let style = WebcamStyle {
                size: 50,
                shape,
                ..Default::default()
            };
            overlay.prepare(Some(&frame), style, (800, 600));
            let old = overlay.image().unwrap().rect;
            let pixels = overlay.image().unwrap().pixels.as_ptr();
            let generation = overlay.image().unwrap().generation;
            let moved = WebcamStyle {
                offset_left: 300,
                offset_bottom: 200,
                ..style
            };
            assert!(overlay.prepare(Some(&frame), moved, (800, 600)));
            assert_ne!(old, overlay.image().unwrap().rect);
            assert_eq!(overlay.image().unwrap().rect, (300, 350, 50));
            assert_eq!(overlay.image().unwrap().pixels.as_ptr(), pixels);
            assert_eq!(
                overlay.image().unwrap().generation,
                generation,
                "movement reuses the rasterized image and native texture"
            );
            assert!(!overlay.prepare(Some(&frame), moved, (800, 600)));
            let mut surface = TileSurface::new((800, 600));
            overlay.draw_to(&mut surface);
            assert!(overlay.prepare(None, moved, (800, 600)));
            assert!(!overlay.visible());
        }
    }
    #[test]
    fn webcam_placement_masks_layering_and_stale_clearing_agree_for_all_surfaces() {
        let frame = Frame {
            revision: 1,
            at: Instant::now(),
            size: 150,
            rgba: Arc::new([220, 40, 10, 255].repeat(150 * 150)),
        };
        for size in [50, 150, 500] {
            for shape in [WebcamShape::Square, WebcamShape::Circle] {
                for output in [(800, 600), (160, 170), (20, 30)] {
                    let style = WebcamStyle {
                        size,
                        shape,
                        ..Default::default()
                    };
                    let (x, y, side) = style.placement(output);
                    assert!(x + side <= output.0 && y + side <= output.1);
                    let mut overlay = WebcamOverlay::default();
                    assert!(overlay.prepare(Some(&frame), style, output));
                    assert!(!overlay.prepare(Some(&frame), style, output));
                    let mut rgba = [0, 0, 0, 255].repeat((output.0 * output.1) as usize);
                    overlay.draw_to(&mut RgbaSurface {
                        pixels: &mut rgba,
                        dimensions: output,
                    });
                    let center = (((y + side / 2) * output.0 + x + side / 2) * 4) as usize;
                    assert_eq!(&rgba[center..center + 4], &[220, 40, 10, 255]);
                    let corner = ((y * output.0 + x) * 4) as usize;
                    assert_eq!(
                        rgba[corner],
                        if shape == WebcamShape::Square { 220 } else { 0 }
                    );
                    let mut tiles = TileSurface::new(output);
                    overlay.draw_to(&mut tiles);
                    assert!(tiles.allocated_bytes() <= 36 * 128 * 128 * 4);
                    if shape == WebcamShape::Circle {
                        assert!(overlay.mask.iter().any(|a| *a > 0 && *a < 255));
                    }
                    assert!(overlay.prepare(None, style, output));
                    assert!(!overlay.visible());
                    assert!(!overlay.prepare(None, style, output));
                }
            }
        }
    }
}
