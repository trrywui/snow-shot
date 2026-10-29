use snow_image_codecs::*;
use snow_image_core::*;
use std::{borrow::Cow, io::Write};

fn rgba(budget: &Budget) -> Image {
    Image::from_vec(
        ImageDescriptor::packed(3, 2, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap(),
        vec![
            12, 34, 56, 0, 255, 0, 127, 128, 1, 2, 3, 255, 77, 99, 33, 255, 0, 0, 0, 0, 255, 255,
            255, 255,
        ],
        Metadata::default(),
        budget,
    )
    .unwrap()
}
fn encoded(image: &dyn RasterSource, options: EncodeOptions, budget: &Budget) -> Vec<u8> {
    let mut bytes = vec![];
    let receipt = encode(
        image,
        &options,
        &mut bytes,
        budget,
        &CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(receipt.bytes, bytes.len() as u64);
    bytes
}

#[test]
fn png_roundtrip_preserves_transparent_rgb_and_padding() {
    let budget = Budget::default();
    let descriptor = ImageDescriptor {
        width: 2,
        height: 2,
        row_stride: 12,
        format: PixelFormat::BGRA8,
        color: ColorEncoding::Srgb,
        orientation: Orientation::Normal,
    };
    let image = Image::from_vec(
        descriptor,
        vec![
            1, 2, 3, 0, 4, 5, 6, 255, 9, 9, 9, 9, 7, 8, 9, 128, 10, 11, 12, 255, 8, 8, 8, 8,
        ],
        Metadata::default(),
        &budget,
    )
    .unwrap();
    let bytes = encoded(&image, EncodeOptions::default(), &budget);
    let result = decode(
        &bytes,
        &DecodeOptions::default(),
        &budget,
        &CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(
        result.bytes(),
        [3, 2, 1, 0, 6, 5, 4, 255, 9, 8, 7, 128, 12, 11, 10, 255]
    );
    drop(image);
    drop(result);
    assert_eq!(budget.total_used(), 0);
}

#[test]
fn png_u16_roundtrip_uses_native_endian_samples() {
    let budget = Budget::default();
    let format = PixelFormat {
        sample: SampleType::U16,
        significant_bits: 16,
        ..PixelFormat::RGBA8
    };
    let samples = [1u16, 256, 65535, 0, 500, 0, 12345, 40000];
    let pixels = samples.into_iter().flat_map(u16::to_ne_bytes).collect();
    let image = Image::from_vec(
        ImageDescriptor::packed(2, 1, format, ColorEncoding::Srgb).unwrap(),
        pixels,
        Metadata::default(),
        &budget,
    )
    .unwrap();
    let bytes = encoded(&image, EncodeOptions::default(), &budget);
    let output = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(output.bytes(), image.bytes());
    assert_eq!(output.descriptor().format.sample, SampleType::U16);
}

#[test]
fn bmp_alpha_interoperates_with_mature_decoder() {
    let budget = Budget::default();
    let image = rgba(&budget);
    let bytes = encoded(&image, EncodeOptions::Bmp(Default::default()), &budget);
    let result = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(result.bytes(), image.bytes());
}

#[test]
fn cancellation_and_output_failure_release_temporary_reservations() {
    struct Failing;
    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("injected disk full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let budget = Budget::default();
    let image = rgba(&budget);
    let baseline = budget.total_used();
    assert!(
        encode(
            &image,
            &Default::default(),
            &mut Failing,
            &budget,
            &Default::default()
        )
        .is_err()
    );
    assert_eq!(budget.total_used(), baseline);
    let cancel = CancellationToken::default();
    cancel.cancel();
    assert!(matches!(
        encode(
            &image,
            &Default::default(),
            &mut Vec::new(),
            &budget,
            &cancel
        ),
        Err(Error::Cancelled)
    ));
    assert_eq!(budget.total_used(), baseline);
}

#[test]
fn budgets_reject_before_decoding_large_pixels() {
    let original = Budget::default();
    let bytes = encoded(&rgba(&original), Default::default(), &original);
    let budget = Budget::new(ResourcePolicy {
        max_raster_bytes: 4,
        ..Default::default()
    });
    assert!(matches!(
        decode(&bytes, &Default::default(), &budget, &Default::default()),
        Err(Error::ResourceLimit { .. })
    ));
    assert_eq!(budget.total_used(), 0);
    for n in 0..bytes.len() {
        let _ = decode(
            &bytes[..n],
            &Default::default(),
            &budget,
            &Default::default(),
        );
        assert_eq!(budget.total_used(), 0);
    }
}

fn apng(hidden_poster: bool) -> Vec<u8> {
    let mut bytes = vec![];
    {
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(3, 0).unwrap();
        encoder.set_sep_def_img(hidden_poster).unwrap();
        encoder.set_frame_delay(1, 10).unwrap();
        let mut writer = encoder.write_header().unwrap();
        if hidden_poster {
            writer
                .write_image_data(&[9, 9, 9, 255, 9, 9, 9, 255])
                .unwrap();
        }
        writer
            .write_image_data(&[255, 0, 0, 255, 255, 0, 0, 255])
            .unwrap();
        writer.set_frame_dimension(1, 1).unwrap();
        writer.set_frame_position(0, 0).unwrap();
        writer.set_dispose_op(png::DisposeOp::Previous).unwrap();
        writer.set_blend_op(png::BlendOp::Source).unwrap();
        writer.write_image_data(&[0, 0, 255, 255]).unwrap();
        writer.set_frame_position(1, 0).unwrap();
        writer.set_dispose_op(png::DisposeOp::None).unwrap();
        writer.write_image_data(&[0, 255, 0, 255]).unwrap();
        writer.finish().unwrap();
    }
    bytes
}

#[test]
fn apng_hidden_poster_disposal_replay_and_snapshot_ownership() {
    for hidden in [false, true] {
        let budget = Budget::default();
        let bytes = apng(hidden);
        let mut animation =
            AnimationSession::open(&bytes, &Default::default(), &budget, &Default::default())
                .unwrap();
        assert_eq!(animation.info().has_hidden_poster, hidden);
        let first = animation.frame(0, &Default::default()).unwrap();
        assert_eq!(first.image.bytes(), [255, 0, 0, 255, 255, 0, 0, 255]);
        let second = animation.next_frame(&Default::default()).unwrap().unwrap();
        assert_eq!(second.image.bytes(), [0, 0, 255, 255, 255, 0, 0, 255]);
        let third = animation.next_frame(&Default::default()).unwrap().unwrap();
        assert_eq!(third.image.bytes(), [255, 0, 0, 255, 0, 255, 0, 255]);
        let replay = animation.frame(0, &Default::default()).unwrap();
        assert_eq!(replay.image.bytes(), first.image.bytes());
        drop(animation);
        assert_eq!(first.image.bytes(), [255, 0, 0, 255, 255, 0, 0, 255]);
        drop(first);
        drop(second);
        drop(third);
        drop(replay);
        assert_eq!(budget.total_used(), 0);
    }
}

#[test]
fn gif_lazy_frames_and_malformed_trailer() {
    let mut bytes = vec![];
    {
        let mut encoder =
            gif::Encoder::new(&mut bytes, 2, 1, &[0, 0, 0, 255, 0, 0, 0, 255, 0]).unwrap();
        encoder.set_repeat(gif::Repeat::Infinite).unwrap();
        let first = gif::Frame {
            width: 2,
            height: 1,
            buffer: Cow::Borrowed(&[1, 1]),
            delay: 4,
            dispose: gif::DisposalMethod::Keep,
            ..Default::default()
        };
        encoder.write_frame(&first).unwrap();
        let second = gif::Frame {
            width: 1,
            height: 1,
            left: 1,
            buffer: Cow::Borrowed(&[2]),
            delay: 7,
            ..Default::default()
        };
        encoder.write_frame(&second).unwrap();
    }
    let budget = Budget::default();
    let info = inspect(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(info.frame_count, 2);
    let mut animation =
        AnimationSession::open(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(
        animation
            .next_frame(&Default::default())
            .unwrap()
            .unwrap()
            .duration
            .as_millis(),
        40
    );
    let last = animation.next_frame(&Default::default()).unwrap().unwrap();
    assert_eq!(last.image.bytes(), [255, 0, 0, 255, 0, 255, 0, 255]);
    assert!(
        inspect(
            &bytes[..bytes.len() - 1],
            &Default::default(),
            &budget,
            &Default::default()
        )
        .is_err()
    );
}

#[test]
fn screenshot_sdr_float_does_not_infer_hdr_from_storage() {
    let budget = Budget::default();
    let format = PixelFormat {
        sample: SampleType::F32,
        significant_bits: 32,
        ..PixelFormat::RGBA8
    };
    let pixels = [0.25f32, 0.5, 1.0, 0.5]
        .into_iter()
        .flat_map(f32::to_ne_bytes)
        .collect();
    let image = Image::from_vec(
        ImageDescriptor::packed(1, 1, format, ColorEncoding::Srgb).unwrap(),
        pixels,
        Default::default(),
        &budget,
    )
    .unwrap();
    let output = transform(&image, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(output.bytes(), [64, 128, 255, 128]);
    assert_eq!(output.descriptor().color, ColorEncoding::Srgb);
}

#[test]
fn screenshot_scrgb_preserves_sdr_colors() {
    let budget = Budget::default();
    let format = PixelFormat {
        sample: SampleType::F16,
        significant_bits: 16,
        ..PixelFormat::RGBA8
    };
    let pixels = [0.0f32, 0.5, 1.0, 1.0]
        .into_iter()
        .flat_map(|v| half::f16::from_f32(v).to_bits().to_ne_bytes())
        .collect();
    let image = Image::from_vec(
        ImageDescriptor::packed(
            1,
            1,
            format,
            ColorEncoding::ScRgb {
                reference_white_nits: 80.0,
            },
        )
        .unwrap(),
        pixels,
        Default::default(),
        &budget,
    )
    .unwrap();
    let result = transform(&image, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(result.bytes(), [0, 188, 255, 255]);
}

#[test]
fn orientation_rotates_exact_samples() {
    let budget = Budget::default();
    let mut image = rgba(&budget).descriptor().clone();
    image.orientation = Orientation::Rotate90;
    let image = Image::from_vec(
        image,
        rgba(&budget).bytes().to_vec(),
        Default::default(),
        &budget,
    )
    .unwrap();
    let result = transform(
        &image,
        &TransformOptions {
            dynamic_range: DynamicRangePolicy::PreserveDynamicRange,
            ..Default::default()
        },
        &budget,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        (result.descriptor().width, result.descriptor().height),
        (2, 3)
    );
    assert_eq!(&result.bytes()[..8], &[77, 99, 33, 255, 12, 34, 56, 0]);
    assert_eq!(result.descriptor().orientation, Orientation::Normal);
}

#[test]
fn orientation_strips_geometry_metadata_but_retains_icc_and_the_original_owner() {
    let budget = Budget::default();
    {
        let mut descriptor = rgba(&budget).descriptor().clone();
        descriptor.orientation = Orientation::Rotate90;
        descriptor.color = ColorEncoding::Icc;
        let metadata = Metadata {
            icc: Some(include_bytes!("fixtures/linear_srgb.icc").to_vec()),
            exif: Some(vec![6, 7, 8]),
            xmp: Some(b"<tiff:Orientation>6</tiff:Orientation>".to_vec()),
        };
        let source = Image::from_vec(
            descriptor,
            rgba(&budget).bytes().to_vec(),
            metadata.clone(),
            &budget,
        )
        .unwrap();
        let mut options = TransformOptions {
            apply_orientation: false,
            dynamic_range: DynamicRangePolicy::PreserveDynamicRange,
            ..Default::default()
        };
        let untouched = transform(&source, &options, &budget, &Default::default()).unwrap();
        assert_eq!(untouched.metadata(), &metadata);
        assert!(std::ptr::eq(untouched.metadata(), source.metadata()));
        options.apply_orientation = true;
        let oriented = transform(&source, &options, &budget, &Default::default()).unwrap();
        assert_eq!(oriented.descriptor().orientation, Orientation::Normal);
        assert_eq!(
            (oriented.descriptor().width, oriented.descriptor().height),
            (2, 3)
        );
        assert_eq!(oriented.descriptor().color, ColorEncoding::Icc);
        assert_eq!(oriented.metadata().icc, metadata.icc);
        assert!(oriented.metadata().exif.is_none());
        assert!(oriented.metadata().xmp.is_none());
        assert_eq!(source.metadata(), &metadata);
        let again = transform(&oriented, &options, &budget, &Default::default()).unwrap();
        assert!(std::ptr::eq(oriented.metadata(), again.metadata()));
    }
    assert_eq!(budget.total_used(), 0);
}

#[cfg(feature = "native-backends")]
#[test]
fn resize_outputs_strip_source_geometry_metadata_in_resident_and_streaming_paths() {
    let budget = Budget::default();
    {
        let mut descriptor = rgba(&budget).descriptor().clone();
        descriptor.color = ColorEncoding::Icc;
        let source = Image::from_vec(
            descriptor,
            rgba(&budget).bytes().to_vec(),
            Metadata {
                icc: Some(include_bytes!("fixtures/linear_srgb.icc").to_vec()),
                exif: Some(vec![1, 2, 3]),
                xmp: Some(b"<tiff:ImageWidth>3</tiff:ImageWidth>".to_vec()),
            },
            &budget,
        )
        .unwrap();
        for dynamic_range in [
            DynamicRangePolicy::ScreenshotSdr,
            DynamicRangePolicy::PreserveDynamicRange,
        ] {
            let options = TransformOptions {
                max_dimensions: Some((1, 1)),
                dynamic_range,
                ..Default::default()
            };
            let resident = transform(&source, &options, &budget, &Default::default()).unwrap();
            let streamed =
                transform_source(&source, &options, &budget, &Default::default()).unwrap();
            assert_eq!(
                (resident.descriptor().width, resident.descriptor().height),
                (1, 1)
            );
            assert_eq!(resident.metadata(), &Metadata::default());
            assert_eq!(streamed.metadata(), Some(&Metadata::default()));
            // These resize paths convert ICC samples to their declared sRGB or
            // linear-sRGB output, so the original profile must not be reattached.
            assert_ne!(resident.descriptor().color, ColorEncoding::Icc);
            assert_eq!(resident.descriptor().color, streamed.descriptor().color);
        }
        assert!(source.metadata().exif.is_some());
        assert!(source.metadata().xmp.is_some());
    }
    assert_eq!(budget.total_used(), 0);
    assert_eq!(budget.used(ResourceKind::TempDisk), 0);
}

#[test]
fn streamed_identity_transform_keeps_transparent_rgb_without_native_backend() {
    let budget = Budget::default();
    let image = rgba(&budget);
    let source =
        transform_source(&image, &Default::default(), &budget, &Default::default()).unwrap();
    let mut bytes = vec![0; image.bytes().len()];
    source.read_rows(0, 2, &mut bytes).unwrap();
    assert_eq!(bytes, image.bytes());
    drop(source);
    drop(image);
    assert_eq!(budget.total_used(), 0);
}

#[test]
fn streamed_sdr_transform_preserves_hidden_rgb_and_releases_disk() {
    let budget = Budget::default();
    let format = PixelFormat {
        sample: SampleType::F32,
        significant_bits: 32,
        ..PixelFormat::RGBA8
    };
    let pixels = [0.25f32, 0.5, 1.0, 0.0]
        .into_iter()
        .flat_map(f32::to_ne_bytes)
        .collect();
    let image = Image::from_vec(
        ImageDescriptor::packed(1, 1, format, ColorEncoding::Srgb).unwrap(),
        pixels,
        Default::default(),
        &budget,
    )
    .unwrap();
    let source =
        transform_source(&image, &Default::default(), &budget, &Default::default()).unwrap();
    let mut bytes = [0; 4];
    source.read_rows(0, 1, &mut bytes).unwrap();
    assert_eq!(bytes, [64, 128, 255, 0]);
    drop(source);
    assert_eq!(budget.used(ResourceKind::TempDisk), 0);
}

#[cfg(feature = "native-backends")]
mod native_workflows {
    use super::*;
    #[test]
    fn still_native_backend_roundtrips() {
        let budget = Budget::default();
        let image = rgba(&budget);
        for options in [
            EncodeOptions::WebP(Default::default()),
            EncodeOptions::Jxl(Default::default()),
            EncodeOptions::Avif(AvifOptions {
                quality: 100,
                alpha_quality: 100,
                speed: 8,
                bit_depth: 8,
                ..Default::default()
            }),
        ] {
            let exact = matches!(options, EncodeOptions::WebP(_) | EncodeOptions::Jxl(_));
            let bytes = encoded(&image, options, &budget);
            let info = inspect(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
            assert_eq!((info.width, info.height), (3, 2));
            let decoded =
                decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
            if exact {
                assert_eq!(decoded.bytes(), image.bytes());
            }
            assert_eq!(
                (decoded.descriptor().width, decoded.descriptor().height),
                (3, 2)
            );
        }
    }
    #[test]
    fn avif_default_expands_precision_without_reducing_source() {
        let budget = Budget::default();
        let image = rgba(&budget);
        let bytes = encoded(&image, EncodeOptions::Avif(Default::default()), &budget);
        let decoded = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
        assert_eq!(decoded.descriptor().format.sample, SampleType::U16);
        assert_eq!(decoded.descriptor().format.significant_bits, 10);
        let transformed =
            transform(&decoded, &Default::default(), &budget, &Default::default()).unwrap();
        assert!(transformed.bytes().iter().any(|&v| v > 200));
    }
    struct TallRows {
        descriptor: ImageDescriptor,
    }
    impl RasterSource for TallRows {
        fn descriptor(&self) -> &ImageDescriptor {
            &self.descriptor
        }
        fn access(&self) -> RasterAccess {
            RasterAccess::SequentialRows
        }
        fn read_rows(&self, first: u32, rows: u32, out: &mut [u8]) -> Result<()> {
            if first + rows > self.descriptor.height
                || out.len() != rows as usize * self.descriptor.row_bytes()?
            {
                return Err(Error::InvalidInput("invalid generated source read".into()));
            }
            for p in out.chunks_exact_mut(4) {
                p.copy_from_slice(&[200, 100, 50, 255]);
            }
            Ok(())
        }
    }
    #[test]
    fn jpeg_streams_tall_sources_beyond_working_allocation() {
        let budget = Budget::new(ResourcePolicy {
            working_bytes_per_job: 1024 * 1024,
            total_memory_bytes: 8 * 1024 * 1024,
            ..Default::default()
        });
        let source = TallRows {
            descriptor: ImageDescriptor::packed(32, 30000, PixelFormat::RGBA8, ColorEncoding::Srgb)
                .unwrap(),
        };
        let bytes = encoded(&source, EncodeOptions::Jpeg(Default::default()), &budget);
        let info = inspect(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
        assert_eq!((info.width, info.height), (32, 30000));
        assert_eq!(budget.total_used(), 0);
    }
    #[test]
    fn resize_uses_linear_premultiplied_samples() {
        let budget = Budget::default();
        let mut image = MutableImage::new(
            ImageDescriptor::packed(64, 64, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap(),
            Default::default(),
            &budget,
        )
        .unwrap();
        for (index, p) in image.bytes_mut().chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(if index % 2 == 0 {
                &[255, 0, 0, 255]
            } else {
                &[0, 0, 255, 0]
            });
        }
        let image = image.seal();
        let options = TransformOptions {
            max_dimensions: Some((8, 8)),
            ..Default::default()
        };
        let resized = transform(&image, &options, &budget, &Default::default()).unwrap();
        for pixel in resized.bytes().chunks_exact(4) {
            assert!(pixel[0] >= 250);
            assert!(pixel[2] <= 2);
            assert!((110..=145).contains(&pixel[3]));
        }
        let file = transform_source(&image, &options, &budget, &Default::default()).unwrap();
        let mut bytes = vec![0; 8 * 8 * 4];
        file.read_rows(0, 8, &mut bytes).unwrap();
        assert_eq!(bytes, resized.bytes());
    }
    #[test]
    fn staged_streaming_resize_reduces_more_than_sixteenfold() {
        let budget = Budget::new(ResourcePolicy {
            working_bytes_per_job: 1024 * 1024,
            total_memory_bytes: 8 * 1024 * 1024,
            ..Default::default()
        });
        let source = TallRows {
            descriptor: ImageDescriptor::packed(2, 257, PixelFormat::RGBA8, ColorEncoding::Srgb)
                .unwrap(),
        };
        let result = transform_source(
            &source,
            &TransformOptions {
                max_dimensions: Some((1, 1)),
                ..Default::default()
            },
            &budget,
            &Default::default(),
        )
        .unwrap();
        let mut pixel = [0; 4];
        result.read_rows(0, 1, &mut pixel).unwrap();
        assert_eq!(pixel, [200, 100, 50, 255]);
        drop(result);
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    }
}

#[test]
fn capability_report_matches_compiled_backends() {
    let capabilities = capabilities();
    assert_eq!(capabilities.len(), 8);
    let png = capabilities
        .iter()
        .find(|c| c.format == Format::Png)
        .unwrap();
    assert!(png.decode && png.encode && png.animation_decode);
    assert_eq!(png.input_sample_types, [SampleType::U8, SampleType::U16]);
    let jpeg = capabilities
        .iter()
        .find(|c| c.format == Format::Jpeg)
        .unwrap();
    assert_eq!(jpeg.encode, cfg!(feature = "native-backends"));
    assert!(!jpeg.supports_alpha);
}

#[cfg(feature = "native-backends")]
#[test]
fn actual_icc_transform_changes_samples_before_labeling_srgb() {
    let budget = Budget::default();
    let profile = include_bytes!("fixtures/linear_srgb.icc").to_vec();
    let image = Image::from_vec(
        ImageDescriptor::packed(1, 1, PixelFormat::RGBA8, ColorEncoding::Icc).unwrap(),
        vec![128, 128, 128, 128],
        Metadata {
            icc: Some(profile),
            exif: Some(vec![1, 2, 3]),
            xmp: Some(vec![4, 5, 6]),
        },
        &budget,
    )
    .unwrap();
    let bytes = encoded(&image, EncodeOptions::default(), &budget);
    let decoded = decode(
        &bytes,
        &DecodeOptions {
            preserve_metadata: false,
            ..Default::default()
        },
        &budget,
        &Default::default(),
    )
    .unwrap();
    assert!(decoded.metadata().icc.is_some());
    assert_eq!(decoded.descriptor().color, ColorEncoding::Icc);
    let sdr = transform(&decoded, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(sdr.descriptor().color, ColorEncoding::Srgb);
    assert!((187..=189).contains(&sdr.bytes()[0]));
    assert_eq!(sdr.bytes()[3], 128);
    assert_eq!(sdr.metadata(), &Metadata::default());
    let file =
        transform_source(&decoded, &Default::default(), &budget, &Default::default()).unwrap();
    let mut pixel = [0; 4];
    file.read_rows(0, 1, &mut pixel).unwrap();
    assert_eq!(pixel, sdr.bytes());
}

#[cfg(feature = "native-backends")]
#[test]
fn hdr_pq_jxl_preserves_transfer_and_samples() {
    let budget = Budget::default();
    let format = PixelFormat {
        sample: SampleType::U16,
        significant_bits: 16,
        ..PixelFormat::RGBA8
    };
    let pixels = [30000u16, 33000, 37000, 65535]
        .into_iter()
        .flat_map(u16::to_ne_bytes)
        .collect();
    let image = Image::from_vec(
        ImageDescriptor::packed(1, 1, format, ColorEncoding::Bt2020Pq).unwrap(),
        pixels,
        Default::default(),
        &budget,
    )
    .unwrap();
    let bytes = encoded(&image, EncodeOptions::Jxl(Default::default()), &budget);
    let decoded = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(decoded.descriptor().color, ColorEncoding::Bt2020Pq);
    assert_eq!(decoded.bytes(), image.bytes());
    let sdr = transform(&decoded, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(sdr.descriptor().color, ColorEncoding::Srgb);
    assert_eq!(sdr.bytes()[3], 255);
}

#[test]
fn selected_ico_png_obeys_png_limits_and_directory_dimensions() {
    let budget = Budget::default();
    let image = rgba(&budget);
    let png = encoded(&image, Default::default(), &budget);
    let mut ico = vec![0, 0, 1, 0, 1, 0, 3, 2, 0, 0, 1, 0, 32, 0];
    ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(&png);
    let decoded = decode(&ico, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(decoded.bytes(), image.bytes());
    assert_eq!(
        inspect(&ico, &Default::default(), &budget, &Default::default())
            .unwrap()
            .format,
        Format::Ico
    );
    ico[6] = 4;
    assert!(decode(&ico, &Default::default(), &budget, &Default::default()).is_err());
}

#[test]
fn cancellation_during_writer_io_returns_instead_of_retrying_forever() {
    struct Cancelling<'a>(&'a CancellationToken);
    impl Write for Cancelling<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.cancel();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let budget = Budget::default();
    let image = rgba(&budget);
    let cancel = CancellationToken::default();
    assert!(
        encode(
            &image,
            &Default::default(),
            &mut Cancelling(&cancel),
            &budget,
            &cancel
        )
        .is_err()
    );
    assert_eq!(budget.used(ResourceKind::Scratch), 0);
}

#[cfg(feature = "native-backends")]
#[test]
fn png_preview_spools_large_source_with_bounded_rows() {
    struct Source(ImageDescriptor);
    impl RasterSource for Source {
        fn descriptor(&self) -> &ImageDescriptor {
            &self.0
        }
        fn access(&self) -> RasterAccess {
            RasterAccess::SequentialRows
        }
        fn read_rows(&self, _: u32, _: u32, output: &mut [u8]) -> Result<()> {
            for pixel in output.chunks_exact_mut(4) {
                pixel.copy_from_slice(&[200, 100, 50, 255]);
            }
            Ok(())
        }
    }
    let budget = Budget::new(ResourcePolicy {
        working_bytes_per_job: 10 * 1024 * 1024,
        total_memory_bytes: 16 * 1024 * 1024,
        ..Default::default()
    });
    let source = Source(
        ImageDescriptor::packed(32, 30000, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap(),
    );
    let png = encoded(
        &source,
        EncodeOptions::Png(PngOptions {
            compression: PngCompression::Fast,
            ..Default::default()
        }),
        &budget,
    );
    let preview = preview(&png, (32, 128), &budget, &Default::default()).unwrap();
    assert_eq!(
        (preview.descriptor().width, preview.descriptor().height),
        (1, 128)
    );
    for pixel in preview.bytes().chunks_exact(4) {
        assert_eq!(pixel, [200, 100, 50, 255]);
    }
    drop(preview);
    assert_eq!(budget.total_used(), 0);
    assert_eq!(budget.used(ResourceKind::TempDisk), 0);
}

#[cfg(feature = "native-backends")]
#[test]
fn jpeg_preview_reserves_reduced_dimensions() {
    let source_budget = Budget::default();
    let mut input = MutableImage::new(
        ImageDescriptor::packed(2048, 2048, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap(),
        Default::default(),
        &source_budget,
    )
    .unwrap();
    for pixel in input.bytes_mut().chunks_exact_mut(4) {
        pixel.copy_from_slice(&[150, 100, 50, 255]);
    }
    let jpeg = encoded(
        &input.seal(),
        EncodeOptions::Jpeg(Default::default()),
        &source_budget,
    );
    let budget = Budget::new(ResourcePolicy {
        working_bytes_per_job: 4 * 1024 * 1024,
        total_memory_bytes: 8 * 1024 * 1024,
        ..Default::default()
    });
    let preview = preview(&jpeg, (256, 256), &budget, &Default::default()).unwrap();
    assert_eq!(
        (preview.descriptor().width, preview.descriptor().height),
        (256, 256)
    );
}

#[test]
fn screenshot_hdr_matches_capture_highlight_goldens_across_display_peaks() {
    // Independent expected bytes retained from Snow Capture's 87fb21f8^ scalar oracle.
    let colors: [[f32; 3]; 14] = [
        [1.25, 1.25, 1.25],
        [2.0, 2.0, 2.0],
        [4.0, 4.0, 4.0],
        [8.0, 8.0, 8.0],
        [1.25, 1.0625, 0.875],
        [2.0, 1.5, 1.0],
        [4.0, 3.0, 2.0],
        [8.0, 6.0, 4.0],
        [0.875, 1.25, 1.0625],
        [1.0625, 0.875, 1.25],
        [4.0, 0.5, 0.125],
        [1.25, 0.0, 0.0],
        [0.0, 1.25, 0.5],
        [0.5, 0.0, 1.25],
    ];
    let goldens = [
        (
            400.0,
            [
                [255, 255, 255],
                [255, 255, 255],
                [255, 255, 255],
                [255, 255, 255],
                [255, 237, 218],
                [255, 225, 188],
                [255, 225, 188],
                [255, 225, 188],
                [218, 255, 237],
                [237, 218, 255],
                [255, 99, 49],
                [255, 0, 0],
                [0, 255, 170],
                [170, 0, 255],
            ],
        ),
        (
            1000.0,
            [
                [241, 241, 241],
                [255, 255, 255],
                [255, 255, 255],
                [255, 255, 255],
                [249, 231, 212],
                [255, 225, 188],
                [255, 225, 188],
                [255, 225, 188],
                [209, 245, 228],
                [237, 218, 255],
                [255, 99, 49],
                [255, 0, 0],
                [0, 255, 170],
                [170, 0, 255],
            ],
        ),
        (
            4000.0,
            [
                [220, 220, 220],
                [239, 239, 239],
                [255, 255, 255],
                [255, 255, 255],
                [227, 212, 194],
                [255, 225, 188],
                [255, 225, 188],
                [255, 225, 188],
                [191, 224, 209],
                [219, 201, 235],
                [255, 99, 49],
                [255, 0, 0],
                [0, 236, 156],
                [170, 0, 255],
            ],
        ),
    ];
    for white in [80.0, 160.0, 203.0, 280.0, 480.0] {
        for (peak, expected) in goldens {
            let budget = Budget::default();
            let pixels = colors
                .into_iter()
                .flat_map(|rgb| {
                    [
                        rgb[0] * white / 80.0,
                        rgb[1] * white / 80.0,
                        rgb[2] * white / 80.0,
                        0.5,
                    ]
                })
                .flat_map(|sample| half::f16::from_f32(sample).to_bits().to_ne_bytes())
                .collect();
            let format = PixelFormat {
                sample: SampleType::F16,
                significant_bits: 16,
                ..PixelFormat::RGBA8
            };
            let image = Image::from_vec(
                ImageDescriptor::packed(
                    14,
                    1,
                    format,
                    ColorEncoding::ScRgb {
                        reference_white_nits: white,
                    },
                )
                .unwrap(),
                pixels,
                Default::default(),
                &budget,
            )
            .unwrap();
            let result = transform(
                &image,
                &TransformOptions {
                    hdr_peak_nits: peak,
                    ..Default::default()
                },
                &budget,
                &Default::default(),
            )
            .unwrap();
            for (index, (pixel, golden)) in result.bytes().chunks_exact(4).zip(expected).enumerate()
            {
                for c in 0..3 {
                    assert!(
                        pixel[c].abs_diff(golden[c]) <= 1,
                        "white={white}, peak={peak}, index={index}, channel={c}, actual={}, expected={}",
                        pixel[c],
                        golden[c]
                    );
                }
                assert_eq!(pixel[3], 128);
            }
        }
    }
}

#[test]
fn screenshot_hdr_restores_boosted_sdr_palette() {
    let decode = |byte: u8| {
        let value = f32::from(byte) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let mut palette: Vec<[u8; 3]> = (0..=255).map(|v| [v; 3]).collect();
    let levels = [0, 1, 8, 16, 32, 64, 96, 128, 160, 192, 224, 254, 255];
    for r in levels {
        for g in levels {
            for b in levels {
                palette.push([r, g, b]);
            }
        }
    }
    for white in [80.0, 160.0, 203.0, 280.0, 480.0] {
        let budget = Budget::default();
        let pixels = palette
            .iter()
            .flat_map(|rgb| {
                [
                    decode(rgb[0]) * white / 80.0,
                    decode(rgb[1]) * white / 80.0,
                    decode(rgb[2]) * white / 80.0,
                    0.5,
                ]
            })
            .flat_map(|sample| half::f16::from_f32(sample).to_bits().to_ne_bytes())
            .collect();
        let format = PixelFormat {
            sample: SampleType::F16,
            significant_bits: 16,
            ..PixelFormat::RGBA8
        };
        let image = Image::from_vec(
            ImageDescriptor::packed(
                palette.len() as u32,
                1,
                format,
                ColorEncoding::ScRgb {
                    reference_white_nits: white,
                },
            )
            .unwrap(),
            pixels,
            Default::default(),
            &budget,
        )
        .unwrap();
        for peak in [400.0, 1000.0, 4000.0] {
            let result = transform(
                &image,
                &TransformOptions {
                    hdr_peak_nits: peak,
                    ..Default::default()
                },
                &budget,
                &Default::default(),
            )
            .unwrap();
            for (pixel, expected) in result.bytes().chunks_exact(4).zip(&palette) {
                for c in 0..3 {
                    assert!(
                        pixel[c].abs_diff(expected[c]) <= 1,
                        "white={white}, peak={peak}, got={pixel:?}, expected={expected:?}"
                    );
                }
                assert_eq!(pixel[3], 128);
            }
        }
    }
}

#[cfg(feature = "native-backends")]
#[test]
fn preserve_dynamic_range_resize_keeps_scrgb_absolute_light() {
    let budget = Budget::default();
    let format = PixelFormat {
        sample: SampleType::F32,
        significant_bits: 32,
        ..PixelFormat::RGBA8
    };
    let pixels = [2.0f32, 1.0, 0.5, 1.0]
        .into_iter()
        .cycle()
        .take(16 * 16 * 4)
        .flat_map(f32::to_ne_bytes)
        .collect();
    let image = Image::from_vec(
        ImageDescriptor::packed(
            16,
            16,
            format,
            ColorEncoding::ScRgb {
                reference_white_nits: 160.0,
            },
        )
        .unwrap(),
        pixels,
        Default::default(),
        &budget,
    )
    .unwrap();
    let result = transform(
        &image,
        &TransformOptions {
            max_dimensions: Some((4, 4)),
            dynamic_range: DynamicRangePolicy::PreserveDynamicRange,
            ..Default::default()
        },
        &budget,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        result.descriptor().color,
        ColorEncoding::ScRgb {
            reference_white_nits: 160.0
        }
    );
    for (pixel, expected) in result
        .bytes()
        .chunks_exact(4)
        .zip([2.0f32, 1.0, 0.5, 1.0].into_iter().cycle())
    {
        assert!((f32::from_ne_bytes(pixel.try_into().unwrap()) - expected).abs() < 0.0001);
    }
}

#[cfg(feature = "native-backends")]
#[test]
fn encoders_reject_implicit_precision_reduction() {
    let budget = Budget::default();
    let format = PixelFormat {
        sample: SampleType::F32,
        significant_bits: 32,
        ..PixelFormat::RGBA8
    };
    let image = Image::from_vec(
        ImageDescriptor::packed(1, 1, format, ColorEncoding::LinearSrgb).unwrap(),
        [0.25f32, 0.5, 0.75, 1.0]
            .into_iter()
            .flat_map(f32::to_ne_bytes)
            .collect(),
        Default::default(),
        &budget,
    )
    .unwrap();
    for options in [
        EncodeOptions::Png(Default::default()),
        EncodeOptions::Bmp(Default::default()),
        EncodeOptions::Jpeg(Default::default()),
        EncodeOptions::WebP(Default::default()),
        EncodeOptions::Avif(Default::default()),
        EncodeOptions::Jxl(Default::default()),
    ] {
        assert!(matches!(
            encode(
                &image,
                &options,
                &mut Vec::new(),
                &budget,
                &Default::default()
            ),
            Err(Error::Unsupported(_))
        ));
    }
}

#[test]
fn encoders_reject_custom_hlg_reference_white_before_consuming_rows() {
    struct UnreadSource {
        descriptor: ImageDescriptor,
        access: RasterAccess,
    }
    impl RasterSource for UnreadSource {
        fn descriptor(&self) -> &ImageDescriptor {
            &self.descriptor
        }
        fn access(&self) -> RasterAccess {
            self.access
        }
        fn read_rows(&self, _: u32, _: u32, _: &mut [u8]) -> Result<()> {
            panic!("unsupported HLG reference white must be rejected before pixel reads")
        }
    }
    let budget = Budget::default();
    for white in [80.0, 160.0, f32::from_bits(203.0f32.to_bits() + 1), 400.0] {
        for access in [RasterAccess::SequentialRows, RasterAccess::RandomRows] {
            let source = UnreadSource {
                descriptor: ImageDescriptor::packed(
                    2,
                    1,
                    PixelFormat {
                        sample: SampleType::U16,
                        significant_bits: 12,
                        ..PixelFormat::RGBA8
                    },
                    ColorEncoding::Bt2020Hlg {
                        reference_white_nits: white,
                    },
                )
                .unwrap(),
                access,
            };
            for options in [
                EncodeOptions::Png(Default::default()),
                EncodeOptions::Jxl(Default::default()),
                EncodeOptions::Avif(AvifOptions {
                    bit_depth: 12,
                    ..Default::default()
                }),
            ] {
                let mut output = Vec::new();
                let result = encode(&source, &options, &mut output, &budget, &Default::default());
                assert!(matches!(result, Err(Error::Unsupported(_))), "{result:?}");
                assert!(output.is_empty());
                assert_eq!(budget.total_used(), 0);
                assert_eq!(budget.used(ResourceKind::TempDisk), 0);
            }
        }
    }
}

#[cfg(feature = "native-backends")]
#[test]
fn default_hlg_reference_white_roundtrips_through_jxl_and_avif() {
    let budget = Budget::default();
    let image = Image::from_vec(
        ImageDescriptor::packed(
            2,
            1,
            PixelFormat {
                sample: SampleType::U16,
                significant_bits: 12,
                ..PixelFormat::RGBA8
            },
            ColorEncoding::Bt2020Hlg {
                reference_white_nits: 203.0,
            },
        )
        .unwrap(),
        [1u32, 1024, 2048, 0, 3072, 4094, 4095, 4095]
            .into_iter()
            .flat_map(|value| (((value * 65535 + 2047) / 4095) as u16).to_ne_bytes())
            .collect(),
        Default::default(),
        &budget,
    )
    .unwrap();
    for options in [
        EncodeOptions::Jxl(JxlOptions {
            distance: 0.0,
            effort: 1,
            exact: true,
        }),
        EncodeOptions::Avif(AvifOptions {
            bit_depth: 12,
            quality: 100,
            alpha_quality: 100,
            speed: 10,
            exact: true,
        }),
    ] {
        let mut output = Vec::new();
        let receipt = encode(&image, &options, &mut output, &budget, &Default::default()).unwrap();
        assert_eq!(receipt.fidelity, PixelFidelity::Exact);
        let info = inspect(&output, &Default::default(), &budget, &Default::default()).unwrap();
        assert_eq!(info.color, image.descriptor().color);
        let decoded = decode(&output, &Default::default(), &budget, &Default::default()).unwrap();
        assert_eq!(decoded.descriptor().color, image.descriptor().color);
        assert_eq!(decoded.bytes(), image.bytes());
    }
    // PNG HLG export is not implemented, including the default convention.
    let mut output = Vec::new();
    assert!(matches!(
        encode(
            &image,
            &EncodeOptions::Png(Default::default()),
            &mut output,
            &budget,
            &Default::default()
        ),
        Err(Error::Unsupported(_))
    ));
    assert!(output.is_empty());
    drop(image);
    assert_eq!(budget.total_used(), 0);
}

#[test]
fn encoded_output_limit_returns_typed_resource_error() {
    let budget = Budget::new(ResourcePolicy {
        max_artifact_bytes: 16,
        ..Default::default()
    });
    let image = rgba(&budget);
    let error = encode(
        &image,
        &Default::default(),
        &mut Vec::new(),
        &budget,
        &Default::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        Error::ResourceLimit {
            resource: ResourceKind::EncodedOutput,
            ..
        }
    ));
    assert_eq!(budget.used(ResourceKind::Scratch), 0);
}

#[test]
fn gif_finite_repeat_count_is_normalized_to_total_playbacks() {
    let mut bytes = vec![];
    {
        let mut encoder = gif::Encoder::new(&mut bytes, 1, 1, &[0, 0, 0, 255, 255, 255]).unwrap();
        encoder.set_repeat(gif::Repeat::Finite(2)).unwrap();
        encoder
            .write_frame(&gif::Frame {
                width: 1,
                height: 1,
                buffer: Cow::Borrowed(&[1]),
                ..Default::default()
            })
            .unwrap();
    }
    let budget = Budget::default();
    let session =
        AnimationSession::open(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(session.info().loop_count, Some(3));
}

#[cfg(feature = "native-backends")]
#[test]
fn scrgb_jxl_retains_explicit_hdr_white_metadata() {
    let budget = Budget::default();
    let format = PixelFormat {
        sample: SampleType::F32,
        significant_bits: 32,
        ..PixelFormat::RGBA8
    };
    let image = Image::from_vec(
        ImageDescriptor::packed(
            2,
            1,
            format,
            ColorEncoding::ScRgb {
                reference_white_nits: 160.0,
            },
        )
        .unwrap(),
        [2.0f32, 1.0, 0.5, 1.0, 4.0, 2.0, 1.0, 1.0]
            .into_iter()
            .flat_map(f32::to_ne_bytes)
            .collect(),
        Default::default(),
        &budget,
    )
    .unwrap();
    let bytes = encoded(
        &image,
        EncodeOptions::Jxl(JxlOptions {
            distance: 0.01,
            effort: 7,
            exact: false,
        }),
        &budget,
    );
    let decoded = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(
        decoded.descriptor().color,
        ColorEncoding::ScRgb {
            reference_white_nits: 160.0
        }
    );
    assert_eq!(decoded.descriptor().format.sample, SampleType::F32);
    let first = f32::from_ne_bytes(decoded.bytes()[..4].try_into().unwrap());
    assert!(
        (first - 2.0).abs() < 0.05,
        "decoded first scRGB sample: {first}"
    );
    let info = inspect(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(info.color, decoded.descriptor().color);
}

#[cfg(feature = "native-backends")]
#[test]
fn wide_resize_accepts_zimg_horizontal_tiles() {
    struct Source(ImageDescriptor);
    impl RasterSource for Source {
        fn descriptor(&self) -> &ImageDescriptor {
            &self.0
        }
        fn access(&self) -> RasterAccess {
            RasterAccess::RandomRows
        }
        fn read_rows(&self, _: u32, _: u32, output: &mut [u8]) -> Result<()> {
            for pixel in output.chunks_exact_mut(4) {
                pixel.copy_from_slice(&[150, 100, 50, 255]);
            }
            Ok(())
        }
    }
    let budget = Budget::default();
    let source = Source(
        ImageDescriptor::packed(3840, 2160, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap(),
    );
    let resized = transform_source(
        &source,
        &TransformOptions {
            max_dimensions: Some((2048, 2048)),
            ..Default::default()
        },
        &budget,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        (resized.descriptor().width, resized.descriptor().height),
        (2048, 1152)
    );
    let mut row = vec![0; 2048 * 4];
    for y in [0, 1, 575, 1150, 1151] {
        resized.read_rows(y, 1, &mut row).unwrap();
        for pixel in row.chunks_exact(4) {
            assert_eq!(pixel, [150, 100, 50, 255], "row={y}");
        }
    }
    drop(resized);
    assert_eq!(budget.total_used(), 0);
    assert_eq!(budget.used(ResourceKind::TempDisk), 0);
}

#[test]
fn explicit_pq_export_quantizes_absolute_luminance_and_matches_streaming() {
    let budget = Budget::default();
    let samples = [0.0f32, 80.0, 203.0, 1000.0, 4000.0, 10000.0];
    let image = Image::from_vec(
        ImageDescriptor::packed(
            samples.len() as u32,
            1,
            PixelFormat {
                sample: SampleType::F32,
                significant_bits: 32,
                ..PixelFormat::RGBA8
            },
            ColorEncoding::ScRgb {
                reference_white_nits: 203.0,
            },
        )
        .unwrap(),
        samples
            .iter()
            .flat_map(|nits| [*nits / 80.0, *nits / 80.0, *nits / 80.0, 1.0])
            .flat_map(f32::to_ne_bytes)
            .collect(),
        Default::default(),
        &budget,
    )
    .unwrap();
    for target in [HdrOutput::Bt2020Pq10, HdrOutput::Bt2020Pq12] {
        let options = TransformOptions {
            dynamic_range: DynamicRangePolicy::PreserveDynamicRange,
            hdr_output: Some(target),
            ..Default::default()
        };
        let output = transform(&image, &options, &budget, &Default::default()).unwrap();
        assert_eq!(output.descriptor().color, ColorEncoding::Bt2020Pq);
        assert_eq!(output.descriptor().format.sample, SampleType::U16);
        assert_eq!(
            output.descriptor().format.significant_bits,
            target.bit_depth()
        );
        let streamed = transform_source(&image, &options, &budget, &Default::default()).unwrap();
        let mut bytes = vec![0; output.bytes().len()];
        streamed.read_rows(0, 1, &mut bytes).unwrap();
        assert_eq!(bytes, output.bytes());
        for (pixel, nits) in output.bytes().chunks_exact(8).zip(samples) {
            let m1 = 2610.0 / 16384.0;
            let m2 = 2523.0 / 32.0;
            let power = (f64::from(nits) / 10000.0).powf(m1);
            let pq = ((3424.0 / 4096.0 + (2413.0 / 128.0) * power)
                / (1.0 + (2392.0 / 128.0) * power))
                .powf(m2);
            let maximum = (1u32 << target.bit_depth()) - 1;
            let expected = (pq * f64::from(maximum)).round() as u32;
            for sample in pixel[..6].chunks_exact(2) {
                let normalized = u16::from_ne_bytes(sample.try_into().unwrap());
                let actual = (u32::from(normalized) * maximum + 32767) / 65535;
                assert_eq!(actual, expected, "{nits} nits, {} bits", target.bit_depth());
            }
            assert_eq!(&pixel[6..], &u16::MAX.to_ne_bytes());
        }
    }
}

#[test]
fn hdr_export_requires_explicit_preservation_and_rejects_out_of_range_light() {
    let budget = Budget::default();
    let invalid = TransformOptions {
        hdr_output: Some(HdrOutput::Bt2020Pq10),
        ..Default::default()
    };
    assert!(invalid.validate().is_err());
    for rgb in [[126.0f32; 3], [-1.0, 0.0, 0.0]] {
        let image = Image::from_vec(
            ImageDescriptor::packed(
                1,
                1,
                PixelFormat {
                    sample: SampleType::F32,
                    significant_bits: 32,
                    ..PixelFormat::RGBA8
                },
                ColorEncoding::ScRgb {
                    reference_white_nits: 80.0,
                },
            )
            .unwrap(),
            rgb.into_iter()
                .chain([1.0])
                .flat_map(f32::to_ne_bytes)
                .collect(),
            Default::default(),
            &budget,
        )
        .unwrap();
        let result = transform(
            &image,
            &TransformOptions {
                dynamic_range: DynamicRangePolicy::PreserveDynamicRange,
                hdr_output: Some(HdrOutput::Bt2020Pq10),
                ..Default::default()
            },
            &budget,
            &Default::default(),
        );
        assert!(matches!(result, Err(Error::Unsupported(_))));
    }
}

#[cfg(feature = "native-backends")]
#[test]
fn scrgb_prepared_pq_encodes_to_avif_with_explicit_ten_or_twelve_bit_precision() {
    let budget = Budget::default();
    let image = Image::from_vec(
        ImageDescriptor::packed(
            16,
            16,
            PixelFormat {
                sample: SampleType::F32,
                significant_bits: 32,
                ..PixelFormat::RGBA8
            },
            ColorEncoding::ScRgb {
                reference_white_nits: 203.0,
            },
        )
        .unwrap(),
        (0..256)
            .flat_map(|_| [2.5375f32, 2.5375, 2.5375, 1.0])
            .flat_map(f32::to_ne_bytes)
            .collect(),
        Default::default(),
        &budget,
    )
    .unwrap();
    for target in [HdrOutput::Bt2020Pq10, HdrOutput::Bt2020Pq12] {
        let prepared = transform(
            &image,
            &TransformOptions {
                dynamic_range: DynamicRangePolicy::PreserveDynamicRange,
                hdr_output: Some(target),
                ..Default::default()
            },
            &budget,
            &Default::default(),
        )
        .unwrap();
        let bytes = encoded(
            &prepared,
            EncodeOptions::Avif(AvifOptions {
                quality: 100,
                alpha_quality: 100,
                speed: 10,
                bit_depth: target.bit_depth(),
                ..Default::default()
            }),
            &budget,
        );
        let decoded = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
        assert_eq!(decoded.descriptor().color, ColorEncoding::Bt2020Pq);
        assert_eq!(
            decoded.descriptor().format.significant_bits,
            target.bit_depth()
        );
        for (a, b) in decoded
            .bytes()
            .chunks_exact(2)
            .zip(prepared.bytes().chunks_exact(2))
        {
            assert!(
                (i32::from(u16::from_ne_bytes(a.try_into().unwrap()))
                    - i32::from(u16::from_ne_bytes(b.try_into().unwrap())))
                .abs()
                    <= 1
            );
        }
    }
}

#[cfg(feature = "native-backends")]
#[test]
fn jpeg_direct_capture_rows_match_channel_orders_and_reject_transparency() {
    let budget = Budget::default();
    let descriptor =
        ImageDescriptor::packed(32, 16, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
    let pixels: Vec<u8> = (0..512)
        .flat_map(|i| [(i % 251) as u8, (i % 173) as u8, (i % 97) as u8, 255])
        .collect();
    let rgba = Image::from_vec(
        descriptor.clone(),
        pixels.clone(),
        Default::default(),
        &budget,
    )
    .unwrap();
    let reference = encoded(&rgba, EncodeOptions::Jpeg(Default::default()), &budget);
    let mut bgra = pixels.clone();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let mut d = descriptor.clone();
    d.format.channels = ChannelOrder::Bgra;
    let image = Image::from_vec(d, bgra, Default::default(), &budget).unwrap();
    assert_eq!(
        encoded(&image, EncodeOptions::Jpeg(Default::default()), &budget),
        reference
    );
    let mut transparent = pixels;
    transparent[3] = 254;
    let image = Image::from_vec(descriptor, transparent, Default::default(), &budget).unwrap();
    assert!(matches!(
        encode(
            &image,
            &EncodeOptions::Jpeg(Default::default()),
            &mut Vec::new(),
            &budget,
            &Default::default()
        ),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn png_retains_significant_precision_after_storage_expansion() {
    let budget = Budget::default();
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 4, 1);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Two);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0b00011011]).unwrap();
    }
    let decoded = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(decoded.bytes(), [0, 85, 170, 255]);
    assert_eq!(decoded.descriptor().format.significant_bits, 2);
    let exported = encoded(&decoded, EncodeOptions::default(), &budget);
    let again = decode(&exported, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(again.bytes(), decoded.bytes());
    assert_eq!(again.descriptor().format.significant_bits, 2);
    assert_eq!(
        inspect(&exported, &Default::default(), &budget, &Default::default())
            .unwrap()
            .bit_depth,
        2
    );
    let descriptor = ImageDescriptor::packed(
        1,
        1,
        PixelFormat {
            sample: SampleType::U16,
            significant_bits: 10,
            ..PixelFormat::RGBA8
        },
        ColorEncoding::Srgb,
    )
    .unwrap();
    let image = Image::from_vec(
        descriptor,
        [0u16, 65535, 0, 65535]
            .into_iter()
            .flat_map(u16::to_ne_bytes)
            .collect(),
        Default::default(),
        &budget,
    )
    .unwrap();
    let mut bytes = Vec::new();
    let receipt = encode(
        &image,
        &EncodeOptions::default(),
        &mut bytes,
        &budget,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(receipt.bit_depth, 16);
    assert_eq!(
        decode(&bytes, &Default::default(), &budget, &Default::default())
            .unwrap()
            .descriptor()
            .format
            .significant_bits,
        10
    );
}

#[test]
fn rotated_apng_reports_published_dimensions_and_late_metadata() {
    // Little-endian TIFF orientation IFD: rotate 90 degrees clockwise.
    let exif = [
        b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
    ];
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(2, 1).unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255])
            .unwrap();
        writer
            .write_image_data(&[0, 0, 255, 255, 255, 255, 255, 255])
            .unwrap();
        writer
            .write_chunk(png::chunk::ChunkType(*b"eXIf"), &exif)
            .unwrap();
        writer
            .write_text_chunk(&png::text_metadata::ITXtChunk::new(
                "XML:com.adobe.xmp",
                "<tiff:Orientation>6</tiff:Orientation>",
            ))
            .unwrap();
        writer.finish().unwrap();
    }
    let budget = Budget::default();
    for apply in [false, true] {
        let mut session = AnimationSession::open(
            &bytes,
            &DecodeOptions {
                apply_orientation: apply,
                ..Default::default()
            },
            &budget,
            &Default::default(),
        )
        .unwrap();
        let dimensions = if apply { (1, 2) } else { (2, 1) };
        assert_eq!((session.info().width, session.info().height), dimensions);
        let frame = session.next_frame(&Default::default()).unwrap().unwrap();
        assert_eq!(
            (
                frame.image.descriptor().width,
                frame.image.descriptor().height
            ),
            dimensions
        );
        assert_eq!(
            frame.image.metadata().exif.as_deref(),
            if apply { None } else { Some(exif.as_slice()) }
        );
        assert_eq!(frame.image.metadata().xmp.is_none(), apply);
        let second = session.next_frame(&Default::default()).unwrap().unwrap();
        assert!(std::ptr::eq(
            frame.image.metadata(),
            second.image.metadata()
        ));
        assert_eq!(
            frame.image.descriptor().orientation,
            if apply {
                Orientation::Normal
            } else {
                Orientation::Rotate90
            }
        );
        assert_eq!(
            session.frame(0, &Default::default()).unwrap().image.bytes(),
            frame.image.bytes()
        );
    }
    assert_eq!(budget.total_used(), 0);
}

#[cfg(feature = "native-backends")]
#[test]
fn sdr_row_resize_matches_float_reference_across_alpha_and_channel_orders() {
    let budget = Budget::default();
    for channels in [
        ChannelOrder::Gray,
        ChannelOrder::GrayAlpha,
        ChannelOrder::Rgb,
        ChannelOrder::Bgr,
        ChannelOrder::Rgba,
        ChannelOrder::Bgra,
    ] {
        for alpha in [AlphaMode::Straight, AlphaMode::Opaque] {
            let descriptor = ImageDescriptor::packed(
                2049,
                41,
                PixelFormat {
                    channels,
                    alpha: if channels.has_alpha() {
                        alpha
                    } else {
                        AlphaMode::Opaque
                    },
                    ..PixelFormat::RGBA8
                },
                ColorEncoding::Srgb,
            )
            .unwrap();
            let pixels = (0..descriptor.required_bytes().unwrap())
                .map(|index| ((index * 17 + index / 29) % 256) as u8)
                .collect();
            let source = Image::from_vec(descriptor, pixels, Default::default(), &budget).unwrap();
            let options = TransformOptions {
                max_dimensions: Some((1025, 21)),
                ..Default::default()
            };
            let actual = transform(&source, &options, &budget, &Default::default()).unwrap();
            let reference_linear = transform(
                &source,
                &TransformOptions {
                    dynamic_range: DynamicRangePolicy::PreserveDynamicRange,
                    ..options
                },
                &budget,
                &Default::default(),
            )
            .unwrap();
            let reference = transform(
                &reference_linear,
                &Default::default(),
                &budget,
                &Default::default(),
            )
            .unwrap();
            assert_eq!(actual.bytes(), reference.bytes(), "{channels:?} {alpha:?}");
        }
    }
}

#[cfg(feature = "native-backends")]
#[test]
fn sdr_row_resize_uses_bounded_rings_without_float_rasters_or_temporary_disk() {
    let budget = Budget::new(ResourcePolicy {
        total_memory_bytes: 32 * 1024 * 1024,
        working_bytes_per_job: 32 * 1024 * 1024,
        ..Default::default()
    });
    let source = MutableImage::new(
        ImageDescriptor::packed(
            2400,
            1350,
            PixelFormat {
                alpha: AlphaMode::Opaque,
                ..PixelFormat::RGBA8
            },
            ColorEncoding::Srgb,
        )
        .unwrap(),
        Default::default(),
        &budget,
    )
    .unwrap()
    .seal();
    let resized = transform(
        &source,
        &TransformOptions {
            max_dimensions: Some((2048, 2048)),
            ..Default::default()
        },
        &budget,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        (resized.descriptor().width, resized.descriptor().height),
        (2048, 1152)
    );
    assert_eq!(budget.peak_used(ResourceKind::TempDisk), 0);
    assert!(budget.peak_used(ResourceKind::Scratch) < 4 * 1024 * 1024);
    drop(resized);
    drop(source);
    assert_eq!(budget.total_used(), 0);
}

#[cfg(feature = "native-backends")]
#[test]
fn direct_jpeg_plan_preserves_decode_semantics_and_rejects_required_transforms() {
    let budget = Budget::default();
    let source = Image::from_vec(
        ImageDescriptor::packed(
            3,
            2,
            PixelFormat {
                alpha: AlphaMode::Opaque,
                ..PixelFormat::RGBA8
            },
            ColorEncoding::Srgb,
        )
        .unwrap(),
        vec![73; 24],
        Default::default(),
        &budget,
    )
    .unwrap();
    let plain = encoded(&source, EncodeOptions::Jpeg(Default::default()), &budget);
    for orientation in [1u8, 6] {
        let tiff = [
            b'I',
            b'I',
            42,
            0,
            8,
            0,
            0,
            0,
            1,
            0,
            0x12,
            1,
            3,
            0,
            1,
            0,
            0,
            0,
            orientation,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
        bytes.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        bytes.extend_from_slice(b"Exif\0\0");
        bytes.extend_from_slice(&tiff);
        bytes.extend_from_slice(&plain[2..]);
        let normalized = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
        assert_eq!(normalized.descriptor().orientation, Orientation::Normal);
        assert_eq!(normalized.metadata().exif.is_none(), orientation != 1);
        let exported = encoded(&normalized, EncodeOptions::default(), &budget);
        let roundtrip =
            decode(&exported, &Default::default(), &budget, &Default::default()).unwrap();
        assert_eq!(roundtrip.descriptor().width, normalized.descriptor().width);
        assert_eq!(
            roundtrip.descriptor().height,
            normalized.descriptor().height
        );
        assert_eq!(roundtrip.bytes(), normalized.bytes());
        assert!(roundtrip.metadata().exif.is_none());
        for preserve_metadata in [false, true] {
            let options = DecodeOptions {
                apply_orientation: false,
                preserve_metadata,
                preferred_frame: None,
            };
            let plan =
                DirectJpegDecode::prepare(&bytes, &options, None, &budget, &Default::default())
                    .unwrap()
                    .unwrap();
            let expected = decode(&bytes, &options, &budget, &Default::default()).unwrap();
            let mut pixels = vec![0; plan.descriptor().required_bytes().unwrap()];
            let (descriptor, metadata) = plan
                .decode_into(&mut pixels, &budget, &Default::default())
                .unwrap();
            assert_eq!(&descriptor, expected.descriptor());
            assert_eq!(metadata.get(), expected.metadata());
            assert_eq!(pixels, expected.bytes());
        }
        let plan = DirectJpegDecode::prepare(
            &bytes,
            &Default::default(),
            Some(2048),
            &budget,
            &Default::default(),
        )
        .unwrap();
        if orientation == 1 {
            let mut pixels = vec![0; 24];
            let (_, metadata) = plan
                .unwrap()
                .decode_into(&mut pixels, &budget, &Default::default())
                .unwrap();
            assert_eq!(metadata.get(), &Metadata::default());
            assert_eq!(
                pixels,
                preview(&bytes, (2048, 2048), &budget, &Default::default())
                    .unwrap()
                    .bytes()
            );
        } else {
            assert!(plan.is_none());
            assert!(
                DirectJpegDecode::prepare(
                    &bytes,
                    &Default::default(),
                    None,
                    &budget,
                    &Default::default()
                )
                .unwrap()
                .is_none()
            );
        }
    }
    assert!(
        DirectJpegDecode::prepare(
            &plain,
            &Default::default(),
            Some(2),
            &budget,
            &Default::default()
        )
        .unwrap()
        .is_none()
    );
    assert!(
        DirectJpegDecode::prepare(
            &plain,
            &DecodeOptions {
                preferred_frame: Some(1),
                ..Default::default()
            },
            None,
            &budget,
            &Default::default()
        )
        .unwrap()
        .is_none()
    );
    let png = encoded(&source, EncodeOptions::default(), &budget);
    assert!(
        DirectJpegDecode::prepare(
            &png,
            &Default::default(),
            None,
            &budget,
            &Default::default()
        )
        .unwrap()
        .is_none()
    );
    let profile = include_bytes!("fixtures/linear_srgb.icc");
    let mut profiled = vec![0xff, 0xd8, 0xff, 0xe2];
    profiled.extend_from_slice(&((profile.len() + 16) as u16).to_be_bytes());
    profiled.extend_from_slice(b"ICC_PROFILE\0\x01\x01");
    profiled.extend_from_slice(profile);
    profiled.extend_from_slice(&plain[2..]);
    assert!(
        DirectJpegDecode::prepare(
            &profiled,
            &Default::default(),
            Some(2048),
            &budget,
            &Default::default()
        )
        .unwrap()
        .is_none()
    );
    let plan = DirectJpegDecode::prepare(
        &profiled,
        &Default::default(),
        None,
        &budget,
        &Default::default(),
    )
    .unwrap()
    .unwrap();
    let mut pixels = vec![0; 24];
    let (descriptor, metadata) = plan
        .decode_into(&mut pixels, &budget, &Default::default())
        .unwrap();
    let expected = decode(&profiled, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(&descriptor, expected.descriptor());
    assert_eq!(metadata.get(), expected.metadata());
    assert_eq!(pixels, expected.bytes());
}

#[cfg(feature = "native-backends")]
#[test]
fn avif_exact_preserves_integer_samples_and_transparent_rgb() {
    let budget = Budget::default();
    for bits in [8, 10, 12] {
        let maximum = (1u32 << bits) - 1;
        let samples = [1, maximum / 3, maximum - 1, 0, maximum / 2, 17, 3, maximum];
        let (sample, pixels) = if bits == 8 {
            (
                SampleType::U8,
                samples.into_iter().map(|v| v as u8).collect(),
            )
        } else {
            (
                SampleType::U16,
                samples
                    .into_iter()
                    .flat_map(|v| (((v * 65535 + maximum / 2) / maximum) as u16).to_ne_bytes())
                    .collect(),
            )
        };
        let descriptor = ImageDescriptor::packed(
            2,
            1,
            PixelFormat {
                sample,
                significant_bits: bits,
                ..PixelFormat::RGBA8
            },
            ColorEncoding::Srgb,
        )
        .unwrap();
        let image = Image::from_vec(descriptor, pixels, Default::default(), &budget).unwrap();
        let options = AvifOptions {
            exact: true,
            quality: 100,
            alpha_quality: 100,
            speed: 10,
            bit_depth: bits,
        };
        let mut bytes = Vec::new();
        let receipt = encode(
            &image,
            &EncodeOptions::Avif(options.clone()),
            &mut bytes,
            &budget,
            &Default::default(),
        )
        .unwrap();
        assert_eq!(receipt.fidelity, PixelFidelity::Exact);
        assert_eq!(receipt.bit_depth, bits);
        let decoded = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
        assert_eq!(decoded.bytes(), image.bytes());
        for invalid in [
            AvifOptions {
                quality: 99,
                ..options.clone()
            },
            AvifOptions {
                alpha_quality: 99,
                ..options.clone()
            },
        ] {
            assert!(EncodeOptions::Avif(invalid).validate().is_err());
        }
        let incompatible = AvifOptions {
            bit_depth: if bits == 12 { 10 } else { 12 },
            ..options
        };
        assert!(matches!(
            encode(
                &image,
                &EncodeOptions::Avif(incompatible),
                &mut Vec::new(),
                &budget,
                &Default::default()
            ),
            Err(Error::Unsupported(_))
        ));
    }
}

#[test]
fn png_does_not_silently_discard_corrupt_icc() {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_chunk(
                png::chunk::ChunkType(*b"iCCP"),
                b"icc\0\0invalid zlib stream",
            )
            .unwrap();
        writer.write_image_data(&[1, 2, 3, 255]).unwrap();
    }
    let budget = Budget::default();
    assert!(matches!(
        inspect(&bytes, &Default::default(), &budget, &Default::default()),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        decode(&bytes, &Default::default(), &budget, &Default::default()),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(budget.total_used(), 0);
}

#[test]
fn png_preserves_late_compressed_xmp_and_strips_it_from_screenshot_export() {
    let xmp = "<x:xmpmeta>example metadata</x:xmpmeta>";
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[1, 2, 3, 255]).unwrap();
        let mut chunk = png::text_metadata::ITXtChunk::new("XML:com.adobe.xmp", xmp);
        chunk.compress_text().unwrap();
        chunk.compressed = true;
        writer.write_text_chunk(&chunk).unwrap();
    }
    let budget = Budget::default();
    let inspected = inspect(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(inspected.metadata.xmp.as_deref(), Some(xmp.as_bytes()));
    let decoded = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(decoded.metadata().xmp.as_deref(), Some(xmp.as_bytes()));
    let stripped = decode(
        &bytes,
        &DecodeOptions {
            preserve_metadata: false,
            ..Default::default()
        },
        &budget,
        &Default::default(),
    )
    .unwrap();
    assert!(stripped.metadata().xmp.is_none());
    let exported = encoded(&decoded, EncodeOptions::default(), &budget);
    assert!(
        inspect(&exported, &Default::default(), &budget, &Default::default())
            .unwrap()
            .metadata
            .xmp
            .is_none()
    );
}

#[test]
fn png_metadata_limit_bounds_compressed_and_plain_payloads() {
    for compressed in [false, true] {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let mut chunk =
                png::text_metadata::ITXtChunk::new("XML:com.adobe.xmp", "x".repeat(4096));
            if compressed {
                chunk.compress_text().unwrap();
                chunk.compressed = true;
            }
            writer.write_text_chunk(&chunk).unwrap();
            writer.write_image_data(&[1, 2, 3, 255]).unwrap();
        }
        let budget = Budget::new(ResourcePolicy {
            max_metadata_bytes: 1024,
            ..Default::default()
        });
        assert!(decode(&bytes, &Default::default(), &budget, &Default::default()).is_err());
        assert_eq!(budget.total_used(), 0);
    }
}

#[cfg(feature = "native-backends")]
#[test]
fn webp_lazy_animation_disposal_timing_reset_and_retained_snapshots() {
    fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
        output.extend_from_slice(kind);
        output.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        output.extend_from_slice(payload);
        if !payload.len().is_multiple_of(2) {
            output.push(0);
        }
    }
    fn u24(output: &mut Vec<u8>, value: u32) {
        output.extend_from_slice(&value.to_le_bytes()[..3]);
    }
    let budget = Budget::default();
    let mut container = b"WEBP".to_vec();
    chunk(&mut container, b"VP8X", &[0x12, 0, 0, 0, 3, 0, 0, 1, 0, 0]);
    chunk(&mut container, b"ANIM", &[0, 0, 0, 0, 2, 0]);
    for (width, x, rgba, milliseconds, flags) in [
        (4, 0, [255, 0, 0, 255], 10, 2),
        (2, 2, [0, 0, 255, 255], 20, 3),
        (2, 0, [0, 255, 0, 255], 30, 2),
    ] {
        let image = Image::from_vec(
            ImageDescriptor::packed(width, 2, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap(),
            (0..width * 2).flat_map(|_| rgba).collect(),
            Default::default(),
            &budget,
        )
        .unwrap();
        let frame = encoded(
            &image,
            EncodeOptions::WebP(WebPOptions {
                lossless: true,
                exact: true,
                ..Default::default()
            }),
            &budget,
        );
        let mut payload = Vec::new();
        for value in [x / 2, 0, width - 1, 1, milliseconds] {
            u24(&mut payload, value);
        }
        payload.push(flags);
        payload.extend_from_slice(&frame[12..]);
        chunk(&mut container, b"ANMF", &payload);
    }
    let mut input = b"RIFF".to_vec();
    input.extend_from_slice(&(container.len() as u32).to_le_bytes());
    input.extend_from_slice(&container);
    let mut session =
        AnimationSession::open(&input, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(session.info().frame_count, 3);
    assert_eq!(session.info().loop_count, Some(2));
    let first = session.next_frame(&Default::default()).unwrap().unwrap();
    assert_eq!(first.duration.as_millis(), 10);
    assert!(
        first
            .image
            .bytes()
            .chunks_exact(4)
            .all(|pixel| pixel == [255, 0, 0, 255])
    );
    let second = session.next_frame(&Default::default()).unwrap().unwrap();
    assert_eq!(second.duration.as_millis(), 20);
    assert_eq!(
        &second.image.bytes()[..16],
        &[
            255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255
        ]
    );
    let third = session.next_frame(&Default::default()).unwrap().unwrap();
    assert_eq!(third.duration.as_millis(), 30);
    assert_eq!(
        &third.image.bytes()[..16],
        &[0, 255, 0, 255, 0, 255, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    assert!(session.next_frame(&Default::default()).unwrap().is_none());
    session.reset(&Default::default()).unwrap();
    assert_eq!(
        session.frame(2, &Default::default()).unwrap().image.bytes(),
        third.image.bytes()
    );
    drop(session);
    assert!(
        first
            .image
            .bytes()
            .chunks_exact(4)
            .all(|pixel| pixel == [255, 0, 0, 255])
    );
    drop((first, second, third));
    assert_eq!(budget.total_used(), 0);
}

#[cfg(feature = "native-backends")]
#[test]
fn bmp_embedded_icc_is_preserved_and_transformed() {
    let budget = Budget::default();
    let source = Image::from_vec(
        ImageDescriptor::packed(1, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap(),
        vec![128, 128, 128, 255],
        Default::default(),
        &budget,
    )
    .unwrap();
    let mut bytes = encoded(&source, EncodeOptions::Bmp(BmpOptions::default()), &budget);
    let profile = include_bytes!("fixtures/linear_srgb.icc");
    let profile_offset = (bytes.len() - 14) as u32;
    bytes[70..74].copy_from_slice(&0x4d42_4544u32.to_le_bytes());
    bytes[126..130].copy_from_slice(&profile_offset.to_le_bytes());
    bytes[130..134].copy_from_slice(&(profile.len() as u32).to_le_bytes());
    bytes.extend_from_slice(profile);
    let length = bytes.len() as u32;
    bytes[2..6].copy_from_slice(&length.to_le_bytes());
    let image = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
    assert_eq!(image.descriptor().color, ColorEncoding::Icc);
    assert_eq!(image.metadata().icc.as_deref(), Some(profile.as_slice()));
    let converted = transform(&image, &Default::default(), &budget, &Default::default()).unwrap();
    assert!((187..=189).contains(&converted.bytes()[0]));
    let mut truncated = bytes.clone();
    truncated.pop();
    assert!(
        decode(
            &truncated,
            &Default::default(),
            &budget,
            &Default::default()
        )
        .is_err()
    );
    let mut linked = bytes;
    linked[70..74].copy_from_slice(&0x4c49_4e4bu32.to_le_bytes());
    assert!(matches!(
        decode(&linked, &Default::default(), &budget, &Default::default()),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn png_calibrated_color_metadata_survives_decode_and_reencode_exactly() {
    let budget = Budget::default();
    for (gamma, chromaticities) in [
        (Some(45455), true),
        (Some(4_294_912_345), false),
        (None, true),
    ] {
        let pair = |x, y| {
            (
                png::ScaledFloat::from_scaled(x),
                png::ScaledFloat::from_scaled(y),
            )
        };
        let chroma = png::SourceChromaticities {
            white: pair(31271, 32903),
            red: pair(64001, 32999),
            green: pair(30001, 59999),
            blue: pair(15001, 6001),
        };
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            if let Some(value) = gamma {
                encoder.set_source_gamma(png::ScaledFloat::from_scaled(value));
            }
            if chromaticities {
                encoder.set_source_chromaticities(chroma);
            }
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[128, 64, 32, 0])
                .unwrap();
        }
        let decoded = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
        assert!(matches!(
            decoded.descriptor().color,
            ColorEncoding::CalibratedRgb { .. }
        ));
        let encoded = encoded(&decoded, EncodeOptions::default(), &budget);
        let reader = png::Decoder::new(std::io::Cursor::new(encoded))
            .read_info()
            .unwrap();
        assert_eq!(
            reader.info().gama_chunk.map(png::ScaledFloat::into_scaled),
            gamma
        );
        assert_eq!(reader.info().chrm_chunk, chromaticities.then_some(chroma));
        if gamma.is_none() || !chromaticities {
            assert!(matches!(
                transform(&decoded, &Default::default(), &budget, &Default::default()),
                Err(Error::Unsupported(_))
            ));
        }
    }
}

#[test]
fn bmp_incomplete_calibrated_metadata_is_preserved_without_guessing_srgb() {
    let budget = Budget::default();
    let color = ColorEncoding::CalibratedBmp {
        endpoints: [[0.0; 3]; 3],
        gamma: [0.0, 1.0, 2.0],
    };
    let image = Image::from_vec(
        ImageDescriptor::packed(1, 1, PixelFormat::RGBA8, color.clone()).unwrap(),
        vec![128, 64, 32, 255],
        Default::default(),
        &budget,
    )
    .unwrap();
    for alpha in [false, true] {
        let bytes = encoded(
            &image,
            EncodeOptions::Bmp(BmpOptions {
                preserve_alpha: alpha,
            }),
            &budget,
        );
        let decoded = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
        assert_eq!(decoded.descriptor().color, color);
        assert_eq!(&decoded.bytes()[..3], &image.bytes()[..3]);
        assert!(matches!(
            transform(&decoded, &Default::default(), &budget, &Default::default()),
            Err(Error::Unsupported(_))
        ));
    }
}

#[cfg(feature = "native-backends")]
#[test]
fn complete_calibrated_sources_use_actual_icc_conversion() {
    let budget = Budget::default();
    let fixed = |value: f64| (value * 1_073_741_824.0).round() / 1_073_741_824.0;
    let colors = [
        ColorEncoding::CalibratedRgb {
            gamma: Some(1.0),
            chromaticities: Some(RgbChromaticities {
                white: [0.3127, 0.3290],
                red: [0.64, 0.33],
                green: [0.30, 0.60],
                blue: [0.15, 0.06],
            }),
        },
        ColorEncoding::CalibratedBmp {
            endpoints: [
                [fixed(0.4124564), fixed(0.2126729), fixed(0.0193339)],
                [fixed(0.3575761), fixed(0.7151522), fixed(0.1191920)],
                [fixed(0.1804375), fixed(0.0721750), fixed(0.9503041)],
            ],
            gamma: [1.0; 3],
        },
    ];
    for color in colors {
        let image = Image::from_vec(
            ImageDescriptor::packed(1, 1, PixelFormat::RGBA8, color).unwrap(),
            vec![128, 128, 128, 127],
            Default::default(),
            &budget,
        )
        .unwrap();
        let transformed =
            transform(&image, &Default::default(), &budget, &Default::default()).unwrap();
        assert_eq!(transformed.descriptor().color, ColorEncoding::Srgb);
        assert!((187..=189).contains(&transformed.bytes()[0]));
        assert_eq!(transformed.bytes()[3], 127);
        let streamed =
            transform_source(&image, &Default::default(), &budget, &Default::default()).unwrap();
        let mut bytes = [0; 4];
        streamed.read_rows(0, 1, &mut bytes).unwrap();
        assert_eq!(bytes, transformed.bytes());
    }
}

#[test]
fn opaque_capture_alpha_bytes_are_ignored_by_exporters() {
    let budget = Budget::default();
    let image = Image::from_vec(
        ImageDescriptor::packed(
            2,
            1,
            PixelFormat {
                alpha: AlphaMode::Opaque,
                ..PixelFormat::BGRA8
            },
            ColorEncoding::Srgb,
        )
        .unwrap(),
        vec![30, 20, 10, 0, 60, 50, 40, 17],
        Default::default(),
        &budget,
    )
    .unwrap();
    let mut formats = vec![
        EncodeOptions::Png(Default::default()),
        EncodeOptions::Bmp(Default::default()),
    ];
    if cfg!(feature = "native-backends") {
        formats.extend([
            EncodeOptions::WebP(WebPOptions {
                lossless: true,
                exact: true,
                ..Default::default()
            }),
            EncodeOptions::Jxl(Default::default()),
        ]);
    }
    for format in formats {
        let name = format.format();
        let bytes = encoded(&image, format, &budget);
        let decoded = decode(&bytes, &Default::default(), &budget, &Default::default()).unwrap();
        assert_eq!(
            decoded.bytes(),
            [10, 20, 30, 255, 40, 50, 60, 255],
            "{name:?}"
        );
    }
    #[cfg(feature = "native-backends")]
    {
        let reference = Image::from_vec(
            ImageDescriptor::packed(2, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap(),
            vec![10, 20, 30, 255, 40, 50, 60, 255],
            Default::default(),
            &budget,
        )
        .unwrap();
        assert_eq!(
            encoded(&image, EncodeOptions::Jpeg(Default::default()), &budget),
            encoded(&reference, EncodeOptions::Jpeg(Default::default()), &budget)
        );
    }
}
