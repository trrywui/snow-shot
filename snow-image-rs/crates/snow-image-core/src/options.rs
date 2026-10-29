use serde::{Deserialize, Serialize};

use crate::{ColorEncoding, Error, Metadata, Orientation, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    WebP,
    Jxl,
    Avif,
    Bmp,
    Ico,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecodeOptions {
    pub apply_orientation: bool,
    pub preserve_metadata: bool,
    pub preferred_frame: Option<u32>,
}
impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            apply_orientation: true,
            preserve_metadata: true,
            preferred_frame: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum PngCompression {
    Fast,
    #[default]
    Default,
    Best,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum PngFilter {
    None,
    Sub,
    Up,
    Average,
    Paeth,
    #[default]
    Adaptive,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PngOptions {
    pub compression: PngCompression,
    pub filter: PngFilter,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum JpegSubsampling {
    #[default]
    S444,
    S422,
    S420,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JpegOptions {
    pub quality: u8,
    pub subsampling: JpegSubsampling,
    pub progressive: bool,
}
impl Default for JpegOptions {
    fn default() -> Self {
        Self {
            quality: 95,
            subsampling: JpegSubsampling::S444,
            progressive: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WebPOptions {
    pub quality: u8,
    pub lossless: bool,
    pub exact: bool,
    pub effort: u8,
}
impl Default for WebPOptions {
    fn default() -> Self {
        Self {
            quality: 90,
            lossless: true,
            exact: true,
            effort: 4,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JxlOptions {
    pub distance: f32,
    pub effort: u8,
    pub exact: bool,
}
impl Default for JxlOptions {
    fn default() -> Self {
        Self {
            distance: 0.0,
            effort: 7,
            exact: true,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AvifOptions {
    /// Require exact accepted samples, including transparent RGB. Both qualities must be 100.
    pub exact: bool,
    pub quality: u8,
    pub alpha_quality: u8,
    pub speed: u8,
    pub bit_depth: u8,
}
impl Default for AvifOptions {
    fn default() -> Self {
        Self {
            exact: false,
            quality: 90,
            alpha_quality: 100,
            speed: 6,
            bit_depth: 10,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BmpOptions {
    pub preserve_alpha: bool,
}
impl Default for BmpOptions {
    fn default() -> Self {
        Self {
            preserve_alpha: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EncodeOptions {
    Png(PngOptions),
    Jpeg(JpegOptions),
    WebP(WebPOptions),
    Jxl(JxlOptions),
    Avif(AvifOptions),
    Bmp(BmpOptions),
}
impl Default for EncodeOptions {
    fn default() -> Self {
        Self::Png(PngOptions::default())
    }
}
impl EncodeOptions {
    pub fn format(&self) -> Format {
        match self {
            Self::Png(_) => Format::Png,
            Self::Jpeg(_) => Format::Jpeg,
            Self::WebP(_) => Format::WebP,
            Self::Jxl(_) => Format::Jxl,
            Self::Avif(_) => Format::Avif,
            Self::Bmp(_) => Format::Bmp,
        }
    }
    pub fn validate(&self) -> Result<()> {
        let valid = match self {
            Self::Png(_) | Self::Bmp(_) => true,
            Self::Jpeg(options) => (1..=100).contains(&options.quality),
            Self::WebP(options) => {
                options.quality <= 100
                    && options.effort <= 6
                    && (!options.exact || options.lossless)
            }
            Self::Jxl(options) => {
                options.distance.is_finite()
                    && (0.0..=25.0).contains(&options.distance)
                    && (1..=10).contains(&options.effort)
                    && (!options.exact || options.distance == 0.0)
            }
            Self::Avif(options) => {
                options.quality <= 100
                    && options.alpha_quality <= 100
                    && options.speed <= 10
                    && matches!(options.bit_depth, 8 | 10 | 12)
                    && (!options.exact || (options.quality == 100 && options.alpha_quality == 100))
            }
        };
        if valid {
            Ok(())
        } else {
            Err(Error::InvalidInput(
                "invalid encoder options or contradictory exactness request".into(),
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum DynamicRangePolicy {
    PreserveDynamicRange,
    #[default]
    ScreenshotSdr,
}
/// Explicit HDR color conversion and quantization for interoperable AVIF/JXL export.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum HdrOutput {
    Bt2020Pq10,
    Bt2020Pq12,
}
impl HdrOutput {
    pub fn bit_depth(self) -> u8 {
        match self {
            Self::Bt2020Pq10 => 10,
            Self::Bt2020Pq12 => 12,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransformOptions {
    /// Fit inside these bounds without upscaling. None preserves dimensions.
    pub max_dimensions: Option<(u32, u32)>,
    pub dynamic_range: DynamicRangePolicy,
    /// None preserves source precision under PreserveDynamicRange. Some explicitly converts
    /// absolute linear light to BT.2020 PQ and quantizes RGB/alpha to the selected precision.
    pub hdr_output: Option<HdrOutput>,
    /// Source mastering/display peak used by ScreenshotSdr tone mapping, in cd/m².
    pub hdr_peak_nits: f32,
    pub apply_orientation: bool,
}
impl Default for TransformOptions {
    fn default() -> Self {
        Self {
            max_dimensions: None,
            dynamic_range: DynamicRangePolicy::ScreenshotSdr,
            hdr_output: None,
            hdr_peak_nits: 1000.0,
            apply_orientation: true,
        }
    }
}
impl TransformOptions {
    pub fn validate(&self) -> Result<()> {
        if self.hdr_output.is_some()
            && self.dynamic_range != DynamicRangePolicy::PreserveDynamicRange
        {
            return Err(Error::InvalidInput(
                "HDR export preparation requires PreserveDynamicRange".into(),
            ));
        }
        if !self.hdr_peak_nits.is_finite() || self.hdr_peak_nits <= 0.0 {
            return Err(Error::InvalidInput(
                "HDR source peak must be finite and positive".into(),
            ));
        }
        if self
            .max_dimensions
            .is_some_and(|(width, height)| width == 0 || height == 0)
        {
            Err(Error::InvalidInput(
                "transform bounds must be nonzero".into(),
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageInfo {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub has_alpha: bool,
    pub frame_count: u32,
    pub animated: bool,
    pub color: ColorEncoding,
    pub orientation: Orientation,
    pub metadata: Metadata,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PixelFidelity {
    /// Every accepted sample, including RGB under zero alpha, is reproducible.
    Exact,
    /// Reversible codec after an explicit transform (such as color conversion).
    LosslessTransformed,
    Lossy,
}
pub type Fidelity = PixelFidelity;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EncodeReceipt {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub bytes: u64,
    pub options: EncodeOptions,
    pub fidelity: PixelFidelity,
}

/// Capabilities of the actual worker build, not merely advertised file extensions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodecCapability {
    pub format: Format,
    pub decode: bool,
    pub encode: bool,
    pub animation_decode: bool,
    pub input_sample_types: Vec<crate::SampleType>,
    pub supports_alpha: bool,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
}
