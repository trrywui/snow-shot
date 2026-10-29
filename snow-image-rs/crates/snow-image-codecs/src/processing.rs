use crate::{codec_error, rgba_descriptor};
use snow_image_core::*;
use snow_image_native_sys as native;

/// Reuses immutable transformed pixels across export settings. Reservations follow the final owner.
#[derive(Clone, Debug)]
pub struct PreparedTransform {
    image: Image,
}
impl PreparedTransform {
    pub fn image(&self) -> &Image {
        &self.image
    }
    pub fn into_image(self) -> Image {
        self.image
    }
}

pub fn prepare_transform(
    image: &Image,
    options: &TransformOptions,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<PreparedTransform> {
    let result = transform(image, options, budget, cancel)?;
    if result.bytes().len() as u64 > budget.policy().max_prepared_bytes {
        return Err(Error::ResourceLimit {
            resource: ResourceKind::PreparedPixels,
            requested: result.bytes().len() as u64,
            limit: budget.policy().max_prepared_bytes,
        });
    }
    Ok(PreparedTransform { image: result })
}

pub fn preview(
    input: &[u8],
    max_dimensions: (u32, u32),
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<Image> {
    if max_dimensions.0 == 0 || max_dimensions.1 == 0 {
        return Err(Error::InvalidInput("preview bounds must be nonzero".into()));
    }
    let limit = budget.policy().preview_max_dimension;
    let bounds = (max_dimensions.0.min(limit), max_dimensions.1.min(limit));
    let format = crate::headers::identify(input)?;
    if format == Format::Png {
        let info = crate::inspect(input, &DecodeOptions::default(), budget, cancel)?;
        let resident = crate::checked_bytes(info.width, info.height, 20)? as u64;
        if resident
            > budget.policy().working_bytes_per_job.min(
                budget
                    .policy()
                    .total_memory_bytes
                    .saturating_sub(budget.total_used()),
            )
        {
            let source = crate::decode::decode_png_to_file(input, info, budget, cancel)?;
            let result = transform_source(
                &source,
                &TransformOptions {
                    max_dimensions: Some(bounds),
                    ..Default::default()
                },
                budget,
                cancel,
            )?;
            let mut pixels =
                MutableImage::new(result.descriptor().clone(), Metadata::default(), budget)?;
            result.read_rows(0, result.descriptor().height, pixels.bytes_mut())?;
            return Ok(pixels.seal());
        }
    }
    let image = if format == Format::Jpeg {
        let info = crate::inspect(input, &DecodeOptions::default(), budget, cancel)?;
        let target = if info.orientation.swaps_dimensions() {
            let fitted = fit(info.height, info.width, Some(bounds));
            (fitted.1, fitted.0)
        } else {
            fit(info.width, info.height, Some(bounds))
        };
        crate::decode::decode_native(
            format,
            input,
            &DecodeOptions::default(),
            budget,
            Some(target),
        )?
    } else {
        crate::decode(input, &DecodeOptions::default(), budget, cancel)?
    };
    transform(
        &image,
        &TransformOptions {
            max_dimensions: Some(bounds),
            ..Default::default()
        },
        budget,
        cancel,
    )
}

pub fn transform(
    image: &Image,
    options: &TransformOptions,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<Image> {
    options.validate()?;
    cancel.check()?;
    let image = if options.apply_orientation {
        crate::decode::apply_orientation(image.clone(), budget, cancel)?
    } else {
        image.clone()
    };
    let d = image.descriptor();
    let (out_width, out_height) = fit(d.width, d.height, options.max_dimensions);
    let resize = (out_width, out_height) != (d.width, d.height);
    if !resize
        && options.dynamic_range == DynamicRangePolicy::PreserveDynamicRange
        && options.hdr_output.is_none()
    {
        return Ok(image);
    }
    if !resize
        && options.hdr_output.is_none()
        && d.color == ColorEncoding::Srgb
        && d.format.sample == SampleType::U8
        && d.format.alpha != AlphaMode::Premultiplied
    {
        // Screenshot export removes imported EXIF/XMP without copying image samples.
        if image.metadata().exif.is_none() && image.metadata().xmp.is_none() {
            return Ok(image);
        }
    }
    if resize
        && options.dynamic_range == DynamicRangePolicy::ScreenshotSdr
        && options.hdr_output.is_none()
        && d.color == ColorEncoding::Srgb
        && d.format.sample == SampleType::U8
        && d.format.alpha != AlphaMode::Premultiplied
        && d.width.div_ceil(16) <= out_width
        && d.height.div_ceil(16) <= out_height
    {
        return resize_srgb_rows(&image, out_width, out_height, budget, cancel);
    }
    let generated_profile = calibrated_profile(&d.color, budget)?;
    let input_len = crate::checked_bytes(d.width, d.height, 4)?;
    let input_bytes = (input_len as u64)
        .checked_mul(4)
        .ok_or(Error::AllocationFailed)?;
    if input_bytes
        > budget.policy().working_bytes_per_job.min(
            budget
                .policy()
                .total_memory_bytes
                .saturating_sub(budget.total_used()),
        )
    {
        let file = transform_source(&image, options, budget, cancel)?;
        let mut output = MutableImage::new(
            file.descriptor().clone(),
            file.metadata().cloned().unwrap_or_default(),
            budget,
        )?;
        file.read_rows(0, file.descriptor().height, output.bytes_mut())?;
        return Ok(output.seal());
    }
    let mut scratch = budget.reserve(ResourceKind::Scratch, input_bytes)?;
    let mut linear = Vec::new();
    linear
        .try_reserve_exact(input_len)
        .map_err(|_| Error::AllocationFailed)?;
    linear.resize(input_len, 0.0f32);
    let curve = HdrCurve::new(options.hdr_peak_nits);
    for (y, row) in image.bytes().chunks_exact(d.row_stride).enumerate() {
        cancel.check()?;
        linear_row(
            &row[..d.row_bytes()?],
            &mut linear[y * d.width as usize * 4..(y + 1) * d.width as usize * 4],
            d,
            options.dynamic_range == DynamicRangePolicy::PreserveDynamicRange,
        )?;
    }
    if needs_icc_transform(&d.color) {
        native::transform_rgba_f32(
            generated_profile
                .as_ref()
                .map(|p| p.0.as_slice())
                .or(image.metadata().icc.as_deref())
                .ok_or_else(|| Error::InvalidInput("ICC profile is missing".into()))?,
            &mut linear,
            true,
        )
        .map_err(codec_error)?;
    }
    if options.dynamic_range == DynamicRangePolicy::ScreenshotSdr && d.color.is_hdr() {
        for p in linear.chunks_exact_mut(4) {
            let mut rgb = [p[0], p[1], p[2]];
            tone_map_with_curve(&mut rgb, &curve);
            p[..3].copy_from_slice(&rgb);
        }
    }
    let mut width = d.width;
    let mut height = d.height;
    let opaque = d.format.alpha == AlphaMode::Opaque || !d.format.channels.has_alpha();
    if resize {
        if !opaque {
            for p in linear.chunks_exact_mut(4) {
                for c in 0..3 {
                    p[c] *= p[3];
                }
            }
        }
        while width != out_width || height != out_height {
            cancel.check()?;
            let next_width = out_width.max(width.div_ceil(16));
            let next_height = out_height.max(height.div_ceil(16));
            let requirements = native::resize_requirements(width, height, next_width, next_height)
                .map_err(codec_error)?;
            let output_bytes = crate::checked_bytes(next_width, next_height, 16)? as u64;
            let _operation = budget.reserve(
                ResourceKind::Scratch,
                output_bytes
                    .checked_add(requirements.scratch_bytes)
                    .and_then(|v| v.checked_add(requirements.row_buffer_bytes))
                    .ok_or(Error::AllocationFailed)?,
            )?;
            let next = native::resize_rgba_f32(
                &linear,
                width,
                height,
                next_width,
                next_height,
                requirements.scratch_bytes + requirements.row_buffer_bytes,
            )
            .map_err(codec_error)?;
            linear = next;
            width = next_width;
            height = next_height;
            drop(_operation);
            scratch.resize(output_bytes)?;
        }
        if !opaque {
            for p in linear.chunks_exact_mut(4) {
                if p[3] > 0.0 {
                    for c in 0..3 {
                        p[c] /= p[3];
                    }
                } else {
                    p[..3].fill(0.0);
                }
            }
        }
    }
    let preserve = options.dynamic_range == DynamicRangePolicy::PreserveDynamicRange;
    let mut descriptor = rgba_descriptor(
        width,
        height,
        if options.hdr_output.is_some() {
            SampleType::U16
        } else if preserve {
            SampleType::F32
        } else {
            SampleType::U8
        },
        if options.hdr_output.is_some() {
            ColorEncoding::Bt2020Pq
        } else if preserve {
            ColorEncoding::LinearSrgb
        } else {
            ColorEncoding::Srgb
        },
    )?;
    if let Some(target) = options.hdr_output {
        descriptor.format.significant_bits = target.bit_depth();
    }
    if opaque {
        descriptor.format.alpha = AlphaMode::Opaque;
    }
    if preserve && d.color.is_hdr() && options.hdr_output.is_none() {
        descriptor.color = preserved_hdr_color(&d.color);
    }
    if !options.apply_orientation {
        descriptor.orientation = d.orientation;
    }
    let mut output = MutableImage::new(descriptor, Metadata::default(), budget)?;
    if let Some(target) = options.hdr_output {
        for (pixel, bytes) in linear
            .chunks_exact(4)
            .zip(output.bytes_mut().chunks_exact_mut(8))
        {
            encode_pq_pixel(
                [pixel[0], pixel[1], pixel[2], pixel[3]],
                target.bit_depth(),
                bytes,
            )?;
        }
    } else if preserve {
        for (sample, bytes) in linear.iter().zip(output.bytes_mut().chunks_exact_mut(4)) {
            bytes.copy_from_slice(&sample.to_ne_bytes());
        }
    } else {
        let quantizer = srgb_quantizer();
        for (p, o) in linear
            .chunks_exact(4)
            .zip(output.bytes_mut().chunks_exact_mut(4))
        {
            o[0] = quantizer.encode(p[0]);
            o[1] = quantizer.encode(p[1]);
            o[2] = quantizer.encode(p[2]);
            o[3] = (p[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
    Ok(output.seal())
}

// Convert rows directly into zimg's admitted ring buffers and quantize each output region
// into its final owner. This avoids full-size floating-point intermediates for SDR previews.
fn resize_srgb_rows(
    image: &Image,
    width: u32,
    height: u32,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<Image> {
    let d = image.descriptor();
    let opaque = d.format.alpha == AlphaMode::Opaque || !d.format.channels.has_alpha();
    let requirements =
        native::resize_requirements(d.width, d.height, width, height).map_err(codec_error)?;
    let scratch_bytes = requirements
        .scratch_bytes
        .checked_add(requirements.row_buffer_bytes)
        .ok_or(Error::AllocationFailed)?;
    let _scratch = budget.reserve(ResourceKind::Scratch, scratch_bytes)?;
    let mut descriptor = rgba_descriptor(width, height, SampleType::U8, ColorEncoding::Srgb)?;
    descriptor.orientation = d.orientation;
    if opaque {
        descriptor.format.alpha = AlphaMode::Opaque;
    }
    let mut output = MutableImage::new(descriptor, Metadata::default(), budget)?;
    let row_bytes = d.row_bytes()?;
    let mut read = |y: u32, destination: &mut [f32]| -> native::Result<()> {
        cancel
            .check()
            .map_err(|e| native::Error::Codec(e.to_string()))?;
        let start = y as usize * d.row_stride;
        linear_row(
            &image.bytes()[start..start + row_bytes],
            destination,
            d,
            false,
        )
        .map_err(|e| native::Error::Codec(e.to_string()))?;
        if !opaque {
            for pixel in destination.chunks_exact_mut(4) {
                for c in 0..3 {
                    pixel[c] *= pixel[3];
                }
            }
        }
        Ok(())
    };
    let quantizer = srgb_quantizer();
    let mut write = |y: u32, x: u32, source: &[f32]| -> native::Result<()> {
        cancel
            .check()
            .map_err(|e| native::Error::Codec(e.to_string()))?;
        let start = (y as usize * width as usize + x as usize) * 4;
        for (pixel, bytes) in source
            .chunks_exact(4)
            .zip(output.bytes_mut()[start..start + source.len()].chunks_exact_mut(4))
        {
            let alpha = pixel[3].clamp(0.0, 1.0);
            if opaque {
                bytes[0] = quantizer.encode(pixel[0]);
                bytes[1] = quantizer.encode(pixel[1]);
                bytes[2] = quantizer.encode(pixel[2]);
                bytes[3] = 255;
            } else {
                if pixel[3] > 0.0 {
                    bytes[0] = quantizer.encode(pixel[0] / pixel[3]);
                    bytes[1] = quantizer.encode(pixel[1] / pixel[3]);
                    bytes[2] = quantizer.encode(pixel[2] / pixel[3]);
                } else {
                    bytes[..3].fill(0);
                }
                bytes[3] = (alpha * 255.0 + 0.5) as u8;
            }
        }
        Ok(())
    };
    native::resize_regions_f32(
        d.width,
        d.height,
        width,
        height,
        scratch_bytes,
        &mut read,
        &mut write,
    )
    .map_err(|error| {
        if cancel.is_cancelled() {
            Error::Cancelled
        } else {
            codec_error(error)
        }
    })?;
    Ok(output.seal())
}

fn fit(width: u32, height: u32, bounds: Option<(u32, u32)>) -> (u32, u32) {
    let Some((w, h)) = bounds else {
        return (width, height);
    };
    if width <= w && height <= h {
        return (width, height);
    }
    if u64::from(w) * u64::from(height) <= u64::from(h) * u64::from(width) {
        (
            w,
            ((u64::from(height) * u64::from(w) / u64::from(width)) as u32).max(1),
        )
    } else {
        (
            ((u64::from(width) * u64::from(h) / u64::from(height)) as u32).max(1),
            h,
        )
    }
}

fn needs_icc_transform(color: &ColorEncoding) -> bool {
    matches!(
        color,
        ColorEncoding::Icc
            | ColorEncoding::CalibratedRgb { .. }
            | ColorEncoding::CalibratedBmp { .. }
    )
}
fn calibrated_profile(
    color: &ColorEncoding,
    budget: &Budget,
) -> Result<Option<(Vec<u8>, Reservation)>> {
    let (white, primaries, gamma) = match color {
        ColorEncoding::CalibratedRgb {
            gamma: Some(gamma),
            chromaticities: Some(c),
        } => (c.white, [c.red, c.green, c.blue], [1.0 / gamma; 3]),
        ColorEncoding::CalibratedRgb { .. } => {
            return Err(Error::Unsupported(
                "calibrated PNG requires both gamma and chromaticities for color conversion".into(),
            ));
        }
        ColorEncoding::CalibratedBmp { endpoints, gamma } => {
            if gamma.iter().any(|v| *v <= 0.0) {
                return Err(Error::Unsupported(
                    "calibrated BMP has unspecified channel gamma".into(),
                ));
            }
            let xy = |xyz: [f64; 3]| -> Result<[f64; 2]> {
                let sum = xyz.iter().sum::<f64>();
                if sum <= 0.0 || !sum.is_finite() {
                    return Err(Error::Unsupported(
                        "calibrated BMP has incomplete endpoints".into(),
                    ));
                }
                Ok([xyz[0] / sum, xyz[1] / sum])
            };
            let white =
                std::array::from_fn(|coordinate| endpoints.iter().map(|v| v[coordinate]).sum());
            (
                xy(white)?,
                [xy(endpoints[0])?, xy(endpoints[1])?, xy(endpoints[2])?],
                *gamma,
            )
        }
        _ => return Ok(None),
    };
    let mut reservation =
        budget.reserve(ResourceKind::Scratch, budget.policy().max_metadata_bytes)?;
    let profile =
        native::create_rgb_profile(white, primaries, gamma, budget.policy().max_metadata_bytes)
            .map_err(codec_error)?;
    reservation.resize(profile.capacity() as u64)?;
    Ok(Some((profile, reservation)))
}

fn linear_pixel(
    pixel: &[u8],
    descriptor: &ImageDescriptor,
    preserve_dynamic_range: bool,
) -> Result<[f32; 4]> {
    let format = descriptor.format;
    let b = format.sample.bytes();
    let read = |index: usize| -> f32 {
        let bytes = &pixel[index * b..(index + 1) * b];
        match format.sample {
            SampleType::U8 => f32::from(bytes[0]) / 255.0,
            SampleType::U16 => f32::from(u16::from_ne_bytes(bytes.try_into().unwrap())) / 65535.0,
            SampleType::F16 => {
                half::f16::from_bits(u16::from_ne_bytes(bytes.try_into().unwrap())).to_f32()
            }
            SampleType::F32 => f32::from_ne_bytes(bytes.try_into().unwrap()),
        }
    };
    let mut p = match format.channels {
        ChannelOrder::Gray => [read(0), read(0), read(0), 1.0],
        ChannelOrder::GrayAlpha => [read(0), read(0), read(0), read(1)],
        ChannelOrder::Rgb => [read(0), read(1), read(2), 1.0],
        ChannelOrder::Bgr => [read(2), read(1), read(0), 1.0],
        ChannelOrder::Rgba => [read(0), read(1), read(2), read(3)],
        ChannelOrder::Bgra => [read(2), read(1), read(0), read(3)],
    };
    if p.iter().any(|v| !v.is_finite()) {
        return Err(Error::InvalidInput("non-finite image sample".into()));
    }
    if format.alpha == AlphaMode::Opaque {
        p[3] = 1.0;
    } else {
        p[3] = p[3].clamp(0.0, 1.0);
    }
    if format.alpha == AlphaMode::Premultiplied {
        if p[3] > 0.0 {
            for c in 0..3 {
                p[c] /= p[3];
            }
        } else {
            p[..3].fill(0.0);
        }
    }
    if matches!(
        descriptor.color,
        ColorEncoding::Bt2020Pq | ColorEncoding::Bt2020Hlg { .. }
    ) && p[..3].iter().any(|v| !(0.0..=1.0).contains(v))
    {
        return Err(Error::InvalidInput(
            "PQ and HLG signal values must be in [0,1]".into(),
        ));
    }
    match descriptor.color {
        ColorEncoding::Srgb | ColorEncoding::DisplayP3 => {
            if format.sample == SampleType::U8 && format.alpha != AlphaMode::Premultiplied {
                let table = srgb_byte_table();
                for v in &mut p[..3] {
                    *v = table[(*v * 255.0 + 0.5) as usize];
                }
            } else {
                for v in &mut p[..3] {
                    *v = srgb_to_linear(*v);
                }
            }
        }
        ColorEncoding::ScRgb {
            reference_white_nits,
        } => {
            if !preserve_dynamic_range {
                for v in &mut p[..3] {
                    *v *= 80.0 / reference_white_nits;
                }
            }
        }
        ColorEncoding::Bt2020Pq => {
            for v in &mut p[..3] {
                *v = pq_to_nits(*v) / 80.0;
            }
        }
        ColorEncoding::Bt2020Hlg {
            reference_white_nits,
        } => {
            let mut rgb = [0.0f32; 3];
            for (dst, v) in rgb.iter_mut().zip(&p[..3]) {
                *dst = if *v <= 0.5 {
                    v * v / 3.0
                } else {
                    ((v - 0.559_910_7) / 0.178_832_77)
                        .exp()
                        .mul_add(1.0, 0.284_668_92)
                        / 12.0
                };
            }
            let luminance = (0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2]).max(0.0);
            // HLG diffuse white is the 75% signal, with the reference 1.2 system gamma.
            let white_scene =
                (((0.75f32 - 0.559_910_7) / 0.178_832_77).exp() + 0.284_668_92) / 12.0;
            let gain = luminance.powf(0.2) * reference_white_nits / (80.0 * white_scene.powf(1.2));
            for c in 0..3 {
                p[c] = rgb[c] * gain;
            }
        }
        ColorEncoding::LinearSrgb | ColorEncoding::Icc => {}
        ColorEncoding::CalibratedRgb { .. } | ColorEncoding::CalibratedBmp { .. } => {}
        ColorEncoding::Unknown => {
            return Err(Error::Unsupported(
                "unknown source color requires an explicit profile".into(),
            ));
        }
    }
    if descriptor.color == ColorEncoding::DisplayP3 {
        let rgb = [p[0], p[1], p[2]];
        p[0] = 1.224_940_2 * rgb[0] - 0.224_940_18 * rgb[1];
        p[1] = -0.042_056_95 * rgb[0] + 1.042_057 * rgb[1];
        p[2] = -0.019_637_55 * rgb[0] - 0.078_636_05 * rgb[1] + 1.098_273_6 * rgb[2];
    }
    if matches!(
        descriptor.color,
        ColorEncoding::Bt2020Pq | ColorEncoding::Bt2020Hlg { .. }
    ) {
        let rgb = [p[0], p[1], p[2]];
        p[0] = 1.660_491 * rgb[0] - 0.587_641_1 * rgb[1] - 0.072_849_9 * rgb[2];
        p[1] = -0.124_550_5 * rgb[0] + 1.132_899_9 * rgb[1] - 0.008_349_4 * rgb[2];
        p[2] = -0.018_150_8 * rgb[0] - 0.100_578_9 * rgb[1] + 1.118_729_7 * rgb[2];
    }
    Ok(p)
}
fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}
/// Exact nearest-byte sRGB quantization without per-sample transcendental operations.
/// The coarse table supplies a lower candidate. Each interval is narrower than the
/// minimum distance between adjacent sRGB byte boundaries, so one comparison suffices.
struct SrgbQuantizer {
    candidates: [u8; 4097],
    thresholds: [f32; 256],
}
fn srgb_quantizer() -> &'static SrgbQuantizer {
    static TABLE: std::sync::OnceLock<SrgbQuantizer> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let thresholds = std::array::from_fn(|index| {
            if index == 255 {
                return f32::INFINITY;
            }
            let encoded = (index as f64 + 0.5) / 255.0;
            let boundary = if encoded <= 0.04045 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            };
            let rounded = boundary as f32;
            // Store the first representable f32 at or above the mathematical boundary.
            if f64::from(rounded) < boundary {
                rounded.next_up()
            } else {
                rounded
            }
        });
        let candidates = std::array::from_fn(|index| {
            thresholds.partition_point(|v| *v <= index as f32 / 4096.0) as u8
        });
        SrgbQuantizer {
            candidates,
            thresholds,
        }
    })
}
impl SrgbQuantizer {
    #[inline]
    fn encode(&self, value: f32) -> u8 {
        let v = value.clamp(0.0, 1.0);
        let candidate = self.candidates[(v * 4096.0) as usize];
        candidate + u8::from(v >= self.thresholds[usize::from(candidate)])
    }
}
#[cfg(test)]
fn linear_to_srgb_byte(value: f32) -> u8 {
    srgb_quantizer().encode(value)
}
fn srgb_byte_table() -> &'static [f32; 256] {
    static LUT: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|v| srgb_to_linear(v as f32 / 255.0)))
}

fn linear_row(
    input: &[u8],
    output: &mut [f32],
    descriptor: &ImageDescriptor,
    preserve: bool,
) -> Result<()> {
    let format = descriptor.format;
    if descriptor.color == ColorEncoding::Srgb
        && format.sample == SampleType::U8
        && format.alpha != AlphaMode::Premultiplied
    {
        let table = srgb_byte_table();
        let opaque = format.alpha == AlphaMode::Opaque;
        match format.channels {
            ChannelOrder::Rgba => linear_u8_row::<4, false>(input, output, table, opaque),
            ChannelOrder::Bgra => linear_u8_row::<4, true>(input, output, table, opaque),
            ChannelOrder::Rgb => linear_u8_row::<3, false>(input, output, table, true),
            ChannelOrder::Bgr => linear_u8_row::<3, true>(input, output, table, true),
            _ => {
                for (pixel, out) in input
                    .chunks_exact(format.bytes_per_pixel())
                    .zip(output.chunks_exact_mut(4))
                {
                    out.copy_from_slice(&linear_pixel(pixel, descriptor, preserve)?);
                }
            }
        }
        return Ok(());
    }
    for (pixel, out) in input
        .chunks_exact(format.bytes_per_pixel())
        .zip(output.chunks_exact_mut(4))
    {
        out.copy_from_slice(&linear_pixel(pixel, descriptor, preserve)?);
    }
    Ok(())
}
fn linear_u8_row<const CHANNELS: usize, const BGR: bool>(
    input: &[u8],
    output: &mut [f32],
    table: &[f32; 256],
    opaque: bool,
) {
    for (pixel, out) in input.chunks_exact(CHANNELS).zip(output.chunks_exact_mut(4)) {
        let r = if BGR { 2 } else { 0 };
        let b = if BGR { 0 } else { 2 };
        out[0] = table[usize::from(pixel[r])];
        out[1] = table[usize::from(pixel[1])];
        out[2] = table[usize::from(pixel[b])];
        out[3] = if opaque {
            1.0
        } else {
            f32::from(pixel[3]) / 255.0
        };
    }
}
fn nits_to_pq(nits: f32) -> f32 {
    let p = (nits.max(0.0) / 10000.0).powf(0.159_301_76);
    ((0.835_937_5 + 18.851_563 * p) / (1.0 + 18.687_5 * p)).powf(78.843_75)
}
fn pq_to_nits(value: f32) -> f32 {
    let p = value.max(0.0).powf(1.0 / 78.843_75);
    ((p - 0.835_937_5).max(0.0) / (18.851_563 - 18.687_5 * p).max(1e-6)).powf(1.0 / 0.159_301_76)
        * 10000.0
}

/// Scalar reference adapted from Snow Capture's tested BT.2390 screenshot policy.
/// SDR colors are identity; a smooth handoff ends at the 100/80 output-white headroom.
#[derive(Clone, Copy)]
struct HdrCurve {
    ow: f32,
    ob: f32,
    ib: f32,
    denominator: f32,
    min_lum: f32,
    max_lum: f32,
    knee: f32,
}
impl HdrCurve {
    fn new(peak_nits: f32) -> Self {
        let ow = nits_to_pq(100.0);
        let ob = nits_to_pq(0.1);
        let iw = nits_to_pq(peak_nits.max(100.001));
        let ib = nits_to_pq(0.001).min(ob - 0.001);
        let denominator = (iw - ib).max(1e-6);
        let min_lum = (ob - ib) / denominator;
        let max_lum = (ow - ib) / denominator;
        let knee = 1.5 * max_lum - 0.5;
        Self {
            ow,
            ob,
            ib,
            denominator,
            min_lum,
            max_lum,
            knee,
        }
    }
}
#[cfg(test)]
fn tone_map_screenshot(rgb: &mut [f32; 3], peak_nits: f32) {
    tone_map_with_curve(rgb, &HdrCurve::new(peak_nits));
}
fn tone_map_with_curve(rgb: &mut [f32; 3], curve: &HdrCurve) {
    let HdrCurve {
        ow,
        ob,
        ib,
        denominator,
        min_lum,
        max_lum,
        knee,
    } = *curve;
    let input = *rgb;
    let max = input[0].max(input[1]).max(input[2]);
    if max <= 1.001 {
        return;
    }
    let luma = (input[0] * 0.2126 + input[1] * 0.7152 + input[2] * 0.0722).max(0.0);
    if luma <= 1e-6 {
        rgb.fill(0.0);
        return;
    }
    let mut y = (nits_to_pq((luma * 80.0).max(0.001)) - ib) / denominator;
    if y >= knee {
        let t = (y - knee) / (1.0 - knee).max(1e-6);
        let t2 = t * t;
        let t3 = t2 * t;
        y = (2.0 * t3 - 3.0 * t2 + 1.0) * knee
            + (t3 - 2.0 * t2 + t) * (1.0 - knee)
            + (-2.0 * t3 + 3.0 * t2) * max_lum;
    }
    if y >= 0.0 {
        y += min_lum * (1.0 - y).max(0.0).powi(4);
    }
    let out = pq_to_nits((y * denominator + ib).clamp(ob, ow)) / 80.0;
    let scale = out / luma;
    for channel in rgb.iter_mut() {
        *channel *= scale;
    }
    let max_out = rgb[0].max(rgb[1]).max(rgb[2]);
    if max_out > 1.0 {
        for channel in rgb.iter_mut() {
            *channel /= max_out;
        }
    }
    if max < 1.25 {
        let t = ((max - 1.001) / (1.25 - 1.001)).clamp(0.0, 1.0);
        let weight = t * t * (3.0 - 2.0 * t);
        for (c, original) in rgb.iter_mut().zip(input) {
            *c = original + (*c - original) * weight;
        }
    }
}

/// Transform an arbitrarily tall raster through bounded row buffers and temporary files.
/// zimg requirements are admitted before execution. Each stage reduces by at most 16x.
pub fn transform_source(
    source: &dyn RasterSource,
    options: &TransformOptions,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<FileRasterSource> {
    use std::io::SeekFrom;
    options.validate()?;
    source.descriptor().validate_with_policy(budget)?;
    cancel.check()?;
    let original = source.descriptor();
    if original.orientation != Orientation::Normal {
        return Err(Error::Unsupported(
            "streaming transform currently requires normalized source orientation".into(),
        ));
    }
    let target = fit(original.width, original.height, options.max_dimensions);
    // zimg may revisit earlier rows for a second horizontal tile. Snapshot genuinely
    // sequential inputs before invoking its random-access callback contract.
    if source.access() == RasterAccess::SequentialRows
        && target != (original.width, original.height)
    {
        let mut packed = original.clone();
        packed.row_stride = packed.row_bytes()?;
        let metadata = SharedMetadata::try_copy_from(
            source.metadata().unwrap_or(&Metadata::default()),
            budget,
        )?;
        let _scratch = budget.reserve(ResourceKind::Scratch, packed.row_stride as u64)?;
        let mut row = crate::zeroed(packed.row_stride)?;
        let snapshot = FileRasterSource::create(packed, metadata, budget, cancel, |writer| {
            for y in 0..original.height {
                cancel.check()?;
                source.read_rows(y, 1, &mut row)?;
                writer.write_all(&row)?;
            }
            Ok(())
        })?;
        drop(_scratch);
        return transform_source(&snapshot, options, budget, cancel);
    }
    if target == (original.width, original.height)
        && options.hdr_output.is_none()
        && (options.dynamic_range == DynamicRangePolicy::PreserveDynamicRange
            || (original.color == ColorEncoding::Srgb
                && original.format.sample == SampleType::U8
                && original.format.alpha != AlphaMode::Premultiplied))
    {
        let metadata = if options.dynamic_range == DynamicRangePolicy::PreserveDynamicRange {
            SharedMetadata::try_copy_from(
                source.metadata().unwrap_or(&Metadata::default()),
                budget,
            )?
        } else {
            SharedMetadata::new(Metadata::default(), budget)?
        };
        let mut descriptor = original.clone();
        descriptor.row_stride = descriptor.row_bytes()?;
        let _scratch = budget.reserve(ResourceKind::Scratch, descriptor.row_stride as u64)?;
        let mut row = crate::zeroed(descriptor.row_stride)?;
        return FileRasterSource::create(descriptor, metadata, budget, cancel, |writer| {
            for y in 0..original.height {
                cancel.check()?;
                source.read_rows(y, 1, &mut row)?;
                writer.write_all(&row)?;
            }
            Ok(())
        });
    }
    let generated_profile = calibrated_profile(&original.color, budget)?;
    let curve = HdrCurve::new(options.hdr_peak_nits);
    let mut staged: Option<FileRasterSource> = None;
    loop {
        let stage_source: &dyn RasterSource =
            staged.as_ref().map_or(source, |s| s as &dyn RasterSource);
        let d = stage_source.descriptor();
        let output_width = target.0.max(d.width.div_ceil(16));
        let output_height = target.1.max(d.height.div_ceil(16));
        let final_stage = (output_width, output_height) == target;
        let preserve = options.dynamic_range == DynamicRangePolicy::PreserveDynamicRange;
        let hdr_output = if final_stage {
            options.hdr_output
        } else {
            None
        };
        let output_sample = if hdr_output.is_some() {
            SampleType::U16
        } else if final_stage && !preserve {
            SampleType::U8
        } else {
            SampleType::F32
        };
        let output_color = if hdr_output.is_some() {
            ColorEncoding::Bt2020Pq
        } else if output_sample == SampleType::U8 {
            ColorEncoding::Srgb
        } else if original.color.is_hdr() && preserve {
            preserved_hdr_color(&original.color)
        } else {
            ColorEncoding::LinearSrgb
        };
        let mut descriptor =
            rgba_descriptor(output_width, output_height, output_sample, output_color)?;
        if let Some(target) = hdr_output {
            descriptor.format.significant_bits = target.bit_depth();
        }
        // Intermediate rows are linear premultiplied RGBA. Final rows are straight alpha.
        if !final_stage {
            descriptor.format.alpha = AlphaMode::Premultiplied;
        }
        let identity = d.width == output_width && d.height == output_height;
        let requirements = if identity {
            native::ResizeRequirements {
                row_buffer_bytes: u64::from(d.width) * 16,
                ..Default::default()
            }
        } else {
            native::resize_requirements(d.width, d.height, output_width, output_height)
                .map_err(codec_error)?
        };
        let scratch_bytes = requirements
            .scratch_bytes
            .checked_add(requirements.row_buffer_bytes)
            .and_then(|v| v.checked_add(d.row_bytes().ok()? as u64))
            .and_then(|v| v.checked_add(descriptor.row_bytes().ok()? as u64))
            .ok_or(Error::AllocationFailed)?;
        let _scratch = budget.reserve(ResourceKind::Scratch, scratch_bytes)?;
        let mut input_row = crate::zeroed(d.row_bytes()?)?;
        let mut output_row = crate::zeroed(descriptor.row_bytes()?)?;
        let icc = generated_profile
            .as_ref()
            .map(|p| p.0.as_slice())
            .or(stage_source.metadata().and_then(|m| m.icc.as_deref()));
        let mut read = |row: u32, output: &mut [f32]| -> native::Result<()> {
            cancel
                .check()
                .map_err(|e| native::Error::Codec(e.to_string()))?;
            stage_source
                .read_rows(row, 1, &mut input_row)
                .map_err(|e| native::Error::Codec(e.to_string()))?;
            if staged.is_some() {
                // Every intermediate source already has the required linear premultiplied convention.
                for (bytes, value) in input_row.chunks_exact(4).zip(output.iter_mut()) {
                    *value = f32::from_ne_bytes(bytes.try_into().unwrap());
                }
            } else {
                linear_row(&input_row, output, d, preserve)
                    .map_err(|e| native::Error::Codec(e.to_string()))?;
                if needs_icc_transform(&d.color) {
                    native::transform_rgba_f32(
                        icc.ok_or_else(|| {
                            native::Error::InvalidInput("missing ICC profile".into())
                        })?,
                        output,
                        true,
                    )?;
                }
                for pixel in output.chunks_exact_mut(4) {
                    if !preserve && original.color.is_hdr() {
                        let mut rgb = [pixel[0], pixel[1], pixel[2]];
                        tone_map_with_curve(&mut rgb, &curve);
                        pixel[..3].copy_from_slice(&rgb);
                    }
                    if !identity {
                        for c in 0..3 {
                            pixel[c] *= pixel[3];
                        }
                    }
                }
            }
            Ok(())
        };
        let metadata = SharedMetadata::new(Metadata::default(), budget)?;
        let quantizer = srgb_quantizer();
        let result =
            FileRasterSource::create(descriptor.clone(), metadata, budget, cancel, |file| {
                let mut write = |row: u32, column: u32, input: &[f32]| -> native::Result<()> {
                    cancel
                        .check()
                        .map_err(|e| native::Error::Codec(e.to_string()))?;
                    let output_span = &mut output_row[..input.len() * output_sample.bytes()];
                    for (pixel, output) in input
                        .chunks_exact(4)
                        .zip(output_span.chunks_exact_mut(4 * output_sample.bytes()))
                    {
                        let alpha = pixel[3].clamp(0.0, 1.0);
                        let mut rgb = [pixel[0], pixel[1], pixel[2]];
                        if final_stage && !identity {
                            if alpha > 0.0 {
                                for c in &mut rgb {
                                    *c /= alpha;
                                }
                            } else {
                                rgb.fill(0.0);
                            }
                        }
                        if let Some(target) = hdr_output {
                            encode_pq_pixel(
                                [rgb[0], rgb[1], rgb[2], alpha],
                                target.bit_depth(),
                                output,
                            )
                            .map_err(|e| native::Error::Unsupported(e.to_string()))?;
                        } else if output_sample == SampleType::U8 {
                            for (c, v) in output[..3].iter_mut().zip(rgb) {
                                *c = quantizer.encode(v);
                            }
                            output[3] = (alpha * 255.0 + 0.5) as u8;
                        } else {
                            for (value, bytes) in rgb
                                .into_iter()
                                .chain([alpha])
                                .zip(output.chunks_exact_mut(4))
                            {
                                bytes.copy_from_slice(&value.to_ne_bytes());
                            }
                        }
                    }
                    file.seek(SeekFrom::Start(
                        u64::from(row) * descriptor.row_stride as u64
                            + u64::from(column) * (output_sample.bytes() * 4) as u64,
                    ))
                    .and_then(|_| file.write_all(output_span))
                    .map_err(|e| native::Error::Codec(e.to_string()))?;
                    Ok(())
                };
                if identity {
                    let length = d.width as usize * 4;
                    let mut row = Vec::new();
                    row.try_reserve_exact(length)
                        .map_err(|_| Error::AllocationFailed)?;
                    row.resize(length, 0.0f32);
                    for y in 0..d.height {
                        read(y, &mut row).map_err(codec_error)?;
                        write(y, 0, &row).map_err(codec_error)?;
                    }
                } else {
                    native::resize_regions_f32(
                        d.width,
                        d.height,
                        output_width,
                        output_height,
                        requirements.scratch_bytes + requirements.row_buffer_bytes,
                        &mut read,
                        &mut write,
                    )
                    .map_err(codec_error)?;
                }
                Ok(())
            })?;
        if final_stage {
            return Ok(result);
        }
        staged = Some(result);
    }
}

/// Quantization is only reached through an explicit HdrOutput request. U16 storage
/// is full-range normalized; significant_bits records the selected 10/12-bit grid.
fn encode_pq_pixel(pixel: [f32; 4], bits: u8, output: &mut [u8]) -> Result<()> {
    let rgb = [
        0.627_403_9 * pixel[0] + 0.329_283_03 * pixel[1] + 0.043_313_067 * pixel[2],
        0.069_097_29 * pixel[0] + 0.919_540_4 * pixel[1] + 0.011_362_316 * pixel[2],
        0.016_391_44 * pixel[0] + 0.088_013_306 * pixel[1] + 0.895_595_25 * pixel[2],
    ];
    if rgb
        .iter()
        .any(|v| !v.is_finite() || *v < -0.00001 || *v > 125.00001)
    {
        return Err(Error::Unsupported(
            "HDR export contains light outside BT.2020 gamut or the PQ 10,000-nit range".into(),
        ));
    }
    let maximum = (1u32 << bits) - 1;
    for (index, bytes) in output.chunks_exact_mut(2).enumerate() {
        let signal = if index == 3 {
            pixel[3].clamp(0.0, 1.0)
        } else {
            nits_to_pq(rgb[index].clamp(0.0, 125.0) * 80.0)
        };
        let code = (signal * maximum as f32 + 0.5)
            .floor()
            .clamp(0.0, maximum as f32) as u32;
        let normalized = ((code * 65535 + maximum / 2) / maximum) as u16;
        bytes.copy_from_slice(&normalized.to_ne_bytes());
    }
    Ok(())
}

fn preserved_hdr_color(color: &ColorEncoding) -> ColorEncoding {
    match color {
        ColorEncoding::ScRgb {
            reference_white_nits,
        } => ColorEncoding::ScRgb {
            reference_white_nits: *reference_white_nits,
        },
        _ => ColorEncoding::ScRgb {
            reference_white_nits: 80.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn srgb_quantizer_matches_iec_reference_at_samples_and_every_boundary() {
        let reference = |value: f32| {
            let v = f64::from(value.clamp(0.0, 1.0));
            let s = if v <= 0.0031308 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0 + 0.5).floor() as u8
        };
        for index in 0..=65535 {
            let value = index as f32 / 65535.0;
            assert_eq!(linear_to_srgb_byte(value), reference(value), "{value}");
        }
        for boundary in &srgb_quantizer().thresholds[..255] {
            for value in [boundary.next_down(), *boundary, boundary.next_up()] {
                assert_eq!(linear_to_srgb_byte(value), reference(value), "{value}");
            }
        }
        for byte in 0..=255 {
            assert_eq!(linear_to_srgb_byte(srgb_byte_table()[byte]), byte as u8);
        }
    }
    #[test]
    fn specialized_srgb_rows_match_general_conversion() {
        for channels in [
            ChannelOrder::Rgb,
            ChannelOrder::Rgba,
            ChannelOrder::Bgr,
            ChannelOrder::Bgra,
        ] {
            for alpha in [AlphaMode::Straight, AlphaMode::Opaque] {
                if matches!(channels, ChannelOrder::Rgb | ChannelOrder::Bgr)
                    && alpha != AlphaMode::Opaque
                {
                    continue;
                }
                let format = PixelFormat {
                    channels,
                    alpha,
                    ..PixelFormat::RGBA8
                };
                let descriptor =
                    ImageDescriptor::packed(256, 1, format, ColorEncoding::Srgb).unwrap();
                let bytes: Vec<u8> = (0..descriptor.row_bytes().unwrap())
                    .map(|i| (i % 256) as u8)
                    .collect();
                let mut output = vec![0.0; 1024];
                linear_row(&bytes, &mut output, &descriptor, false).unwrap();
                for (pixel, actual) in bytes
                    .chunks_exact(format.bytes_per_pixel())
                    .zip(output.chunks_exact(4))
                {
                    assert_eq!(actual, linear_pixel(pixel, &descriptor, false).unwrap());
                }
            }
        }
    }
    #[test]
    fn screenshot_preserves_sdr_and_continuous_white() {
        for v in [0.0, 0.2, 0.7, 1.0] {
            let mut rgb = [v, v * 0.4, v * 0.2];
            let old = rgb;
            tone_map_screenshot(&mut rgb, 1000.0);
            assert_eq!(rgb, old);
        }
        let mut previous = [1.0; 3];
        for index in 0..1000 {
            let v = 1.0 + index as f32 / 4000.0;
            let mut rgb = [v, v * 0.8, v * 0.5];
            tone_map_screenshot(&mut rgb, 1000.0);
            if index > 0 {
                assert!((rgb[0] - previous[0]).abs() < 0.002);
            }
            previous = rgb;
        }
    }
    #[test]
    fn hlg_diffuse_white_anchors_seventy_five_percent_signal() {
        let descriptor = rgba_descriptor(
            1,
            1,
            SampleType::F32,
            ColorEncoding::Bt2020Hlg {
                reference_white_nits: 203.0,
            },
        )
        .unwrap();
        let pixel: [u8; 16] = [0.75f32, 0.75, 0.75, 1.0]
            .into_iter()
            .flat_map(f32::to_ne_bytes)
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let output = linear_pixel(&pixel, &descriptor, true).unwrap();
        for value in &output[..3] {
            assert!((*value - 203.0 / 80.0).abs() < 0.0001);
        }
    }
    #[test]
    fn pq_reference_white_roundtrip() {
        for nits in [0.1, 80.0, 203.0, 1000.0, 10000.0] {
            assert!((pq_to_nits(nits_to_pq(nits)) - nits).abs() < nits * 0.0001 + 0.0001);
        }
    }
}
