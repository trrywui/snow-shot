use crate::{CameraBackend, CaptureOptions, RawFrame};
use snow_macos::webcam;
use std::time::Duration;

struct Camera {
    options: CaptureOptions,
    stream: Option<webcam::Stream>,
    permission: Option<crossbeam_channel::Receiver<bool>>,
}
pub fn open(options: CaptureOptions) -> Result<Box<dyn CameraBackend>, String> {
    match webcam::authorization() {
        Some(false) => {
            Err("Camera access was denied. Enable camera access in System Settings.".into())
        }
        Some(true) => Ok(Box::new(Camera {
            options,
            stream: Some(webcam::Stream::open(options.size, options.fps)?),
            permission: None,
        })),
        None => Ok(Box::new(Camera {
            options,
            stream: None,
            permission: Some(webcam::request_access()),
        })),
    }
}
impl CameraBackend for Camera {
    fn starting(&self) -> bool {
        self.permission.is_some()
    }
    fn poll(&mut self, timeout: Duration) -> Result<Option<RawFrame>, String> {
        if let Some(permission) = &self.permission {
            match permission.recv_timeout(timeout) {
                Ok(true) => {
                    self.permission = None;
                    self.stream = Some(webcam::Stream::open(self.options.size, self.options.fps)?);
                }
                Ok(false) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    return Err(
                        "Camera access was denied. Enable camera access in System Settings.".into(),
                    );
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => return Ok(None),
            }
        }
        self.stream.as_ref().unwrap().poll(timeout).map(|frame| {
            frame.map(|frame| RawFrame {
                width: frame.width,
                height: frame.height,
                stride: frame.stride,
                rgba: frame.rgba,
                at: frame.at,
            })
        })
    }
}
