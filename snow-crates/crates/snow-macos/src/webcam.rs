//! AVFoundation video-only capture. Session control and conversion stay on its owner worker.
use block2::RcBlock;
use crossbeam_channel::{Receiver, Sender};
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::{
    AnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
};
use objc2_av_foundation::*;
use objc2_core_media::{
    CMSampleBuffer, CMTime, CMTimeFlags, CMVideoFormatDescriptionGetDimensions,
};
use objc2_foundation::{NSBundle, NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSString};
use snow_media::{ColorDescription, PixelFormat, macos::PixelBuffer};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

mod configuration;
use configuration::{ConfigurationLock, preference};

struct State {
    frames: Sender<(PixelBuffer, Instant)>,
    closed: AtomicBool,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "SnowWebcamOutput"]
    #[ivars = Arc<State>]
    struct Output;
    unsafe impl NSObjectProtocol for Output {}
    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for Output {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        unsafe fn receive(&self, _output: &AVCaptureOutput, sample: &CMSampleBuffer, _connection: &AVCaptureConnection) {
            let state = self.ivars();
            if state.closed.load(Ordering::Acquire) || state.frames.is_full() { return; }
            let at = Instant::now();
            if let Some(image) = unsafe { sample.image_buffer() }
                && let Ok(image) = unsafe { PixelBuffer::from_retained(image, ColorDescription::SRGB) } {
                let _ = state.frames.try_send((image, at));
            }
        }
    }
);

/// None means that the OS has not yet asked the user.
pub fn authorization() -> Option<bool> {
    let status =
        unsafe { AVCaptureDevice::authorizationStatusForMediaType(AVMediaTypeVideo.unwrap()) };
    if status == AVAuthorizationStatus::NotDetermined {
        None
    } else {
        Some(status == AVAuthorizationStatus::Authorized)
    }
}
pub fn request_access() -> Receiver<bool> {
    let (sender, receiver) = crossbeam_channel::bounded(1);
    // Command-line hosts and tests may not carry an app privacy manifest. macOS would
    // terminate that process if we requested camera permission without this key.
    if NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str("NSCameraUsageDescription"))
        .is_none()
    {
        let _ = sender.try_send(false);
        return receiver;
    }
    let block = RcBlock::new(move |granted: objc2::runtime::Bool| {
        let _ = sender.try_send(granted.as_bool());
    });
    unsafe {
        AVCaptureDevice::requestAccessForMediaType_completionHandler(
            AVMediaTypeVideo.unwrap(),
            &block,
        );
    }
    receiver
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub stride: usize,
    pub at: Instant,
}
pub struct Stream {
    session: Retained<AVCaptureSession>,
    output: Retained<AVCaptureVideoDataOutput>,
    _delegate: Retained<Output>,
    _queue: DispatchRetained<DispatchQueue>,
    state: Arc<State>,
    frames: Receiver<(PixelBuffer, Instant)>,
    _configuration: ConfigurationLock<Retained<AVCaptureDevice>>,
}
impl Stream {
    pub fn open(size: u32, fps: u32) -> Result<Self, String> {
        if !(50..=500).contains(&size) || !(1..=30).contains(&fps) {
            return Err("Invalid camera capture size or frame rate.".into());
        }
        objc2::rc::autoreleasepool(|_| unsafe {
            if authorization() != Some(true) {
                return Err(
                    "Camera access was denied. Enable camera access in System Settings.".into(),
                );
            }
            let media = AVMediaTypeVideo.unwrap();
            #[allow(deprecated)]
            let device = AVCaptureDevice::defaultDeviceWithMediaType(media)
                .or_else(|| AVCaptureDevice::devicesWithMediaType(media).firstObject())
                .ok_or("No camera is connected.")?;
            let session = AVCaptureSession::new();
            let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device)
                .map_err(|e| e.to_string())?;
            let output = AVCaptureVideoDataOutput::new();
            if !session.canAddInput(&input) || !session.canAddOutput(&output) {
                return Err("The camera is unavailable or in use.".into());
            }
            session.addInput(&input);
            session.addOutput(&output);
            let configuration = ConfigurationLock::new(device)?;
            let device = &configuration.0;
            let format = device
                .formats()
                .iter()
                .filter_map(|format| {
                    let dimensions =
                        CMVideoFormatDescriptionGetDimensions(&format.formatDescription());
                    let (selected, interval) = format
                        .videoSupportedFrameRateRanges()
                        .iter()
                        .filter_map(|range| {
                            let selected = preference(
                                (dimensions.width, dimensions.height),
                                size,
                                fps,
                                (range.minFrameRate(), range.maxFrameRate()),
                            )?;
                            // Use the device's exact CMTime at range boundaries (e.g.
                            // 30000/1001 fps), avoiding rounded, unsupported durations.
                            let interval = if selected.fps == range.minFrameRate() {
                                range.maxFrameDuration()
                            } else if selected.fps == range.maxFrameRate() {
                                range.minFrameDuration()
                            } else {
                                CMTime {
                                    value: 1,
                                    timescale: fps as i32,
                                    flags: CMTimeFlags::Valid,
                                    epoch: 0,
                                }
                            };
                            Some((selected, interval))
                        })
                        .min_by_key(|(selected, _)| selected.score)?;
                    Some((selected.score, format, interval))
                })
                .min_by_key(|(score, _, _)| *score)
                .ok_or("The camera does not support a usable video format.")?;
            device.setActiveFormat(&format.1);
            device.setActiveVideoMinFrameDuration(format.2);
            device.setActiveVideoMaxFrameDuration(format.2);
            let settings = NSDictionary::from_slices(
                &[&*NSString::from_str("PixelFormatType")],
                &[
                    &*NSNumber::new_u32(objc2_core_video::kCVPixelFormatType_32BGRA)
                        as &objc2::runtime::AnyObject,
                ],
            );
            output.setVideoSettings(Some(&settings));
            output.setAlwaysDiscardsLateVideoFrames(true);
            if let Some(connection) = output.connectionWithMediaType(media)
                && connection.isVideoMirroringSupported()
            {
                connection.setAutomaticallyAdjustsVideoMirroring(false);
                connection.setVideoMirrored(false);
            }
            let (sender, frames) = crossbeam_channel::bounded(1);
            let state = Arc::new(State {
                frames: sender,
                closed: AtomicBool::new(false),
            });
            let allocated = Output::alloc().set_ivars(Arc::clone(&state));
            let delegate: Retained<Output> = msg_send![super(allocated), init];
            let queue = crate::qos::application_queue("app.snow.webcam.frames");
            output.setSampleBufferDelegate_queue(
                Some(ProtocolObject::from_ref(&*delegate)),
                Some(&queue),
            );
            let stream = Self {
                session,
                output,
                _delegate: delegate,
                _queue: queue,
                state,
                frames,
                _configuration: configuration,
            };
            stream.session.startRunning();
            if !stream.session.isRunning() {
                return Err("The camera could not start.".into());
            }
            Ok(stream)
        })
    }
    pub fn poll(&self, timeout: Duration) -> Result<Option<Frame>, String> {
        match self.frames.recv_timeout(timeout) {
            Ok((image, at)) => rgba_frame(&image, at).map(Some),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Ok(None),
            Err(_) => Err("The camera disconnected.".into()),
        }
    }
}

fn rgba_frame(image: &PixelBuffer, at: Instant) -> Result<Frame, String> {
    let size = image.size();
    let stride = size.width as usize * 4;
    let mut rgba = vec![0; stride * size.height as usize];
    // Copy and channel-swap directly from CoreVideo while its read lock is held.
    // Avoid a full BGRA allocation, a conversion allocation and a final clone.
    image
        .copy_packed(&mut rgba, image.format() == PixelFormat::Bgra8)
        .map_err(|error| error.to_string())?;
    Ok(Frame {
        width: size.width,
        height: size.height,
        stride,
        rgba,
        at,
    })
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.state.closed.store(true, Ordering::Release);
        unsafe {
            self.output.setSampleBufferDelegate_queue(None, None);
            self.session.stopRunning();
        }
        // The capture owner is never the callback queue. Drain already queued callbacks
        // before releasing the delegate; their closed flag prevents any further delivery.
        self._queue.exec_sync(|| {});
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::{Compositor, RgbaOverlay};
    use snow_media::geometry::PixelSize;

    #[test]
    fn native_camera_copy_preserves_top_down_unmirrored_rgba_and_timestamp() {
        let pixels = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255, 255, 0, 255, 255, 0,
            255, 255, 255,
        ];
        let mut compositor =
            Compositor::new(PixelSize::new(2, 3).unwrap(), PixelFormat::Bgra8, 2).unwrap();
        let native = compositor
            .compose_with_overlays(
                &[],
                &[RgbaOverlay {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 3,
                    stride: 8,
                    bytes: &pixels,
                }],
                true,
            )
            .unwrap();
        let at = Instant::now();
        let frame = rgba_frame(&native, at).unwrap();
        assert_eq!(
            (frame.width, frame.height, frame.stride, frame.at),
            (2, 3, 8, at)
        );
        assert_eq!(frame.rgba, pixels);
    }

    #[test]
    fn invalid_capture_options_are_rejected_before_requesting_hardware() {
        for (size, fps) in [(49, 30), (501, 30), (150, 0), (150, 31)] {
            assert!(Stream::open(size, fps).is_err());
        }
    }
}
