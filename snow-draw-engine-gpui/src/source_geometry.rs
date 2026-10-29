//! Pure source sampling and effect geometry shared by GPU preparation and CPU workers.
use image::Rgba;
use snow_draw_engine::{FilterDisplayItem, Point};

use crate::SourceImage;

pub(crate) fn filter_canvas_bounds(filter: &FilterDisplayItem) -> (f64, f64, f64, f64) {
    if filter.is_pen_filter && !filter.points.is_empty() {
        let radius = filter.stroke_width / 2.0 + 2.0;
        let mut bounds = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for [x, y] in filter.points.iter().copied() {
            bounds.0 = bounds.0.min(x - radius);
            bounds.1 = bounds.1.min(y - radius);
            bounds.2 = bounds.2.max(x + radius);
            bounds.3 = bounds.3.max(y + radius);
        }
        bounds
    } else {
        let (sin, cos) = filter.rotation.sin_cos();
        let width = cos.abs() * filter.width / 2.0 + sin.abs() * filter.height / 2.0 + 2.0;
        let height = sin.abs() * filter.width / 2.0 + cos.abs() * filter.height / 2.0 + 2.0;
        (
            filter.center_x - width,
            filter.center_y - height,
            filter.center_x + width,
            filter.center_y + height,
        )
    }
}

pub(crate) fn sample_source(source: &SourceImage, world: Point<f64>) -> Option<Rgba<u8>> {
    let pixels = source.raster_pixels()?;
    let x = (world.x - source.origin().x) / source.units_per_pixel();
    let y = (world.y - source.origin().y) / source.units_per_pixel();
    if x < 0.0 || y < 0.0 || x >= f64::from(pixels.width()) || y >= f64::from(pixels.height()) {
        return None;
    }
    // Stable 16-bit fractional texel coordinates keep adjacent donor regions
    // consistent across floating-point coordinate reassociation.
    // The quantization error is below 1/65536 source pixel, including at alpha edges.
    let fx = ((x - 0.5) * 65536.0).round() / 65536.0;
    let fy = ((y - 0.5) * 65536.0).round() / 65536.0;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    let mut sums = [0.0; 4];
    for (dx, dy, weight) in [
        (0.0, 0.0, (1.0 - tx) * (1.0 - ty)),
        (1.0, 0.0, tx * (1.0 - ty)),
        (0.0, 1.0, (1.0 - tx) * ty),
        (1.0, 1.0, tx * ty),
    ] {
        let pixel = pixels.get_pixel(
            (x0 + dx).clamp(0.0, f64::from(pixels.width() - 1)) as u32,
            (y0 + dy).clamp(0.0, f64::from(pixels.height() - 1)) as u32,
        );
        for channel in 0..3 {
            sums[channel] += f64::from(pixel[channel]) * f64::from(pixel[3]) / 255.0 * weight;
        }
        sums[3] += f64::from(pixel[3]) * weight;
    }
    let alpha = sums[3].round().clamp(0.0, 255.0) as u8;
    Some(Rgba([
        if alpha > 0 {
            (sums[0] * 255.0 / sums[3]).round().clamp(0.0, 255.0) as u8
        } else {
            0
        },
        if alpha > 0 {
            (sums[1] * 255.0 / sums[3]).round().clamp(0.0, 255.0) as u8
        } else {
            0
        },
        if alpha > 0 {
            (sums[2] * 255.0 / sums[3]).round().clamp(0.0, 255.0) as u8
        } else {
            0
        },
        alpha,
    ]))
}
