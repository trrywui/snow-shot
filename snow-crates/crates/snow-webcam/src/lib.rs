//! One asynchronous camera owner shared by preview and recording. No callback enters the UI.
pub mod ffi;
#[cfg(target_os = "macos")]
mod macos;
mod resize;
#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows;

use crossbeam_channel::{Sender, bounded};
pub use snow_recording_model::{WebcamOrientation, WebcamShape, WebcamStyle};
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

const STALE_AFTER: Duration = Duration::from_secs(2);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
// Negotiate once for the full editable range. Software crop/resize and admission
// handle presentation changes without reopening the device or its permission flow.
const CAPTURE_CAPACITY: CaptureOptions = CaptureOptions { size: 500, fps: 30 };

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureOptions {
    pub size: u32,
    pub fps: u32,
}
impl CaptureOptions {
    pub fn validate(self) -> Result<(), String> {
        WebcamStyle {
            size: self.size,
            ..Default::default()
        }
        .validate()?;
        if !(1..=30).contains(&self.fps) {
            return Err("webcam frame rate must be 1..=30".into());
        }
        Ok(())
    }
}

/// Backends deliver top-down, unmirrored, opaque RGBA; row padding is explicit.
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub rgba: Vec<u8>,
    pub at: Instant,
}

/// Injectable hardware boundary. Poll must honor its timeout, including device failure.
pub trait CameraBackend {
    fn starting(&self) -> bool {
        false
    }
    fn poll(&mut self, timeout: Duration) -> Result<Option<RawFrame>, String>;
}

#[derive(Debug)]
pub struct Frame {
    pub revision: u64,
    pub at: Instant,
    pub size: u32,
    pub rgba: Arc<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub revision: u64,
    pub frame: Option<Arc<Frame>>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub source: Source,
    pub style: WebcamStyle,
}

/// Decode the shared C configuration, retaining the source rather than its borrowed handle.
///
/// # Safety
/// A non-null source must point to a live Source for the duration of this call.
pub unsafe fn config_from_raw(
    source: *const Source,
    size: u32,
    shape: u32,
) -> Result<Option<Config>, String> {
    let Some(source) = (unsafe { source.as_ref() }) else {
        return Ok(None);
    };
    let shape = match shape {
        0 => WebcamShape::Square,
        1 => WebcamShape::Circle,
        _ => return Err("invalid webcam shape".into()),
    };
    let style = WebcamStyle {
        size,
        shape,
        ..Default::default()
    };
    style.validate()?;
    Ok(Some(Config {
        source: source.clone(),
        style,
    }))
}

struct State {
    options: CaptureOptions,
    active: bool,
    generation: u64,
    revision: u64,
    frames: VecDeque<Arc<Frame>>,
    error: Option<String>,
}
struct Shared {
    state: Mutex<State>,
    stopped: AtomicBool,
}
struct Owner {
    shared: Arc<Shared>,
    wake: Sender<()>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
        // The worker owns only Shared, never Owner. Hardware teardown stays off the UI thread.
    }
}

#[derive(Clone)]
pub struct Source(Arc<Owner>);
impl PartialEq for Source {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Source {}
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WebcamSource")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl Source {
    pub fn new(options: CaptureOptions) -> Result<Self, String> {
        Self::with_backend(options, |options| {
            #[cfg(windows)]
            {
                windows::open(options)
            }
            #[cfg(target_os = "macos")]
            {
                macos::open(options)
            }
            #[cfg(not(any(windows, target_os = "macos")))]
            {
                let _ = options;
                Err("webcam capture is unsupported on this platform".into())
            }
        })
    }

    pub fn with_backend(
        options: CaptureOptions,
        factory: impl Fn(CaptureOptions) -> Result<Box<dyn CameraBackend>, String> + Send + 'static,
    ) -> Result<Self, String> {
        options.validate()?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                options,
                active: false,
                generation: 0,
                revision: 0,
                frames: VecDeque::with_capacity(4),
                error: None,
            }),
            stopped: AtomicBool::new(false),
        });
        let (wake, receiver) = bounded(1);
        let worker = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("snow-webcam".into())
            .spawn(move || {
                snow_core::qos::apply_current_thread();
                let mut backend = None;
                let mut generation = u64::MAX;
                let mut converter = resize::CropResize::default();
                let mut started = Instant::now();
                let mut last = None;
                let mut next_admission = None;
                let mut admission_fps = 0;
                while !worker.stopped.load(Ordering::Acquire) {
                    let (active, next) = {
                        let state = worker.state.lock().unwrap_or_else(|e| e.into_inner());
                        (state.active, state.generation)
                    };
                    if generation != next {
                        backend = None;
                        generation = next;
                        last = None;
                        next_admission = None;
                        started = Instant::now();
                        if active {
                            match factory(CAPTURE_CAPACITY) {
                                Ok(camera) => backend = Some(camera),
                                Err(error) => fail(&worker, generation, error),
                            }
                        }
                    }
                    let Some(camera) = backend.as_mut() else {
                        let _ = receiver.recv_timeout(Duration::from_millis(50));
                        continue;
                    };
                    let result = camera.poll(Duration::from_millis(25));
                    match result {
                        Ok(Some(frame)) => {
                            // Read the latest processing options after polling: a resize may
                            // have arrived while the backend was waiting for this sample.
                            let options = worker
                                .state
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .options;
                            let at = frame.at;
                            let interval = Duration::from_secs_f64(1.0 / f64::from(options.fps));
                            if admission_fps != options.fps {
                                admission_fps = options.fps;
                                next_admission = last.map(|at| at + interval);
                            }
                            // Keep the phase across ordinary camera jitter; comparing each interval
                            // directly can accidentally halve a nominal 30 fps stream.
                            if next_admission.is_some_and(|next| at + interval / 10 < next) {
                                continue;
                            }
                            next_admission = Some(
                                next_admission
                                    .map_or(at + interval, |next| (next + interval).max(at)),
                            );
                            let pixels = match converter.convert(&frame, options.size) {
                                Ok(pixels) => pixels,
                                Err(error) => {
                                    fail(&worker, generation, error);
                                    backend = None;
                                    continue;
                                }
                            };
                            last = Some(at);
                            let mut state = worker.state.lock().unwrap_or_else(|e| e.into_inner());
                            if state.generation != generation || !state.active {
                                continue;
                            }
                            state.revision += 1;
                            let frame = Arc::new(Frame {
                                revision: state.revision,
                                at,
                                size: options.size,
                                rgba: Arc::new(pixels),
                            });
                            while state.frames.len() >= 4 {
                                state.frames.pop_front();
                            }
                            state.frames.push_back(frame);
                        }
                        Ok(None) => {
                            let now = Instant::now();
                            if camera.starting() {
                                started = now;
                                continue;
                            }
                            if last
                                .is_some_and(|at| now.saturating_duration_since(at) > STALE_AFTER)
                                || (last.is_none()
                                    && !camera.starting()
                                    && now.duration_since(started) > STARTUP_TIMEOUT)
                            {
                                fail(
                                    &worker,
                                    generation,
                                    "The webcam stopped delivering video frames.".into(),
                                );
                                backend = None;
                            }
                        }
                        Err(error) => {
                            fail(&worker, generation, error);
                            backend = None;
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self(Arc::new(Owner { shared, wake })))
    }

    pub fn configure(&self, options: CaptureOptions) -> Result<(), String> {
        options.validate()?;
        let mut state = self
            .0
            .shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if state.options != options {
            state.options = options;
            // Retain a fresh image for preview resampling until the next processed
            // sample arrives. Only activation/retry changes the hardware generation.
        }
        let _ = self.0.wake.try_send(());
        Ok(())
    }
    pub fn set_active(&self, active: bool) {
        let mut state = self
            .0
            .shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if state.active != active {
            state.active = active;
            reset(&mut state);
        }
        let _ = self.0.wake.try_send(());
    }
    /// Pausing never grants permission to retry a failed camera in the same recording.
    pub fn set_recording_active(&self, active: bool) {
        let mut state = self
            .0
            .shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if active && state.error.is_some() {
            return;
        }
        if state.active != active {
            let error = state.error.take();
            state.active = active;
            reset(&mut state);
            state.error = error;
        }
        let _ = self.0.wake.try_send(());
    }
    pub fn retry_failed(&self) {
        let mut state = self
            .0
            .shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if state.error.is_some() {
            reset(&mut state);
        }
        let _ = self.0.wake.try_send(());
    }
    /// A bounded historical lookup: never admits a camera sample from the future.
    pub fn snapshot(&self, at: Instant) -> Snapshot {
        let state = self
            .0
            .shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let frame = state
            .frames
            .iter()
            .rev()
            .find(|frame| frame.at <= at && at.duration_since(frame.at) <= STALE_AFTER)
            .cloned();
        Snapshot {
            revision: state.revision,
            frame,
            error: state.error.clone(),
        }
    }
    pub fn recording_snapshot(
        &self,
        clock: &snow_core::recording_clock::RecordingClock,
        slot: Duration,
    ) -> Snapshot {
        let state = self
            .0
            .shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let frame = state
            .frames
            .iter()
            .rev()
            .find(|frame| {
                frame.at <= now
                    && frame.size == state.options.size
                    && now.saturating_duration_since(frame.at) <= STALE_AFTER
                    && clock.is_active_at(frame.at)
                    && clock.active_elapsed_duration(frame.at) <= slot
            })
            .cloned();
        Snapshot {
            revision: state.revision,
            frame,
            error: state.error.clone(),
        }
    }
    pub fn report_error(&self, error: String) {
        let mut state = self
            .0
            .shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.active = false;
        reset(&mut state);
        state.error = Some(error);
        let _ = self.0.wake.try_send(());
    }
}

fn reset(state: &mut State) {
    state.generation += 1;
    state.revision += 1;
    state.frames.clear();
    state.error = None;
}
fn fail(shared: &Shared, generation: u64, error: String) {
    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    if state.generation == generation && state.active {
        state.frames.clear();
        state.revision += 1;
        state.error = Some(error);
    }
}
