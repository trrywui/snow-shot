use crate::{CameraBackend, CaptureOptions, RawFrame};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::time::{Duration, Instant};
use windows::Devices::Enumeration::{DeviceClass, DeviceInformation};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{
    COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::core::{HRESULT, Interface, PWSTR, Ref, Result as WinResult, implement};

struct Runtime;
impl Runtime {
    fn new() -> WinResult<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                CoUninitialize();
                return Err(error);
            }
        }
        Ok(Self)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

struct Sample {
    sample: Option<IMFSample>,
    at: Instant,
    flags: u32,
}
// Media Foundation samples are free-threaded. Only the camera worker reads their buffers.
unsafe impl Send for Sample {}
#[implement(IMFSourceReaderCallback)]
struct Callback {
    sender: Sender<Result<Sample, String>>,
}
impl IMFSourceReaderCallback_Impl for Callback_Impl {
    fn OnReadSample(
        &self,
        status: HRESULT,
        _: u32,
        flags: u32,
        _: i64,
        sample: Ref<IMFSample>,
    ) -> WinResult<()> {
        let result = status
            .ok()
            .map(|_| Sample {
                sample: sample.as_ref().cloned(),
                at: Instant::now(),
                flags,
            })
            .map_err(|e| e.to_string());
        let _ = self.sender.try_send(result);
        Ok(())
    }
    fn OnFlush(&self, _: u32) -> WinResult<()> {
        Ok(())
    }
    fn OnEvent(&self, _: u32, _: Ref<IMFMediaEvent>) -> WinResult<()> {
        Ok(())
    }
}

struct Camera {
    reader: IMFSourceReader,
    source: IMFMediaSource,
    receiver: Receiver<Result<Sample, String>>,
    width: u32,
    height: u32,
    stride: i32,
    _runtime: Option<Runtime>,
}
impl Drop for Camera {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .reader
                .Flush(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32);
            let _ = self.source.Shutdown();
        }
    }
}

pub fn open(options: CaptureOptions) -> Result<Box<dyn CameraBackend>, String> {
    unsafe { open_inner(options) }
        .map(|camera| Box::new(camera) as Box<dyn CameraBackend>)
        .map_err(|e| format!("Cannot open the webcam: {e}"))
}

unsafe fn open_inner(options: CaptureOptions) -> WinResult<Camera> {
    let runtime = Runtime::new()?;
    let mut attributes = None;
    unsafe {
        MFCreateAttributes(&mut attributes, 2)?;
    }
    let attributes = attributes.unwrap();
    unsafe {
        attributes.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )?;
    }
    let mut pointer = std::ptr::null_mut();
    let mut count = 0;
    unsafe {
        MFEnumDeviceSources(&attributes, &mut pointer, &mut count)?;
    }
    let devices = if count == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(pointer, count as usize) }.to_vec()
    };
    if !pointer.is_null() {
        for index in 0..count as usize {
            unsafe {
                std::ptr::drop_in_place(pointer.add(index));
            }
        }
        unsafe {
            CoTaskMemFree(Some(pointer.cast()));
        }
    }
    let default = DeviceInformation::FindAllAsyncDeviceClass(DeviceClass::VideoCapture)
        .and_then(|task| task.join())
        .ok()
        .and_then(|list| {
            (0..list.Size().ok()?)
                .filter_map(|i| list.GetAt(i).ok())
                .find(|d| d.IsDefault().unwrap_or(false))
                .and_then(|d| d.Id().ok())
                .map(|id| id.to_string().to_lowercase())
        });
    let selected = default
        .as_ref()
        .and_then(|default| {
            devices.iter().flatten().find(|device| {
                let mut text = PWSTR::null();
                let mut length = 0;
                if unsafe {
                    device.GetAllocatedString(
                        &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                        &mut text,
                        &mut length,
                    )
                }
                .is_err()
                {
                    return false;
                }
                let id = unsafe { text.to_string() }.unwrap_or_default();
                unsafe {
                    CoTaskMemFree(Some(text.0.cast()));
                }
                id.to_lowercase() == *default
            })
        })
        .or_else(|| devices.iter().flatten().next())
        .ok_or_else(|| windows::core::Error::from_hresult(HRESULT::from_win32(1168)))?;
    let source: IMFMediaSource = unsafe { selected.ActivateObject()? };
    // Any subsequent failure must still shut the activated media source down.
    match unsafe { configure(&source, options) } {
        Ok(mut camera) => {
            camera._runtime = Some(runtime);
            Ok(camera)
        }
        Err(error) => {
            unsafe {
                let _ = source.Shutdown();
            }
            Err(error)
        }
    }
}

unsafe fn configure(source: &IMFMediaSource, options: CaptureOptions) -> WinResult<Camera> {
    let (sender, receiver) = bounded(1);
    let callback: IMFSourceReaderCallback = Callback { sender }.into();
    let mut attributes = None;
    unsafe {
        MFCreateAttributes(&mut attributes, 3)?;
    }
    let attributes = attributes.unwrap();
    unsafe {
        attributes.SetUnknown(&MF_SOURCE_READER_ASYNC_CALLBACK, &callback)?;
        attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        attributes.SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, 0)?;
    }
    let reader = unsafe { MFCreateSourceReaderFromMediaSource(source, &attributes)? };
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mut best = None;
    let mut best_score = u64::MAX;
    for index in 0..1024 {
        let Ok(media) = (unsafe { reader.GetNativeMediaType(stream, index) }) else {
            break;
        };
        let Ok(size) = (unsafe { media.GetUINT64(&MF_MT_FRAME_SIZE) }) else {
            continue;
        };
        let (width, height) = ((size >> 32) as u32, size as u32);
        if width == 0 || height == 0 || width > 8192 || height > 8192 {
            continue;
        }
        let rate = unsafe { media.GetUINT64(&MF_MT_FRAME_RATE) }.unwrap_or(30u64 << 32 | 1);
        let fps = (rate >> 32) as f64 / f64::from((rate as u32).max(1));
        let short = width.min(height);
        let score = u64::from(short < options.size) * 10_000_000_000
            + u64::from(fps + 0.5 < f64::from(options.fps)) * 1_000_000_000
            + if short < options.size {
                100_000_000 - u64::from(width) * u64::from(height)
            } else {
                u64::from(width) * u64::from(height)
            }
            + ((fps - f64::from(options.fps)).abs() * 100.0) as u64;
        if score < best_score {
            best_score = score;
            best = Some(media);
        }
    }
    if let Some(media) = best {
        unsafe {
            reader.SetCurrentMediaType(stream, None, &media)?;
        }
    }
    let output = unsafe { MFCreateMediaType()? };
    unsafe {
        output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        reader.SetCurrentMediaType(stream, None, &output)?;
    }
    let output = unsafe { reader.GetCurrentMediaType(stream)? };
    let size = unsafe { output.GetUINT64(&MF_MT_FRAME_SIZE)? };
    let (width, height) = ((size >> 32) as u32, size as u32);
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(windows::core::Error::from_hresult(
            windows::Win32::Foundation::E_INVALIDARG,
        ));
    }
    let stride =
        unsafe { output.GetUINT32(&MF_MT_DEFAULT_STRIDE) }.map_or((width * 4) as i32, |v| v as i32);
    if stride.unsigned_abs() < width * 4 {
        return Err(windows::core::Error::from_hresult(
            windows::Win32::Foundation::E_INVALIDARG,
        ));
    }
    unsafe {
        reader.ReadSample(stream, 0, None, None, None, None)?;
    }
    Ok(Camera {
        reader,
        source: source.clone(),
        receiver,
        width,
        height,
        stride,
        _runtime: None,
    })
}

impl CameraBackend for Camera {
    fn poll(&mut self, timeout: Duration) -> Result<Option<RawFrame>, String> {
        let sample = match self.receiver.recv_timeout(timeout) {
            Ok(sample) => sample?,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if sample.flags
            & (MF_SOURCE_READERF_ERROR.0 as u32 | MF_SOURCE_READERF_ENDOFSTREAM.0 as u32)
            != 0
        {
            return Err("The webcam disconnected.".into());
        }
        if sample.flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
            return Err(
                "The webcam video format changed. Enable the webcam again to reconnect.".into(),
            );
        }
        let result = sample
            .sample
            .as_ref()
            .map(|frame| unsafe { self.copy_frame(frame, sample.at) })
            .transpose()
            .map_err(|e| e.to_string());
        unsafe {
            self.reader.ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                None,
                None,
                None,
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        result
    }
}
impl Camera {
    unsafe fn copy_frame(&self, sample: &IMFSample, at: Instant) -> WinResult<RawFrame> {
        let buffer = unsafe { sample.ConvertToContiguousBuffer()? };
        let mut rgba = vec![255; self.width as usize * self.height as usize * 4];
        let mut pointer = std::ptr::null_mut();
        if let Ok(two) = buffer.cast::<IMF2DBuffer>() {
            let mut stride = 0;
            unsafe {
                two.Lock2D(&mut pointer, &mut stride)?;
                if pointer.is_null() || stride.unsigned_abs() < self.width * 4 {
                    two.Unlock2D()?;
                    return Err(windows::core::Error::from_hresult(
                        windows::Win32::Foundation::E_INVALIDARG,
                    ));
                }
                copy_rows(pointer, stride, self.width, self.height, &mut rgba);
                two.Unlock2D()?;
            }
        } else {
            let mut len = 0;
            unsafe {
                buffer.Lock(&mut pointer, None, Some(&mut len))?;
            }
            if pointer.is_null()
                || (self.stride.unsigned_abs() as usize).saturating_mul(self.height as usize)
                    > len as usize
            {
                unsafe {
                    buffer.Unlock()?;
                }
                return Err(windows::core::Error::from_hresult(
                    windows::Win32::Foundation::E_INVALIDARG,
                ));
            }
            if self.stride < 0 {
                pointer = unsafe {
                    pointer.add(self.stride.unsigned_abs() as usize * (self.height as usize - 1))
                };
            }
            unsafe {
                copy_rows(pointer, self.stride, self.width, self.height, &mut rgba);
                buffer.Unlock()?;
            }
        }
        Ok(RawFrame {
            width: self.width,
            height: self.height,
            stride: self.width as usize * 4,
            rgba,
            at,
        })
    }
}
unsafe fn copy_rows(pointer: *const u8, stride: i32, width: u32, height: u32, output: &mut [u8]) {
    for y in 0..height as usize {
        let row = unsafe {
            std::slice::from_raw_parts(
                pointer.offset(y as isize * stride as isize),
                width as usize * 4,
            )
        };
        for (from, to) in row
            .chunks_exact(4)
            .zip(output[y * width as usize * 4..][..width as usize * 4].chunks_exact_mut(4))
        {
            to.copy_from_slice(&[from[2], from[1], from[0], 255]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_camera_pitch_preserves_top_down_unmirrored_orientation() {
        let top_down = [
            10, 20, 30, 0, 40, 50, 60, 0, 70, 80, 90, 0, 100, 110, 120, 0,
        ];
        let bottom_up = [
            70, 80, 90, 0, 100, 110, 120, 0, 10, 20, 30, 0, 40, 50, 60, 0,
        ];
        let expected = [
            30, 20, 10, 255, 60, 50, 40, 255, 90, 80, 70, 255, 120, 110, 100, 255,
        ];
        let mut rgba = [0; 16];
        unsafe {
            copy_rows(top_down.as_ptr(), 8, 2, 2, &mut rgba);
        }
        assert_eq!(rgba, expected);
        unsafe {
            copy_rows(bottom_up.as_ptr().add(8), -8, 2, 2, &mut rgba);
        }
        assert_eq!(rgba, expected);
    }
}
