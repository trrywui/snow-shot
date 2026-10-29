use crate::RawFrame;

/// Cached separable area filter. Crop before filtering, so portrait/wide sources never stretch.
#[derive(Default)]
pub(crate) struct CropResize {
    geometry: (u32, u32, u32),
    weights: Vec<Vec<(usize, f32)>>,
    scratch: Vec<f32>,
}
impl CropResize {
    pub fn convert(&mut self, frame: &RawFrame, size: u32) -> Result<Vec<u8>, String> {
        if frame.width == 0
            || frame.height == 0
            || frame.width > 8192
            || frame.height > 8192
            || frame.stride < frame.width as usize * 4
            || frame
                .stride
                .checked_mul(frame.height.saturating_sub(1) as usize)
                .and_then(|len| len.checked_add(frame.width as usize * 4))
                .is_none_or(|len| len > frame.rgba.len())
        {
            return Err("Invalid webcam frame dimensions or stride.".into());
        }
        let side = frame.width.min(frame.height) as usize;
        let offset_x = (frame.width as usize - side) / 2;
        let offset_y = (frame.height as usize - side) / 2;
        let n = size as usize;
        if self.geometry != (frame.width, frame.height, size) {
            self.geometry = (frame.width, frame.height, size);
            self.weights = (0..n)
                .map(|out| {
                    let begin = out as f32 * side as f32 / n as f32;
                    let end = (out + 1) as f32 * side as f32 / n as f32;
                    (begin.floor() as usize..end.ceil() as usize)
                        .map(|i| {
                            (
                                i.min(side - 1),
                                (end.min((i + 1) as f32) - begin.max(i as f32)) / (end - begin),
                            )
                        })
                        .collect()
                })
                .collect();
        }
        self.scratch.resize(side * n * 3, 0.0);
        for y in 0..side {
            for (x, weights) in self.weights.iter().enumerate() {
                for c in 0..3 {
                    self.scratch[(y * n + x) * 3 + c] = weights
                        .iter()
                        .map(|&(sx, w)| {
                            f32::from(
                                frame.rgba[(y + offset_y) * frame.stride + (sx + offset_x) * 4 + c],
                            ) * w
                        })
                        .sum();
                }
            }
        }
        let mut pixels = vec![255; n * n * 4];
        for (y, weights) in self.weights.iter().enumerate() {
            for x in 0..n {
                for c in 0..3 {
                    let value: f32 = weights
                        .iter()
                        .map(|&(sy, w)| self.scratch[(sy * n + x) * 3 + c] * w)
                        .sum();
                    pixels[(y * n + x) * 4 + c] = value.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        Ok(pixels)
    }
}
