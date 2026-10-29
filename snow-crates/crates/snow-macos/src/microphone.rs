//! CoreAudio device selection and AVAudioEngine input taps with a fixed buffer pool.
use crate::{MacError, MacResult, audio::AudioSamples};
use block2::RcBlock;
use crossbeam_channel::Receiver;
use objc2::rc::Retained;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_avf_audio::{AVAudioPCMBuffer, AVAudioTime};
use objc2_core_audio::*;
use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_media::CMClock;
use objc2_foundation::NSError;
use snow_media::time::{ClockDomain, MediaTime};
use std::{
    ffi::c_void,
    ptr::NonNull,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct InputDevice {
    pub id: u32,
    pub uid: String,
    pub name: String,
    pub is_default: bool,
}
fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}
fn property<T: Copy + Default>(id: u32, selector: u32) -> MacResult<T> {
    let mut result = T::default();
    let mut size = size_of::<T>() as u32;
    let mut addr = address(selector, kAudioObjectPropertyScopeGlobal);
    let status = unsafe {
        AudioObjectGetPropertyData(
            id,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut result).cast(),
        )
    };
    if status != 0 || size != size_of::<T>() as u32 {
        return Err(MacError::TargetUnavailable);
    }
    Ok(result)
}
fn string_property(id: u32, selector: u32) -> MacResult<String> {
    let pointer: *mut CFString = property(id, selector)?;
    let pointer = NonNull::new(pointer).ok_or(MacError::TargetUnavailable)?;
    // CoreAudio transfers a retained CFString for name/UID properties.
    Ok(unsafe { CFRetained::from_raw(pointer) }.to_string())
}
pub fn input_devices() -> MacResult<Vec<InputDevice>> {
    let default: u32 = property(
        kAudioObjectSystemObject as u32,
        kAudioHardwarePropertyDefaultInputDevice,
    )?;
    let mut addr = address(
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    );
    let mut size = 0;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            kAudioObjectSystemObject as u32,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    if status != 0 || size % 4 != 0 {
        return Err(MacError::TargetUnavailable);
    }
    let mut ids = vec![0u32; size as usize / 4];
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as u32,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(ids.as_mut_ptr().cast()).unwrap(),
        )
    };
    if status != 0 || size as usize > ids.len() * 4 {
        return Err(MacError::TargetUnavailable);
    }
    let mut devices = Vec::new();
    for &id in &ids[..size as usize / 4] {
        let mut addr = address(kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput);
        let mut size = 0;
        let status = unsafe {
            AudioObjectGetPropertyDataSize(
                id,
                NonNull::from(&mut addr),
                0,
                std::ptr::null(),
                NonNull::from(&mut size),
            )
        };
        if status == 0 && size > 0 {
            devices.push(InputDevice {
                id,
                uid: string_property(id, kAudioDevicePropertyDeviceUID)?,
                name: string_property(id, kAudioObjectPropertyName)?,
                is_default: id == default,
            });
        }
    }
    Ok(devices)
}
pub fn microphone_authorized() -> bool {
    unsafe {
        AVCaptureDevice::authorizationStatusForMediaType(
            AVMediaTypeAudio.expect("AVMediaTypeAudio is available on macOS 15"),
        ) == AVAuthorizationStatus::Authorized
    }
}
/// Explicit asynchronous permission request. The host must provide microphone usage metadata.
pub fn request_microphone_access(completion: impl Fn(bool) + Send + 'static) {
    let block = RcBlock::new(move |granted: objc2::runtime::Bool| completion(granted.as_bool()));
    unsafe {
        AVCaptureDevice::requestAccessForMediaType_completionHandler(
            AVMediaTypeAudio.expect("AVMediaTypeAudio is available on macOS 15"),
            &block,
        );
    }
}

/// Called synchronously on the capture worker. Copy the UTF-8 message before
/// returning. The callback must remain valid for the process lifetime.
pub type MicrophoneDiagnosticCallback = extern "C" fn(*const u8, usize);
static DIAGNOSTICS: Mutex<Option<MicrophoneDiagnosticCallback>> = Mutex::new(None);

pub fn set_diagnostic_callback(callback: Option<MicrophoneDiagnosticCallback>) {
    *DIAGNOSTICS.lock().unwrap_or_else(|e| e.into_inner()) = callback;
}

fn report_native_error(error: &MacError) {
    let callback = *DIAGNOSTICS.lock().unwrap_or_else(|e| e.into_inner());
    let message = error.to_string();
    if let Some(callback) = callback {
        callback(message.as_ptr(), message.len());
    } else {
        eprintln!("{message}");
    }
}

#[repr(C)]
#[derive(Default)]
struct MicrophoneFormat {
    sample_rate: f64,
    channels: u32,
}

unsafe extern "C" {
    fn snow_microphone_create(
        device: u32,
        format: *mut MicrophoneFormat,
        error: *mut *mut NSError,
    ) -> *mut c_void;
    fn snow_microphone_start(
        capture: *mut c_void,
        tap: *const c_void,
        error: *mut *mut NSError,
    ) -> bool;
    fn snow_microphone_is_running(capture: *mut c_void, error: *mut *mut NSError) -> bool;
    fn snow_microphone_destroy(capture: *mut c_void, error: *mut *mut NSError);
}

// Owns the native engine and tracks its installed tap even if prepare/start
// fails. All throwing operations and cleanup stay inside the native boundary.
struct NativeCapture(NonNull<c_void>);

fn native_result(error: *mut NSError) -> MacResult<()> {
    // SAFETY: the bridge transfers a retained NSError, or null on success.
    let Some(error) = (unsafe { Retained::from_raw(error) }) else {
        return Ok(());
    };
    let error = MacError::from_native(&error);
    report_native_error(&error);
    Err(error)
}

impl NativeCapture {
    fn create(device: u32) -> MacResult<(Self, MicrophoneFormat)> {
        let mut format = MicrophoneFormat::default();
        let mut error = std::ptr::null_mut();
        let capture = unsafe { snow_microphone_create(device, &mut format, &mut error) };
        native_result(error)?;
        let capture = NonNull::new(capture).ok_or(MacError::TargetUnavailable)?;
        Ok((Self(capture), format))
    }

    fn start(&self, tap: *const c_void) -> MacResult<()> {
        let mut error = std::ptr::null_mut();
        unsafe { snow_microphone_start(self.0.as_ptr(), tap, &mut error) };
        native_result(error)
    }

    fn is_running(&self) -> bool {
        let mut error = std::ptr::null_mut();
        let running = unsafe { snow_microphone_is_running(self.0.as_ptr(), &mut error) };
        native_result(error).is_ok() && running
    }
}

impl Drop for NativeCapture {
    fn drop(&mut self) {
        let mut error = std::ptr::null_mut();
        unsafe { snow_microphone_destroy(self.0.as_ptr(), &mut error) };
        let _ = native_result(error);
    }
}

pub struct MicrophoneStream {
    uid: String,
    capture: NativeCapture,
    samples: Receiver<AudioSamples>,
    dropped: Arc<AtomicU64>,
}
// SAFETY: engine control has one owner; the callback communicates through bounded channels.
unsafe impl Send for MicrophoneStream {}
impl MicrophoneStream {
    pub fn start(uid: Option<&str>) -> MacResult<Self> {
        Self::start_cancelable(uid, &crate::CancellationToken::default())
    }
    pub fn start_cancelable(
        uid: Option<&str>,
        cancellation: &crate::CancellationToken,
    ) -> MacResult<Self> {
        if cancellation.is_canceled() {
            return Err(MacError::Canceled);
        }
        if !microphone_authorized() {
            return Err(MacError::MicrophonePermissionDenied);
        }
        let device = input_devices()?
            .into_iter()
            .find(|d| uid.map_or(d.is_default, |uid| d.uid == uid))
            .ok_or(MacError::TargetUnavailable)?;
        if cancellation.is_canceled() {
            return Err(MacError::Canceled);
        }
        let (capture, format) = NativeCapture::create(device.id)?;
        let channels = format.channels as usize;
        let rate = format.sample_rate;
        let (sender, samples) = crossbeam_channel::bounded(8);
        let (recycle, available) = crossbeam_channel::bounded(8);
        for _ in 0..8 {
            let _ = recycle.try_send(Vec::<f32>::with_capacity(8192 * channels));
        }
        let dropped = Arc::new(AtomicU64::new(0));
        let loss = dropped.clone();
        let tap = RcBlock::new(
            move |buffer: NonNull<AVAudioPCMBuffer>, when: NonNull<AVAudioTime>| unsafe {
                let buffer = buffer.as_ref();
                let when = when.as_ref();
                let frames = buffer.frameLength() as usize;
                let ptr = buffer.floatChannelData();
                if ptr.is_null()
                    || frames > 8192
                    || !when.isHostTimeValid()
                    || buffer.format().channelCount() as usize != channels
                    || buffer.format().sampleRate() != rate
                {
                    loss.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                let Ok(mut data) = available.try_recv() else {
                    loss.fetch_add(1, Ordering::Relaxed);
                    return;
                };
                let stride = buffer.stride();
                data.clear();
                for frame in 0..frames {
                    for channel in 0..channels {
                        data.push(*(*ptr.add(channel)).as_ptr().add(frame * stride));
                    }
                }
                let time = CMClock::make_host_time_from_system_units(when.hostTime());
                let sample = AudioSamples {
                    data,
                    channels: channels as u16,
                    sample_rate: rate as u32,
                    timestamp: MediaTime {
                        value: time.value,
                        timescale: time.timescale as u32,
                        domain: ClockDomain::MacHostTime,
                        epoch: time.epoch,
                    },
                    discontinuity: false,
                    recycle: Some(recycle.clone()),
                };
                if sender.try_send(sample).is_err() {
                    loss.fetch_add(1, Ordering::Relaxed);
                }
            },
        );
        let result = Self {
            uid: device.uid,
            capture,
            samples,
            dropped,
        };
        if cancellation.is_canceled() {
            return Err(MacError::Canceled);
        }
        result.capture.start(RcBlock::as_ptr(&tap).cast())?;
        if cancellation.is_canceled() {
            return Err(MacError::Canceled);
        }
        Ok(result)
    }
    pub fn device_uid(&self) -> &str {
        &self.uid
    }
    pub fn is_running(&self) -> bool {
        self.capture.is_running()
    }
    pub fn next_samples(&self, timeout: Duration) -> MacResult<AudioSamples> {
        let mut result = self
            .samples
            .recv_timeout(timeout)
            .map_err(|_| MacError::Timeout)?;
        result.discontinuity = self.dropped.swap(0, Ordering::AcqRel) != 0;
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn microphone_native_reason_reaches_diagnostics_and_error() {
        use objc2::rc::Retained;
        use objc2_foundation::{NSDictionary, NSError, NSLocalizedDescriptionKey, NSString};
        static MESSAGES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        extern "C" fn receive(bytes: *const u8, length: usize) {
            let message = unsafe { std::slice::from_raw_parts(bytes, length) };
            MESSAGES
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(message).into_owned());
            // The diagnostic callback runs outside the registration lock.
            super::set_diagnostic_callback(None);
        }
        let reason =
            NSString::from_str("microphone install-tap: hardware format — 音频; device=123");
        let reason_object: &objc2::runtime::AnyObject = &reason;
        let info =
            NSDictionary::from_slices(&[unsafe { NSLocalizedDescriptionKey }], &[reason_object]);
        let error = unsafe {
            NSError::errorWithDomain_code_userInfo(
                &NSString::from_str("com.apple.coreaudio.avfaudio"),
                0,
                Some(&info),
            )
        };
        super::set_diagnostic_callback(Some(receive));
        let result = super::native_result(Retained::into_raw(error));
        let message = result.unwrap_err().to_string();
        assert!(message.contains("com.apple.coreaudio.avfaudio"));
        assert!(message.contains("install-tap: hardware format — 音频; device=123"));
        assert_eq!(*MESSAGES.lock().unwrap(), [message]);
    }

    #[test]
    fn canceled_microphone_start_never_checks_permission_or_opens_a_device() {
        let cancellation = crate::CancellationToken::default();
        cancellation.cancel();
        assert!(matches!(
            super::MicrophoneStream::start_cancelable(None, &cancellation),
            Err(crate::MacError::Canceled)
        ));
    }
}
