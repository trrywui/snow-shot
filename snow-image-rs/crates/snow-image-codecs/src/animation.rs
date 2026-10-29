use crate::{
    checked_bytes, codec_error,
    decode::{apply_orientation, png_color},
    headers, rgba_descriptor, zeroed,
};
use snow_image_core::*;
use std::{io::Cursor, num::NonZeroU64, sync::Arc, time::Duration};

#[derive(Clone, Debug)]
pub struct AnimationInfo {
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
    pub loop_count: Option<u32>,
    pub has_hidden_poster: bool,
}
#[derive(Clone, Debug)]
pub struct AnimationFrame {
    pub image: Image,
    pub index: u32,
    pub duration: Duration,
}

enum Decoder {
    Png(Box<png::Reader<Cursor<Arc<[u8]>>>>),
    Gif(Box<gif::Decoder<Cursor<Arc<[u8]>>>>),
    WebP(Box<snow_image_native_sys::WebPAnimation>),
}
#[derive(Clone, Copy)]
enum Disposal {
    None,
    Background,
    Previous,
}
#[derive(Clone, Copy)]
struct Rectangle {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// A lazy session owns one canvas, one saved canvas, and one raw subframe buffer.
/// Published snapshots own their pixels and remain valid after reset or session destruction.
pub struct AnimationSession {
    input: Arc<[u8]>,
    format: Format,
    options: DecodeOptions,
    budget: Budget,
    info: AnimationInfo,
    decoder: Decoder,
    canvas: MutableImage,
    raw: Vec<u8>,
    saved: Vec<u8>,
    previous: Option<(Rectangle, Disposal)>,
    next: u32,
    skip_poster: bool,
    _input: Reservation,
    _scratch: Reservation,
    _decoder_scratch: Reservation,
}

impl AnimationSession {
    pub fn open(
        input: &[u8],
        options: &DecodeOptions,
        budget: &Budget,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        let reservation = budget.reserve(ResourceKind::Input, input.len() as u64)?;
        Self::open_owned(
            crate::clone_bytes(input)?,
            reservation,
            options,
            budget,
            cancel,
        )
    }

    /// Adopts a worker's already charged input, avoiding a second input reservation.
    /// The supplied reservation must belong to `budget` and cover `input`'s capacity.
    pub fn open_owned(
        input: Vec<u8>,
        input_reservation: Reservation,
        options: &DecodeOptions,
        budget: &Budget,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        cancel.check()?;
        crate::decode::input_limit(&input, budget)?;
        if input_reservation.resource() != ResourceKind::Input
            || input_reservation.amount() < input.capacity() as u64
        {
            return Err(Error::InvalidInput(
                "animation input reservation does not cover owned data".into(),
            ));
        }
        let format = headers::identify(&input)?;
        if !matches!(format, Format::Png | Format::Gif | Format::WebP) {
            return Err(Error::Unsupported(
                "animation backend unavailable for this format".into(),
            ));
        }
        let input_copy = budget.reserve(ResourceKind::Scratch, input.len() as u64)?;
        let input: Arc<[u8]> = Arc::from(input);
        drop(input_copy);
        let (decoder, mut info, descriptor, mut metadata, decoder_scratch) =
            make_decoder(input.clone(), format, options, budget)?;
        if options.apply_orientation && descriptor.orientation != Orientation::Normal {
            // Strip once, before publishing the shared session metadata. Every
            // oriented snapshot then shares the same retained ICC allocation.
            metadata.exif = None;
            metadata.xmp = None;
        }
        if options.apply_orientation
            && matches!(
                descriptor.orientation,
                Orientation::Transpose
                    | Orientation::Rotate90
                    | Orientation::Transverse
                    | Orientation::Rotate270
            )
        {
            std::mem::swap(&mut info.width, &mut info.height);
        }
        let raw_len = if format == Format::WebP {
            0
        } else {
            descriptor.required_bytes()?
        };
        let scratch = budget.reserve(
            ResourceKind::Scratch,
            (raw_len as u64)
                .checked_mul(2)
                .ok_or(Error::AllocationFailed)?,
        )?;
        let canvas = MutableImage::new(descriptor, metadata, budget)?;
        Ok(Self {
            input,
            format,
            options: options.clone(),
            budget: budget.clone(),
            skip_poster: info.has_hidden_poster,
            info,
            decoder,
            canvas,
            raw: zeroed(raw_len)?,
            saved: zeroed(raw_len)?,
            previous: None,
            next: 0,
            _input: input_reservation,
            _scratch: scratch,
            _decoder_scratch: decoder_scratch,
        })
    }
    pub fn info(&self) -> &AnimationInfo {
        &self.info
    }
    pub fn reset(&mut self, cancel: &CancellationToken) -> Result<()> {
        cancel.check()?;
        let (decoder, _, _, _, reservation) =
            make_decoder(self.input.clone(), self.format, &self.options, &self.budget)?;
        self.decoder = decoder;
        self._decoder_scratch = reservation;
        self.canvas.bytes_mut().fill(0);
        self.saved.fill(0);
        self.previous = None;
        self.next = 0;
        self.skip_poster = self.info.has_hidden_poster;
        Ok(())
    }
    pub fn frame(&mut self, index: u32, cancel: &CancellationToken) -> Result<AnimationFrame> {
        if index >= self.info.frame_count {
            return Err(Error::InvalidInput(
                "animation frame index outside declared count".into(),
            ));
        }
        if index < self.next {
            self.reset(cancel)?;
        }
        loop {
            let frame = self.next_frame(cancel)?.ok_or_else(|| {
                Error::InvalidInput("animation ended before declared frame count".into())
            })?;
            if frame.index == index {
                return Ok(frame);
            }
        }
    }
    pub fn next_frame(&mut self, cancel: &CancellationToken) -> Result<Option<AnimationFrame>> {
        cancel.check()?;
        if self.next >= self.info.frame_count {
            return Ok(None);
        }
        let descriptor = self.canvas.descriptor().clone();
        let sample_bytes = descriptor.format.sample.bytes();
        if let Decoder::WebP(decoder) = &mut self.decoder {
            let mut pixels = self
                .budget
                .reserve(ResourceKind::Pixels, descriptor.required_bytes()? as u64)?;
            let frame = decoder.next_frame().map_err(codec_error)?.ok_or_else(|| {
                Error::InvalidInput("WebP ended before declared frame count".into())
            })?;
            if descriptor.required_bytes()? != frame.image.pixels.len() {
                return Err(Error::InvalidInput("WebP frame size changed".into()));
            }
            pixels.resize(frame.image.pixels.capacity() as u64)?;
            let snapshot = Image::from_reserved(
                descriptor,
                frame.image.pixels,
                self.canvas.shared_metadata(),
                pixels,
            )?;
            let image = if self.options.apply_orientation {
                apply_orientation(snapshot, &self.budget, cancel)?
            } else {
                snapshot
            };
            let index = self.next;
            self.next += 1;
            return Ok(Some(AnimationFrame {
                image,
                index,
                duration: Duration::from_millis(u64::from(frame.duration_ms)),
            }));
        }
        if let Some((rectangle, disposal)) = self.previous.take() {
            match disposal {
                Disposal::None => {}
                // GIF follows web compositing convention: background disposal exposes
                // transparency, matching the existing image decoder used by Snow Shot.
                Disposal::Background => {
                    for y in rectangle.y..rectangle.y + rectangle.height {
                        let start = y as usize * descriptor.row_stride
                            + rectangle.x as usize * 4 * sample_bytes;
                        let end = start + rectangle.width as usize * 4 * sample_bytes;
                        self.canvas.bytes_mut()[start..end].fill(0);
                    }
                }
                Disposal::Previous => self.canvas.bytes_mut().copy_from_slice(&self.saved),
            }
        }
        let (rectangle, disposal, blend, duration, raw_channels, raw_depth, raw_stride) =
            match &mut self.decoder {
                Decoder::WebP(_) => unreachable!(),
                Decoder::Png(reader) => {
                    if self.skip_poster {
                        reader.next_frame(&mut self.raw).map_err(codec_error)?;
                        self.skip_poster = false;
                    }
                    let output = reader.next_frame(&mut self.raw).map_err(codec_error)?;
                    let control = reader.info().frame_control.ok_or_else(|| {
                        Error::InvalidInput("APNG frame missing frame control".into())
                    })?;
                    let denominator = if control.delay_den == 0 {
                        100
                    } else {
                        control.delay_den
                    };
                    (
                        Rectangle {
                            x: control.x_offset,
                            y: control.y_offset,
                            width: output.width,
                            height: output.height,
                        },
                        match control.dispose_op {
                            png::DisposeOp::None => Disposal::None,
                            png::DisposeOp::Background => Disposal::Background,
                            png::DisposeOp::Previous => Disposal::Previous,
                        },
                        control.blend_op == png::BlendOp::Over,
                        Duration::from_nanos(
                            u64::from(control.delay_num) * 1_000_000_000 / u64::from(denominator),
                        ),
                        output.color_type.samples(),
                        output.bit_depth as u8,
                        output.line_size,
                    )
                }
                Decoder::Gif(reader) => {
                    let control =
                        reader
                            .next_frame_info()
                            .map_err(codec_error)?
                            .ok_or_else(|| {
                                Error::InvalidInput("GIF frame count does not match data".into())
                            })?;
                    let rectangle = Rectangle {
                        x: control.left as u32,
                        y: control.top as u32,
                        width: control.width as u32,
                        height: control.height as u32,
                    };
                    let disposal = match control.dispose {
                        gif::DisposalMethod::Background => Disposal::Background,
                        gif::DisposalMethod::Previous => Disposal::Previous,
                        _ => Disposal::None,
                    };
                    let duration = Duration::from_millis(u64::from(control.delay) * 10);
                    let transparent = control.transparent;
                    let required = checked_bytes(rectangle.width, rectangle.height, 4)?;
                    reader
                        .read_into_buffer(&mut self.raw[..required / 4])
                        .map_err(codec_error)?;
                    expand_gif_indices(
                        &mut self.raw[..required],
                        reader.palette().map_err(codec_error)?,
                        transparent,
                        rectangle.width as usize,
                        cancel,
                    )?;
                    (
                        rectangle,
                        disposal,
                        transparent.is_some(),
                        duration,
                        4,
                        8,
                        rectangle.width as usize * 4,
                    )
                }
            };
        if rectangle
            .x
            .checked_add(rectangle.width)
            .is_none_or(|v| v > descriptor.width)
            || rectangle
                .y
                .checked_add(rectangle.height)
                .is_none_or(|v| v > descriptor.height)
        {
            return Err(Error::InvalidInput(
                "animation frame lies outside canvas".into(),
            ));
        }
        if matches!(disposal, Disposal::Previous) {
            self.saved.copy_from_slice(self.canvas.bytes_mut());
        }
        composite(
            self.canvas.bytes_mut(),
            &descriptor,
            rectangle,
            RawFrame {
                bytes: &self.raw,
                channels: raw_channels,
                depth: raw_depth,
                row_stride: raw_stride,
            },
            blend,
            cancel,
        )?;
        if self.next + 1 == self.info.frame_count
            && let Decoder::Png(reader) = &mut self.decoder
        {
            reader.finish().map_err(codec_error)?;
        }
        self.previous = Some((rectangle, disposal));
        let mut snapshot =
            MutableImage::new_shared(descriptor, self.canvas.shared_metadata(), &self.budget)?;
        snapshot
            .bytes_mut()
            .copy_from_slice(self.canvas.bytes_mut());
        let image = if self.options.apply_orientation {
            apply_orientation(snapshot.seal(), &self.budget, cancel)?
        } else {
            snapshot.seal()
        };
        let index = self.next;
        self.next += 1;
        Ok(Some(AnimationFrame {
            image,
            index,
            duration,
        }))
    }
}

// Decode into the existing raw allocation, then expand backward so unread indices
// cannot be overwritten. Validate palette indices here: gif's RGBA converter leaves
// destination bytes untouched for missing palette entries, which would reuse stale
// pixels and invalidate an opacity guarantee on malformed input.
fn expand_gif_indices(
    raw: &mut [u8],
    palette: &[u8],
    transparent: Option<u8>,
    width: usize,
    cancel: &CancellationToken,
) -> Result<()> {
    for row in (0..raw.len() / 4 / width).rev() {
        cancel.check()?;
        for index in (row * width..(row + 1) * width).rev() {
            let color = raw[index];
            let offset = usize::from(color) * 3;
            let rgb = palette.get(offset..offset + 3).ok_or_else(|| {
                Error::InvalidInput("GIF pixel references a missing palette entry".into())
            })?;
            let pixel = &mut raw[index * 4..index * 4 + 4];
            pixel[..3].copy_from_slice(rgb);
            pixel[3] = if transparent == Some(color) { 0 } else { 255 };
        }
    }
    Ok(())
}

struct RawFrame<'a> {
    bytes: &'a [u8],
    channels: usize,
    depth: u8,
    row_stride: usize,
}

fn composite(
    canvas: &mut [u8],
    descriptor: &ImageDescriptor,
    rectangle: Rectangle,
    raw: RawFrame<'_>,
    blend: bool,
    cancel: &CancellationToken,
) -> Result<()> {
    let sample_bytes = descriptor.format.sample.bytes();
    let maximum = if sample_bytes == 1 { 255u64 } else { 65535 };
    // Source replaces every accepted sample, including the RGB of fully
    // transparent pixels. Copy only active row bytes, preserving canvas padding.
    if !blend && raw.channels == 4 && raw.depth == 8 && sample_bytes == 1 {
        let row_bytes = rectangle.width as usize * 4;
        for y in 0..rectangle.height {
            cancel.check()?;
            let source = y as usize * raw.row_stride;
            let target =
                (rectangle.y + y) as usize * descriptor.row_stride + rectangle.x as usize * 4;
            canvas[target..target + row_bytes]
                .copy_from_slice(&raw.bytes[source..source + row_bytes]);
        }
        return Ok(());
    }
    for y in 0..rectangle.height {
        cancel.check()?;
        for x in 0..rectangle.width {
            let source =
                y as usize * raw.row_stride + x as usize * raw.channels * (raw.depth as usize / 8);
            let target = (rectangle.y + y) as usize * descriptor.row_stride
                + (rectangle.x + x) as usize * 4 * sample_bytes;
            let mut pixel = [maximum; 4];
            let read = |channel: usize| -> u64 {
                if raw.depth == 8 {
                    u64::from(raw.bytes[source + channel])
                } else {
                    let i = source + channel * 2;
                    u64::from(u16::from_be_bytes([raw.bytes[i], raw.bytes[i + 1]]))
                }
            };
            match raw.channels {
                1 => {
                    pixel[..3].fill(read(0));
                }
                2 => {
                    pixel[..3].fill(read(0));
                    pixel[3] = read(1);
                }
                3 | 4 => {
                    for (c, value) in pixel.iter_mut().enumerate().take(raw.channels) {
                        *value = read(c);
                    }
                }
                _ => return Err(Error::Unsupported("animation channel layout".into())),
            }
            let destination = &mut canvas[target..target + 4 * sample_bytes];
            // An opaque source is already the exact Over result. Avoid reading
            // destination samples or dividing for this common screenshot case.
            if blend && pixel[3] != maximum {
                if pixel[3] == 0 {
                    continue;
                }
                let mut old = [0u64; 4];
                for (c, value) in old.iter_mut().enumerate() {
                    *value = if sample_bytes == 1 {
                        u64::from(destination[c])
                    } else {
                        u64::from(u16::from_ne_bytes([
                            destination[c * 2],
                            destination[c * 2 + 1],
                        ]))
                    };
                }
                let alpha = pixel[3] * maximum + old[3] * (maximum - pixel[3]);
                for c in 0..3 {
                    pixel[c] = (pixel[c] * pixel[3] * maximum
                        + old[c] * old[3] * (maximum - pixel[3])
                        + alpha / 2)
                        / alpha;
                }
                pixel[3] = (alpha + maximum / 2) / maximum;
            }
            for (c, value) in pixel.into_iter().enumerate() {
                if sample_bytes == 1 {
                    destination[c] = value as u8;
                } else {
                    destination[c * 2..c * 2 + 2].copy_from_slice(&(value as u16).to_ne_bytes());
                }
            }
        }
    }
    Ok(())
}

fn make_decoder(
    input: Arc<[u8]>,
    format: Format,
    options: &DecodeOptions,
    budget: &Budget,
) -> Result<(
    Decoder,
    AnimationInfo,
    ImageDescriptor,
    Metadata,
    Reservation,
)> {
    match format {
        Format::Png => {
            // Metadata may legally follow IDAT; inspect the complete container once without
            // decoding frame pixels, then share the final description across every snapshot.
            let inspected = crate::inspect(&input, options, budget, &CancellationToken::default())?;
            let (has_icc, encoded_metadata_bytes) =
                crate::decode::audit_png_metadata(&input, budget)?;
            let width = input
                .get(16..20)
                .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
                .unwrap_or(0);
            let limit = u64::from(width) * 64 + budget.policy().max_metadata_bytes + 512 * 1024;
            let mut scratch =
                budget.reserve(ResourceKind::Scratch, limit + encoded_metadata_bytes)?;
            let mut png = png::Decoder::new(Cursor::new(input));
            png.set_limits(png::Limits {
                bytes: usize::try_from(limit).map_err(|_| Error::AllocationFailed)?,
            });
            png.set_transformations(png::Transformations::EXPAND);
            let reader = png.read_info().map_err(codec_error)?;
            let info = reader.info();
            let copied_metadata = info.icc_profile.as_ref().map_or(0, |v| v.len())
                + info.exif_metadata.as_ref().map_or(0, |v| v.len());
            scratch.resize(limit + encoded_metadata_bytes + copied_metadata as u64)?;
            if has_icc && info.icc_profile.is_none() {
                return Err(Error::InvalidInput(
                    "PNG contains an invalid or oversized ICC profile".into(),
                ));
            }
            let animation = info
                .animation_control
                .ok_or_else(|| Error::InvalidInput("PNG is not animated".into()))?;
            if animation.num_frames == 0
                || animation.num_frames > budget.policy().max_animation_frames
            {
                return Err(Error::InvalidInput("APNG frame limit exceeded".into()));
            }
            let metadata = inspected.metadata;
            scratch.resize(scratch.amount().saturating_add(metadata.byte_len()?))?;
            let (_, depth) = reader.output_color_type();
            let mut descriptor = rgba_descriptor(
                info.width,
                info.height,
                if depth == png::BitDepth::Sixteen {
                    SampleType::U16
                } else {
                    SampleType::U8
                },
                png_color(info),
            )?;
            descriptor.orientation = inspected.orientation;
            let animation_info = AnimationInfo {
                width: info.width,
                height: info.height,
                frame_count: animation.num_frames,
                loop_count: Some(animation.num_plays),
                has_hidden_poster: info.frame_control.is_none(),
            };
            Ok((
                Decoder::Png(Box::new(reader)),
                animation_info,
                descriptor,
                metadata,
                scratch,
            ))
        }
        Format::Gif => {
            let header = headers::gif_header(&input, budget.policy().max_animation_frames)?;
            let raw_size = checked_bytes(header.width, header.height, 4)?;
            let scratch = budget.reserve(ResourceKind::Scratch, 256 * 1024)?;
            let mut options = gif::DecodeOptions::new();
            options.set_color_output(gif::ColorOutput::Indexed);
            options.set_memory_limit(gif::MemoryLimit::Bytes(
                NonZeroU64::new(raw_size as u64).ok_or(Error::AllocationFailed)?,
            ));
            options.check_frame_consistency(true);
            let reader = options.read_info(Cursor::new(input)).map_err(codec_error)?;
            let descriptor = rgba_descriptor(
                header.width,
                header.height,
                SampleType::U8,
                ColorEncoding::Srgb,
            )?;
            Ok((
                Decoder::Gif(Box::new(reader)),
                AnimationInfo {
                    width: header.width,
                    height: header.height,
                    frame_count: header.frames,
                    loop_count: header
                        .loops
                        .map(|repeats| if repeats == 0 { 0 } else { repeats + 1 }),
                    has_hidden_poster: false,
                },
                descriptor,
                Metadata::default(),
                scratch,
            ))
        }
        Format::WebP => {
            let opts = crate::decode::native_decode_options(budget);
            let info = snow_image_native_sys::inspect(
                snow_image_native_sys::NativeFormat::WebP,
                &input,
                opts,
            )
            .map_err(codec_error)?;
            if info.frame_count == 0 || info.frame_count > budget.policy().max_animation_frames {
                return Err(Error::InvalidInput("WebP frame limit exceeded".into()));
            }
            let size = checked_bytes(info.width, info.height, 4)?;
            let scratch = budget.reserve(
                ResourceKind::Scratch,
                (size as u64)
                    .checked_mul(3)
                    .ok_or(Error::AllocationFailed)?,
            )?;
            let mut descriptor = rgba_descriptor(
                info.width,
                info.height,
                SampleType::U8,
                crate::decode::native_color(info.transfer, info.primaries, !info.icc.is_empty()),
            )?;
            descriptor.orientation = crate::decode::orientation(info.orientation);
            let metadata = Metadata {
                icc: if !info.icc.is_empty() {
                    Some(info.icc)
                } else {
                    None
                },
                exif: if options.preserve_metadata && !info.exif.is_empty() {
                    Some(info.exif)
                } else {
                    None
                },
                xmp: if options.preserve_metadata && !info.xmp.is_empty() {
                    Some(info.xmp)
                } else {
                    None
                },
            };
            let decoder =
                snow_image_native_sys::WebPAnimation::open(input, opts).map_err(codec_error)?;
            Ok((
                Decoder::WebP(Box::new(decoder)),
                AnimationInfo {
                    width: info.width,
                    height: info.height,
                    frame_count: info.frame_count,
                    loop_count: Some(info.loop_count),
                    has_hidden_poster: false,
                },
                descriptor,
                metadata,
                scratch,
            ))
        }
        _ => Err(Error::Unsupported("animation backend unavailable".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    // Reference straight-alpha Over evaluated with wider integers and no opaque
    // special case. This is independent of the compositor's copy/bypass choices.
    fn over(source: [u16; 4], destination: [u16; 4], maximum: u16) -> [u16; 4] {
        if source[3] == 0 {
            return destination;
        }
        let maximum = u128::from(maximum);
        let a = u128::from(source[3]);
        let b = u128::from(destination[3]);
        let alpha = a * maximum + b * (maximum - a);
        let mut result = [0; 4];
        for channel in 0..3 {
            let numerator = u128::from(source[channel]) * a * maximum
                + u128::from(destination[channel]) * b * (maximum - a);
            result[channel] = ((numerator + alpha / 2) / alpha) as u16;
        }
        result[3] = ((alpha + maximum / 2) / maximum) as u16;
        result
    }

    fn samples(values: &[[u16; 4]], depth: u8, big_endian: bool) -> Vec<u8> {
        values
            .iter()
            .flatten()
            .flat_map(|value| {
                if depth == 8 {
                    vec![*value as u8]
                } else if big_endian {
                    value.to_be_bytes().to_vec()
                } else {
                    value.to_ne_bytes().to_vec()
                }
            })
            .collect()
    }

    #[test]
    fn compositor_matches_general_over_with_padded_rows_and_all_channel_layouts() {
        for depth in [8, 16] {
            let bytes = usize::from(depth / 8);
            let maximum: u16 = if depth == 8 { 255 } else { 65535 };
            for channels in 1..=4 {
                for blend in [false, true] {
                    let mut descriptor = rgba_descriptor(
                        6,
                        4,
                        if depth == 8 {
                            SampleType::U8
                        } else {
                            SampleType::U16
                        },
                        ColorEncoding::Srgb,
                    )
                    .unwrap();
                    descriptor.row_stride += 13;
                    let rectangle = Rectangle {
                        x: 1,
                        y: 1,
                        width: 4,
                        height: 2,
                    };
                    let stride = rectangle.width as usize * channels * bytes + 7;
                    let mut raw = vec![0xe7; stride * rectangle.height as usize];
                    let mut canvas = vec![0xa5; descriptor.required_bytes().unwrap()];
                    let mut expected = canvas.clone();
                    for y in 0..rectangle.height as usize {
                        for x in 0..rectangle.width as usize {
                            let alpha = [0, 1, maximum / 2, maximum][x];
                            let components = [maximum - 17, 23, maximum / 3, alpha];
                            let source = match channels {
                                1 => [components[0], components[0], components[0], maximum],
                                2 => [components[0], components[0], components[0], alpha],
                                3 => [components[0], components[1], components[2], maximum],
                                4 => components,
                                _ => unreachable!(),
                            };
                            for (channel, component) in components.iter().enumerate().take(channels)
                            {
                                let value = if channels == 2 && channel == 1 {
                                    alpha
                                } else {
                                    *component
                                };
                                let offset = y * stride + (x * channels + channel) * bytes;
                                if depth == 8 {
                                    raw[offset] = value as u8;
                                } else {
                                    raw[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
                                }
                            }
                            let old = [17, maximum - 11, 29, if y == 0 { 0 } else { maximum / 3 }];
                            let offset = (rectangle.y as usize + y) * descriptor.row_stride
                                + (rectangle.x as usize + x) * 4 * bytes;
                            canvas[offset..offset + 4 * bytes].copy_from_slice(&samples(
                                &[old],
                                depth,
                                false,
                            ));
                            let result = if blend {
                                over(source, old, maximum)
                            } else {
                                source
                            };
                            expected[offset..offset + 4 * bytes].copy_from_slice(&samples(
                                &[result],
                                depth,
                                false,
                            ));
                        }
                    }
                    composite(
                        &mut canvas,
                        &descriptor,
                        rectangle,
                        RawFrame {
                            bytes: &raw,
                            channels,
                            depth,
                            row_stride: stride,
                        },
                        blend,
                        &CancellationToken::new(),
                    )
                    .unwrap();
                    assert_eq!(
                        canvas, expected,
                        "depth={depth}, channels={channels}, blend={blend}"
                    );
                }
            }
        }
    }

    #[test]
    fn source_row_copy_checks_cancellation_before_modifying_pixels() {
        let descriptor = rgba_descriptor(2, 1, SampleType::U8, ColorEncoding::Srgb).unwrap();
        let mut canvas = [93; 8];
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            composite(
                &mut canvas,
                &descriptor,
                Rectangle {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 1
                },
                RawFrame {
                    bytes: &[1; 8],
                    channels: 4,
                    depth: 8,
                    row_stride: 8
                },
                false,
                &cancel,
            ),
            Err(Error::Cancelled)
        ));
        assert_eq!(canvas, [93; 8]);
    }

    #[test]
    fn apng_source_and_over_keep_transparent_rgb_disposal_and_sixteen_bit_endianness() {
        for depth in [8, 16] {
            let maximum = if depth == 8 { 255 } else { 65535 };
            let base = [maximum - 20, 37, maximum / 4, maximum];
            let transparent = [7, 8, 9, 0];
            let opaque = [11, 12, 13, maximum];
            let partial = [80, 60, 40, maximum / 2];
            let mut encoded = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut encoded, 2, 2);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(if depth == 8 {
                    png::BitDepth::Eight
                } else {
                    png::BitDepth::Sixteen
                });
                encoder.set_animated(5, 1).unwrap();
                let mut writer = encoder.write_header().unwrap();
                writer
                    .write_image_data(&samples(&[base; 4], depth, true))
                    .unwrap();
                writer.set_frame_dimension(1, 1).unwrap();
                writer.set_frame_position(0, 0).unwrap();
                writer.set_blend_op(png::BlendOp::Source).unwrap();
                writer.set_dispose_op(png::DisposeOp::Previous).unwrap();
                writer
                    .write_image_data(&samples(&[transparent], depth, true))
                    .unwrap();
                writer.set_frame_position(1, 0).unwrap();
                writer.set_blend_op(png::BlendOp::Over).unwrap();
                writer.set_dispose_op(png::DisposeOp::Background).unwrap();
                writer
                    .write_image_data(&samples(&[opaque], depth, true))
                    .unwrap();
                writer.set_frame_position(0, 1).unwrap();
                writer.set_dispose_op(png::DisposeOp::None).unwrap();
                writer
                    .write_image_data(&samples(&[partial], depth, true))
                    .unwrap();
                writer.set_frame_position(0, 0).unwrap();
                writer
                    .write_image_data(&samples(&[transparent], depth, true))
                    .unwrap();
                writer.finish().unwrap();
            }
            let budget = Budget::default();
            let cancel = CancellationToken::new();
            let mut session =
                AnimationSession::open(&encoded, &DecodeOptions::default(), &budget, &cancel)
                    .unwrap();
            let mixed = over(partial, base, maximum);
            let expected = [
                [base; 4],
                [transparent, base, base, base],
                [base, opaque, base, base],
                [base, [0; 4], mixed, base],
                [base, [0; 4], mixed, base],
            ];
            for _ in 0..2 {
                for pixels in expected {
                    let frame = session.next_frame(&cancel).unwrap().unwrap();
                    assert_eq!(frame.image.bytes(), samples(&pixels, depth, false));
                }
                assert!(session.next_frame(&cancel).unwrap().is_none());
                session.reset(&cancel).unwrap();
            }
            drop(session);
            assert_eq!(budget.total_used(), 0);
        }
    }

    #[test]
    fn gif_opaque_copy_and_transparent_over_preserve_disposal_and_reject_bad_indices() {
        let palette = [31, 32, 33, 201, 17, 19, 11, 209, 23, 29, 31, 211];
        let mut encoded = Vec::new();
        {
            let mut encoder = gif::Encoder::new(&mut encoded, 2, 1, &palette).unwrap();
            for frame in [
                gif::Frame {
                    width: 2,
                    height: 1,
                    buffer: Cow::Borrowed(&[1, 1]),
                    ..Default::default()
                },
                gif::Frame {
                    width: 2,
                    height: 1,
                    buffer: Cow::Borrowed(&[0, 2]),
                    transparent: Some(0),
                    dispose: gif::DisposalMethod::Previous,
                    ..Default::default()
                },
                gif::Frame {
                    width: 1,
                    height: 1,
                    left: 1,
                    buffer: Cow::Borrowed(&[3]),
                    dispose: gif::DisposalMethod::Background,
                    ..Default::default()
                },
                gif::Frame {
                    width: 1,
                    height: 1,
                    buffer: Cow::Borrowed(&[2]),
                    ..Default::default()
                },
            ] {
                encoder.write_frame(&frame).unwrap();
            }
        }
        let budget = Budget::default();
        let cancel = CancellationToken::new();
        let mut session =
            AnimationSession::open(&encoded, &DecodeOptions::default(), &budget, &cancel).unwrap();
        let red = [201, 17, 19, 255];
        let green = [11, 209, 23, 255];
        let blue = [29, 31, 211, 255];
        for expected in [[red, red], [red, green], [red, blue], [green, [0; 4]]] {
            let frame = session.next_frame(&cancel).unwrap().unwrap();
            assert_eq!(frame.image.bytes(), samples(&expected, 8, false));
        }
        drop(session);
        assert_eq!(budget.total_used(), 0);

        let mut invalid = Vec::new();
        {
            let mut encoder = gif::Encoder::new(&mut invalid, 1, 1, &palette[..6]).unwrap();
            encoder
                .write_frame(&gif::Frame {
                    width: 1,
                    height: 1,
                    buffer: Cow::Borrowed(&[2]),
                    ..Default::default()
                })
                .unwrap();
        }
        let mut session =
            AnimationSession::open(&invalid, &DecodeOptions::default(), &budget, &cancel).unwrap();
        assert!(matches!(
            session.next_frame(&cancel),
            Err(Error::InvalidInput(_))
        ));
        drop(session);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn gif_index_expansion_preserves_interlaced_rows_and_local_palettes() {
        let global = [13, 17, 19, 31, 37, 41, 53, 59, 61, 71, 73, 79];
        let local = [101, 103, 107, 127, 131, 137, 149, 151, 157, 173, 179, 181];
        let row_order = [0, 8, 4, 2, 6, 1, 3, 5, 7];
        let indices = row_order
            .iter()
            .flat_map(|row| (0..3).map(move |x| ((row + x) % 4) as u8))
            .collect::<Vec<_>>();
        let mut encoded = Vec::new();
        {
            let mut encoder = gif::Encoder::new(&mut encoded, 3, 9, &global).unwrap();
            for palette in [None, Some(local.to_vec())] {
                encoder
                    .write_frame(&gif::Frame {
                        width: 3,
                        height: 9,
                        interlaced: true,
                        palette,
                        buffer: Cow::Borrowed(&indices),
                        ..Default::default()
                    })
                    .unwrap();
            }
        }
        let budget = Budget::default();
        let cancel = CancellationToken::new();
        let mut session =
            AnimationSession::open(&encoded, &DecodeOptions::default(), &budget, &cancel).unwrap();
        for palette in [global, local] {
            let frame = session.next_frame(&cancel).unwrap().unwrap();
            let expected = (0..9)
                .flat_map(|row| {
                    (0..3).flat_map(move |x| {
                        let index = (row + x) % 4 * 3;
                        [palette[index], palette[index + 1], palette[index + 2], 255]
                    })
                })
                .collect::<Vec<_>>();
            assert_eq!(frame.image.bytes(), expected);
        }
        drop(session);
        assert_eq!(budget.total_used(), 0);
    }
}
