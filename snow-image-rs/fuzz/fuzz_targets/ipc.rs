#![no_main]
use libfuzzer_sys::fuzz_target;
use snow_image_core::*;
use snow_image_protocol::*;
fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > snow_image_fuzz::MAX_INPUT {
        return;
    }
    let mut input = bytes;
    if let Ok(command) = read_control::<Command>(&mut input) {
        if let Command::Run(mut request) = command {
            let budget = snow_image_fuzz::budget();
            request.policy = budget.policy().clone();
            if request.validate().is_ok() {
                if let Ok(reservation) =
                    budget.reserve(ResourceKind::Metadata, request.metadata.total().unwrap())
                {
                    let metadata = request
                        .metadata
                        .read(&mut input, budget.policy().max_metadata_bytes);
                    if let Ok(metadata) = metadata {
                        let _ = SharedMetadata::from_reserved(metadata, reservation);
                    }
                }
            }
            assert_eq!(budget.total_used(), 0);
        }
    }
    let mut input = bytes;
    let _ = read_control::<Response>(&mut input);
});
