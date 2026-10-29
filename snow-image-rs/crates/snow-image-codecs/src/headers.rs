use snow_image_core::*;

pub(crate) fn identify(input: &[u8]) -> Result<Format> {
    if input.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(Format::Png);
    }
    if input.starts_with(b"GIF87a") || input.starts_with(b"GIF89a") {
        return Ok(Format::Gif);
    }
    if input.starts_with(b"BM") {
        return Ok(Format::Bmp);
    }
    if input.starts_with(b"\0\0\x01\0") {
        return Ok(Format::Ico);
    }
    if input.starts_with(b"\xff\xd8\xff") {
        return Ok(Format::Jpeg);
    }
    if input.len() >= 12 && &input[..4] == b"RIFF" && &input[8..12] == b"WEBP" {
        return Ok(Format::WebP);
    }
    if input.starts_with(b"\xff\x0a") || input.starts_with(b"\0\0\0\x0cJXL \r\n\x87\n") {
        return Ok(Format::Jxl);
    }
    if input.len() >= 16 && &input[4..8] == b"ftyp" {
        let length = u32::from_be_bytes(input[..4].try_into().unwrap()) as usize;
        if length >= 16
            && length <= input.len()
            && input[8..length]
                .chunks_exact(4)
                .any(|brand| brand == b"avif" || brand == b"avis")
        {
            return Ok(Format::Avif);
        }
    }
    Err(Error::Unsupported("unrecognized image format".into()))
}

/// Read only bounded TIFF IFD0 entries; malformed EXIF never controls unchecked indexing.
pub(crate) fn exif_orientation(input: &[u8]) -> Orientation {
    let input = input.strip_prefix(b"Exif\0\0").unwrap_or(input);
    let little = match input.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return Orientation::Normal,
    };
    let read16 = |offset: usize| -> Option<u16> {
        let bytes: [u8; 2] = input.get(offset..offset.checked_add(2)?)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    };
    let read32 = |offset: usize| -> Option<u32> {
        let bytes: [u8; 4] = input.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    };
    if read16(2) != Some(42) {
        return Orientation::Normal;
    }
    let Some(ifd) = read32(4).map(|v| v as usize) else {
        return Orientation::Normal;
    };
    let Some(entries) = read16(ifd) else {
        return Orientation::Normal;
    };
    for index in 0..usize::from(entries).min(4096) {
        let Some(offset) = ifd.checked_add(2).and_then(|v| v.checked_add(index * 12)) else {
            break;
        };
        if read16(offset) == Some(0x0112)
            && read16(offset + 2) == Some(3)
            && read32(offset + 4) == Some(1)
        {
            return match read16(offset + 8) {
                Some(2) => Orientation::FlipHorizontal,
                Some(3) => Orientation::Rotate180,
                Some(4) => Orientation::FlipVertical,
                Some(5) => Orientation::Transpose,
                Some(6) => Orientation::Rotate90,
                Some(7) => Orientation::Transverse,
                Some(8) => Orientation::Rotate270,
                _ => Orientation::Normal,
            };
        }
    }
    Orientation::Normal
}

pub(crate) struct GifHeader {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub loops: Option<u32>,
}

/// GIF block scan counts frames without LZW decoding or collecting frame payloads.
pub(crate) fn gif_header(input: &[u8], max_frames: u32) -> Result<GifHeader> {
    if input.len() < 13 {
        return Err(Error::InvalidInput("truncated GIF header".into()));
    }
    let width = u16::from_le_bytes([input[6], input[7]]) as u32;
    let height = u16::from_le_bytes([input[8], input[9]]) as u32;
    super::checked_bytes(width, height, 4)?;
    let mut position = 13usize;
    if input[10] & 0x80 != 0 {
        position += 3 * (1usize << ((input[10] & 7) + 1));
    }
    let mut frames = 0u32;
    let mut loops = None;
    loop {
        let kind = *input
            .get(position)
            .ok_or_else(|| Error::InvalidInput("GIF missing trailer".into()))?;
        position += 1;
        match kind {
            0x3b => break,
            0x2c => {
                let descriptor = input
                    .get(position..position + 9)
                    .ok_or_else(|| Error::InvalidInput("truncated GIF frame".into()))?;
                let x = u16::from_le_bytes([descriptor[0], descriptor[1]]) as u32;
                let y = u16::from_le_bytes([descriptor[2], descriptor[3]]) as u32;
                let w = u16::from_le_bytes([descriptor[4], descriptor[5]]) as u32;
                let h = u16::from_le_bytes([descriptor[6], descriptor[7]]) as u32;
                if w == 0 || h == 0 || x + w > width || y + h > height {
                    return Err(Error::InvalidInput("GIF frame lies outside canvas".into()));
                }
                position += 9;
                if descriptor[8] & 0x80 != 0 {
                    position += 3 * (1usize << ((descriptor[8] & 7) + 1));
                }
                if input.get(position).is_none() {
                    return Err(Error::InvalidInput("missing GIF LZW size".into()));
                }
                position += 1;
                skip_gif_blocks(input, &mut position)?;
                frames = frames
                    .checked_add(1)
                    .ok_or_else(|| Error::InvalidInput("GIF frame count overflow".into()))?;
                if frames > max_frames {
                    return Err(Error::InvalidInput("animation frame limit exceeded".into()));
                }
            }
            0x21 => {
                let label = *input
                    .get(position)
                    .ok_or_else(|| Error::InvalidInput("truncated GIF extension".into()))?;
                position += 1;
                if label == 0xff
                    && input.get(position..position + 12) == Some(b"\x0bNETSCAPE2.0")
                    && let Some(loop_data) = input.get(position + 12..position + 17)
                    && loop_data[0] == 3
                    && loop_data[1] == 1
                {
                    loops = Some(u16::from_le_bytes([loop_data[2], loop_data[3]]) as u32);
                }
                skip_gif_blocks(input, &mut position)?;
            }
            _ => return Err(Error::InvalidInput("invalid GIF block".into())),
        }
    }
    if frames == 0 {
        return Err(Error::InvalidInput("GIF contains no images".into()));
    }
    Ok(GifHeader {
        width,
        height,
        frames,
        loops,
    })
}

fn skip_gif_blocks(input: &[u8], position: &mut usize) -> Result<()> {
    loop {
        let size = usize::from(
            *input
                .get(*position)
                .ok_or_else(|| Error::InvalidInput("truncated GIF data".into()))?,
        );
        *position += 1;
        if size == 0 {
            return Ok(());
        }
        *position = position
            .checked_add(size)
            .ok_or_else(|| Error::InvalidInput("GIF offset overflow".into()))?;
        if *position > input.len() {
            return Err(Error::InvalidInput("truncated GIF data".into()));
        }
    }
}

/// Chooses the same depth/area preference as the mature ICO decoder, but bounds embedded PNG metadata.
pub(crate) fn ico_png(input: &[u8]) -> Result<Option<(&[u8], u32, u32)>> {
    let count = input
        .get(4..6)
        .map(|v| u16::from_le_bytes(v.try_into().unwrap()) as usize)
        .ok_or_else(|| Error::InvalidInput("truncated ICO header".into()))?;
    if count == 0 {
        return Err(Error::InvalidInput("ICO has no entries".into()));
    }
    let directory_end = 6 + count * 16;
    if directory_end > input.len() {
        return Err(Error::InvalidInput("truncated ICO directory".into()));
    }
    let mut best = None;
    for entry in input[6..directory_end].chunks_exact(16).rev() {
        let width = if entry[0] == 0 {
            256
        } else {
            u32::from(entry[0])
        };
        let height = if entry[1] == 0 {
            256
        } else {
            u32::from(entry[1])
        };
        let depth = u16::from_le_bytes([entry[6], entry[7]]);
        let size = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        if offset < directory_end
            || size == 0
            || offset.checked_add(size).is_none_or(|end| end > input.len())
        {
            return Err(Error::InvalidInput("ICO entry exceeds file bounds".into()));
        }
        let score = (depth, width * height);
        if best.as_ref().is_none_or(|(old, _, _, _)| score > *old) {
            best = Some((score, &input[offset..offset + size], width, height));
        }
    }
    let (_, payload, width, height) = best.unwrap();
    Ok(payload
        .starts_with(b"\x89PNG\r\n\x1a\n")
        .then_some((payload, width, height)))
}

/// BITMAPV4/V5 color tags. Linked profiles are external mutable files and are never followed.
pub(crate) fn bmp_color<'a>(
    input: &'a [u8],
    budget: &Budget,
) -> Result<(ColorEncoding, Option<&'a [u8]>)> {
    let header_size = input
        .get(14..18)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
        .ok_or_else(|| Error::InvalidInput("truncated BMP DIB header".into()))?;
    if header_size < 108 {
        return Ok((ColorEncoding::Srgb, None));
    }
    let dib = input
        .get(
            14..14usize
                .checked_add(header_size)
                .ok_or_else(|| Error::InvalidInput("BMP header size overflow".into()))?,
        )
        .ok_or_else(|| Error::InvalidInput("truncated BMP color header".into()))?;
    let color = u32::from_le_bytes(dib[56..60].try_into().unwrap());
    match color {
        0x7352_4742 | 0x5769_6e20 => Ok((ColorEncoding::Srgb, None)),
        0x4c49_4e4b => Err(Error::Unsupported(
            "BMP linked color profiles require an explicit imported profile".into(),
        )),
        0x4d42_4544 if header_size >= 124 => {
            let offset = u32::from_le_bytes(dib[112..116].try_into().unwrap()) as usize;
            let size = u32::from_le_bytes(dib[116..120].try_into().unwrap()) as usize;
            if size as u64 > budget.policy().max_metadata_bytes {
                return Err(Error::ResourceLimit {
                    resource: ResourceKind::Metadata,
                    requested: size as u64,
                    limit: budget.policy().max_metadata_bytes,
                });
            }
            if offset < header_size || size == 0 {
                return Err(Error::InvalidInput(
                    "invalid BMP embedded profile range".into(),
                ));
            }
            let start = 14usize
                .checked_add(offset)
                .ok_or_else(|| Error::InvalidInput("BMP profile offset overflow".into()))?;
            let end = start
                .checked_add(size)
                .ok_or_else(|| Error::InvalidInput("BMP profile size overflow".into()))?;
            let profile = input
                .get(start..end)
                .ok_or_else(|| Error::InvalidInput("truncated BMP embedded profile".into()))?;
            Ok((ColorEncoding::Icc, Some(profile)))
        }
        0 => {
            let endpoints = std::array::from_fn(|channel| {
                std::array::from_fn(|coordinate| {
                    let offset = 60 + (channel * 3 + coordinate) * 4;
                    f64::from(i32::from_le_bytes(
                        dib[offset..offset + 4].try_into().unwrap(),
                    )) / 1_073_741_824.0
                })
            });
            let gamma = std::array::from_fn(|channel| {
                let offset = 96 + channel * 4;
                f64::from(u32::from_le_bytes(
                    dib[offset..offset + 4].try_into().unwrap(),
                )) / 65_536.0
            });
            Ok((ColorEncoding::CalibratedBmp { endpoints, gamma }, None))
        }
        _ => Ok((ColorEncoding::Unknown, None)),
    }
}
