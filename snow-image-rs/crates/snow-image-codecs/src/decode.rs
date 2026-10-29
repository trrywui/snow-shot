use crate::{checked_bytes, codec_error, headers, rgba_descriptor};
use image::ImageDecoder;
use snow_image_core::*;
use snow_image_native_sys as native;
use std::io::Cursor;

/// An inspected JPEG whose pixels can be decoded directly into a caller-owned output.
/// The immutable input borrow prevents inspection and decoding from using different data.
/// The caller must reserve the destination's pixel storage before allocating or mapping it.
pub struct DirectJpegDecode<'a> {
    input: &'a [u8],
    descriptor: ImageDescriptor,
    preserve_metadata: bool,
    screenshot: bool,
}

impl<'a> DirectJpegDecode<'a> {
    /// `preview_maximum` selects screenshot-preview semantics. Unsupported transforms,
    /// frame selection, color conversion, and resizing return None for the normal path.
    pub fn prepare(
        input: &'a [u8],
        options: &DecodeOptions,
        preview_maximum: Option<u32>,
        budget: &Budget,
        cancel: &CancellationToken,
    ) -> Result<Option<Self>> {
        cancel.check()?;
        if !input.starts_with(&[0xff, 0xd8]) || options.preferred_frame.unwrap_or(0) != 0 {
            return Ok(None);
        }
        if preview_maximum
            .is_some_and(|maximum| maximum == 0 || maximum > budget.policy().preview_max_dimension)
        {
            return Err(Error::InvalidInput("preview size exceeds policy".into()));
        }
        let info = inspect(input, options, budget, cancel)?;
        let screenshot = preview_maximum.is_some();
        if ((options.apply_orientation || screenshot) && info.orientation != Orientation::Normal)
            || preview_maximum.is_some_and(|maximum| info.width > maximum || info.height > maximum)
            || (screenshot && info.color != ColorEncoding::Srgb)
        {
            return Ok(None);
        }
        let mut descriptor = rgba_descriptor(info.width, info.height, SampleType::U8, info.color)?;
        descriptor.format.alpha = AlphaMode::Opaque;
        descriptor.orientation = info.orientation;
        descriptor.validate_with_policy(budget)?;
        Ok(Some(Self {
            input,
            descriptor,
            preserve_metadata: options.preserve_metadata,
            screenshot,
        }))
    }

    pub fn descriptor(&self) -> &ImageDescriptor {
        &self.descriptor
    }

    /// The destination remains exclusively borrowed until all native callbacks finish.
    /// On error its contents are unspecified and must never be published.
    pub fn decode_into(
        self,
        destination: &mut [u8],
        budget: &Budget,
        cancel: &CancellationToken,
    ) -> Result<(ImageDescriptor, SharedMetadata)> {
        cancel.check()?;
        if destination.len() != self.descriptor.required_bytes()? {
            return Err(Error::InvalidInput(
                "direct JPEG destination length mismatch".into(),
            ));
        }
        let available_metadata = budget
            .policy()
            .max_metadata_bytes
            .saturating_sub(budget.used(ResourceKind::Metadata))
            .min(
                budget
                    .policy()
                    .total_memory_bytes
                    .saturating_sub(budget.total_used()),
            );
        let mut metadata_charge = budget.reserve(ResourceKind::Metadata, available_metadata)?;
        let mut options = native_decode_options(budget);
        options.max_output_bytes = options.max_output_bytes.min(destination.len() as u64);
        options.max_metadata_bytes = available_metadata;
        let decoded =
            native::decode_jpeg_into(self.input, options, destination).map_err(codec_error)?;
        cancel.check()?;
        if decoded.width != self.descriptor.width
            || decoded.height != self.descriptor.height
            || decoded.sample != native::SampleType::U8
            || decoded.significant_bits != 8
            || decoded.has_alpha
            || orientation(decoded.orientation) != self.descriptor.orientation
            || native_color(decoded.transfer, decoded.primaries, !decoded.icc.is_empty())
                != self.descriptor.color
        {
            return Err(Error::InvalidInput(
                "direct JPEG result differs from inspected description".into(),
            ));
        }
        let metadata = if self.screenshot {
            drop(decoded);
            Metadata::default()
        } else {
            Metadata {
                icc: (!decoded.icc.is_empty()).then_some(decoded.icc),
                exif: (self.preserve_metadata && !decoded.exif.is_empty()).then_some(decoded.exif),
                xmp: (self.preserve_metadata && !decoded.xmp.is_empty()).then_some(decoded.xmp),
            }
        };
        metadata_charge.resize(metadata.allocation_bytes()?)?;
        Ok((
            self.descriptor,
            SharedMetadata::from_reserved(metadata, metadata_charge)?,
        ))
    }
}

pub fn inspect(
    input: &[u8],
    options: &DecodeOptions,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<ImageInfo> {
    cancel.check()?;
    input_limit(input, budget)?;
    let format = headers::identify(input)?;
    let info = match format {
        Format::Png => {
            let (_scratch, mut reader) = png_reader(input, budget)?;
            let (color_type, _) = reader.output_color_type();
            reader.finish().map_err(codec_error)?;
            let png = reader.info();
            let animation = png.animation_control;
            let frame_count = animation.map_or(1, |a| a.num_frames);
            if frame_count == 0 || frame_count > budget.policy().max_animation_frames {
                return Err(Error::InvalidInput(
                    "PNG animation frame limit exceeded".into(),
                ));
            }
            let metadata = png_metadata(png, options.preserve_metadata, budget)?;
            ImageInfo {
                format,
                width: png.width,
                height: png.height,
                bit_depth: png_significant_bits(png),
                has_alpha: matches!(
                    color_type,
                    png::ColorType::Rgba | png::ColorType::GrayscaleAlpha
                ),
                frame_count,
                animated: animation.is_some(),
                color: png_color(png),
                orientation: png
                    .exif_metadata
                    .as_deref()
                    .map_or(Orientation::Normal, headers::exif_orientation),
                metadata,
            }
        }
        Format::Gif => {
            let gif = headers::gif_header(input, budget.policy().max_animation_frames)?;
            ImageInfo {
                format,
                width: gif.width,
                height: gif.height,
                bit_depth: 8,
                has_alpha: true,
                frame_count: gif.frames,
                animated: gif.frames > 1,
                color: ColorEncoding::Srgb,
                orientation: Orientation::Normal,
                metadata: Metadata::default(),
            }
        }
        Format::Bmp => {
            let (color, profile) = headers::bmp_color(input, budget)?;
            let _profile_copy =
                budget.reserve(ResourceKind::Scratch, profile.map_or(0, |p| p.len()) as u64)?;
            let mut info = image_info(
                image::codecs::bmp::BmpDecoder::new(Cursor::new(input)).map_err(codec_error)?,
                format,
            );
            info.color = color;
            info.metadata.icc = profile.map(crate::clone_bytes).transpose()?;
            info
        }
        Format::Ico => {
            if let Some((payload, width, height)) = headers::ico_png(input)? {
                let mut info = inspect(payload, options, budget, cancel)?;
                if info.width != width || info.height != height || info.animated {
                    return Err(Error::InvalidInput(
                        "ICO directory dimensions differ from embedded PNG".into(),
                    ));
                }
                info.format = Format::Ico;
                info
            } else {
                image_info(
                    image::codecs::ico::IcoDecoder::new(Cursor::new(input)).map_err(codec_error)?,
                    format,
                )
            }
        }
        _ => {
            let metadata_limit = budget
                .policy()
                .max_metadata_bytes
                .saturating_sub(budget.used(ResourceKind::Metadata))
                .min(
                    budget
                        .policy()
                        .total_memory_bytes
                        .saturating_sub(budget.total_used()),
                );
            let _metadata_scratch = budget.reserve(ResourceKind::Metadata, metadata_limit)?;
            let mut native_options = native_decode_options(budget);
            native_options.max_metadata_bytes = metadata_limit;
            let n = native::inspect(native_format(format)?, input, native_options)
                .map_err(codec_error)?;
            if n.frame_count > budget.policy().max_animation_frames {
                return Err(Error::InvalidInput("animation frame limit exceeded".into()));
            }
            let color = if n.hdr_reference_white_nits > 0.0 {
                ColorEncoding::ScRgb {
                    reference_white_nits: n.hdr_reference_white_nits,
                }
            } else {
                native_color(n.transfer, n.primaries, !n.icc.is_empty())
            };
            ImageInfo {
                format,
                width: n.width,
                height: n.height,
                bit_depth: n.significant_bits,
                has_alpha: n.has_alpha,
                frame_count: n.frame_count,
                animated: n.animated,
                color,
                orientation: orientation(n.orientation),
                metadata: Metadata {
                    icc: if !n.icc.is_empty() { Some(n.icc) } else { None },
                    exif: if options.preserve_metadata && !n.exif.is_empty() {
                        Some(n.exif)
                    } else {
                        None
                    },
                    xmp: if options.preserve_metadata && !n.xmp.is_empty() {
                        Some(n.xmp)
                    } else {
                        None
                    },
                },
            }
        }
    };
    let bytes = checked_bytes(
        info.width,
        info.height,
        if info.bit_depth > 16 {
            16
        } else if info.bit_depth > 8 {
            8
        } else {
            4
        },
    )? as u64;
    if bytes > budget.policy().max_raster_bytes {
        return Err(Error::ResourceLimit {
            resource: ResourceKind::Pixels,
            requested: bytes,
            limit: budget.policy().max_raster_bytes,
        });
    }
    cancel.check()?;
    Ok(info)
}

pub fn decode(
    input: &[u8],
    options: &DecodeOptions,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<Image> {
    cancel.check()?;
    input_limit(input, budget)?;
    let format = headers::identify(input)?;
    if options.preferred_frame.unwrap_or(0) > 0 || format == Format::Gif {
        let mut session = crate::AnimationSession::open(input, options, budget, cancel)?;
        return session
            .frame(options.preferred_frame.unwrap_or(0), cancel)
            .map(|f| f.image);
    }
    let image = match format {
        Format::Png => decode_png(input, options, budget, cancel)?,
        Format::Bmp => {
            let (color, profile) = headers::bmp_color(input, budget)?;
            let _profile_copy =
                budget.reserve(ResourceKind::Scratch, profile.map_or(0, |p| p.len()) as u64)?;
            decode_image(
                image::codecs::bmp::BmpDecoder::new(Cursor::new(input)).map_err(codec_error)?,
                budget,
                color,
                Metadata {
                    icc: profile.map(crate::clone_bytes).transpose()?,
                    ..Default::default()
                },
            )?
        }
        Format::Ico => {
            if let Some((payload, width, height)) = headers::ico_png(input)? {
                let image = decode_png(payload, options, budget, cancel)?;
                if image.descriptor().width != width || image.descriptor().height != height {
                    return Err(Error::InvalidInput(
                        "ICO directory dimensions differ from embedded PNG".into(),
                    ));
                }
                image
            } else {
                decode_image(
                    image::codecs::ico::IcoDecoder::new(Cursor::new(input)).map_err(codec_error)?,
                    budget,
                    ColorEncoding::Srgb,
                    Metadata::default(),
                )?
            }
        }
        _ => decode_native(format, input, options, budget, None)?,
    };
    cancel.check()?;
    if options.apply_orientation {
        apply_orientation(image, budget, cancel)
    } else {
        Ok(image)
    }
}

pub(crate) fn input_limit(input: &[u8], budget: &Budget) -> Result<()> {
    if input.len() as u64 > budget.policy().max_input_bytes {
        return Err(Error::ResourceLimit {
            resource: ResourceKind::Input,
            requested: input.len() as u64,
            limit: budget.policy().max_input_bytes,
        });
    }
    Ok(())
}

pub(crate) fn png_reader<'a>(
    input: &'a [u8],
    budget: &Budget,
) -> Result<(Reservation, png::Reader<Cursor<&'a [u8]>>)> {
    let (has_icc, encoded_metadata_bytes) = audit_png_metadata(input, budget)?;
    // The png decoder accounts its own row buffers and inflated metadata against this cap.
    let row = if input.len() >= 24 {
        u32::from_be_bytes(input[16..20].try_into().unwrap()) as u64 * 8
    } else {
        0
    };
    let limit = row
        .checked_mul(8)
        .and_then(|n| n.checked_add(budget.policy().max_metadata_bytes))
        .and_then(|n| n.checked_add(512 * 1024))
        .ok_or_else(|| Error::InvalidInput("PNG scratch overflow".into()))?;
    // Keep room for metadata copies until the final immutable owner adopts them.
    let mut reservation = budget.reserve(ResourceKind::Scratch, limit + encoded_metadata_bytes)?;
    let mut decoder = png::Decoder::new(Cursor::new(input));
    decoder.set_limits(png::Limits {
        bytes: usize::try_from(limit).map_err(|_| Error::AllocationFailed)?,
    });
    decoder.set_transformations(png::Transformations::EXPAND);
    let reader = decoder.read_info().map_err(codec_error)?;
    let copied_metadata = reader.info().icc_profile.as_ref().map_or(0, |v| v.len())
        + reader.info().exif_metadata.as_ref().map_or(0, |v| v.len());
    reservation.resize(limit + encoded_metadata_bytes + copied_metadata as u64)?;
    if has_icc && reader.info().icc_profile.is_none() {
        return Err(Error::InvalidInput(
            "PNG contains an invalid or oversized ICC profile".into(),
        ));
    }
    Ok((reservation, reader))
}

pub(crate) fn png_color(info: &png::Info<'_>) -> ColorEncoding {
    if info.icc_profile.is_some() {
        ColorEncoding::Icc
    } else if let Some(c) = info.coding_independent_code_points {
        match (c.color_primaries, c.transfer_function) {
            (9, 16) => ColorEncoding::Bt2020Pq,
            (9, 18) => ColorEncoding::Bt2020Hlg {
                reference_white_nits: 203.0,
            },
            (1, 8) => ColorEncoding::LinearSrgb,
            (12, 13) => ColorEncoding::DisplayP3,
            (1, 13) => ColorEncoding::Srgb,
            _ => ColorEncoding::Unknown,
        }
    } else if info.srgb.is_some() || (info.gama_chunk.is_none() && info.chrm_chunk.is_none()) {
        ColorEncoding::Srgb
    } else {
        let xy = |pair: (png::ScaledFloat, png::ScaledFloat)| {
            [
                f64::from(pair.0.into_scaled()) / 100_000.0,
                f64::from(pair.1.into_scaled()) / 100_000.0,
            ]
        };
        ColorEncoding::CalibratedRgb {
            gamma: info
                .gama_chunk
                .map(|g| f64::from(g.into_scaled()) / 100_000.0),
            chromaticities: info.chrm_chunk.map(|c| RgbChromaticities {
                white: xy(c.white),
                red: xy(c.red),
                green: xy(c.green),
                blue: xy(c.blue),
            }),
        }
    }
}

pub(crate) fn png_metadata(
    info: &png::Info<'_>,
    preserve: bool,
    budget: &Budget,
) -> Result<Metadata> {
    let size = info.icc_profile.as_ref().map_or(0, |v| v.len())
        + info.exif_metadata.as_ref().map_or(0, |v| v.len());
    if size as u64 > budget.policy().max_metadata_bytes {
        return Err(Error::ResourceLimit {
            resource: ResourceKind::Metadata,
            requested: size as u64,
            limit: budget.policy().max_metadata_bytes,
        });
    }
    let xmp = if preserve {
        let mut chunks = info
            .utf8_text
            .iter()
            .filter(|chunk| chunk.keyword == "XML:com.adobe.xmp");
        let first = chunks.next();
        if chunks.next().is_some() {
            return Err(Error::InvalidInput(
                "PNG contains duplicate XMP metadata".into(),
            ));
        }
        if let Some(chunk) = first {
            let _scratch = budget.reserve(
                ResourceKind::Scratch,
                budget.policy().max_metadata_bytes.saturating_mul(3),
            )?;
            let mut chunk = chunk.clone();
            let remaining = (budget.policy().max_metadata_bytes - size as u64) as usize;
            chunk
                .decompress_text_with_limit(remaining)
                .map_err(|error| {
                    // png 0.18.1 hides its text-error discriminant; this pinned diagnostic
                    // distinguishes its bounded inflater from malformed compressed data.
                    if error.to_string().starts_with("Out of decompression space.") {
                        Error::ResourceLimit {
                            resource: ResourceKind::Metadata,
                            requested: budget.policy().max_metadata_bytes.saturating_add(1),
                            limit: budget.policy().max_metadata_bytes,
                        }
                    } else {
                        codec_error(error)
                    }
                })?;
            let bytes = chunk.get_text().map_err(codec_error)?.into_bytes();
            if bytes.len() > remaining {
                return Err(Error::ResourceLimit {
                    resource: ResourceKind::Metadata,
                    requested: size as u64 + bytes.len() as u64,
                    limit: budget.policy().max_metadata_bytes,
                });
            }
            Some(bytes)
        } else {
            None
        }
    } else {
        None
    };
    Ok(Metadata {
        icc: info
            .icc_profile
            .as_deref()
            .map(crate::clone_bytes)
            .transpose()?,
        exif: if preserve {
            info.exif_metadata
                .as_deref()
                .map(crate::clone_bytes)
                .transpose()?
        } else {
            None
        },
        xmp,
    })
}

/// Audit ancillary payload bounds before png's permissive ICC parser can ignore errors.
pub(crate) fn audit_png_metadata(input: &[u8], budget: &Budget) -> Result<(bool, u64)> {
    let mut offset = 8usize;
    let mut metadata = 0u64;
    let mut has_icc = false;
    while offset < input.len() {
        let header = input
            .get(offset..offset + 8)
            .ok_or_else(|| Error::InvalidInput("truncated PNG chunk header".into()))?;
        let length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let next = offset
            .checked_add(length)
            .and_then(|n| n.checked_add(12))
            .filter(|n| *n <= input.len())
            .ok_or_else(|| Error::InvalidInput("truncated PNG chunk payload".into()))?;
        let kind = &header[4..8];
        has_icc |= kind == b"iCCP";
        if matches!(kind, b"iCCP" | b"eXIf" | b"iTXt" | b"tEXt" | b"zTXt") {
            metadata = metadata.saturating_add(length as u64);
            if metadata > budget.policy().max_metadata_bytes {
                return Err(Error::ResourceLimit {
                    resource: ResourceKind::Metadata,
                    requested: metadata,
                    limit: budget.policy().max_metadata_bytes,
                });
            }
        }
        offset = next;
        if kind == b"IEND" {
            break;
        }
    }
    Ok((has_icc, metadata))
}
fn png_significant_bits(info: &png::Info<'_>) -> u8 {
    info.sbit
        .as_deref()
        .and_then(|sbit| sbit.iter().copied().max())
        .unwrap_or_else(|| {
            if info.color_type == png::ColorType::Indexed {
                8
            } else {
                info.bit_depth as u8
            }
        })
}

fn decode_png(
    input: &[u8],
    options: &DecodeOptions,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<Image> {
    let (_scratch, mut reader) = png_reader(input, budget)?;
    let (color_type, depth) = reader.output_color_type();
    let channels = match color_type {
        png::ColorType::Grayscale => ChannelOrder::Gray,
        png::ColorType::GrayscaleAlpha => ChannelOrder::GrayAlpha,
        png::ColorType::Rgb => ChannelOrder::Rgb,
        png::ColorType::Rgba => ChannelOrder::Rgba,
        _ => return Err(Error::Unsupported("unexpanded PNG palette".into())),
    };
    let channel_count = match channels {
        ChannelOrder::Gray => 1,
        ChannelOrder::GrayAlpha => 2,
        ChannelOrder::Rgb => 3,
        _ => 4,
    };
    let sample = if depth == png::BitDepth::Sixteen {
        SampleType::U16
    } else {
        SampleType::U8
    };
    let info = reader.info();
    let mut descriptor = ImageDescriptor {
        width: info.width,
        height: info.height,
        row_stride: checked_bytes(
            info.width,
            1,
            channel_count * if sample == SampleType::U16 { 2 } else { 1 },
        )?,
        format: PixelFormat {
            channels,
            sample,
            alpha: if channel_count == 2 || channel_count == 4 {
                AlphaMode::Straight
            } else {
                AlphaMode::Opaque
            },
            significant_bits: png_significant_bits(info),
        },
        color: png_color(info),
        orientation: info
            .exif_metadata
            .as_deref()
            .map_or(Orientation::Normal, headers::exif_orientation),
    };
    descriptor.validate_with_policy(budget)?;
    let mut reservation =
        budget.reserve(ResourceKind::Pixels, descriptor.required_bytes()? as u64)?;
    let mut pixels = crate::zeroed(descriptor.required_bytes()?)?;
    reservation.resize(pixels.capacity() as u64)?;
    let output = reader.next_frame(&mut pixels).map_err(codec_error)?;
    if output.width != descriptor.width || output.height != descriptor.height {
        return Err(Error::InvalidInput(
            "PNG first frame does not cover canvas".into(),
        ));
    }
    if sample == SampleType::U16 && cfg!(target_endian = "little") {
        for word in pixels.chunks_exact_mut(2) {
            word.swap(0, 1);
        }
    }
    reader.finish().map_err(codec_error)?;
    let info = reader.info();
    descriptor.orientation = info
        .exif_metadata
        .as_deref()
        .map_or(Orientation::Normal, headers::exif_orientation);
    let metadata = SharedMetadata::new(
        png_metadata(info, options.preserve_metadata, budget)?,
        budget,
    )?;
    cancel.check()?;
    Image::from_reserved(descriptor, pixels, metadata, reservation)
}

fn image_info(decoder: impl ImageDecoder, format: Format) -> ImageInfo {
    let (width, height) = decoder.dimensions();
    ImageInfo {
        format,
        width,
        height,
        bit_depth: 8,
        has_alpha: decoder.color_type().has_alpha(),
        frame_count: 1,
        animated: false,
        color: ColorEncoding::Srgb,
        orientation: Orientation::Normal,
        metadata: Metadata::default(),
    }
}

fn decode_image(
    decoder: impl ImageDecoder,
    budget: &Budget,
    color_encoding: ColorEncoding,
    metadata: Metadata,
) -> Result<Image> {
    let (width, height) = decoder.dimensions();
    let color = decoder.color_type();
    let (channels, count) = match color {
        image::ColorType::Rgb8 => (ChannelOrder::Rgb, 3),
        image::ColorType::Rgba8 => (ChannelOrder::Rgba, 4),
        _ => {
            return Err(Error::Unsupported(
                "BMP/ICO decoder returned unsupported samples".into(),
            ));
        }
    };
    let descriptor = ImageDescriptor {
        width,
        height,
        row_stride: checked_bytes(width, 1, count)?,
        format: PixelFormat {
            channels,
            sample: SampleType::U8,
            alpha: if count == 4 {
                AlphaMode::Straight
            } else {
                AlphaMode::Opaque
            },
            significant_bits: 8,
        },
        color: color_encoding,
        orientation: Orientation::Normal,
    };
    let _scratch = budget.reserve(
        ResourceKind::Scratch,
        (checked_bytes(width, 1, 4)? + 256 * 1024) as u64,
    )?;
    let mut result = MutableImage::new(descriptor, metadata, budget)?;
    decoder
        .read_image(result.bytes_mut())
        .map_err(codec_error)?;
    Ok(result.seal())
}

pub(crate) fn native_format(format: Format) -> Result<native::NativeFormat> {
    match format {
        Format::Jpeg => Ok(native::NativeFormat::Jpeg),
        Format::WebP => Ok(native::NativeFormat::WebP),
        Format::Jxl => Ok(native::NativeFormat::Jxl),
        Format::Avif => Ok(native::NativeFormat::Avif),
        _ => Err(Error::Unsupported("not a native codec".into())),
    }
}
pub(crate) fn native_decode_options(budget: &Budget) -> native::DecodeOptions {
    native::DecodeOptions {
        max_output_bytes: budget
            .policy()
            .working_bytes_per_job
            .min(budget.policy().max_raster_bytes),
        max_metadata_bytes: budget.policy().max_metadata_bytes,
        max_memory_bytes: budget
            .policy()
            .working_bytes_per_job
            .saturating_sub(budget.used(ResourceKind::Scratch)),
        threads: budget.policy().max_codec_threads,
        ..Default::default()
    }
}
pub(crate) fn native_color(
    transfer: native::TransferFunction,
    primaries: native::ColorPrimaries,
    icc: bool,
) -> ColorEncoding {
    if icc
        && !matches!(
            (transfer, primaries),
            (
                native::TransferFunction::Pq | native::TransferFunction::Hlg,
                native::ColorPrimaries::Bt2020
            )
        )
    {
        return ColorEncoding::Icc;
    }
    match (transfer, primaries) {
        (native::TransferFunction::Srgb, native::ColorPrimaries::Srgb) => ColorEncoding::Srgb,
        (native::TransferFunction::Linear, native::ColorPrimaries::Srgb) => {
            ColorEncoding::LinearSrgb
        }
        (native::TransferFunction::Srgb, native::ColorPrimaries::DisplayP3) => {
            ColorEncoding::DisplayP3
        }
        (native::TransferFunction::Pq, native::ColorPrimaries::Bt2020) => ColorEncoding::Bt2020Pq,
        (native::TransferFunction::Hlg, native::ColorPrimaries::Bt2020) => {
            ColorEncoding::Bt2020Hlg {
                reference_white_nits: 203.0,
            }
        }
        _ => ColorEncoding::Unknown,
    }
}
pub(crate) fn orientation(value: u8) -> Orientation {
    match value {
        2 => Orientation::FlipHorizontal,
        3 => Orientation::Rotate180,
        4 => Orientation::FlipVertical,
        5 => Orientation::Transpose,
        6 => Orientation::Rotate90,
        7 => Orientation::Transverse,
        8 => Orientation::Rotate270,
        _ => Orientation::Normal,
    }
}

pub(crate) fn decode_native(
    format: Format,
    input: &[u8],
    options: &DecodeOptions,
    budget: &Budget,
    target: Option<(u32, u32)>,
) -> Result<Image> {
    let mut opts = native_decode_options(budget);
    if let Some((width, height)) = target {
        opts.target_width = width;
        opts.target_height = height;
    }
    let info = native::inspect(native_format(format)?, input, opts).map_err(codec_error)?;
    let (reserve_width, reserve_height) =
        if format == Format::Jpeg && opts.target_width > 0 && opts.target_height > 0 {
            let numerator = (1u64..=8)
                .find(|n| {
                    (u64::from(info.width) * n).div_ceil(8) >= u64::from(opts.target_width)
                        && (u64::from(info.height) * n).div_ceil(8) >= u64::from(opts.target_height)
                })
                .unwrap_or(8);
            (
                ((u64::from(info.width) * numerator).div_ceil(8)) as u32,
                ((u64::from(info.height) * numerator).div_ceil(8)) as u32,
            )
        } else {
            (info.width, info.height)
        };
    let bytes = checked_bytes(
        reserve_width,
        reserve_height,
        match info.sample {
            native::SampleType::U8 => 4,
            native::SampleType::U16 | native::SampleType::F16 => 8,
            native::SampleType::F32 => 16,
        },
    )?;
    let reservation = budget.reserve(ResourceKind::Pixels, bytes as u64)?;
    let decoded = native::decode(native_format(format)?, input, opts).map_err(codec_error)?;
    let sample = match decoded.sample {
        native::SampleType::U8 => SampleType::U8,
        native::SampleType::U16 => SampleType::U16,
        native::SampleType::F16 => SampleType::F16,
        native::SampleType::F32 => SampleType::F32,
    };
    let mut descriptor = rgba_descriptor(
        decoded.width,
        decoded.height,
        sample,
        if decoded.hdr_reference_white_nits > 0.0 {
            ColorEncoding::ScRgb {
                reference_white_nits: decoded.hdr_reference_white_nits,
            }
        } else {
            native_color(decoded.transfer, decoded.primaries, !decoded.icc.is_empty())
        },
    )?;
    descriptor.format.significant_bits = decoded.significant_bits;
    if !info.has_alpha {
        descriptor.format.alpha = AlphaMode::Opaque;
    }
    descriptor.orientation = orientation(decoded.orientation);
    // Move the allocation into Image; no intermediate pixel copy is made.
    let metadata = SharedMetadata::new(
        Metadata {
            icc: if !decoded.icc.is_empty() {
                Some(decoded.icc)
            } else {
                None
            },
            exif: if options.preserve_metadata && !decoded.exif.is_empty() {
                Some(decoded.exif)
            } else {
                None
            },
            xmp: if options.preserve_metadata && !decoded.xmp.is_empty() {
                Some(decoded.xmp)
            } else {
                None
            },
        },
        budget,
    )?;
    Image::from_reserved(descriptor, decoded.pixels, metadata, reservation)
}

pub(crate) fn apply_orientation(
    image: Image,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<Image> {
    let source = image.descriptor();
    let orientation = source.orientation;
    if orientation == Orientation::Normal {
        return Ok(image);
    }
    let swap = matches!(
        orientation,
        Orientation::Transpose
            | Orientation::Rotate90
            | Orientation::Transverse
            | Orientation::Rotate270
    );
    let mut descriptor = source.clone();
    if swap {
        std::mem::swap(&mut descriptor.width, &mut descriptor.height);
    }
    descriptor.orientation = Orientation::Normal;
    let pixel_bytes = source.row_bytes()? / source.width as usize;
    descriptor.row_stride = checked_bytes(descriptor.width, 1, pixel_bytes)?;
    // EXIF and XMP can describe orientation, dimensions, crops or thumbnails.
    // Arbitrary packets cannot be safely rewritten, so geometry changes remove
    // them while preserving the ICC profile that still describes these samples.
    let metadata = if image.metadata().exif.is_none() && image.metadata().xmp.is_none() {
        image.shared_metadata()
    } else {
        let icc = image.metadata().icc.as_deref();
        let reservation = budget.reserve(
            ResourceKind::Metadata,
            icc.map_or(0, |bytes| bytes.len() as u64),
        )?;
        SharedMetadata::from_reserved(
            Metadata {
                icc: icc.map(crate::clone_bytes).transpose()?,
                exif: None,
                xmp: None,
            },
            reservation,
        )?
    };
    let mut output = MutableImage::new_shared(descriptor.clone(), metadata, budget)?;
    for y in 0..descriptor.height {
        cancel.check()?;
        for x in 0..descriptor.width {
            let (sx, sy) = match orientation {
                Orientation::Normal => (x, y),
                Orientation::FlipHorizontal => (source.width - 1 - x, y),
                Orientation::Rotate180 => (source.width - 1 - x, source.height - 1 - y),
                Orientation::FlipVertical => (x, source.height - 1 - y),
                Orientation::Transpose => (y, x),
                Orientation::Rotate90 => (y, source.height - 1 - x),
                Orientation::Transverse => (source.width - 1 - y, source.height - 1 - x),
                Orientation::Rotate270 => (source.width - 1 - y, x),
            };
            let from = sy as usize * source.row_stride + sx as usize * pixel_bytes;
            let to = y as usize * descriptor.row_stride + x as usize * pixel_bytes;
            output.bytes_mut()[to..to + pixel_bytes]
                .copy_from_slice(&image.bytes()[from..from + pixel_bytes]);
        }
    }
    Ok(output.seal())
}

/// Decode a noninterlaced PNG into private temporary storage without a resident raster.
/// Interlaced inputs need random access reconstruction and retain the bounded resident path.
pub(crate) fn decode_png_to_file(
    input: &[u8],
    complete: ImageInfo,
    budget: &Budget,
    cancel: &CancellationToken,
) -> Result<FileRasterSource> {
    let (_scratch, mut reader) = png_reader(input, budget)?;
    let info = reader.info();
    if info.interlaced {
        return Err(Error::Unsupported(
            "streaming preview of interlaced PNG requires a resident decode".into(),
        ));
    }
    if info.animation_control.is_some() {
        return Err(Error::Unsupported(
            "streaming preview of animated PNG requires a frame session".into(),
        ));
    }
    let (color, depth) = reader.output_color_type();
    let channels = match color {
        png::ColorType::Grayscale => ChannelOrder::Gray,
        png::ColorType::GrayscaleAlpha => ChannelOrder::GrayAlpha,
        png::ColorType::Rgb => ChannelOrder::Rgb,
        png::ColorType::Rgba => ChannelOrder::Rgba,
        _ => return Err(Error::Unsupported("unexpanded PNG palette".into())),
    };
    let sample = if depth == png::BitDepth::Sixteen {
        SampleType::U16
    } else {
        SampleType::U8
    };
    let mut descriptor = ImageDescriptor::packed(
        info.width,
        info.height,
        PixelFormat {
            channels,
            sample,
            alpha: if channels.has_alpha() {
                AlphaMode::Straight
            } else {
                AlphaMode::Opaque
            },
            significant_bits: png_significant_bits(info),
        },
        png_color(info),
    )?;
    descriptor.orientation = complete.orientation;
    let metadata = SharedMetadata::new(complete.metadata, budget)?;
    let _row_budget = budget.reserve(ResourceKind::Scratch, descriptor.row_stride as u64)?;
    let mut row = crate::zeroed(descriptor.row_stride)?;
    let height = descriptor.height;
    FileRasterSource::create(descriptor, metadata, budget, cancel, |writer| {
        for _ in 0..height {
            cancel.check()?;
            let decoded = reader
                .next_row()
                .map_err(codec_error)?
                .ok_or_else(|| Error::InvalidInput("PNG ended before final row".into()))?;
            if decoded.data().len() != row.len() {
                return Err(Error::InvalidInput("PNG row length mismatch".into()));
            }
            row.copy_from_slice(decoded.data());
            if sample == SampleType::U16 && cfg!(target_endian = "little") {
                for word in row.chunks_exact_mut(2) {
                    word.swap(0, 1);
                }
            }
            writer.write_all(&row)?;
        }
        reader.finish().map_err(codec_error)?;
        Ok(())
    })
}
