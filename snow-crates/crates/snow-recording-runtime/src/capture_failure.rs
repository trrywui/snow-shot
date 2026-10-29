//! Capture owns elapsed time; encoder admission may intentionally omit static slots.
use std::time::Instant;

use snow_core::recording_clock::RecordingClock;
use snow_recording_export::StreamingEncoder;

use crate::ScreenRecorderError;

pub(crate) fn preserve(
    encoder: &mut StreamingEncoder,
    clock: &RecordingClock,
    at: Instant,
    cause: ScreenRecorderError,
) -> ScreenRecorderError {
    if matches!(
        cause,
        ScreenRecorderError::ExportCanceled
            | ScreenRecorderError::Capture(snow_capture::error::CaptureError::Canceled)
    ) {
        return ScreenRecorderError::ExportCanceled;
    }
    encoder
        .preserve_failure_at_duration(&cause.to_string(), clock.active_elapsed_duration(at))
        .map(ScreenRecorderError::from)
        .unwrap_or(cause)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffmpeg_next::{self as ffmpeg, Rescale};
    use snow_recording_export::{
        ExportExecutionMode, ExportFormat, SoftwareH264Priority, StreamingEncoderConfig, VideoCodec,
    };
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn encoder(path: PathBuf, fps: u32) -> StreamingEncoder {
        let mut encoder = StreamingEncoder::builder(StreamingEncoderConfig {
            output_path: path,
            loop_animated_images: true,
            format: ExportFormat::Mp4,
            width: 16,
            height: 16,
            fps,
            codec: VideoCodec::H264,
            prefer_hardware_h264: false,
            execution_mode: ExportExecutionMode::SoftwareOnly,
            software_h264_priority: SoftwareH264Priority::X264First,
            video: Default::default(),
            encode_threads: 1,
            audio: Vec::new(),
        })
        .recording_source()
        .software_only()
        .create()
        .unwrap();
        encoder.retain_failed_output();
        encoder
    }

    fn retained_video_end(path: &Path, error: ScreenRecorderError) -> i64 {
        let message = error.to_string();
        let retained = PathBuf::from(
            message
                .split("recoverable media is retained in ")
                .nth(1)
                .unwrap(),
        )
        .join(path.file_name().unwrap());
        let mut input = ffmpeg::format::input(&retained).unwrap();
        let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
        let index = stream.index();
        let time_base = stream.time_base();
        input
            .packets()
            .filter(|(stream, _)| stream.index() == index)
            .map(|(_, packet)| {
                (packet.pts().unwrap() + packet.duration()).rescale(time_base, (1, 1000))
            })
            .max()
            .unwrap()
    }

    #[test]
    fn skipped_static_slots_preserve_capture_time_through_pause_and_stop() {
        let directory = tempfile::tempdir().unwrap();
        for fps in [10, 60] {
            for changed in [false, true] {
                for boundary in ["active", "paused", "stopped"] {
                    let path = directory
                        .path()
                        .join(format!("{fps}-{changed}-{boundary}.mp4"));
                    let mut encoder = encoder(path.clone(), fps);
                    let start = Instant::now();
                    let clock = RecordingClock::new(start);
                    clock
                        .controller()
                        .mark_pause(start + Duration::from_secs(2));
                    clock
                        .controller()
                        .mark_resume(start + Duration::from_secs(4));
                    // Exercise production admission: no encoder calls for unchanged slots.
                    for pts in 0..u64::from(fps) * 10 {
                        if encoder.needs_video_frame(pts == 0 || (changed && pts == u64::from(fps)))
                        {
                            encoder
                                .push_owned_rgba_frame_at_pts(pts, vec![pts as u8; 16 * 16 * 4])
                                .unwrap();
                        }
                    }
                    let end = start + Duration::from_millis(12_037);
                    if boundary != "active" {
                        clock.controller().mark_pause(end);
                    }
                    let failed_at = if boundary == "active" {
                        end
                    } else {
                        end + Duration::from_secs(5)
                    };
                    let error = preserve(
                        &mut encoder,
                        &clock,
                        failed_at,
                        ScreenRecorderError::Encode(
                            "injected input/capture teardown failure".into(),
                        ),
                    );
                    drop(encoder);
                    assert_eq!(
                        retained_video_end(&path, error),
                        10_037,
                        "{fps} {changed} {boundary}"
                    );
                    assert!(!path.exists());
                }
            }
        }
    }

    #[test]
    fn canceled_capture_discards_staging_even_with_a_pending_static_image() {
        for native in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut encoder = encoder(directory.path().join("source.mp4"), 10);
            encoder
                .push_owned_rgba_frame_at_pts(0, vec![80; 16 * 16 * 4])
                .unwrap();
            let start = Instant::now();
            let cause = if native {
                snow_capture::error::CaptureError::Canceled.into()
            } else {
                ScreenRecorderError::ExportCanceled
            };
            preserve(
                &mut encoder,
                &RecordingClock::new(start),
                start + Duration::from_secs(10),
                cause,
            );
            drop(encoder);
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        }
    }
}
