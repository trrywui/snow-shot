// Compile optimized bytecode once at build time, also when targeting Windows ARM64.
#[cfg(windows)]
fn main() {
    use windows::{Win32::Graphics::Direct3D::Fxc::*, core::PCSTR};
    println!("cargo:rerun-if-changed=src/gpu_pipeline.hlsl");
    println!("cargo:rerun-if-changed=src/smart_erase_compute.hlsl");
    println!("cargo:rerun-if-changed=src/gpu_donors.hlsl");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let output =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"));
    for (entry, profile) in [
        ("mesh_vertex", "vs_4_1"),
        ("quad_vertex", "vs_4_1"),
        ("mesh_pixel", "ps_4_1"),
        ("effect_pixel", "ps_4_1"),
        ("load_source", "cs_5_0"),
        ("downsample", "cs_5_0"),
        ("erode_rows", "cs_5_0"),
        ("prefix_rows", "cs_5_0"),
        ("compact_rows", "cs_5_0"),
        ("nearest_rows", "cs_5_0"),
        ("nearest_columns", "cs_5_0"),
        ("lab_colors", "cs_5_0"),
        ("moments_horizontal", "cs_5_0"),
        ("moments_vertical", "cs_5_0"),
        ("guide_lines", "cs_5_0"),
        ("initialize_matches", "cs_5_0"),
        ("clear_reuse", "cs_5_0"),
        ("count_reuse", "cs_5_0"),
        ("match_patches", "cs_5_0"),
        ("vote_patches", "cs_5_0"),
        ("finish_pixels", "cs_5_0"),
        ("present_original", "cs_5_0"),
        ("source_flags", "cs_5_0"),
        ("domain_initialize", "cs_5_0"),
        ("domain_pixels", "cs_5_0"),
        ("domain_advance", "cs_5_0"),
        ("domain_margin", "cs_5_0"),
        ("detect_periodic", "cs_5_0"),
        ("periodic_lines", "cs_5_0"),
        ("assemble_donors", "cs_5_0"),
        ("copy_donors", "cs_5_0"),
        ("present_donors", "cs_5_0"),
    ] {
        let path = if matches!(entry, "assemble_donors" | "copy_donors" | "present_donors") {
            "src/gpu_donors.hlsl"
        } else if profile.starts_with("cs_") {
            "src/smart_erase_compute.hlsl"
        } else {
            "src/gpu_pipeline.hlsl"
        };
        let source = std::fs::read(path).expect("canvas shader source");
        let entry_name = std::ffi::CString::new(entry).unwrap();
        let profile_name = std::ffi::CString::new(profile).unwrap();
        let mut code = None;
        let mut errors = None;
        let result = unsafe {
            D3DCompile(
                source.as_ptr().cast(),
                source.len(),
                PCSTR::null(),
                None,
                None,
                PCSTR(entry_name.as_ptr().cast()),
                PCSTR(profile_name.as_ptr().cast()),
                D3DCOMPILE_OPTIMIZATION_LEVEL3
                    | D3DCOMPILE_ENABLE_STRICTNESS
                    | if path.ends_with("gpu_donors.hlsl") {
                        D3DCOMPILE_IEEE_STRICTNESS
                    } else {
                        0
                    },
                0,
                &mut code,
                Some(&mut errors),
            )
        };
        if let Err(error) = result {
            let diagnostic = errors
                .map(|blob| unsafe {
                    String::from_utf8_lossy(std::slice::from_raw_parts(
                        blob.GetBufferPointer().cast(),
                        blob.GetBufferSize(),
                    ))
                    .into_owned()
                })
                .unwrap_or_default();
            panic!("Canvas shader {entry}: {error}\n{diagnostic}");
        }
        let code = code.expect("shader compiler bytecode");
        let bytes = unsafe {
            std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize())
        };
        std::fs::write(output.join(format!("{entry}.cso")), bytes).expect("write optimized shader");
    }
}

#[cfg(not(windows))]
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        panic!("The Windows GPU backend requires a Windows build host for D3D shader compilation");
    }
}
