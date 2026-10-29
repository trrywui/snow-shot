use crate::{codec_error, zeroed};
use snow_image_core::*;
use snow_image_native_sys as native;
use std::io::{self, Write};

pub fn encode(
    source: &dyn RasterSource,
    options: &EncodeOptions,
    writer: &mut dyn Write,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<EncodeReceipt> {
    options.validate()?;
    cancel.check()?;
    let descriptor = source.descriptor();
    descriptor.validate_with_policy(budget)?;
    if descriptor.orientation != Orientation::Normal {
        return Err(Error::Unsupported(
            "normalize orientation before encoding".into(),
        ));
    }
    if descriptor.format.alpha == AlphaMode::Premultiplied {
        return Err(Error::Unsupported(
            "unpremultiply samples explicitly before encoding".into(),
        ));
    }
    let mut output = CountedWriter {
        writer,
        bytes: 0,
        limit: budget.policy().max_artifact_bytes,
        exceeded: None,
        cancel,
    };
    let result = (|| -> Result<PixelFidelity> {
        Ok(match options {
            EncodeOptions::Png(opts) => {
                encode_png(source, opts, &mut output, budget, cancel)?;
                PixelFidelity::Exact
            }
            EncodeOptions::Jpeg(opts) => {
                encode_jpeg(source, opts, &mut output, budget, cancel)?;
                PixelFidelity::Lossy
            }
            EncodeOptions::Jxl(opts) => encode_jxl(source, opts, &mut output, budget, cancel)?,
            EncodeOptions::Bmp(opts) => {
                encode_bmp(source, opts, &mut output, budget, cancel)?;
                if descriptor.format.channels.has_alpha() && !opts.preserve_alpha {
                    PixelFidelity::LosslessTransformed
                } else {
                    PixelFidelity::Exact
                }
            }
            _ => encode_native(source, options, &mut output, budget, cancel)?,
        })
    })();
    if let Some(requested) = output.exceeded {
        return Err(Error::ResourceLimit {
            resource: ResourceKind::EncodedOutput,
            requested,
            limit: output.limit,
        });
    }
    let fidelity = result.map_err(|error| {
        if cancel.is_cancelled() {
            Error::Cancelled
        } else {
            error
        }
    })?;
    output.flush()?;
    Ok(EncodeReceipt {
        format: options.format(),
        width: descriptor.width,
        height: descriptor.height,
        bit_depth: match options {
            EncodeOptions::Jpeg(_) | EncodeOptions::WebP(_) | EncodeOptions::Bmp(_) => 8,
            EncodeOptions::Avif(v) => v.bit_depth,
            EncodeOptions::Png(_) => (descriptor.format.sample.bytes() * 8) as u8,
            _ => descriptor.format.significant_bits,
        },
        bytes: output.bytes,
        options: options.clone(),
        fidelity,
    })
}

struct CountedWriter<'a> {
    writer: &'a mut dyn Write,
    bytes: u64,
    limit: u64,
    exceeded: Option<u64>,
    cancel: &'a CancellationToken,
}
impl Write for CountedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::other("encoding cancelled"));
        }
        if self
            .bytes
            .checked_add(bytes.len() as u64)
            .is_none_or(|n| n > self.limit)
        {
            self.exceeded = Some(self.bytes.saturating_add(bytes.len() as u64));
            return Err(io::Error::other("encoded artifact limit exceeded"));
        }
        let n = self.writer.write(bytes)?;
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

fn encode_png(
    source: &dyn RasterSource,
    options: &PngOptions,
    writer: &mut dyn Write,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<()> {
    let d = source.descriptor();
    if !matches!(d.format.sample, SampleType::U8 | SampleType::U16) {
        return Err(Error::Unsupported(
            "PNG accepts U8 or U16 samples; choose an explicit quantization transform".into(),
        ));
    }
    if !matches!(
        d.color,
        ColorEncoding::Srgb
            | ColorEncoding::LinearSrgb
            | ColorEncoding::Icc
            | ColorEncoding::CalibratedRgb { .. }
    ) {
        return Err(Error::Unsupported(
            "PNG export requires explicit color conversion to sRGB, linear sRGB, or ICC".into(),
        ));
    }
    let row_bytes = d.row_bytes()?;
    let _scratch = budget.reserve(
        ResourceKind::Scratch,
        (row_bytes as u64)
            .checked_mul(8)
            .and_then(|n| n.checked_add(4 * 1024 * 1024))
            .ok_or(Error::AllocationFailed)?,
    )?;
    let mut row = zeroed(row_bytes)?;
    let mut png_info = png::Info::with_size(d.width, d.height);
    png_info.color_type = match d.format.channels {
        ChannelOrder::Gray => png::ColorType::Grayscale,
        ChannelOrder::GrayAlpha => png::ColorType::GrayscaleAlpha,
        ChannelOrder::Rgb | ChannelOrder::Bgr => png::ColorType::Rgb,
        ChannelOrder::Rgba | ChannelOrder::Bgra => png::ColorType::Rgba,
    };
    png_info.bit_depth = if d.format.sample == SampleType::U16 {
        png::BitDepth::Sixteen
    } else {
        png::BitDepth::Eight
    };
    match d.color {
        ColorEncoding::Srgb => png_info.srgb = Some(png::SrgbRenderingIntent::RelativeColorimetric),
        ColorEncoding::LinearSrgb => {
            png_info.coding_independent_code_points = Some(png::CodingIndependentCodePoints {
                color_primaries: 1,
                transfer_function: 8,
                matrix_coefficients: 0,
                is_video_full_range_image: true,
            });
        }
        ColorEncoding::CalibratedRgb {
            gamma,
            chromaticities,
        } => {
            let scaled = |value: f64| -> Result<png::ScaledFloat> {
                let encoded = (value * 100_000.0).round();
                if !encoded.is_finite() || encoded < 0.0 || encoded > f64::from(u32::MAX) {
                    return Err(Error::Unsupported(
                        "PNG calibrated color value exceeds fixed-point range".into(),
                    ));
                }
                Ok(png::ScaledFloat::from_scaled(encoded as u32))
            };
            png_info.source_gamma = gamma.map(scaled).transpose()?;
            if let Some(c) = chromaticities {
                let xy = |point: [f64; 2]| -> Result<(png::ScaledFloat, png::ScaledFloat)> {
                    Ok((scaled(point[0])?, scaled(point[1])?))
                };
                png_info.source_chromaticities = Some(png::SourceChromaticities {
                    white: xy(c.white)?,
                    red: xy(c.red)?,
                    green: xy(c.green)?,
                    blue: xy(c.blue)?,
                });
            }
        }
        ColorEncoding::Icc => {
            let icc = source
                .metadata()
                .and_then(|m| m.icc.as_deref())
                .ok_or_else(|| Error::InvalidInput("ICC pixels require a profile".into()))?;
            png_info.icc_profile = Some(std::borrow::Cow::Borrowed(icc));
        }
        _ => {}
    }
    let mut encoder = png::Encoder::with_info(writer, png_info).map_err(codec_error)?;
    encoder.set_compression(match options.compression {
        PngCompression::Fast => png::Compression::Fast,
        PngCompression::Default => png::Compression::Balanced,
        PngCompression::Best => png::Compression::High,
    });
    encoder.set_filter(match options.filter {
        PngFilter::None => png::Filter::NoFilter,
        PngFilter::Sub => png::Filter::Sub,
        PngFilter::Up => png::Filter::Up,
        PngFilter::Average => png::Filter::Avg,
        PngFilter::Paeth => png::Filter::Paeth,
        PngFilter::Adaptive => png::Filter::Adaptive,
    });
    let mut writer = encoder.write_header().map_err(codec_error)?;
    if usize::from(d.format.significant_bits) < d.format.sample.bytes() * 8 {
        // png 0.18.1 reads Info::sbit but does not emit it from the encoder's Info.
        let significant = [d.format.significant_bits; 4];
        writer
            .write_chunk(
                png::chunk::ChunkType(*b"sBIT"),
                &significant[..d.format.channels.count()],
            )
            .map_err(codec_error)?;
    }
    {
        let mut stream = writer
            .stream_writer_with_size(128 * 1024)
            .map_err(codec_error)?;
        for y in 0..d.height {
            cancel.check()?;
            source.read_rows(y, 1, &mut row)?;
            normalize_row(&mut row, d, true);
            stream.write_all(&row)?;
        }
        stream.finish().map_err(codec_error)?;
    }
    writer.finish().map_err(codec_error)?;
    Ok(())
}

/// Channel swaps and endian conversion preserve every accepted integer sample, including RGB at alpha=0.
fn normalize_row(row: &mut [u8], descriptor: &ImageDescriptor, png_endian: bool) {
    let bytes = descriptor.format.sample.bytes();
    let channels = descriptor.format.channels.count();
    if matches!(
        descriptor.format.channels,
        ChannelOrder::Bgr | ChannelOrder::Bgra
    ) {
        for pixel in row.chunks_exact_mut(channels * bytes) {
            for i in 0..bytes {
                pixel.swap(i, 2 * bytes + i);
            }
        }
    }
    if descriptor.format.alpha == AlphaMode::Opaque && descriptor.format.channels.has_alpha() {
        let alpha = match descriptor.format.sample {
            SampleType::U8 => [255, 0, 0, 0],
            SampleType::U16 => [255, 255, 0, 0],
            SampleType::F16 => {
                let one = half::f16::ONE.to_bits().to_ne_bytes();
                [one[0], one[1], 0, 0]
            }
            SampleType::F32 => 1.0f32.to_ne_bytes(),
        };
        for pixel in row.chunks_exact_mut(channels * bytes) {
            pixel[(channels - 1) * bytes..].copy_from_slice(&alpha[..bytes]);
        }
    }
    if png_endian && bytes == 2 && cfg!(target_endian = "little") {
        for word in row.chunks_exact_mut(2) {
            word.swap(0, 1);
        }
    }
}

fn encode_bmp(
    source: &dyn RasterSource,
    options: &BmpOptions,
    writer: &mut dyn Write,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<()> {
    let d = source.descriptor();
    if d.format.sample != SampleType::U8
        || !matches!(
            d.color,
            ColorEncoding::Srgb | ColorEncoding::CalibratedBmp { .. }
        )
    {
        return Err(Error::Unsupported(
            "BMP export requires U8 sRGB samples".into(),
        ));
    }
    if d.width > i32::MAX as u32 || d.height > i32::MAX as u32 {
        return Err(Error::Unsupported(
            "BMP dimensions exceed signed 32-bit range".into(),
        ));
    }
    let alpha = options.preserve_alpha && d.format.channels.has_alpha();
    let output_channels = if alpha { 4 } else { 3 };
    let packed = d.width as usize * output_channels;
    let stride = packed.checked_add(3).ok_or(Error::AllocationFailed)? & !3;
    let extended = alpha || matches!(d.color, ColorEncoding::CalibratedBmp { .. });
    let header_size = if extended { 138usize } else { 54 };
    let file_size = stride
        .checked_mul(d.height as usize)
        .and_then(|v| v.checked_add(header_size))
        .filter(|v| *v <= u32::MAX as usize)
        .ok_or_else(|| Error::Unsupported("BMP file exceeds 4 GiB".into()))?;
    let _scratch = budget.reserve(ResourceKind::Scratch, (stride + d.row_bytes()?) as u64)?;
    let mut header = [0u8; 138];
    header[..2].copy_from_slice(b"BM");
    header[2..6].copy_from_slice(&(file_size as u32).to_le_bytes());
    header[10..14].copy_from_slice(&(header_size as u32).to_le_bytes());
    header[14..18].copy_from_slice(&(if extended { 124u32 } else { 40 }).to_le_bytes());
    header[18..22].copy_from_slice(&d.width.to_le_bytes());
    header[22..26].copy_from_slice(&(-(d.height as i32)).to_le_bytes());
    header[26..28].copy_from_slice(&1u16.to_le_bytes());
    header[28..30].copy_from_slice(&((output_channels * 8) as u16).to_le_bytes());
    header[34..38].copy_from_slice(&((stride * d.height as usize) as u32).to_le_bytes());
    if alpha {
        header[30..34].copy_from_slice(&3u32.to_le_bytes());
        header[54..58].copy_from_slice(&0x00ff0000u32.to_le_bytes());
        header[58..62].copy_from_slice(&0x0000ff00u32.to_le_bytes());
        header[62..66].copy_from_slice(&0x000000ffu32.to_le_bytes());
        header[66..70].copy_from_slice(&0xff000000u32.to_le_bytes());
        header[70..74].copy_from_slice(&0x73524742u32.to_le_bytes());
    }
    if let ColorEncoding::CalibratedBmp { endpoints, gamma } = &d.color {
        header[70..74].fill(0);
        for (index, value) in endpoints.iter().flatten().enumerate() {
            let fixed = (*value * 1_073_741_824.0).round();
            if fixed < f64::from(i32::MIN) || fixed > f64::from(i32::MAX) {
                return Err(Error::Unsupported(
                    "BMP calibrated endpoint exceeds fixed-point range".into(),
                ));
            }
            header[74 + index * 4..78 + index * 4].copy_from_slice(&(fixed as i32).to_le_bytes());
        }
        for (index, value) in gamma.iter().enumerate() {
            let fixed = (*value * 65_536.0).round();
            if fixed < 0.0 || fixed > f64::from(u32::MAX) {
                return Err(Error::Unsupported(
                    "BMP calibrated gamma exceeds fixed-point range".into(),
                ));
            }
            header[110 + index * 4..114 + index * 4].copy_from_slice(&(fixed as u32).to_le_bytes());
        }
    }
    writer.write_all(&header[..header_size])?;
    let mut input = zeroed(d.row_bytes()?)?;
    let mut output = zeroed(stride)?;
    for y in 0..d.height {
        cancel.check()?;
        source.read_rows(y, 1, &mut input)?;
        for (p, out) in input
            .chunks_exact(d.format.channels.count())
            .zip(output[..packed].chunks_exact_mut(output_channels))
        {
            let rgba = match d.format.channels {
                ChannelOrder::Gray => [p[0], p[0], p[0], 255],
                ChannelOrder::GrayAlpha => [p[0], p[0], p[0], p[1]],
                ChannelOrder::Rgb => [p[0], p[1], p[2], 255],
                ChannelOrder::Bgr => [p[2], p[1], p[0], 255],
                ChannelOrder::Rgba => [p[0], p[1], p[2], p[3]],
                ChannelOrder::Bgra => [p[2], p[1], p[0], p[3]],
            };
            out[..3].copy_from_slice(&[rgba[2], rgba[1], rgba[0]]);
            if alpha {
                out[3] = if d.format.alpha == AlphaMode::Opaque {
                    255
                } else {
                    rgba[3]
                };
            }
        }
        writer.write_all(&output)?;
    }
    Ok(())
}

fn encode_native(
    source: &dyn RasterSource,
    options: &EncodeOptions,
    writer: &mut dyn Write,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<PixelFidelity> {
    let d = source.descriptor();
    let (transfer, primaries) = match d.color {
        ColorEncoding::ScRgb { .. } => {
            return Err(Error::Unsupported(
                "source color encoding requires an explicit transform before native encoding"
                    .into(),
            ));
        }
        _ => native_encoding(&d.color)?,
    };
    let mut opts = native::EncodeOptions {
        threads: budget.policy().max_codec_threads,
        max_output_bytes: budget.policy().max_artifact_bytes,
        max_memory_bytes: budget
            .policy()
            .working_bytes_per_job
            .saturating_sub(budget.used(ResourceKind::Scratch)),
        ..Default::default()
    };
    let fidelity = match options {
        EncodeOptions::Jpeg(v) => {
            if d.format.channels.has_alpha() {
                return Err(Error::Unsupported(
                    "JPEG requires explicitly flattened RGB samples".into(),
                ));
            }
            opts.quality = v.quality as f32;
            opts.jpeg_subsampling = match v.subsampling {
                JpegSubsampling::S444 => 444,
                JpegSubsampling::S422 => 422,
                JpegSubsampling::S420 => 420,
            };
            opts.progressive = v.progressive;
            PixelFidelity::Lossy
        }
        EncodeOptions::WebP(v) => {
            opts.quality = v.quality as f32;
            opts.lossless = v.lossless;
            opts.exact = v.exact;
            opts.effort = v.effort as u32;
            if v.lossless && v.exact {
                PixelFidelity::Exact
            } else if v.lossless {
                PixelFidelity::LosslessTransformed
            } else {
                PixelFidelity::Lossy
            }
        }
        EncodeOptions::Jxl(v) => {
            opts.distance = v.distance;
            opts.lossless = v.distance == 0.0;
            opts.exact = v.exact;
            opts.effort = v.effort as u32;
            if v.distance == 0.0 && v.exact {
                PixelFidelity::Exact
            } else if v.distance == 0.0 {
                PixelFidelity::LosslessTransformed
            } else {
                PixelFidelity::Lossy
            }
        }
        EncodeOptions::Avif(v) => {
            opts.quality = v.quality as f32;
            opts.alpha_quality = v.alpha_quality as f32;
            opts.speed = v.speed as u32;
            opts.bit_depth = v.bit_depth;
            opts.lossless = v.exact;
            opts.exact = v.exact;
            if v.exact {
                PixelFidelity::Exact
            } else {
                PixelFidelity::Lossy
            }
        }
        _ => unreachable!(),
    };
    if matches!(options, EncodeOptions::Jpeg(_) | EncodeOptions::WebP(_))
        && d.format.sample != SampleType::U8
    {
        return Err(Error::Unsupported(
            "selected encoder requires U8 samples; quantization must be explicit".into(),
        ));
    }
    if let EncodeOptions::Avif(v) = options
        && (d.format.sample.is_float() || d.format.significant_bits > v.bit_depth)
    {
        return Err(Error::Unsupported(
            "AVIF precision reduction requires an explicit quantization transform".into(),
        ));
    }
    let bytes = crate::checked_bytes(d.width, d.height, 4 * d.format.sample.bytes())?;
    let reservation =
        budget.reserve(ResourceKind::Scratch, bytes as u64 + d.row_bytes()? as u64)?;
    let mut pixels = zeroed(bytes)?;
    let mut row = zeroed(d.row_bytes()?)?;
    let bytes_per_sample = d.format.sample.bytes();
    let alpha = match d.format.sample {
        SampleType::U8 => [255, 0, 0, 0],
        SampleType::U16 => {
            let b = u16::MAX.to_ne_bytes();
            [b[0], b[1], 0, 0]
        }
        SampleType::F16 => {
            let b = half::f16::ONE.to_bits().to_ne_bytes();
            [b[0], b[1], 0, 0]
        }
        SampleType::F32 => 1.0f32.to_ne_bytes(),
    };
    for y in 0..d.height {
        cancel.check()?;
        source.read_rows(y, 1, &mut row)?;
        normalize_row(&mut row, d, false);
        for (p, o) in row.chunks_exact(d.format.bytes_per_pixel()).zip(
            pixels[y as usize * d.width as usize * 4 * bytes_per_sample
                ..(y as usize + 1) * d.width as usize * 4 * bytes_per_sample]
                .chunks_exact_mut(4 * bytes_per_sample),
        ) {
            let gray = matches!(
                d.format.channels,
                ChannelOrder::Gray | ChannelOrder::GrayAlpha
            );
            for c in 0..3 {
                let from = if gray { 0 } else { c * bytes_per_sample };
                o[c * bytes_per_sample..(c + 1) * bytes_per_sample]
                    .copy_from_slice(&p[from..from + bytes_per_sample]);
            }
            let a = if d.format.channels.has_alpha() {
                &p[p.len() - bytes_per_sample..]
            } else {
                &alpha[..bytes_per_sample]
            };
            o[3 * bytes_per_sample..].copy_from_slice(a);
        }
    }
    let image = native::NativeImage {
        width: d.width,
        height: d.height,
        sample: match d.format.sample {
            SampleType::U8 => native::SampleType::U8,
            SampleType::U16 => native::SampleType::U16,
            SampleType::F16 => native::SampleType::F16,
            SampleType::F32 => native::SampleType::F32,
        },
        pixels,
        exif: Vec::new(),
        xmp: Vec::new(),
        icc: source
            .metadata()
            .and_then(|m| m.icc.as_deref())
            .map(crate::clone_bytes)
            .transpose()?
            .unwrap_or_default(),
        significant_bits: d.format.significant_bits,
        hdr_reference_white_nits: match d.color {
            ColorEncoding::ScRgb {
                reference_white_nits,
            } => reference_white_nits,
            _ => 0.0,
        },
        transfer,
        primaries,
        orientation: 1,
    };
    native::encode_to_writer(
        crate::decode::native_format(options.format())?,
        &image,
        opts,
        &mut |bytes| {
            cancel
                .check()
                .map_err(|e| native::Error::Codec(e.to_string()))?;
            writer
                .write_all(bytes)
                .map_err(|e| native::Error::Codec(e.to_string()))
        },
    )
    .map_err(codec_error)?;
    drop(reservation);
    Ok(fidelity)
}

fn encode_jpeg(
    source: &dyn RasterSource,
    options: &JpegOptions,
    writer: &mut dyn Write,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<()> {
    let descriptor = source.descriptor();
    if descriptor.format.sample != SampleType::U8 || descriptor.color != ColorEncoding::Srgb {
        return Err(Error::Unsupported(
            "streamed JPEG requires U8 sRGB samples".into(),
        ));
    }
    let scratch = descriptor.row_bytes()? as u64 + u64::from(descriptor.width) * 4 + 256 * 1024;
    let _reservation = budget.reserve(ResourceKind::Scratch, scratch)?;
    let mut row = crate::zeroed(descriptor.row_bytes()?)?;
    let opts = native::EncodeOptions {
        quality: options.quality as f32,
        jpeg_subsampling: match options.subsampling {
            JpegSubsampling::S444 => 444,
            JpegSubsampling::S422 => 422,
            JpegSubsampling::S420 => 420,
        },
        progressive: options.progressive,
        max_output_bytes: budget.policy().max_artifact_bytes,
        max_memory_bytes: budget
            .policy()
            .working_bytes_per_job
            .saturating_sub(budget.used(ResourceKind::Scratch)),
        ..Default::default()
    };
    let mut read = |y: u32, rgba: &mut [u8]| -> native::Result<()> {
        cancel
            .check()
            .map_err(|e| native::Error::Codec(e.to_string()))?;
        if matches!(
            descriptor.format.channels,
            ChannelOrder::Rgba | ChannelOrder::Bgra
        ) {
            // The native row destination is already RGBA. Capture rows need no intermediate copy.
            source
                .read_rows(y, 1, rgba)
                .map_err(|e| native::Error::Codec(e.to_string()))?;
            if descriptor.format.alpha == AlphaMode::Opaque {
                for pixel in rgba.chunks_exact_mut(4) {
                    pixel[3] = 255;
                }
            } else if rgba.chunks_exact(4).any(|pixel| pixel[3] != 255) {
                return Err(native::Error::Unsupported(
                    "JPEG requires explicitly flattened opaque pixels".into(),
                ));
            }
            if descriptor.format.channels == ChannelOrder::Bgra {
                for pixel in rgba.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
            }
            return Ok(());
        }
        source
            .read_rows(y, 1, &mut row)
            .map_err(|e| native::Error::Codec(e.to_string()))?;
        for (pixel, out) in row
            .chunks_exact(descriptor.format.channels.count())
            .zip(rgba.chunks_exact_mut(4))
        {
            let mut p = match descriptor.format.channels {
                ChannelOrder::Gray => [pixel[0], pixel[0], pixel[0], 255],
                ChannelOrder::GrayAlpha => [pixel[0], pixel[0], pixel[0], pixel[1]],
                ChannelOrder::Rgb => [pixel[0], pixel[1], pixel[2], 255],
                ChannelOrder::Bgr => [pixel[2], pixel[1], pixel[0], 255],
                ChannelOrder::Rgba => [pixel[0], pixel[1], pixel[2], pixel[3]],
                ChannelOrder::Bgra => [pixel[2], pixel[1], pixel[0], pixel[3]],
            };
            if descriptor.format.alpha == AlphaMode::Opaque {
                p[3] = 255;
            }
            if p[3] != 255 {
                return Err(native::Error::Unsupported(
                    "JPEG requires explicitly flattened opaque pixels".into(),
                ));
            }
            out.copy_from_slice(&p);
        }
        Ok(())
    };
    let mut write = |bytes: &[u8]| -> native::Result<()> {
        cancel
            .check()
            .map_err(|e| native::Error::Codec(e.to_string()))?;
        writer
            .write_all(bytes)
            .map_err(|e| native::Error::Codec(e.to_string()))
    };
    native::encode_jpeg_rows(
        descriptor.width,
        descriptor.height,
        opts,
        &mut read,
        &mut write,
    )
    .map_err(codec_error)
}

fn encode_jxl(
    source: &dyn RasterSource,
    options: &JxlOptions,
    writer: &mut dyn Write,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<PixelFidelity> {
    let descriptor = source.descriptor();
    let (transfer, primaries) = native_encoding(&descriptor.color)?;
    let staged = if source.access() == RasterAccess::SequentialRows {
        let mut packed = descriptor.clone();
        packed.row_stride = packed.row_bytes()?;
        let metadata = SharedMetadata::try_copy_from(
            source.metadata().unwrap_or(&Metadata::default()),
            budget,
        )?;
        let _scratch = budget.reserve(ResourceKind::Scratch, packed.row_stride as u64)?;
        let mut row = crate::zeroed(packed.row_stride)?;
        Some(FileRasterSource::create(
            packed,
            metadata,
            budget,
            cancel,
            |writer| {
                for y in 0..descriptor.height {
                    cancel.check()?;
                    source.read_rows(y, 1, &mut row)?;
                    writer.write_all(&row)?;
                }
                Ok(())
            },
        )?)
    } else {
        None
    };
    let source = staged
        .as_ref()
        .map_or(source, |file| file as &dyn RasterSource);
    let descriptor = source.descriptor();
    let profile = source
        .metadata()
        .and_then(|m| m.icc.as_deref())
        .unwrap_or_default();
    let scratch_bytes = descriptor.row_bytes()? as u64
        + u64::from(descriptor.width) * (descriptor.format.sample.bytes() * 4) as u64
        + profile.len() as u64;
    let _scratch = budget.reserve(ResourceKind::Scratch, scratch_bytes)?;
    let mut row = crate::zeroed(descriptor.row_bytes()?)?;
    let info = native::NativeInfo {
        width: descriptor.width,
        height: descriptor.height,
        sample: native_sample(descriptor.format.sample),
        significant_bits: descriptor.format.significant_bits,
        hdr_reference_white_nits: match descriptor.color {
            ColorEncoding::ScRgb {
                reference_white_nits,
            } => reference_white_nits,
            _ => 0.0,
        },
        transfer,
        primaries,
        orientation: 1,
        has_alpha: descriptor.format.channels.has_alpha(),
        animated: false,
        frame_count: 1,
        loop_count: 0,
        icc: crate::clone_bytes(profile)?,
        exif: Vec::new(),
        xmp: Vec::new(),
    };
    let opts = native::EncodeOptions {
        distance: options.distance,
        lossless: options.distance == 0.0,
        exact: options.exact,
        effort: options.effort as u32,
        threads: budget.policy().max_codec_threads,
        max_output_bytes: budget.policy().max_artifact_bytes,
        max_memory_bytes: budget
            .policy()
            .working_bytes_per_job
            .saturating_sub(budget.used(ResourceKind::Scratch)),
        ..Default::default()
    };
    let mut read = |y: u32, output: &mut [u8]| -> native::Result<()> {
        cancel
            .check()
            .map_err(|e| native::Error::Codec(e.to_string()))?;
        source
            .read_rows(y, 1, &mut row)
            .map_err(|e| native::Error::Codec(e.to_string()))?;
        rgba_row(&row, output, descriptor);
        Ok(())
    };
    let mut write = |bytes: &[u8]| -> native::Result<()> {
        cancel
            .check()
            .map_err(|e| native::Error::Codec(e.to_string()))?;
        writer
            .write_all(bytes)
            .map_err(|e| native::Error::Codec(e.to_string()))
    };
    native::encode_jxl_rows(&info, opts, &mut read, &mut write).map_err(codec_error)?;
    Ok(if options.distance == 0.0 && options.exact {
        PixelFidelity::Exact
    } else if options.distance == 0.0 {
        PixelFidelity::LosslessTransformed
    } else {
        PixelFidelity::Lossy
    })
}

fn native_sample(sample: SampleType) -> native::SampleType {
    match sample {
        SampleType::U8 => native::SampleType::U8,
        SampleType::U16 => native::SampleType::U16,
        SampleType::F16 => native::SampleType::F16,
        SampleType::F32 => native::SampleType::F32,
    }
}
fn native_encoding(
    color: &ColorEncoding,
) -> Result<(native::TransferFunction, native::ColorPrimaries)> {
    match color {
        ColorEncoding::Srgb => Ok((native::TransferFunction::Srgb, native::ColorPrimaries::Srgb)),
        ColorEncoding::LinearSrgb | ColorEncoding::ScRgb { .. } => Ok((
            native::TransferFunction::Linear,
            native::ColorPrimaries::Srgb,
        )),
        ColorEncoding::DisplayP3 => Ok((
            native::TransferFunction::Srgb,
            native::ColorPrimaries::DisplayP3,
        )),
        ColorEncoding::Bt2020Pq => {
            Ok((native::TransferFunction::Pq, native::ColorPrimaries::Bt2020))
        }
        ColorEncoding::Bt2020Hlg {
            reference_white_nits,
        } => {
            // These adapters serialize the HLG transfer and BT.2020 primaries,
            // not a custom diffuse-white convention. Decoding uses 203 nits.
            if *reference_white_nits != 203.0 {
                return Err(Error::Unsupported(
                    "HLG encoding requires a 203-nit reference white; transform custom white explicitly"
                        .into(),
                ));
            }
            Ok((
                native::TransferFunction::Hlg,
                native::ColorPrimaries::Bt2020,
            ))
        }
        ColorEncoding::Icc => Ok((
            native::TransferFunction::Unknown,
            native::ColorPrimaries::Unknown,
        )),
        ColorEncoding::Unknown
        | ColorEncoding::CalibratedRgb { .. }
        | ColorEncoding::CalibratedBmp { .. } => Err(Error::Unsupported(
            "unknown source color requires an explicit profile".into(),
        )),
    }
}
fn rgba_row(input: &[u8], output: &mut [u8], descriptor: &ImageDescriptor) {
    let b = descriptor.format.sample.bytes();
    let alpha = match descriptor.format.sample {
        SampleType::U8 => [255, 0, 0, 0],
        SampleType::U16 => {
            let value = u16::MAX.to_ne_bytes();
            [value[0], value[1], 0, 0]
        }
        SampleType::F16 => {
            let value = half::f16::ONE.to_bits().to_ne_bytes();
            [value[0], value[1], 0, 0]
        }
        SampleType::F32 => 1.0f32.to_ne_bytes(),
    };
    for (pixel, out) in input
        .chunks_exact(descriptor.format.bytes_per_pixel())
        .zip(output.chunks_exact_mut(4 * b))
    {
        let indices = match descriptor.format.channels {
            ChannelOrder::Gray | ChannelOrder::GrayAlpha => [0, 0, 0],
            ChannelOrder::Rgb | ChannelOrder::Rgba => [0, 1, 2],
            ChannelOrder::Bgr | ChannelOrder::Bgra => [2, 1, 0],
        };
        for (c, src) in indices.into_iter().enumerate() {
            out[c * b..(c + 1) * b].copy_from_slice(&pixel[src * b..(src + 1) * b]);
        }
        let a = if descriptor.format.channels.has_alpha()
            && descriptor.format.alpha != AlphaMode::Opaque
        {
            &pixel[pixel.len() - b..]
        } else {
            &alpha[..b]
        };
        out[3 * b..].copy_from_slice(a);
    }
}
