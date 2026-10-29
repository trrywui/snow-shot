//! Direct codec timings that exclude worker transport, startup, and publication.
use snow_image_codecs::{decode, encode, preview, transform};
use snow_image_core::*;
use snow_image_native_sys as native;
use std::{hint::black_box, time::Instant};
fn measure<T>(name: &str, mut f: impl FnMut() -> T) {
    drop(f());
    let mut timings = Vec::new();
    for _ in 0..9 {
        let start = Instant::now();
        let value = f();
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
        black_box(&value);
        drop(value);
    }
    timings.sort_by(f64::total_cmp);
    println!("{name}: {:.3} ms", timings[4]);
}
fn main() {
    if cfg!(debug_assertions) {
        eprintln!("use --profile performance");
        std::process::exit(2);
    }
    let corpus = std::env::args().nth(1).expect("corpus directory");
    let legacy = std::env::args().nth(2);
    let budget = Budget::default();
    let cancel = CancellationToken::default();
    for (width, height, target_width, target_height) in [
        (1920, 1080, 1920, 1080),
        (3840, 2160, 2048, 1152),
        (7680, 4320, 2048, 1152),
    ] {
        let bytes = std::fs::read(format!("{corpus}/ui-{width}x{height}-jpeg.jpg")).unwrap();
        println!("{width}x{height}");
        measure("decode full", || {
            decode(&bytes, &Default::default(), &budget, &cancel).unwrap()
        });
        measure("preview", || {
            preview(&bytes, (2048, 2048), &budget, &cancel).unwrap()
        });
        if let Some(directory) = &legacy {
            let reference =
                std::fs::read(format!("{directory}/legacy-preview-{width}x{height}.rgba")).unwrap();
            let actual = preview(&bytes, (2048, 2048), &budget, &cancel).unwrap();
            assert_eq!(reference.len(), actual.bytes().len());
            let mut squared = 0.0;
            let mut maximum = 0u8;
            let mut different = 0usize;
            for (reference, actual) in reference.iter().zip(actual.bytes()) {
                let delta = reference.abs_diff(*actual);
                maximum = maximum.max(delta);
                different += usize::from(delta != 0);
                squared += f64::from(delta).powi(2);
            }
            let mse = squared / reference.len() as f64;
            println!(
                "legacy pixels: PSNR {:.3} dB, max delta {maximum}, differing bytes {different}/{}",
                10.0 * (255.0 * 255.0 / mse).log10(),
                reference.len()
            );
        }
        let options = native::DecodeOptions {
            target_width,
            target_height,
            ..Default::default()
        };
        let mut decoded = native::decode(native::NativeFormat::Jpeg, &bytes, options).unwrap();
        let image = Image::from_vec(
            ImageDescriptor::packed(
                decoded.width,
                decoded.height,
                PixelFormat {
                    alpha: AlphaMode::Opaque,
                    ..PixelFormat::RGBA8
                },
                ColorEncoding::Srgb,
            )
            .unwrap(),
            std::mem::take(&mut decoded.pixels),
            Default::default(),
            &budget,
        )
        .unwrap();
        println!(
            "reduced dimensions {}x{}",
            image.descriptor().width,
            image.descriptor().height
        );
        measure("decode reduced native", || {
            native::decode(native::NativeFormat::Jpeg, &bytes, options).unwrap()
        });
        measure("transform reduced", || {
            transform(
                &image,
                &TransformOptions {
                    max_dimensions: Some((2048, 2048)),
                    ..Default::default()
                },
                &budget,
                &cancel,
            )
            .unwrap()
        });
        drop(image);
        let mut image = MutableImage::new(
            ImageDescriptor::packed(width, height, PixelFormat::RGBA8, ColorEncoding::Srgb)
                .unwrap(),
            Default::default(),
            &budget,
        )
        .unwrap();
        for (i, pixel) in image.bytes_mut().chunks_exact_mut(4).enumerate() {
            let x = i as u32 % width;
            let y = i as u32 / width;
            let rgb = if y % 24 < 13 && x % 11 < 2 {
                [16, 83, 219]
            } else if y % 180 < 36 {
                [31, 41, 55]
            } else {
                [232 + (x % 23) as u8, 230 + (y % 25) as u8, 240]
            };
            pixel.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        let image = image.seal();
        measure("encode jpeg", || {
            let mut out = Vec::new();
            encode(
                &image,
                &EncodeOptions::Jpeg(Default::default()),
                &mut out,
                &budget,
                &cancel,
            )
            .unwrap();
            out
        });
        let mut read = |y: u32, out: &mut [u8]| {
            image
                .read_rows(y, 1, out)
                .map_err(|e| native::Error::Codec(e.to_string()))
        };
        measure("encode jpeg native", || {
            let mut out = Vec::new();
            native::encode_jpeg_rows(
                width,
                height,
                native::EncodeOptions {
                    quality: 95.,
                    jpeg_subsampling: 444,
                    ..Default::default()
                },
                &mut read,
                &mut |bytes| {
                    out.extend_from_slice(bytes);
                    Ok(())
                },
            )
            .unwrap();
            out
        });
    }
}
