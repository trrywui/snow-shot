use std::path::PathBuf;
use std::process::{Command, ExitCode};
mod bench;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some(command @ ("fetch-native" | "build-native" | "verify-native")) => {
            let mode = command.trim_end_matches("-native");
            let python = std::env::var_os("PYTHON").unwrap_or_else(|| "python3".into());
            let status = Command::new(python)
                .arg(root.join("native/build.py"))
                .arg(mode)
                .args(arguments)
                .current_dir(&root)
                .status()?;
            if !status.success() {
                return Err(format!("native {mode} failed: {status}").into());
            }
        }
        Some("manifest") => {
            let manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(root.join("native/sources.json"))?)?;
            println!("{}", serde_json::to_string_pretty(&manifest)?);
        }
        Some("licenses") | Some("package") => {
            let command = std::env::args().nth(1).unwrap();
            let python = std::env::var_os("PYTHON").unwrap_or_else(|| "python3".into());
            let status = Command::new(python)
                .arg(root.join(format!("scripts/{command}.py")))
                .args(arguments)
                .current_dir(&root)
                .status()?;
            if !status.success() {
                return Err(format!("{command} failed: {status}").into());
            }
        }
        Some("bench") => {
            let mut worker = root
                .join("target/cargo/performance/snow-image-worker")
                .with_extension(if cfg!(windows) { "exe" } else { "" });
            let mut output = root.join("target/benchmarks/latest.json");
            let mut iterations = 5;
            let mut quick = false;
            let mut corpus = None;
            let mut preview_reference = None;
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--worker" => worker = arguments.next().ok_or("missing worker path")?.into(),
                    "--output" => output = arguments.next().ok_or("missing output path")?.into(),
                    "--iterations" => {
                        iterations = arguments.next().ok_or("missing iteration count")?.parse()?
                    }
                    "--corpus" => {
                        corpus = Some(PathBuf::from(
                            arguments.next().ok_or("missing corpus directory")?,
                        ))
                    }
                    "--quick" => quick = true,
                    "--preview-reference" => {
                        preview_reference = Some(PathBuf::from(
                            arguments
                                .next()
                                .ok_or("missing preview reference directory")?,
                        ))
                    }
                    _ => return Err(format!("unknown benchmark argument {argument}").into()),
                }
            }
            bench::run(
                &worker,
                &output,
                iterations,
                quick,
                corpus.as_deref(),
                preview_reference.as_deref(),
            )?;
        }
        Some("compare") => {
            let baseline = PathBuf::from(arguments.next().ok_or("missing baseline report")?);
            let candidate = PathBuf::from(arguments.next().ok_or("missing candidate report")?);
            bench::compare(&baseline, &candidate)?;
        }
        _ => {
            return Err(
            "usage: cargo xtask fetch-native|build-native|verify-native|manifest|licenses|package|bench|compare [arguments]".into(),
            );
        }
    }
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
