//! Budgeted codec implementation. This crate belongs in the isolated worker, not the UI process.
#![forbid(unsafe_code)]

mod animation;
mod decode;
mod encode;
mod headers;
mod processing;

pub use animation::{AnimationFrame, AnimationInfo, AnimationSession};
pub use decode::{DirectJpegDecode, decode, inspect};
pub use encode::encode;
pub use processing::{PreparedTransform, prepare_transform, preview, transform, transform_source};

use snow_image_core::*;

fn codec_error(error: impl std::fmt::Display + 'static) -> Error {
    if let Some(error) =
        (&error as &dyn std::any::Any).downcast_ref::<snow_image_native_sys::Error>()
    {
        return match error {
            snow_image_native_sys::Error::BackendUnavailable => {
                Error::Unsupported(error.to_string().into())
            }
            snow_image_native_sys::Error::InvalidInput(message) => {
                Error::InvalidInput(message.as_str().into())
            }
            snow_image_native_sys::Error::Unsupported(message) => {
                Error::Unsupported(message.as_str().into())
            }
            snow_image_native_sys::Error::ResourceLimit(message) => {
                Error::BackendResourceLimit(message.as_str().into())
            }
            snow_image_native_sys::Error::Codec(message) => Error::Codec(message.as_str().into()),
        };
    }
    Error::Codec(error.to_string().into())
}

fn checked_bytes(width: u32, height: u32, pixel_bytes: usize) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(Error::InvalidInput(
            "image dimensions must be nonzero".into(),
        ));
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|v| v.checked_mul(pixel_bytes))
        .ok_or_else(|| Error::InvalidInput("image dimensions overflow address space".into()))
}

fn zeroed(length: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| Error::AllocationFailed)?;
    bytes.resize(length, 0);
    Ok(bytes)
}

fn clone_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut copy = zeroed(bytes.len())?;
    copy.copy_from_slice(bytes);
    Ok(copy)
}

fn rgba_descriptor(
    width: u32,
    height: u32,
    sample: SampleType,
    color: ColorEncoding,
) -> Result<ImageDescriptor> {
    let sample_bytes = match sample {
        SampleType::U8 => 1,
        SampleType::U16 | SampleType::F16 => 2,
        SampleType::F32 => 4,
    };
    let row_stride = checked_bytes(width, 1, 4 * sample_bytes)?;
    Ok(ImageDescriptor {
        width,
        height,
        row_stride,
        format: PixelFormat {
            channels: ChannelOrder::Rgba,
            sample,
            alpha: AlphaMode::Straight,
            significant_bits: (8 * sample_bytes) as u8,
        },
        color,
        orientation: Orientation::Normal,
    })
}

/// Reports the linked worker's actual format adapters. Resource policy can impose lower limits.
pub fn capabilities() -> Vec<CodecCapability> {
    let native = snow_image_native_sys::capabilities();
    let available = |format| native.contains(&format);
    let entry =
        |format, decode, encode, animation_decode, input_sample_types, supports_alpha, limit| {
            CodecCapability {
                format,
                decode,
                encode,
                animation_decode,
                input_sample_types,
                supports_alpha,
                max_width: limit,
                max_height: limit,
            }
        };
    vec![
        entry(
            Format::Png,
            true,
            true,
            true,
            vec![SampleType::U8, SampleType::U16],
            true,
            Some(i32::MAX as u32),
        ),
        entry(
            Format::Gif,
            true,
            false,
            true,
            vec![],
            true,
            Some(u16::MAX as u32),
        ),
        entry(
            Format::Bmp,
            true,
            true,
            false,
            vec![SampleType::U8],
            true,
            Some(i32::MAX as u32),
        ),
        entry(Format::Ico, true, false, false, vec![], true, Some(256)),
        entry(
            Format::Jpeg,
            available(snow_image_native_sys::NativeFormat::Jpeg),
            available(snow_image_native_sys::NativeFormat::Jpeg),
            false,
            if available(snow_image_native_sys::NativeFormat::Jpeg) {
                vec![SampleType::U8]
            } else {
                vec![]
            },
            false,
            Some(65_500),
        ),
        entry(
            Format::WebP,
            available(snow_image_native_sys::NativeFormat::WebP),
            available(snow_image_native_sys::NativeFormat::WebP),
            available(snow_image_native_sys::NativeFormat::WebP),
            if available(snow_image_native_sys::NativeFormat::WebP) {
                vec![SampleType::U8]
            } else {
                vec![]
            },
            true,
            Some(16_383),
        ),
        entry(
            Format::Jxl,
            available(snow_image_native_sys::NativeFormat::Jxl),
            available(snow_image_native_sys::NativeFormat::Jxl),
            false,
            if available(snow_image_native_sys::NativeFormat::Jxl) {
                vec![
                    SampleType::U8,
                    SampleType::U16,
                    SampleType::F16,
                    SampleType::F32,
                ]
            } else {
                vec![]
            },
            true,
            None,
        ),
        entry(
            Format::Avif,
            available(snow_image_native_sys::NativeFormat::Avif),
            available(snow_image_native_sys::NativeFormat::Avif),
            false,
            if available(snow_image_native_sys::NativeFormat::Avif) {
                vec![SampleType::U8, SampleType::U16]
            } else {
                vec![]
            },
            true,
            None,
        ),
    ]
}
