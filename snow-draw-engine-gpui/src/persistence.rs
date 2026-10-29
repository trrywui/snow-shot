//! Send-able save/load payloads keep image codec work off the GPUI thread.
use crate::{
    CanvasDocument, CanvasError, SourceImage,
    engine::{Engine, Point, RuntimeConfig},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::io::Write;
const FILE_LIMIT: usize = 512 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrawingFile {
    schema: u32,
    engine: serde_json::Value,
    background: Option<SavedImage>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedImage {
    png: String,
    origin: [f64; 2],
    units_per_pixel: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    smart_erase_png: Option<String>,
}

/// Capture engine bytes on its owner thread, encode/decode assets on a worker,
/// then construct the non-Send engine on the destination owner thread.
#[derive(Clone)]
pub struct DrawingSnapshot {
    engine: serde_json::Value,
    background: Option<SourceImage>,
}
impl DrawingSnapshot {
    pub(crate) fn source(&self) -> Option<&SourceImage> {
        self.background.as_ref()
    }
    /// Freeze layered visual and donor composites on the GPUI device before
    /// moving this snapshot to the encoder worker. Raster-only saves need no
    /// GPU work. The callback must return pixels on the source's native grid.
    pub fn materialize_for_save(
        mut self,
        mut materialize: impl FnMut(&SourceImage, bool) -> Result<SourceImage, CanvasError>,
    ) -> Result<Self, CanvasError> {
        let Some(source) = self
            .background
            .as_ref()
            .filter(|source| source.raster_pixels().is_none())
        else {
            return Ok(self);
        };
        let visual = materialize(source, false)?;
        let eligible = materialize(source, true)?;
        if visual.raster_pixels().is_none()
            || eligible.raster_pixels().is_none()
            || visual.dimensions() != source.dimensions()
            || eligible.dimensions() != source.dimensions()
        {
            return Err(CanvasError::InvalidFile(
                "GPU source materialization dimensions differ".into(),
            ));
        }
        let mut visual = visual.with_placement(source.origin(), source.units_per_pixel())?;
        visual.smart_erase_pixels = Some(eligible.pixels.clone());
        self.background = Some(visual);
        Ok(self)
    }

    pub fn requires_materialization(&self) -> bool {
        self.background
            .as_ref()
            .is_some_and(|source| source.raster_pixels().is_none())
    }
    pub(crate) fn capture(
        engine: &Engine,
        background: Option<SourceImage>,
    ) -> Result<Self, CanvasError> {
        Ok(Self {
            engine: serde_json::from_slice(&engine.serialize_document_session()?).map_err(error)?,
            background,
        })
    }
    pub fn encode(self) -> Result<Vec<u8>, CanvasError> {
        self.encode_with_limit(512 * 1024 * 1024)
    }
    /// Per-asset decoded RGBA budget, paired with `decode_with_limit`.
    pub fn encode_with_limit(self, maximum_source_rgba_bytes: u64) -> Result<Vec<u8>, CanvasError> {
        validate_budget(maximum_source_rgba_bytes)?;
        if self.requires_materialization() {
            return Err(CanvasError::Unsupported(
                "saving layered or GPU sources before GPU materialization",
            ));
        }
        if self.background.as_ref().is_some_and(|source| {
            source.pixels.as_raw().len() as u64 > maximum_source_rgba_bytes
                || source
                    .smart_erase_pixels
                    .as_ref()
                    .is_some_and(|pixels| pixels.as_raw().len() as u64 > maximum_source_rgba_bytes)
        }) {
            return Err(CanvasError::ResourceLimit);
        }
        let engine_bytes = serde_json::to_vec(&self.engine).map_err(error)?.len();
        let mut remaining = FILE_LIMIT
            .checked_sub(engine_bytes + 1024)
            .ok_or(CanvasError::ResourceLimit)?;
        let background = self
            .background
            .map(|source| {
                let png = encode_pixels(&source.pixels, &mut remaining)?;
                let eligible = source
                    .smart_erase_pixels
                    .as_ref()
                    .map(|pixels| encode_pixels(pixels, &mut remaining))
                    .transpose()?;
                Ok::<_, CanvasError>(SavedImage {
                    png,
                    origin: [source.origin().x, source.origin().y],
                    units_per_pixel: source.units_per_pixel(),
                    smart_erase_png: eligible,
                })
            })
            .transpose()?;
        let mut output = Limited {
            bytes: Vec::new(),
            limit: FILE_LIMIT,
        };
        serde_json::to_writer(
            &mut output,
            &DrawingFile {
                schema: 1,
                engine: self.engine,
                background,
            },
        )
        .map_err(error)?;
        Ok(output.bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, CanvasError> {
        Self::decode_with_limit(bytes, 512 * 1024 * 1024)
    }
    pub fn decode_with_limit(
        bytes: &[u8],
        maximum_source_rgba_bytes: u64,
    ) -> Result<Self, CanvasError> {
        validate_budget(maximum_source_rgba_bytes)?;
        if bytes.len() > FILE_LIMIT {
            return Err(CanvasError::ResourceLimit);
        }
        let file: DrawingFile = serde_json::from_slice(bytes).map_err(error)?;
        if file.schema != 1 {
            return Err(CanvasError::Unsupported("this drawing file version"));
        }
        let background = file
            .background
            .map(|source| {
                let png = STANDARD.decode(source.png).map_err(error)?;
                let mut image = SourceImage::decode_with_limit(&png, maximum_source_rgba_bytes)?
                    .with_placement(
                        Point::new(source.origin[0], source.origin[1]),
                        source.units_per_pixel,
                    )?;
                if let Some(png) = source.smart_erase_png {
                    let pixels = SourceImage::decode_with_limit(
                        &STANDARD.decode(png).map_err(error)?,
                        maximum_source_rgba_bytes,
                    )?;
                    if pixels.dimensions() != image.dimensions() {
                        return Err(CanvasError::InvalidFile(
                            "Reconstruction source dimensions differ".into(),
                        ));
                    }
                    image.smart_erase_pixels = Some(pixels.pixels.clone());
                }
                Ok::<_, CanvasError>(image)
            })
            .transpose()?;
        Ok(Self {
            engine: file.engine,
            background,
        })
    }
    pub fn into_document(self) -> Result<CanvasDocument, CanvasError> {
        let engine = Engine::from_serialized_document_session_with_config(
            &serde_json::to_vec(&self.engine).map_err(error)?,
            RuntimeConfig::default(),
        )?;
        Ok(CanvasDocument::from_parts(engine, self.background))
    }
}
fn validate_budget(bytes: u64) -> Result<(), CanvasError> {
    if bytes == 0 || bytes > crate::document::MAX_RASTER_BYTES {
        return Err(CanvasError::ResourceLimit);
    }
    Ok(())
}
fn error(error: impl std::fmt::Display) -> CanvasError {
    CanvasError::InvalidFile(error.to_string())
}
fn encode_pixels(pixels: &image::RgbaImage, remaining: &mut usize) -> Result<String, CanvasError> {
    let mut output = Limited {
        bytes: Vec::new(),
        limit: *remaining / 4 * 3,
    };
    let mut png = png::Encoder::new(&mut output, pixels.width(), pixels.height());
    png.set_color(png::ColorType::Rgba);
    png.set_depth(png::BitDepth::Eight);
    let mut png = png.write_header().map_err(error)?;
    {
        let mut stream = png.stream_writer_with_size(64 * 1024).map_err(error)?;
        stream.write_all(pixels.as_raw()).map_err(error)?;
        stream.finish().map_err(error)?;
    }
    png.finish().map_err(error)?;
    let encoded = STANDARD.encode(output.bytes);
    *remaining = remaining
        .checked_sub(encoded.len())
        .ok_or(CanvasError::ResourceLimit)?;
    Ok(encoded)
}
struct Limited {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            return Err(std::io::Error::other("Drawing exceeds512MiB file limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
