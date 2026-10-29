//! Refcounted raster ownership for swscale's bounded row workers, without an input copy.
use std::sync::Arc;

use crate::error::{RecordingExportError, Result};
use ffmpeg_next as ffmpeg;
use snow_memory::RasterBuffer;

pub(crate) struct FrameConverter {
    context: *mut ffmpeg::ffi::SwsContext,
    input: ffmpeg::frame::Video,
    width: u32,
    height: u32,
    output_format: ffmpeg::format::Pixel,
}

impl Drop for FrameConverter {
    fn drop(&mut self) {
        // SAFETY: this converter exclusively owns the context, including failed initialization.
        unsafe { ffmpeg::ffi::sws_freeContext(self.context) };
    }
}

unsafe extern "C" fn release_raster(opaque: *mut std::ffi::c_void, _: *mut u8) {
    // SAFETY: bind_input transfers one Arc into the AVBuffer. FFmpeg invokes
    // this callback exactly once, after the last AVBufferRef is released.
    drop(unsafe { Arc::from_raw(opaque.cast::<RasterBuffer>()) });
}

impl FrameConverter {
    pub(crate) fn use_bt709(&mut self) -> Result<()> {
        unsafe { crate::streaming::configure_bt709_scaler(self.context) }
    }

    pub(crate) fn thread_count(&self) -> Option<usize> {
        // SAFETY: this converter owns its initialized context.
        unsafe { crate::streaming::swscale_thread_count(self.context) }
    }

    pub(crate) fn new(
        width: u32,
        height: u32,
        pixel: ffmpeg::format::Pixel,
        threads: u8,
    ) -> Result<Self> {
        if width == 0
            || height == 0
            || width > i32::MAX as u32 / 4
            || height > i32::MAX as u32
            || !(1..=4).contains(&threads)
        {
            return Err(RecordingExportError::InvalidConfig(
                "invalid frame converter geometry or worker count".into(),
            ));
        }
        // SAFETY: all options are initialized before use; Drop releases the context on error.
        unsafe {
            let context = ffmpeg::ffi::sws_alloc_context();
            if context.is_null() {
                return Err(RecordingExportError::Encode(
                    "failed to allocate frame converter".into(),
                ));
            }
            let result = Self {
                context,
                input: ffmpeg::frame::Video::empty(),
                width,
                height,
                output_format: pixel,
            };
            let rgba: ffmpeg::ffi::AVPixelFormat = ffmpeg::format::Pixel::RGBA.into();
            let output: ffmpeg::ffi::AVPixelFormat = pixel.into();
            for (key, value) in [
                (c"srcw", i64::from(width)),
                (c"srch", i64::from(height)),
                (c"dstw", i64::from(width)),
                (c"dsth", i64::from(height)),
                (c"src_format", rgba as i64),
                (c"dst_format", output as i64),
                (
                    c"sws_flags",
                    i64::from(ffmpeg::software::scaling::Flags::BICUBIC.bits()),
                ),
                (c"threads", i64::from(threads)),
            ] {
                let code = ffmpeg::ffi::av_opt_set_int(context.cast(), key.as_ptr(), value, 0);
                if code < 0 {
                    return Err(RecordingExportError::Encode(format!(
                        "frame converter option {key:?}: {}",
                        ffmpeg::Error::from(code)
                    )));
                }
            }
            let code =
                ffmpeg::ffi::sws_init_context(context, std::ptr::null_mut(), std::ptr::null_mut());
            if code < 0 {
                return Err(RecordingExportError::Encode(format!(
                    "frame converter initialization: {}",
                    ffmpeg::Error::from(code)
                )));
            }
            Ok(result)
        }
    }

    fn bind_input(&mut self, pixels: Arc<RasterBuffer>) -> Result<()> {
        let data = pixels.as_ptr().cast_mut();
        let len = pixels.len();
        let opaque = Arc::into_raw(pixels).cast_mut().cast();
        // SAFETY: the AVBuffer owns the raster through its Arc callback. Its
        // initialized SIMD padding is part of len. READONLY prevents FFmpeg
        // from treating this shared input as writable storage.
        unsafe {
            let buffer = ffmpeg::ffi::av_buffer_create(
                data,
                len,
                Some(release_raster),
                opaque,
                ffmpeg::ffi::AV_BUFFER_FLAG_READONLY,
            );
            if buffer.is_null() {
                release_raster(opaque, data);
                return Err(RecordingExportError::Encode(
                    "failed to retain conversion raster".into(),
                ));
            }
            ffmpeg::ffi::av_frame_unref(self.input.as_mut_ptr());
            let frame = &mut *self.input.as_mut_ptr();
            frame.format = ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_RGBA as i32;
            frame.width = self.width as i32;
            frame.height = self.height as i32;
            frame.buf[0] = buffer;
            frame.data[0] = data;
            frame.linesize[0] = (self.width * 4) as i32;
            frame.extended_data = frame.data.as_mut_ptr();
        }
        Ok(())
    }

    pub(crate) fn convert(
        &mut self,
        pixels: RasterBuffer,
        output: &mut ffmpeg::frame::Video,
    ) -> Result<RasterBuffer> {
        let required = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|n| n.checked_mul(4))
            .and_then(|n| n.checked_add(ffmpeg::ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize));
        if required.is_none_or(|required| pixels.len() < required)
            || output.format() != self.output_format
            || (output.width(), output.height()) != (self.width, self.height)
        {
            return Err(RecordingExportError::InvalidConfig(
                "invalid owned frame conversion layout".into(),
            ));
        }
        let owner = Arc::new(pixels);
        self.bind_input(Arc::clone(&owner))?;
        // SAFETY: source and destination both own refcounted buffers. The
        // explicitly initialized context retains the legacy color semantics.
        // Conversion joins its slice workers before returning. frame_end also
        // releases a source retained by a partially failed sws_frame_start;
        // sws_scale_frame's early-error path does not perform that cleanup.
        let code = unsafe {
            let code = ffmpeg::ffi::sws_scale_frame(
                self.context,
                output.as_mut_ptr(),
                self.input.as_ptr(),
            );
            ffmpeg::ffi::sws_frame_end(self.context);
            ffmpeg::ffi::av_frame_unref(self.input.as_mut_ptr());
            code
        };
        if code < 0 {
            return Err(RecordingExportError::Encode(format!(
                "frame conversion: {}",
                ffmpeg::Error::from(code)
            )));
        }
        // Never recycle storage while a foreign reference could still read it.
        Arc::try_unwrap(owner).map_err(|_| {
            RecordingExportError::Encode(
                "frame converter retained its input after completion".into(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels(width: usize, height: usize) -> RasterBuffer {
        let mut pixels = RasterBuffer::from(vec![123; width * height * 4]);
        pixels.resize(
            pixels.len() + ffmpeg::ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize,
            0,
        );
        pixels
    }

    #[test]
    fn conversion_recycles_exact_storage_and_releases_frame_references() {
        let mut converter =
            FrameConverter::new(640, 480, ffmpeg::format::Pixel::YUV420P, 4).unwrap();
        let mut output = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::YUV420P, 640, 480);
        let mut raster = pixels(640, 480);
        assert!(raster.is_page_backed());
        let address = raster.as_ptr();
        for value in [123, 80, 20] {
            raster[..640 * 480 * 4].fill(value);
            raster = converter.convert(raster, &mut output).unwrap();
            assert_eq!(raster.as_ptr(), address);
            assert!(raster[..640 * 480 * 4].iter().all(|&v| v == value));
            assert!(raster[640 * 480 * 4..].iter().all(|&v| v == 0));
            assert!(unsafe { (*converter.input.as_ptr()).buf[0].is_null() });
        }
    }

    #[test]
    fn foreign_frame_reference_keeps_raster_alive_after_converter_is_dropped() {
        let mut converter = FrameConverter::new(19, 13, ffmpeg::format::Pixel::NV12, 2).unwrap();
        let owner = Arc::new(pixels(19, 13));
        let weak = Arc::downgrade(&owner);
        let address = owner.as_ptr();
        converter.bind_input(owner).unwrap();
        let mut retained = ffmpeg::frame::Video::empty();
        unsafe {
            assert_eq!(
                ffmpeg::ffi::av_frame_ref(retained.as_mut_ptr(), converter.input.as_ptr()),
                0
            );
            assert_eq!(
                ffmpeg::ffi::av_frame_is_writable(converter.input.as_mut_ptr()),
                0
            );
        }
        drop(converter);
        assert!(weak.upgrade().is_some());
        assert_eq!(retained.data(0).as_ptr(), address);
        assert!(retained.data(0).iter().all(|&v| v == 123));
        drop(retained);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn invalid_conversion_layout_does_not_poison_the_next_frame() {
        let mut converter = FrameConverter::new(19, 13, ffmpeg::format::Pixel::NV12, 2).unwrap();
        let mut output = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::NV12, 19, 13);
        assert!(
            converter
                .convert(RasterBuffer::zeroed(19 * 13 * 4), &mut output)
                .is_err()
        );
        let mut wrong = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::RGBA, 19, 13);
        assert!(converter.convert(pixels(19, 13), &mut wrong).is_err());
        converter.convert(pixels(19, 13), &mut output).unwrap();
    }
}
