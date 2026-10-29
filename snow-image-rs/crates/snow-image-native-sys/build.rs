use sha2::{Digest, Sha256};
use std::{env, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-env-changed=SNOW_IMAGE_NATIVE_PREFIX");
    println!("cargo:rerun-if-env-changed=CMAKE_GENERATOR");
    if env::var_os("CARGO_FEATURE_NATIVE_BACKENDS").is_none() {
        return;
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let target = env::var("TARGET").unwrap();
    if target.contains("windows") {
        assert!(
            !env::var("CARGO_CFG_TARGET_FEATURE")
                .unwrap_or_default()
                .split(',')
                .any(|v| v == "crt-static"),
            "Private native libraries use the dynamic MSVC CRT; remove +crt-static or rebuild every dependency consistently"
        );
    }
    let prefix = env::var_os("SNOW_IMAGE_NATIVE_PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target/native").join(&target).join("prefix"));
    assert!(
        prefix.join(".snow-native-manifest").exists(),
        "Native dependencies are not prepared. Run `cargo xtask fetch-native` then `cargo xtask build-native` (or python3 native/build.py fetch/build). Cargo builds never download native sources."
    );
    let manifest =
        std::fs::read(root.join("native/sources.json")).expect("read pinned source manifest");
    let expected = format!("{:x}", Sha256::digest(&manifest));
    let stamp = std::fs::read_to_string(prefix.join(".snow-native-manifest"))
        .expect("read native prefix identity");
    let mut stamp_lines = stamp.lines();
    assert_eq!(
        stamp_lines.next(),
        Some(expected.as_str()),
        "Native sources changed; rebuild the private prefix"
    );
    assert_eq!(
        stamp_lines.next(),
        Some(target.as_str()),
        "Native prefix target does not match Cargo target"
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("native/sources.json").display()
    );
    let prefix = prefix.canonicalize().unwrap();
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = root.join("native/shim");
    println!("cargo:rerun-if-changed={}", source.display());
    println!(
        "cargo:rerun-if-changed={}",
        prefix.join(".snow-native-manifest").display()
    );
    let mut configure = Command::new("cmake");
    configure
        .arg("-S")
        .arg(&source)
        .arg("-B")
        .arg(&out)
        .args(["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release"])
        .arg(format!("-DSNOW_NATIVE_PREFIX={}", prefix.display()));
    if target.contains("apple") {
        configure
            .arg("-DCMAKE_OSX_DEPLOYMENT_TARGET=15.0")
            .arg(format!(
                "-DCMAKE_OSX_ARCHITECTURES={}",
                if target.starts_with("aarch64") {
                    "arm64"
                } else {
                    "x86_64"
                }
            ));
    }
    if target.contains("windows") {
        configure
            .arg("-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL")
            .args([
                "-DCMAKE_C_COMPILER=clang-cl",
                "-DCMAKE_CXX_COMPILER=clang-cl",
            ])
            .arg(format!("-DCMAKE_C_COMPILER_TARGET={target}"))
            .arg(format!("-DCMAKE_CXX_COMPILER_TARGET={target}"));
    }
    assert!(
        configure.status().expect("run CMake").success(),
        "native adapter configuration failed"
    );
    assert!(
        Command::new("cmake")
            .arg("--build")
            .arg(&out)
            .arg("--parallel")
            .arg(env::var("NUM_JOBS").unwrap_or_else(|_| "2".into()))
            .status()
            .expect("build adapter")
            .success(),
        "native adapter build failed"
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!(
        "cargo:rustc-link-search=native={}",
        prefix.join("lib").display()
    );
    for lib in [
        "snow_image_native",
        "jpeg",
        "webpmux",
        "webpdemux",
        "webp",
        "sharpyuv",
        "jxl",
        "jxl_threads",
        "jxl_cms",
        "hwy",
        "brotlienc",
        "brotlidec",
        "brotlicommon",
        "avif",
        "aom",
        "dav1d",
        "lcms2",
        "zimg",
    ] {
        let name = if target.contains("windows") && lib == "jpeg" {
            "jpeg-static"
        } else {
            lib
        };
        println!("cargo:rustc-link-lib=static={name}");
    }
    if target.contains("apple") {
        println!("cargo:rustc-link-lib=c++");
    }
    if target.contains("windows") {
        for lib in ["user32", "advapi32", "shell32"] {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
}
