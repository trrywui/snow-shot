#![no_main]
use libfuzzer_sys::fuzz_target;
use snow_image_core::*;
fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > snow_image_fuzz::MAX_INPUT {
        return;
    }
    let budget = snow_image_fuzz::budget();
    let result = snow_image_codecs::decode(
        bytes,
        &DecodeOptions::default(),
        &budget,
        &CancellationToken::new(),
    );
    if let Ok(image) = &result {
        image.descriptor().validate().unwrap();
        assert_eq!(
            image.bytes().len(),
            image.descriptor().required_bytes().unwrap()
        );
        let shared = image.clone();
        assert_eq!(shared.bytes().as_ptr(), image.bytes().as_ptr());
    }
    drop(result);
    assert_eq!(budget.total_used(), 0);
});
