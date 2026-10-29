unsafe extern "C" {
    fn snow_microphone_regression() -> std::ffi::c_int;
}

fn main() {
    // Compiled separately with panic=abort, matching the shipping Rust profile.
    std::process::exit(unsafe { snow_microphone_regression() });
}
