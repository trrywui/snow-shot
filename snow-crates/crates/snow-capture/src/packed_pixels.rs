//! Immutable CPU image lease for GPU upload adapters. No image codec, channel
//! conversion, allocator conversion or pixel copy occurs when creating a lease.

use crate::{CapturePixelFormat, frame::Frame};
use snow_memory::RasterBuffer;
use std::sync::Arc;

#[derive(Clone)]
pub struct PackedPixels {
    storage: Arc<RasterBuffer>,
    width: u32,
    height: u32,
    format: CapturePixelFormat,
}

impl PackedPixels {
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn format(&self) -> CapturePixelFormat {
        self.format
    }
    pub fn row_pitch(&self) -> usize {
        self.width as usize * 4
    }
    pub fn bytes(&self) -> &[u8] {
        self.storage.as_slice()
    }
    pub fn row(&self, y: u32) -> Option<&[u8]> {
        if y >= self.height {
            return None;
        }
        let start = y as usize * self.row_pitch();
        Some(&self.bytes()[start..start + self.row_pitch()])
    }
}

impl Frame {
    /// Retains physical packed pixels for asynchronous texture upload. The
    /// consumer must honor `format` and `row_pitch`; alpha is straight, not
    /// premultiplied. Later frame writes detach from this immutable lease.
    pub fn packed_pixels(&self) -> PackedPixels {
        PackedPixels {
            storage: self.shared_bytes(),
            width: self.width(),
            height: self.height(),
            format: self.pixel_format(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packed_pixels_share_storage_preserve_format_and_survive_frame_reuse() {
        for format in [CapturePixelFormat::Rgba8, CapturePixelFormat::Bgra8] {
            let mut frame = match format {
                CapturePixelFormat::Rgba8 => Frame::from_rgba8(2, 2, (0..16).collect()).unwrap(),
                CapturePixelFormat::Bgra8 => Frame::from_bgra8(2, 2, (0..16).collect()).unwrap(),
            };
            let lease = frame.packed_pixels();
            assert_eq!(lease.bytes().as_ptr(), frame.as_bytes().as_ptr());
            assert_eq!(lease.format(), format);
            assert_eq!(lease.row_pitch(), 8);
            assert_eq!(lease.row(1), Some(&[8, 9, 10, 11, 12, 13, 14, 15][..]));
            assert_eq!(lease.row(2), None);
            frame.as_mut_bytes()[0] = 42;
            drop(frame);
            assert_eq!(lease.bytes()[0], 0);
            assert_eq!(lease.clone().bytes().as_ptr(), lease.bytes().as_ptr());
        }
        assert_eq!(Frame::empty().packed_pixels().row(0), None);
    }
}
