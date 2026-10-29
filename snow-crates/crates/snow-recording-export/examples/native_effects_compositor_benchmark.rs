//! Native live-compositor cost, excluding capture, effect rasterization, and encoding.
//! Release only: WIDTH HEIGHT bypass|base|16|32|atlas16|atlas32.
//! Atlas cases include packing time and check exact native output against separate tiles.
#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    use snow_macos::compositor::{Compositor, Layer, RgbaOverlay};
    use snow_media::{
        PixelFormat,
        geometry::{PixelRect, PixelSize},
    };
    use std::{hint::black_box, time::Instant};

    if cfg!(debug_assertions) {
        bail!("use the macOS performance preset's Release build");
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        bail!("usage: WIDTH HEIGHT bypass|base|16|32|atlas16|atlas32");
    }
    let size = PixelSize::new(args[0].parse()?, args[1].parse()?)?;
    anyhow::ensure!(
        size.width >= 1024 && size.height >= 512 && size.width <= 4096 && size.height <= 4096,
        "dimensions must fit the fixture tile grid and be at most 4096"
    );
    let count = match args[2].as_str() {
        "bypass" | "base" => 0,
        "16" | "atlas16" => 16,
        "32" | "atlas32" => 32,
        _ => bail!("case must be bypass, base, 16, 32, atlas16, or atlas32"),
    };
    let mut compositor = Compositor::new(size, PixelFormat::Bgra8, 4)?;
    let source = compositor.compose(&[], true)?;
    let rect = PixelRect {
        x: 0,
        y: 0,
        width: size.width,
        height: size.height,
    };
    let layers = [Layer {
        image: &source,
        source: rect,
        destination: rect,
    }];
    let mut storage: Vec<_> = (0..count)
        .map(|_| [100, 50, 25, 180].repeat(128 * 128))
        .collect();
    let atlas = args[2].starts_with("atlas");
    let mut packed = if atlas {
        vec![0; count * 128 * 128 * 4]
    } else {
        Vec::new()
    };
    let mut samples = Vec::with_capacity(180);
    let mut presented = None;
    for iteration in 0..210 {
        // Change upload contents to prevent a static-image cache from hiding upload costs.
        for pixels in &mut storage {
            pixels[0] = 40 + (iteration % 100) as u8;
        }
        let overlays: Vec<_> = storage
            .iter()
            .enumerate()
            .map(|(index, bytes)| RgbaOverlay {
                x: (index as u32 % 8) * 128,
                y: (index as u32 / 8) * 128,
                width: 128,
                height: 128,
                stride: 512,
                bytes,
            })
            .collect();
        let started = Instant::now();
        let next = if args[2] == "bypass" {
            source.clone()
        } else if atlas {
            for (index, bytes) in storage.iter().enumerate() {
                for row in 0..128 {
                    let start = ((index / 8 * 128 + row) * 1024 + index % 8 * 128) * 4;
                    packed[start..start + 512].copy_from_slice(&bytes[row * 512..(row + 1) * 512]);
                }
            }
            compositor.compose_with_overlays(
                &layers,
                &[RgbaOverlay {
                    x: 0,
                    y: 0,
                    width: 1024,
                    height: count as u32 / 8 * 128,
                    stride: 1024 * 4,
                    bytes: &packed,
                }],
                true,
            )?
        } else {
            compositor.compose_with_overlays(&layers, &overlays, true)?
        };
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if iteration >= 30 {
            samples.push(elapsed);
        }
        black_box(&presented);
        presented = Some(next);
    }
    // Read back once outside timing to verify native output and pixel ownership.
    let pixels = presented.context("no native output")?.to_cpu()?;
    anyhow::ensure!(
        pixels.size == size && !pixels.bytes.is_empty(),
        "invalid output surface"
    );
    if count != 0 {
        anyhow::ensure!(pixels.bytes[0] != 0, "overlay missing");
    }
    if atlas {
        let reference: Vec<_> = storage
            .iter()
            .enumerate()
            .map(|(index, bytes)| RgbaOverlay {
                x: (index as u32 % 8) * 128,
                y: (index as u32 / 8) * 128,
                width: 128,
                height: 128,
                stride: 512,
                bytes,
            })
            .collect();
        let expected = compositor
            .compose_with_overlays(&layers, &reference, true)?
            .to_cpu()?;
        anyhow::ensure!(
            pixels.bytes == expected.bytes,
            "packing changed native output pixels"
        );
    }
    samples.sort_by(f64::total_cmp);
    println!("width,height,case,p50_ms,p95_ms,tile_upload_bytes");
    println!(
        "{},{},{},{:.4},{:.4},{}",
        size.width,
        size.height,
        args[2],
        samples[90],
        samples[171],
        count * 128 * 128 * 4
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("The native effects compositor benchmark requires macOS.");
    std::process::exit(1);
}
