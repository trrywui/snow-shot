fn main() {
    println!("cargo:rerun-if-changed=native/microphone.m");
    println!("cargo:rerun-if-changed=native/microphone.h");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new()
        .file("native/microphone.m")
        .flag("-fobjc-arc")
        .flag("-fobjc-exceptions")
        .flag("-fobjc-arc-exceptions")
        .warnings_into_errors(true)
        .compile("snow_microphone");
}
