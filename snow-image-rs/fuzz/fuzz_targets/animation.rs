#![no_main]
use libfuzzer_sys::fuzz_target;
use snow_image_core::*;
fuzz_target!(|bytes: &[u8]| {
    if bytes.len() < 17 || bytes.len() > snow_image_fuzz::MAX_INPUT {
        return;
    }
    let (operations, input) = bytes.split_at(16);
    let budget = snow_image_fuzz::budget();
    let cancel = CancellationToken::new();
    if let Ok(mut session) = snow_image_codecs::AnimationSession::open(
        input,
        &DecodeOptions::default(),
        &budget,
        &cancel,
    ) {
        let mut snapshot: Option<Image> = None;
        for operation in operations {
            match operation % 4 {
                0 => {
                    if let Ok(Some(frame)) = session.next_frame(&cancel) {
                        snapshot = Some(frame.image);
                    }
                }
                1 => {
                    let _ = session.reset(&cancel);
                }
                2 => {
                    let _ = session.frame(
                        u32::from(*operation) % session.info().frame_count.max(1),
                        &cancel,
                    );
                }
                _ => {
                    cancel.cancel();
                    assert!(session.next_frame(&cancel).is_err());
                    break;
                }
            }
        }
        drop(session);
        if let Some(image) = snapshot {
            image.descriptor().validate().unwrap();
            assert_eq!(
                image.bytes().len(),
                image.descriptor().required_bytes().unwrap()
            );
        }
    }
    assert_eq!(budget.total_used(), 0);
});
