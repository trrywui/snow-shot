//! Bounded packing of consecutive, disjoint overlays. Blending stays in Core Image.
use super::RgbaOverlay;
use snow_media::geometry::{PixelRect, PixelSize};

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_ITEMS: usize = 64;

#[derive(Default)]
pub(super) struct OverlayBatcher {
    pixels: Vec<u8>,
}

fn visible(overlay: &RgbaOverlay<'_>, size: PixelSize) -> PixelRect {
    PixelRect {
        x: overlay.x,
        y: overlay.y,
        width: overlay.width.min(size.width - overlay.x),
        height: overlay.height.min(size.height - overlay.y),
    }
}

fn area(rect: PixelRect) -> usize {
    rect.width as usize * rect.height as usize
}

fn overlaps(a: PixelRect, b: PixelRect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

fn union(a: PixelRect, b: PixelRect) -> PixelRect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    PixelRect {
        x,
        y,
        width: (a.x + a.width).max(b.x + b.width) - x,
        height: (a.y + a.height).max(b.y + b.height) - y,
    }
}

impl OverlayBatcher {
    /// Inputs have already been validated against the destination. A batch never
    /// reorders layers or combines overlapping pixels: source color conversion,
    /// alpha and multiply blending therefore retain their original semantics.
    pub(super) fn prepare<'a>(
        &'a mut self,
        overlays: &'a [RgbaOverlay<'_>],
        size: PixelSize,
    ) -> (RgbaOverlay<'a>, usize) {
        let first = &overlays[0];
        let mut bounds = visible(first, size);
        let mut occupied = area(bounds);
        let mut count = 1;
        for next in overlays.iter().skip(1).take(MAX_ITEMS - 1) {
            let next = visible(next, size);
            let combined = union(bounds, next);
            let next_occupied = occupied + area(next);
            if area(combined) > MAX_BYTES / 4
                || area(combined) > next_occupied * 2
                || overlays[..count]
                    .iter()
                    .any(|prior| overlaps(visible(prior, size), next))
            {
                break;
            }
            bounds = combined;
            occupied = next_occupied;
            count += 1;
        }
        if count == 1 {
            return (
                RgbaOverlay {
                    x: first.x,
                    y: first.y,
                    width: first.width,
                    height: first.height,
                    stride: first.stride,
                    bytes: first.bytes,
                },
                1,
            );
        }
        let length = area(bounds) * 4;
        // Avoid Vec's geometric capacity growth exceeding the scratch budget.
        if self.pixels.capacity() < length {
            self.pixels.reserve_exact(length - self.pixels.len());
        }
        self.pixels.resize(length, 0);
        self.pixels.fill(0);
        let stride = bounds.width as usize * 4;
        for overlay in &overlays[..count] {
            let rect = visible(overlay, size);
            let width = rect.width as usize * 4;
            for row in 0..rect.height as usize {
                let to = (rect.y as usize - bounds.y as usize + row) * stride
                    + (rect.x as usize - bounds.x as usize) * 4;
                let from = row * overlay.stride;
                self.pixels[to..to + width].copy_from_slice(&overlay.bytes[from..from + width]);
            }
        }
        (
            RgbaOverlay {
                x: bounds.x,
                y: bounds.y,
                width: bounds.width,
                height: bounds.height,
                stride,
                bytes: &self.pixels,
            },
            count,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(x: u32, y: u32, bytes: &[u8]) -> RgbaOverlay<'_> {
        RgbaOverlay {
            x,
            y,
            width: 128,
            height: 128,
            stride: 512,
            bytes,
        }
    }

    #[test]
    fn compact_tiles_pack_but_overlap_distance_and_capacity_bound_work() {
        let size = PixelSize::new(3840, 2160).unwrap();
        let bytes = vec![255; 128 * 128 * 4];
        let mut batcher = OverlayBatcher::default();
        let tiles: Vec<_> = (0..64)
            .map(|i| tile((i % 8) * 128, (i / 8) * 128, &bytes))
            .collect();
        let (packed, count) = batcher.prepare(&tiles, size);
        assert_eq!(count, 32);
        assert_eq!(
            (packed.width, packed.height, packed.bytes.len()),
            (1024, 512, MAX_BYTES)
        );
        assert!(packed.bytes.iter().all(|byte| *byte == 255));
        assert!(batcher.pixels.capacity() <= MAX_BYTES);
        for pair in [
            [tile(0, 0, &bytes), tile(64, 0, &bytes)],
            [tile(0, 0, &bytes), tile(3000, 2000, &bytes)],
        ] {
            let (single, count) = batcher.prepare(&pair, size);
            assert_eq!(count, 1);
            assert_eq!(single.bytes.as_ptr(), bytes.as_ptr());
        }
        // An overlapping successor remains after the preceding disjoint group.
        let layered = [
            tile(0, 0, &bytes),
            tile(128, 0, &bytes),
            tile(64, 0, &bytes),
        ];
        assert_eq!(batcher.prepare(&layered, size).1, 2);
    }

    #[test]
    fn reused_batches_clear_holes_clip_edges_and_copy_only_visible_padded_rows() {
        let size = PixelSize::new(4, 3).unwrap();
        let bytes: Vec<_> = (0..48).collect();
        let overlays = [
            RgbaOverlay {
                x: 0,
                y: 0,
                width: 1,
                height: 2,
                stride: 8,
                bytes: &bytes,
            },
            RgbaOverlay {
                x: 2,
                y: 1,
                width: 3,
                height: 3,
                stride: 16,
                bytes: &bytes,
            },
        ];
        let mut batcher = OverlayBatcher {
            pixels: vec![255; 48],
        };
        let (packed, count) = batcher.prepare(&overlays, size);
        assert_eq!(count, 2);
        assert_eq!((packed.width, packed.height), (4, 3));
        let mut expected = vec![0; 48];
        expected[..4].copy_from_slice(&bytes[..4]);
        expected[16..20].copy_from_slice(&bytes[8..12]);
        expected[24..32].copy_from_slice(&bytes[..8]);
        expected[40..48].copy_from_slice(&bytes[16..24]);
        assert_eq!(packed.bytes, expected);
    }
}
