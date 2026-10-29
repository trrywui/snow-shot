#![no_main]
use libfuzzer_sys::fuzz_target;
use snow_image_core::*;
fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > snow_image_fuzz::MAX_INPUT {
        return;
    }
    let budget = snow_image_fuzz::budget();
    let result = snow_image_codecs::inspect(
        bytes,
        &DecodeOptions::default(),
        &budget,
        &CancellationToken::new(),
    );
    if let Ok(info) = &result {
        assert!(info.width > 0 && info.height > 0);
        assert!(info.metadata.byte_len().unwrap() <= budget.policy().max_metadata_bytes);
    }
    drop(result);
    assert_eq!(budget.total_used(), 0);
});
