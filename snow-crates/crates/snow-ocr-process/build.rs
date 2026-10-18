//! Windows version resources and optional Crashpad link integration.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    if env::var_os("CARGO_FEATURE_CRASH_DIAGNOSTICS").is_some() {
        println!("cargo:rerun-if-env-changed=SNOW_CRASHPAD_LINK_FILE");
        if let Some(path) = env::var_os("SNOW_CRASHPAD_LINK_FILE").map(PathBuf::from) {
            println!("cargo:rerun-if-changed={}", path.display());
            let manifest = fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!(
                    "failed to read the Crashpad link manifest {}: {error}",
                    path.display()
                )
            });
            for argument in manifest.lines() {
                if !argument.trim().is_empty() {
                    if std::path::Path::new(argument).is_absolute() {
                        println!("cargo:rerun-if-changed={argument}");
                    }
                    println!("cargo:rustc-link-arg={argument}");
                }
            }
        } else {
            println!(
                "cargo:warning=snow-ocr-process was built with crash-diagnostics but \
                 SNOW_CRASHPAD_LINK_FILE is not set; the link will fail unless the \
                 Crashpad closure is provided another way"
            );
        }
    }
    #[cfg(windows)]
    {
        let platform = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("aarch64") => "windows-arm64",
            Ok("x86_64") => "windows-x64",
            arch => panic!("unsupported Windows OCR target architecture: {arch:?}"),
        };
        let version = env::var("CARGO_PKG_VERSION").expect("Cargo package version is required");
        let mut version_parts = version.split('.');
        let major = version_parts.next().expect("major version is required");
        let minor = version_parts.next().expect("minor version is required");
        let patch = version_parts.next().expect("patch version is required");
        assert!(
            version_parts.next().is_none(),
            "OCR runtime version must be major.minor.patch"
        );
        for part in [major, minor, patch] {
            part.parse::<u16>()
                .expect("OCR runtime version components must be integers");
        }

        // winres stores generated fields in hash maps, so their order changes between build
        // processes. Emit the resource in a fixed order because the versioned runtime is
        // verified and published by its whole-file hash.
        let resource_path =
            PathBuf::from(env::var("OUT_DIR").expect("Cargo output path is required"))
                .join("snow-ocr-process.rc");
        let resource_contents = format!(
            r#"#pragma code_page(65001)
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "000004b0"
        BEGIN
            VALUE "CompanyName", "Snow Apps\0"
            VALUE "FileDescription", "Snow Shot OCR runtime\0"
            VALUE "FileVersion", "{version}.0\0"
            VALUE "InternalName", "snow-ocr-process\0"
            VALUE "LegalCopyright", "Copyright (C) 2025-2026 mg-chao\0"
            VALUE "OriginalFilename", "snow-ocr-process-{version}-{platform}.exe\0"
            VALUE "ProductName", "Snow Shot OCR Runtime\0"
            VALUE "ProductVersion", "{version}\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0, 0x04b0
    END
END
"#,
        );
        fs::write(&resource_path, resource_contents)
            .expect("failed to write deterministic snow-ocr-process Windows resources");
        let mut resource = winres::WindowsResource::new();
        resource.set_resource_file(
            resource_path
                .to_str()
                .expect("Windows resource path must be valid UTF-8"),
        );
        resource
            .compile()
            .expect("failed to compile snow-ocr-process Windows resources");
    }
}
