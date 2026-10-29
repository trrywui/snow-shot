//! Deferred webcam capture stays small even when the screen source is static.
use crate::deferred::FrameIndexWriter;
use crate::{DirectRecordingConfig, ScreenRecorderError, error::Result};
use snow_core::recording_clock::RecordingClock;
use snow_recording_export::{
    ExportExecutionMode, ExportFormat, SoftwareH264Priority, StreamingEncoder,
};
use snow_recording_model::{VideoCodec, VideoEncodingSpeed, webcam::WebcamMetadata};
use std::{
    path::Path,
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub(crate) struct DeferredWebcam {
    stop: crossbeam_channel::Sender<Option<u64>>,
    worker: Option<JoinHandle<()>>,
    source: snow_webcam::Source,
}
struct Product {
    encoder: StreamingEncoder,
    index: FrameIndexWriter,
    metadata: WebcamMetadata,
}

impl DeferredWebcam {
    pub fn start(
        config: &DirectRecordingConfig,
        directory: &Path,
        clock: RecordingClock,
    ) -> Option<Self> {
        let webcam = config.webcam.clone()?;
        webcam.source.set_active(true);
        let (stop, finish) = crossbeam_channel::bounded(1);
        let path = directory.to_path_buf();
        let worker_path = path.clone();
        let mut encoding = config.streaming_config();
        encoding.output_path = path.join("webcam.mp4");
        encoding.format = ExportFormat::Mp4;
        encoding.width = webcam.style.size.div_ceil(2) * 2;
        encoding.height = encoding.width;
        encoding.fps = config.output_fps.min(30);
        encoding.codec = VideoCodec::H264;
        encoding.audio.clear();
        // A tiny source is cheaper on the CPU than a second hardware-encoder session.
        encoding.prefer_hardware_h264 = false;
        encoding.execution_mode = ExportExecutionMode::SoftwareOnly;
        encoding.software_h264_priority = SoftwareH264Priority::X264First;
        encoding.video.quality = 95;
        encoding.video.speed = VideoEncodingSpeed::VeryFast;
        encoding.encode_threads = 1;
        let source = webcam.source.clone();
        let failure_source = source.clone();
        let worker = std::thread::Builder::new()
            .name("snow-webcam-recording".into())
            .spawn(move || {
                snow_core::qos::apply_current_thread();
                let result = (|| -> Result<()> {
                    let mut product = None::<Product>;
                    let mut schedule = crate::output_schedule::OutputSchedule::new(encoding.fps);
                    let mut previous = None;
                    let mut visible = false;
                    let mut pixels = Vec::new();
                    let duration_ms = loop {
                        match finish.try_recv() {
                            Ok(Some(duration)) => break duration,
                            Ok(None) | Err(crossbeam_channel::TryRecvError::Disconnected) => {
                                return Ok(());
                            }
                            Err(crossbeam_channel::TryRecvError::Empty) => {}
                        }
                        let now = Instant::now();
                        if clock.is_active_at(now)
                            && let Some(slot) = schedule.poll(clock.active_elapsed_duration(now))
                        {
                            let snapshot = webcam.source.recording_snapshot(&clock, slot.at);
                            let revision = snapshot.frame.as_ref().map(|frame| frame.revision);
                            let at_ms = slot.at.as_millis() as u64;
                            if let Some(frame) =
                                snapshot.frame.as_ref().filter(|_| revision != previous)
                            {
                                if product.is_none() {
                                    product = Some(Product {
                                        encoder: StreamingEncoder::builder(encoding.clone())
                                            .recording_source()
                                            .create()?,
                                        index: FrameIndexWriter::new(
                                            &worker_path.join("webcam.index"),
                                            encoding.fps,
                                        )?,
                                        metadata: WebcamMetadata {
                                            style: webcam.style,
                                            fps: encoding.fps,
                                            duration_ms: 1,
                                            visibility: Vec::new(),
                                        },
                                    });
                                }
                                let product = product.as_mut().unwrap();
                                let side = webcam.style.size as usize;
                                let coded = encoding.width as usize;
                                pixels.resize(coded * coded * 4, 255);
                                for y in 0..coded {
                                    for x in 0..coded {
                                        let from = (y.min(side - 1) * side + x.min(side - 1)) * 4;
                                        let to = (y * coded + x) * 4;
                                        pixels[to..to + 4]
                                            .copy_from_slice(&frame.rgba[from..from + 4]);
                                    }
                                }
                                product.encoder.push_rgba_frame_at_pts(slot.pts, &pixels)?;
                                product.index.push(slot.pts)?;
                            }
                            if visible != revision.is_some()
                                && let Some(product) = product.as_mut()
                            {
                                if product.metadata.visibility.len() >= 100_000 {
                                    return Err(ScreenRecorderError::Encode(
                                        "webcam availability history exceeded its limit".into(),
                                    ));
                                }
                                product
                                    .metadata
                                    .visibility
                                    .push((at_ms, revision.is_some()));
                            }
                            visible = revision.is_some();
                            previous = revision;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    };
                    if let Some(mut product) = product {
                        product.encoder.finish_at_duration_ms(duration_ms)?;
                        product.index.finish(duration_ms)?;
                        product.metadata.duration_ms = duration_ms;
                        product
                            .metadata
                            .visibility
                            .retain(|(at, _)| *at < duration_ms);
                        snow_recording_model::webcam::write_metadata(
                            &worker_path.join("webcam.bin"),
                            &product.metadata,
                        )?;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    failure_source.report_error(format!("Webcam recording unavailable: {error}"));
                }
            });
        match worker {
            Ok(worker) => Some(Self {
                stop,
                worker: Some(worker),
                source,
            }),
            Err(error) => {
                source.report_error(error.to_string());
                None
            }
        }
    }

    pub fn finish(mut self, duration_ms: u64) {
        self.source.set_recording_active(false);
        let _ = self.stop.send(Some(duration_ms));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for DeferredWebcam {
    fn drop(&mut self) {
        self.source.set_recording_active(false);
        let _ = self.stop.try_send(None);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Owns recording capture independently of the host UI; teardown never blocks that UI.
pub(crate) struct CaptureLease(Option<snow_webcam::Source>);
impl CaptureLease {
    pub fn new(config: &DirectRecordingConfig) -> Self {
        Self::with_options(config.webcam.as_ref(), config.output_fps)
    }
    pub fn with_options(config: Option<&snow_webcam::Config>, fps: u32) -> Self {
        if let Some(webcam) = config {
            webcam.source.retry_failed();
            let _ = webcam.source.configure(snow_webcam::CaptureOptions {
                size: webcam.style.size,
                fps: fps.min(30),
            });
            webcam.source.set_active(true);
        }
        Self(config.map(|webcam| webcam.source.clone()))
    }
    pub fn stop(&mut self) {
        if let Some(source) = self.0.take() {
            source.set_recording_active(false);
        }
    }
}
impl Drop for CaptureLease {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct Camera(Arc<AtomicUsize>);
    impl snow_webcam::CameraBackend for Camera {
        fn poll(
            &mut self,
            timeout: Duration,
        ) -> std::result::Result<Option<snow_webcam::RawFrame>, String> {
            std::thread::sleep(timeout);
            Ok(None)
        }
    }
    impl Drop for Camera {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }
    fn wait_for(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !condition() {
            assert!(Instant::now() < deadline, "camera did not retire");
            std::thread::yield_now();
        }
    }

    #[test]
    fn stopping_capture_releases_device_before_encoder_and_bundle_finalization() {
        let active = Arc::new(AtomicUsize::new(0));
        let observed = active.clone();
        let source = snow_webcam::Source::with_backend(
            snow_webcam::CaptureOptions { size: 150, fps: 30 },
            move |_| {
                assert_eq!(observed.fetch_add(1, Ordering::AcqRel), 0);
                Ok(Box::new(Camera(observed.clone())))
            },
        )
        .unwrap();
        let config = snow_webcam::Config {
            source: source.clone(),
            style: Default::default(),
        };
        let mut capture = CaptureLease::with_options(Some(&config), 30);
        wait_for(|| active.load(Ordering::Acquire) == 1);
        capture.stop();
        capture.stop();
        wait_for(|| active.load(Ordering::Acquire) == 0);
        assert!(source.snapshot(Instant::now()).frame.is_none());
        // A retained configuration/preview and an encoder still finalizing must not keep
        // the hardware running. A later recording can explicitly acquire it again.
        let next = CaptureLease::with_options(Some(&config), 30);
        wait_for(|| active.load(Ordering::Acquire) == 1);
        drop(next);
        wait_for(|| active.load(Ordering::Acquire) == 0);
    }
}
