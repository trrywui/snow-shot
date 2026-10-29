//! Private, checked adapters for the native codecs. Only worker processes should call these APIs.
//! No dependency discovery, dynamic plugins, or native source downloads occur at runtime.
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum NativeFormat {
    Jpeg = 1,
    WebP = 2,
    Jxl = 3,
    Avif = 4,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum SampleType {
    #[default]
    U8 = 0,
    U16 = 1,
    F16 = 2,
    F32 = 3,
}
impl SampleType {
    pub fn bytes(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U16 | Self::F16 => 2,
            Self::F32 => 4,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum TransferFunction {
    #[default]
    Srgb = 1,
    Linear = 2,
    Pq = 3,
    Hlg = 4,
    Unknown = 0,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum ColorPrimaries {
    #[default]
    Srgb = 1,
    DisplayP3 = 2,
    Bt2020 = 3,
    Unknown = 0,
}
#[derive(Clone, Debug)]
pub struct NativeImage {
    pub width: u32,
    pub height: u32,
    pub sample: SampleType,
    /// Tightly packed, straight-alpha RGBA. Multi-byte values use native endianness.
    /// Integer samples span their storage type's full range; significant_bits records source precision.
    pub pixels: Vec<u8>,
    pub icc: Vec<u8>,
    pub exif: Vec<u8>,
    pub xmp: Vec<u8>,
    pub significant_bits: u8,
    pub transfer: TransferFunction,
    pub primaries: ColorPrimaries,
    /// EXIF orientation 1..8; decoding preserves the encoded orientation.
    pub orientation: u8,
    /// Explicit scRGB reference white in nits; zero means no scRGB tag.
    pub hdr_reference_white_nits: f32,
}
#[derive(Clone, Debug)]
pub struct NativeInfo {
    pub width: u32,
    pub height: u32,
    pub sample: SampleType,
    pub significant_bits: u8,
    pub transfer: TransferFunction,
    pub primaries: ColorPrimaries,
    pub orientation: u8,
    /// Explicit scRGB reference white in nits; zero means no scRGB tag.
    pub hdr_reference_white_nits: f32,
    pub has_alpha: bool,
    pub animated: bool,
    pub frame_count: u32,
    pub loop_count: u32,
    pub icc: Vec<u8>,
    pub exif: Vec<u8>,
    pub xmp: Vec<u8>,
}
#[derive(Clone, Copy, Debug)]
pub struct DecodeOptions {
    pub max_output_bytes: u64,
    pub max_memory_bytes: u64,
    pub max_metadata_bytes: u64,
    /// JPEG uses a native reduced decode no smaller than this target. Zero means full resolution.
    pub target_width: u32,
    pub target_height: u32,
    pub threads: u32,
}
impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            max_output_bytes: 512 << 20,
            max_memory_bytes: 512 << 20,
            max_metadata_bytes: 8 << 20,
            target_width: 0,
            target_height: 0,
            threads: 1,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct EncodeOptions {
    pub quality: f32,
    pub lossless: bool,
    pub effort: u32,
    pub exact: bool,
    pub threads: u32,
    pub max_output_bytes: u64,
    pub max_memory_bytes: u64,
    /// 444, 422 or 420. Invalid values are rejected.
    pub jpeg_subsampling: u32,
    pub progressive: bool,
    pub distance: f32,
    pub alpha_quality: f32,
    pub speed: u32,
    pub bit_depth: u8,
}
impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            quality: 90.0,
            lossless: false,
            effort: 5,
            exact: false,
            threads: 1,
            max_output_bytes: 512 << 20,
            max_memory_bytes: 512 << 20,
            jpeg_subsampling: 444,
            progressive: false,
            distance: 1.0,
            alpha_quality: 100.0,
            speed: 6,
            bit_depth: 8,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    BackendUnavailable,
    InvalidInput(String),
    Unsupported(String),
    ResourceLimit(String),
    Codec(String),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackendUnavailable => f.write_str("native backends were disabled at build time"),
            Self::InvalidInput(s)
            | Self::Unsupported(s)
            | Self::ResourceLimit(s)
            | Self::Codec(s) => f.write_str(s),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
/// Receives one contiguous RGBA region beginning at (column, row).
pub type FloatRegionWriter<'a> = dyn FnMut(u32, u32, &[f32]) -> Result<()> + 'a;
#[derive(Clone, Copy, Debug, Default)]
pub struct ResizeRequirements {
    pub scratch_bytes: u64,
    pub input_buffer_rows: u32,
    pub output_buffer_rows: u32,
    pub row_buffer_bytes: u64,
}

pub fn capabilities() -> &'static [NativeFormat] {
    if cfg!(feature = "native-backends") {
        &[
            NativeFormat::Jpeg,
            NativeFormat::WebP,
            NativeFormat::Jxl,
            NativeFormat::Avif,
        ]
    } else {
        &[]
    }
}

#[cfg(feature = "native-backends")]
mod enabled;
#[cfg(feature = "native-backends")]
pub use enabled::*;
#[cfg(not(feature = "native-backends"))]
mod disabled {
    use super::*;
    pub fn inspect(_: NativeFormat, _: &[u8], _: DecodeOptions) -> Result<NativeInfo> {
        Err(Error::BackendUnavailable)
    }
    pub fn decode(_: NativeFormat, _: &[u8], _: DecodeOptions) -> Result<NativeImage> {
        Err(Error::BackendUnavailable)
    }
    pub fn decode_jpeg_into(_: &[u8], _: DecodeOptions, _: &mut [u8]) -> Result<NativeInfo> {
        Err(Error::BackendUnavailable)
    }
    pub fn encode(_: NativeFormat, _: &NativeImage, _: EncodeOptions) -> Result<Vec<u8>> {
        Err(Error::BackendUnavailable)
    }
    pub fn encode_to_writer(
        _: NativeFormat,
        _: &NativeImage,
        _: EncodeOptions,
        _: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        Err(Error::BackendUnavailable)
    }
    pub fn encode_jxl_rows(
        _: &NativeInfo,
        _: EncodeOptions,
        _: &mut (dyn FnMut(u32, &mut [u8]) -> Result<()> + Send),
        _: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        Err(Error::BackendUnavailable)
    }
    pub fn encode_jpeg_rows(
        _: u32,
        _: u32,
        _: EncodeOptions,
        _: &mut dyn FnMut(u32, &mut [u8]) -> Result<()>,
        _: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        Err(Error::BackendUnavailable)
    }
    pub fn resize_regions_f32(
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: u64,
        _: &mut dyn FnMut(u32, &mut [f32]) -> Result<()>,
        _: &mut FloatRegionWriter<'_>,
    ) -> Result<()> {
        Err(Error::BackendUnavailable)
    }
    pub fn transform_rgba_f32(_: &[u8], _: &mut [f32], _: bool) -> Result<()> {
        Err(Error::BackendUnavailable)
    }
    pub fn create_rgb_profile(
        _: [f64; 2],
        _: [[f64; 2]; 3],
        _: [f64; 3],
        _: u64,
    ) -> Result<Vec<u8>> {
        Err(Error::BackendUnavailable)
    }
    pub fn transform_rgba8(_: &[u8], _: &mut [u8]) -> Result<()> {
        Err(Error::BackendUnavailable)
    }
    pub fn transform_rgba16(_: &[u8], _: &mut [u16]) -> Result<()> {
        Err(Error::BackendUnavailable)
    }
    pub fn resize_requirements(_: u32, _: u32, _: u32, _: u32) -> Result<ResizeRequirements> {
        Err(Error::BackendUnavailable)
    }
    pub fn resize_rgba_f32(_: &[f32], _: u32, _: u32, _: u32, _: u32, _: u64) -> Result<Vec<f32>> {
        Err(Error::BackendUnavailable)
    }
    pub struct WebPAnimation;
    impl WebPAnimation {
        pub fn open(_: std::sync::Arc<[u8]>, _: DecodeOptions) -> Result<Self> {
            Err(Error::BackendUnavailable)
        }
        pub fn next_frame(&mut self) -> Result<Option<NativeFrame>> {
            Err(Error::BackendUnavailable)
        }
        pub fn reset(&mut self) {}
    }
}
#[cfg(not(feature = "native-backends"))]
pub use disabled::*;
#[derive(Debug)]
pub struct NativeFrame {
    pub image: NativeImage,
    pub duration_ms: u32,
}
