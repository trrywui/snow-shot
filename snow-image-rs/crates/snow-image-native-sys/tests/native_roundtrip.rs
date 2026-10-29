#![cfg(feature = "native-backends")]
use snow_image_native_sys::*;
fn fixture() -> NativeImage {
    NativeImage {
        width: 7,
        height: 5,
        sample: SampleType::U8,
        pixels: (0..35)
            .flat_map(|i| {
                [
                    (i * 7) as u8,
                    (i * 3) as u8,
                    (255 - i * 5) as u8,
                    if i % 3 == 0 { 0 } else { 255 },
                ]
            })
            .collect(),
        icc: vec![],
        exif: vec![],
        xmp: vec![],
        significant_bits: 8,
        transfer: TransferFunction::Srgb,
        primaries: ColorPrimaries::Srgb,
        orientation: 1,
        hdr_reference_white_nits: 0.0,
    }
}
#[test]
fn lossless_backends_preserve_transparent_rgb() {
    for format in [NativeFormat::WebP, NativeFormat::Jxl, NativeFormat::Avif] {
        let source = fixture();
        let options = EncodeOptions {
            lossless: true,
            exact: true,
            effort: 5,
            speed: 8,
            ..Default::default()
        };
        let bytes = encode(format, &source, options).unwrap_or_else(|e| panic!("{format:?}: {e}"));
        let header = inspect(format, &bytes, DecodeOptions::default()).unwrap();
        assert_eq!((header.width, header.height), (7, 5));
        let decoded = decode(format, &bytes, DecodeOptions::default()).unwrap();
        assert_eq!(decoded.pixels, source.pixels, "{format:?}");
    }
}
#[test]
fn jpeg_streaming_decode_and_limits() {
    let mut source = fixture();
    for alpha in source.pixels.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    let mut encoded = vec![];
    encode_jpeg_rows(
        7,
        5,
        EncodeOptions::default(),
        &mut |row, data| {
            data.copy_from_slice(&source.pixels[row as usize * 28..(row as usize + 1) * 28]);
            Ok(())
        },
        &mut |chunk| {
            encoded.extend_from_slice(chunk);
            Ok(())
        },
    )
    .unwrap();
    let decoded = decode(NativeFormat::Jpeg, &encoded, DecodeOptions::default()).unwrap();
    assert_eq!((decoded.width, decoded.height), (7, 5));
    assert!(matches!(
        decode(
            NativeFormat::Jpeg,
            &encoded,
            DecodeOptions {
                max_output_bytes: 10,
                ..Default::default()
            }
        ),
        Err(Error::ResourceLimit(_))
    ));
    assert!(
        encode(
            NativeFormat::Jpeg,
            &source,
            EncodeOptions {
                max_output_bytes: 10,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn jpeg_decode_into_matches_owned_output_and_keeps_destination_guards() {
    let mut source = fixture();
    for alpha in source.pixels.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    let encoded = encode(NativeFormat::Jpeg, &source, EncodeOptions::default()).unwrap();
    for options in [
        DecodeOptions::default(),
        DecodeOptions {
            target_width: 3,
            target_height: 3,
            ..DecodeOptions::default()
        },
    ] {
        let expected = decode(NativeFormat::Jpeg, &encoded, options).unwrap();
        let length = expected.pixels.len();
        let mut guarded = vec![0xa5; length + 32];
        let info = decode_jpeg_into(&encoded, options, &mut guarded[16..16 + length]).unwrap();
        assert_eq!((info.width, info.height), (expected.width, expected.height));
        assert_eq!(info.sample, SampleType::U8);
        assert_eq!(info.significant_bits, 8);
        assert_eq!(&guarded[16..16 + length], expected.pixels);
        assert!(guarded[..16].iter().all(|value| *value == 0xa5));
        assert!(guarded[16 + length..].iter().all(|value| *value == 0xa5));
    }
    for length in [0, source.pixels.len() - 1, source.pixels.len() + 1] {
        let mut output = vec![0xa5; length];
        assert!(matches!(
            decode_jpeg_into(&encoded, DecodeOptions::default(), &mut output),
            Err(Error::InvalidInput(_))
        ));
        assert!(output.iter().all(|value| *value == 0xa5));
    }
}

#[test]
fn jpeg_decode_into_preserves_metadata_and_handles_limits_and_truncation() {
    let mut source = fixture();
    for alpha in source.pixels.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    source.icc = create_rgb_profile(
        [0.3127, 0.3290],
        [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]],
        [2.2; 3],
        65536,
    )
    .unwrap();
    let encoded = encode(NativeFormat::Jpeg, &source, EncodeOptions::default()).unwrap();
    let tiff = [
        b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
    ];
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend_from_slice(&tiff);
    let mut tagged = encoded[..2].to_vec();
    tagged.extend_from_slice(&[255, 225]);
    tagged.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    tagged.extend_from_slice(&exif);
    tagged.extend_from_slice(&encoded[2..]);
    let length = source.pixels.len();
    let mut guarded = vec![0xa5; length + 32];
    let info = decode_jpeg_into(
        &tagged,
        DecodeOptions::default(),
        &mut guarded[16..16 + length],
    )
    .unwrap();
    assert_eq!(info.icc, source.icc);
    assert_eq!(info.exif, tiff);
    assert_eq!(info.orientation, 6);
    for options in [
        DecodeOptions {
            max_metadata_bytes: 8,
            ..DecodeOptions::default()
        },
        DecodeOptions {
            max_output_bytes: length as u64 - 1,
            ..DecodeOptions::default()
        },
    ] {
        guarded.fill(0xa5);
        assert!(matches!(
            decode_jpeg_into(&tagged, options, &mut guarded[16..16 + length]),
            Err(Error::ResourceLimit(_))
        ));
        assert!(guarded.iter().all(|value| *value == 0xa5));
    }
    for _ in 0..16 {
        assert!(
            decode_jpeg_into(
                &tagged[..tagged.len() - 2],
                DecodeOptions::default(),
                &mut guarded[16..16 + length]
            )
            .is_err()
        );
        assert!(guarded[..16].iter().all(|value| *value == 0xa5));
        assert!(guarded[16 + length..].iter().all(|value| *value == 0xa5));
    }
    // A failed decode does not take ownership of the slice or poison later use.
    decode_jpeg_into(
        &tagged,
        DecodeOptions::default(),
        &mut guarded[16..16 + length],
    )
    .unwrap();
}
#[test]
fn jxl_u16_roundtrip_preserves_depth() {
    let mut source = fixture();
    source.sample = SampleType::U16;
    source.significant_bits = 16;
    source.pixels = source
        .pixels
        .iter()
        .flat_map(|v| ((*v as u16) * 251).to_ne_bytes())
        .collect();
    let encoded = encode(
        NativeFormat::Jxl,
        &source,
        EncodeOptions {
            lossless: true,
            exact: true,
            effort: 5,
            ..Default::default()
        },
    )
    .unwrap();
    let decoded = decode(NativeFormat::Jxl, &encoded, DecodeOptions::default()).unwrap();
    assert_eq!(decoded.sample, SampleType::U16);
    assert_eq!(decoded.pixels, source.pixels);
}
#[test]
fn resize_rows_match_resident_and_budget_is_checked() {
    let pixels: Vec<f32> = (0..32 * 32)
        .flat_map(|i| [(i % 32) as f32 / 32.0, (i / 32) as f32 / 32.0, 0.25, 1.0])
        .collect();
    let q = resize_requirements(32, 32, 9, 7).unwrap();
    let budget = q.scratch_bytes + q.row_buffer_bytes;
    assert!(resize_rgba_f32(&pixels, 32, 32, 9, 7, budget - 1).is_err());
    let expected = resize_rgba_f32(&pixels, 32, 32, 9, 7, budget).unwrap();
    let mut output = vec![0.0; 9 * 7 * 4];
    resize_regions_f32(
        32,
        32,
        9,
        7,
        budget,
        &mut |row, out| {
            out.copy_from_slice(&pixels[row as usize * 128..(row as usize + 1) * 128]);
            Ok(())
        },
        &mut |row, column, input| {
            let start = row as usize * 36 + column as usize * 4;
            output[start..start + input.len()].copy_from_slice(input);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(output, expected);
}
#[test]
fn icc_srgb_to_linear_preserves_alpha() {
    let mut pixels = [0.5, 0.5, 0.5, 0.25];
    transform_rgba_f32(&[], &mut pixels, true).unwrap();
    assert!((pixels[0] - 0.21404).abs() < 0.001);
    assert_eq!(pixels[3], 0.25);
    assert!(transform_rgba8(b"not a profile", &mut [1, 2, 3, 4]).is_err());
}
#[test]
fn callback_errors_never_unwind_across_ffi() {
    let result = encode_jpeg_rows(
        7,
        5,
        EncodeOptions::default(),
        &mut |_, _| Err(Error::InvalidInput("source failed".into())),
        &mut |_| Ok(()),
    );
    assert_eq!(result, Err(Error::InvalidInput("source failed".into())));
    let source = fixture();
    let result = encode_to_writer(
        NativeFormat::WebP,
        &source,
        EncodeOptions {
            effort: 4,
            ..Default::default()
        },
        &mut |_| Err(Error::ResourceLimit("sink quota".into())),
    );
    assert_eq!(result, Err(Error::ResourceLimit("sink quota".into())));
}
#[test]
fn malformed_images_fail_without_output() {
    for format in capabilities() {
        for input in [&[][..], &[0xff, 0xd8, 0xff, 0xd9][..], &[0; 64][..]] {
            assert!(decode(*format, input, DecodeOptions::default()).is_err());
        }
    }
}

#[test]
fn actual_icc_roundtrips_and_converts() {
    let mut source = fixture();
    let jxl = encode(
        NativeFormat::Jxl,
        &source,
        EncodeOptions {
            lossless: true,
            effort: 4,
            ..Default::default()
        },
    )
    .unwrap();
    source.icc = inspect(NativeFormat::Jxl, &jxl, DecodeOptions::default())
        .unwrap()
        .icc;
    assert!(!source.icc.is_empty());
    let webp = encode(
        NativeFormat::WebP,
        &source,
        EncodeOptions {
            lossless: true,
            exact: true,
            effort: 4,
            ..Default::default()
        },
    )
    .unwrap();
    let decoded = decode(NativeFormat::WebP, &webp, DecodeOptions::default()).unwrap();
    assert_eq!(decoded.icc, source.icc);
    assert_eq!(decoded.pixels, source.pixels);
    let mut gray = [0.5f32, 0.5, 0.5, 0.3];
    transform_rgba_f32(&source.icc, &mut gray, true).unwrap();
    assert!((gray[0] - 0.21404).abs() < 0.001);
    assert_eq!(gray[3], 0.3);
}
#[test]
fn chunked_jxl_rows_preserve_samples_and_errors() {
    let source = fixture();
    let info = NativeInfo {
        width: source.width,
        height: source.height,
        sample: source.sample,
        significant_bits: 8,
        transfer: source.transfer,
        primaries: source.primaries,
        orientation: 1,
        hdr_reference_white_nits: 0.0,
        has_alpha: true,
        animated: false,
        frame_count: 1,
        loop_count: 0,
        icc: vec![],
        exif: vec![],
        xmp: vec![],
    };
    let opts = EncodeOptions {
        lossless: true,
        exact: true,
        effort: 5,
        threads: 4,
        ..Default::default()
    };
    let mut output = vec![];
    encode_jxl_rows(
        &info,
        opts,
        &mut |y, bytes| {
            bytes.copy_from_slice(&source.pixels[y as usize * 28..(y as usize + 1) * 28]);
            Ok(())
        },
        &mut |bytes| {
            output.extend_from_slice(bytes);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        decode(NativeFormat::Jxl, &output, DecodeOptions::default())
            .unwrap()
            .pixels,
        source.pixels
    );
    let failed = encode_jxl_rows(
        &info,
        opts,
        &mut |_, _| Err(Error::ResourceLimit("row budget".into())),
        &mut |_| Ok(()),
    );
    assert_eq!(failed, Err(Error::ResourceLimit("row budget".into())));
}
#[test]
fn avif_ten_bit_uses_full_range_u16_samples() {
    let mut source = fixture();
    source.sample = SampleType::U16;
    source.significant_bits = 10;
    source.pixels = (0..140u32)
        .flat_map(|i| {
            let ten = (i * 7) % 1024;
            (((ten * 65535 + 511) / 1023) as u16).to_ne_bytes()
        })
        .collect();
    let bytes = encode(
        NativeFormat::Avif,
        &source,
        EncodeOptions {
            lossless: true,
            exact: true,
            bit_depth: 10,
            speed: 8,
            ..Default::default()
        },
    )
    .unwrap();
    let decoded = decode(NativeFormat::Avif, &bytes, DecodeOptions::default()).unwrap();
    assert_eq!(decoded.significant_bits, 10);
    assert_eq!(decoded.sample, SampleType::U16);
    assert_eq!(decoded.pixels, source.pixels);
}

#[test]
fn jpeg_orientation_and_raw_metadata_are_bounded() {
    let mut source = fixture();
    for alpha in source.pixels.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    let bytes = encode(NativeFormat::Jpeg, &source, EncodeOptions::default()).unwrap();
    let tiff = vec![
        b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
    ];
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&tiff);
    let mut tagged = bytes[..2].to_vec();
    tagged.extend_from_slice(&[255, 225]);
    tagged.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    tagged.extend_from_slice(&payload);
    tagged.extend_from_slice(&bytes[2..]);
    let info = inspect(NativeFormat::Jpeg, &tagged, DecodeOptions::default()).unwrap();
    assert_eq!(info.orientation, 6);
    assert_eq!(info.exif, tiff);
    assert!(matches!(
        inspect(
            NativeFormat::Jpeg,
            &tagged,
            DecodeOptions {
                max_metadata_bytes: 8,
                ..Default::default()
            }
        ),
        Err(Error::ResourceLimit(_))
    ));
    assert!(
        decode(
            NativeFormat::Jpeg,
            &bytes[..bytes.len() - 2],
            DecodeOptions::default()
        )
        .is_err()
    );
}
#[test]
fn explicit_scrgb_roundtrips_without_tagging_ordinary_float() {
    let mut source = fixture();
    source.sample = SampleType::F32;
    source.significant_bits = 32;
    source.transfer = TransferFunction::Linear;
    source.hdr_reference_white_nits = 80.0;
    source.pixels = (0..35)
        .flat_map(|_| {
            [-0.1f32, 0.5, 3.0, 1.0]
                .into_iter()
                .flat_map(f32::to_ne_bytes)
        })
        .collect();
    let bytes = encode(
        NativeFormat::Jxl,
        &source,
        EncodeOptions {
            lossless: true,
            effort: 5,
            ..Default::default()
        },
    )
    .unwrap();
    let decoded = decode(NativeFormat::Jxl, &bytes, DecodeOptions::default()).unwrap();
    assert_eq!(decoded.hdr_reference_white_nits, 80.0);
    assert_eq!(decoded.transfer, TransferFunction::Linear);
    let values: Vec<f32> = decoded
        .pixels
        .chunks_exact(4)
        .map(|p| f32::from_ne_bytes(p.try_into().unwrap()))
        .collect();
    assert!((values[0] + 0.1).abs() < 0.001);
    assert!((values[2] - 3.0).abs() < 0.001);
    source.hdr_reference_white_nits = 0.0;
    let sdr = encode(
        NativeFormat::Jxl,
        &source,
        EncodeOptions {
            lossless: true,
            effort: 5,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        inspect(NativeFormat::Jxl, &sdr, DecodeOptions::default())
            .unwrap()
            .hdr_reference_white_nits,
        0.0
    );
}

#[test]
fn wide_tall_resize_handles_horizontal_tiles() {
    let requirements = resize_requirements(3840, 2160, 2048, 1152).unwrap();
    let mut output_rows = vec![false; 1152 * 2048];
    resize_regions_f32(
        3840,
        2160,
        2048,
        1152,
        requirements.scratch_bytes + requirements.row_buffer_bytes,
        &mut |_, pixels| {
            for rgba in pixels.chunks_exact_mut(4) {
                rgba.copy_from_slice(&[0.25, 0.5, 0.75, 1.0]);
            }
            Ok(())
        },
        &mut |row, column, pixels| {
            for rgba in pixels.chunks_exact(4) {
                for (actual, expected) in rgba.iter().zip([0.25, 0.5, 0.75, 1.0]) {
                    assert!((actual - expected).abs() < 0.00001);
                }
            }
            let start = row as usize * 2048 + column as usize;
            output_rows[start..start + pixels.len() / 4].fill(true);
            Ok(())
        },
    )
    .unwrap();
    assert!(output_rows.iter().all(|&v| v));
}

#[test]
fn jpeg_fractional_native_scale_meets_preview_bounds() {
    let mut source = fixture();
    source.width = 128;
    source.height = 72;
    source.pixels = (0..128 * 72)
        .flat_map(|i| [(i % 256) as u8, 64, 192, 255])
        .collect();
    let encoded = encode(NativeFormat::Jpeg, &source, EncodeOptions::default()).unwrap();
    let decoded = decode(
        NativeFormat::Jpeg,
        &encoded,
        DecodeOptions {
            target_width: 70,
            target_height: 40,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!((decoded.width, decoded.height), (80, 45));
}

#[test]
fn lossy_scrgb_preserves_highlight_scale() {
    let mut source = fixture();
    source.width = 2;
    source.height = 1;
    source.sample = SampleType::F32;
    source.significant_bits = 32;
    source.transfer = TransferFunction::Linear;
    source.hdr_reference_white_nits = 160.0;
    source.pixels = [2.0f32, 1.0, 0.5, 1.0, 4.0, 2.0, 1.0, 1.0]
        .into_iter()
        .flat_map(f32::to_ne_bytes)
        .collect();
    let bytes = encode(
        NativeFormat::Jxl,
        &source,
        EncodeOptions {
            distance: 0.01,
            effort: 7,
            ..Default::default()
        },
    )
    .unwrap();
    let decoded = decode(NativeFormat::Jxl, &bytes, DecodeOptions::default()).unwrap();
    let first = f32::from_ne_bytes(decoded.pixels[..4].try_into().unwrap());
    assert!((first - 2.0).abs() < 0.05, "decoded highlight {first}");
}

#[test]
fn half_float_jxl_preserves_extended_range_and_rejects_nonfinite_samples() {
    let mut source = fixture();
    source.sample = SampleType::F16;
    source.significant_bits = 16;
    source.transfer = TransferFunction::Linear;
    source.hdr_reference_white_nits = 80.0;
    source.pixels = (0..35)
        .flat_map(|_| {
            [0xb800u16, 0x3800, 0x4200, 0x3c00]
                .into_iter()
                .flat_map(u16::to_ne_bytes)
        })
        .collect();
    let options = EncodeOptions {
        lossless: true,
        ..Default::default()
    };
    let bytes = encode(NativeFormat::Jxl, &source, options).unwrap();
    let decoded = decode(NativeFormat::Jxl, &bytes, DecodeOptions::default()).unwrap();
    assert_eq!(decoded.sample, SampleType::F16);
    assert_eq!(decoded.pixels, source.pixels);
    source.pixels[..2].copy_from_slice(&0x7e00u16.to_ne_bytes());
    assert!(encode(NativeFormat::Jxl, &source, options).is_err());
}

#[test]
fn resize_interleave_matches_scalar_samples_at_vector_boundaries() {
    for width in [1u32, 3, 4, 5, 7, 16, 33, 2049] {
        let source: Vec<f32> = (0..width as usize * 3 * 4)
            .map(|i| ((i * 17) % 1000) as f32 / 1000.0)
            .collect();
        let query = resize_requirements(width, 3, width, 3).unwrap();
        let limit = query.scratch_bytes + query.row_buffer_bytes;
        let resident = resize_rgba_f32(&source, width, 3, width, 3, limit).unwrap();
        assert_eq!(resident, source, "resident width {width}");
        let mut streamed = vec![0.0f32; source.len()];
        resize_regions_f32(
            width,
            3,
            width,
            3,
            limit,
            &mut |row, out| {
                let start = row as usize * width as usize * 4;
                out.copy_from_slice(&source[start..start + out.len()]);
                Ok(())
            },
            &mut |row, column, region| {
                let start = (row as usize * width as usize + column as usize) * 4;
                streamed[start..start + region.len()].copy_from_slice(region);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(streamed, source, "streamed width {width}");
    }
}

#[test]
fn calibrated_rgb_profile_uses_real_per_channel_trcs_and_limits() {
    let white = [0.3127, 0.3290];
    let primaries = [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]];
    let profile = create_rgb_profile(white, primaries, [1.0, 2.0, 3.0], 4096).unwrap();
    assert!(!profile.is_empty());
    let mut rgba = [0.5f32, 0.5, 0.5, 0.4];
    transform_rgba_f32(&profile, &mut rgba, true).unwrap();
    for (value, expected) in rgba.into_iter().zip([0.5f32, 0.25, 0.125, 0.4]) {
        assert!((value - expected).abs() < 0.0002, "{value} != {expected}");
    }
    assert!(matches!(
        create_rgb_profile(white, primaries, [2.2; 3], 8),
        Err(Error::ResourceLimit(_))
    ));
    assert!(matches!(
        create_rgb_profile([0.3, 0.0], primaries, [2.2; 3], 4096),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        create_rgb_profile(white, primaries, [f64::NAN; 3], 4096),
        Err(Error::InvalidInput(_))
    ));
}
