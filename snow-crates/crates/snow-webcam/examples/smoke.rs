//! Explicit native camera smoke test. Does not save images or open a microphone.
use snow_webcam::{CaptureOptions, Source};
use std::time::{Duration, Instant};
fn main() -> Result<(), String> {
    let source = Source::new(CaptureOptions { size: 150, fps: 30 })?;
    source.set_active(true);
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut seen = 0;
    let mut last = 0;
    while Instant::now() < deadline {
        let snapshot = source.snapshot(Instant::now());
        if let Some(error) = snapshot.error {
            source.set_active(false);
            return Err(error);
        }
        if let Some(frame) = snapshot.frame
            && frame.revision != last
        {
            assert_eq!(frame.rgba.len(), 150 * 150 * 4);
            last = frame.revision;
            seen += 1;
            if seen == 60 {
                source.set_active(false);
                println!("Received 60 native camera frames; capture retired.");
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    source.set_active(false);
    Err(format!("Native camera smoke timed out after {seen} frames"))
}
