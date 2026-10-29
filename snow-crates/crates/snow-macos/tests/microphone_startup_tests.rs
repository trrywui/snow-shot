//! Native fixtures use fake nodes/engines and never open an audio device.
#![cfg(target_os = "macos")]

#[test]
fn native_microphone_startup_and_exception_regressions() {
    use std::{path::Path, process::Command};
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/microphone_native_tests.m");
    let directory =
        std::env::temp_dir().join(format!("snow-microphone-tests-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let executable = directory.join("microphone-tests");
    let object = directory.join("microphone.o");
    let compilation = Command::new("xcrun")
        .args([
            "--sdk",
            "macosx",
            "clang",
            "-fobjc-arc",
            "-fobjc-exceptions",
            "-fobjc-arc-exceptions",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-mmacosx-version-min=15.0",
            "-c",
        ])
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .output()
        .unwrap();
    assert!(
        compilation.status.success(),
        "{}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    let compilation = Command::new("rustc")
        .arg(
            source
                .parent()
                .unwrap()
                .join("fixtures/microphone_abort.rs"),
        )
        .args([
            "--edition=2024",
            "-Cpanic=abort",
            "-Copt-level=3",
            "-Clink-arg=-framework",
            "-Clink-arg=AVFAudio",
            "-Clink-arg=-framework",
            "-Clink-arg=AudioToolbox",
            "-Clink-arg=-framework",
            "-Clink-arg=Foundation",
        ])
        .arg(format!("-Clink-arg={}", object.display()))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        compilation.status.success(),
        "{}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    let result = Command::new(&executable).output().unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
