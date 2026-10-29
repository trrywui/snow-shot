//! Export dimension limits and bounded encoding output.
use crate::{CanvasError, CanvasSnapshot, document::validate_raster, engine::*};
use std::io::Write;

#[derive(Default)]
pub(crate) struct BoundedOutput {
    pub bytes: Vec<u8>,
}
impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > 96 * 1024 * 1024 {
            return Err(std::io::Error::other(
                "Encoded output exceeds 96 MiB; use the streaming writer API",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) fn dimensions(snapshot: &CanvasSnapshot, scale: f32) -> Result<[u32; 2], CanvasError> {
    scaled_dimensions(snapshot.viewport.surface, scale)
}
pub(crate) fn scaled_dimensions(surface: SurfaceSize, scale: f32) -> Result<[u32; 2], CanvasError> {
    if !scale.is_finite() || !(0.1..=8.0).contains(&scale) {
        return Err(ErrorCode::InvalidArgument.into());
    }
    let result = [
        (f64::from(surface.width) * f64::from(scale)).ceil() as u32,
        (f64::from(surface.height) * f64::from(scale)).ceil() as u32,
    ];
    validate_raster(result[0], result[1])?;
    // An individual PNG scanline is bounded to 8 MiB even for very wide captures.
    if result[0] > 2 * 1024 * 1024 {
        return Err(CanvasError::ResourceLimit);
    }
    Ok(result)
}
