//! Usage: cargo run -p snow-image-rs --example export -- WORKER INPUT OUTPUT [--hdr-pq10]
use std::{path::PathBuf, process::ExitCode, sync::Arc};

use snow_image_rs::*;

fn run() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let worker = args.next().ok_or("missing worker path")?;
    let input = PathBuf::from(args.next().ok_or("missing input path")?);
    let output = PathBuf::from(args.next().ok_or("missing output path")?);
    let hdr = match args.next().as_deref().and_then(|value| value.to_str()) {
        None => false,
        Some("--hdr-pq10") => true,
        Some(_) => return Err("expected --hdr-pq10".into()),
    };
    let extension = output
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let options = match extension.as_str() {
        "png" => EncodeOptions::Png(PngOptions::default()),
        "jpg" | "jpeg" => EncodeOptions::Jpeg(JpegOptions::default()),
        "bmp" => EncodeOptions::Bmp(BmpOptions::default()),
        "webp" => EncodeOptions::WebP(WebPOptions::default()),
        "jxl" => EncodeOptions::Jxl(JxlOptions::default()),
        "avif" => EncodeOptions::Avif(AvifOptions::default()),
        _ => return Err("output extension must be png, jpg, bmp, webp, jxl, or avif".into()),
    };
    if hdr && !matches!(options, EncodeOptions::Avif(_) | EncodeOptions::Jxl(_)) {
        return Err("this example exports HDR only through AVIF or JXL".into());
    }
    let service = ImageService::new(ServiceConfig::new(worker))?;
    let input = service.input_file(input)?;
    let image = service.decode(input, DecodeOptions::default()).wait()?;
    let prepared = service
        .prepare_transform(
            Arc::new(image),
            TransformOptions {
                dynamic_range: if hdr {
                    DynamicRangePolicy::PreserveDynamicRange
                } else {
                    DynamicRangePolicy::ScreenshotSdr
                },
                hdr_output: hdr.then_some(HdrOutput::Bt2020Pq10),
                ..Default::default()
            },
        )
        .wait()?;
    let artifact = service.encode(prepared.source(), options).wait()?;
    artifact.save(output, WriteMode::CreateNew)?;
    let receipt = artifact.receipt();
    println!(
        "{}×{}, {} bits, {} bytes, {:?}",
        receipt.width, receipt.height, receipt.bit_depth, receipt.bytes, receipt.fidelity
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
