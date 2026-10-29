use super::*;
use std::{
    ffi::{c_char, c_int, c_void},
    ptr::NonNull,
    sync::Arc,
};

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Desc {
    width: u32,
    height: u32,
    sample: u32,
    significant_bits: u32,
    transfer: u32,
    primaries: u32,
    orientation: u32,
    has_alpha: u32,
    animated: u32,
    frame_count: u32,
    loop_count: u32,
    hdr_reference_white_nits: f32,
}
#[repr(C)]
struct Decode {
    max_output_bytes: u64,
    max_memory_bytes: u64,
    max_metadata_bytes: u64,
    target_width: u32,
    target_height: u32,
    threads: u32,
}
impl From<DecodeOptions> for Decode {
    fn from(o: DecodeOptions) -> Self {
        Self {
            max_output_bytes: o.max_output_bytes,
            max_memory_bytes: o.max_memory_bytes,
            max_metadata_bytes: o.max_metadata_bytes,
            target_width: o.target_width,
            target_height: o.target_height,
            threads: o.threads,
        }
    }
}
#[repr(C)]
struct Encode {
    quality: f32,
    lossless: u32,
    effort: u32,
    exact: u32,
    threads: u32,
    max_output_bytes: u64,
    max_memory_bytes: u64,
    jpeg_subsampling: u32,
    progressive: u32,
    distance: f32,
    alpha_quality: f32,
    speed: u32,
    bit_depth: u32,
}
impl From<EncodeOptions> for Encode {
    fn from(o: EncodeOptions) -> Self {
        Self {
            quality: o.quality,
            lossless: o.lossless.into(),
            effort: o.effort,
            exact: o.exact.into(),
            threads: o.threads,
            max_output_bytes: o.max_output_bytes,
            max_memory_bytes: o.max_memory_bytes,
            jpeg_subsampling: o.jpeg_subsampling,
            progressive: o.progressive.into(),
            distance: o.distance,
            alpha_quality: o.alpha_quality,
            speed: o.speed,
            bit_depth: o.bit_depth.into(),
        }
    }
}
#[repr(C)]
struct Failure {
    code: u32,
    message: [c_char; 256],
}
impl Default for Failure {
    fn default() -> Self {
        Self {
            code: 0,
            message: [0; 256],
        }
    }
}
impl Failure {
    fn result(self, okay: c_int) -> Result<()> {
        if okay == 1 {
            return Ok(());
        }
        let end = self.message.iter().position(|&b| b == 0).unwrap_or(256);
        let message = String::from_utf8_lossy(
            &self.message[..end]
                .iter()
                .map(|&b| b as u8)
                .collect::<Vec<_>>(),
        )
        .into_owned();
        Err(match self.code {
            1 => Error::InvalidInput(message),
            2 => Error::Unsupported(message),
            3 => Error::ResourceLimit(message),
            _ => Error::Codec(message),
        })
    }
}
#[repr(C)]
struct Buffers {
    context: *mut c_void,
    allocate: unsafe extern "C" fn(*mut c_void, u32, usize) -> *mut u8,
}
type WriteCallback = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> c_int;
type RegionCallback = unsafe extern "C" fn(*mut c_void, u32, u32, *const u8, usize) -> c_int;
type RowCallback = unsafe extern "C" fn(*mut c_void, u32, *mut u8, usize) -> c_int;
#[repr(C)]
#[derive(Default)]
struct ResizeInfo {
    scratch_bytes: u64,
    input_buffer_rows: u32,
    output_buffer_rows: u32,
    row_buffer_bytes: u64,
}
unsafe extern "C" {
    fn snow_native_rgb_profile(
        white: *const f64,
        primaries: *const f64,
        gamma: *const f64,
        maximum: u64,
        buffers: Buffers,
        error: *mut Failure,
    ) -> c_int;
    fn snow_native_decode(
        format: u32,
        data: *const u8,
        size: usize,
        options: *const Decode,
        inspect: c_int,
        desc: *mut Desc,
        buffers: Buffers,
        error: *mut Failure,
    ) -> c_int;
    fn snow_native_encode(
        format: u32,
        desc: *const Desc,
        pixels: *const u8,
        size: usize,
        icc: *const u8,
        icc_size: usize,
        options: *const Encode,
        write: WriteCallback,
        context: *mut c_void,
        error: *mut Failure,
    ) -> c_int;
    fn snow_jxl_encode_rows(
        desc: *const Desc,
        icc: *const u8,
        icc_size: usize,
        options: *const Encode,
        read: RowCallback,
        read_context: *mut c_void,
        write: WriteCallback,
        write_context: *mut c_void,
        error: *mut Failure,
    ) -> c_int;
    fn snow_jpeg_encode_rows(
        desc: *const Desc,
        options: *const Encode,
        read: RowCallback,
        read_context: *mut c_void,
        write: WriteCallback,
        write_context: *mut c_void,
        error: *mut Failure,
    ) -> c_int;
    fn snow_native_transform(
        icc: *const u8,
        icc_size: usize,
        pixels: *mut c_void,
        count: usize,
        sample: u32,
        linear: u32,
        error: *mut Failure,
    ) -> c_int;
    fn snow_native_resize_query(
        w: u32,
        h: u32,
        ow: u32,
        oh: u32,
        info: *mut ResizeInfo,
        error: *mut Failure,
    ) -> c_int;
    fn snow_native_resize(
        input: *const f32,
        w: u32,
        h: u32,
        output: *mut f32,
        ow: u32,
        oh: u32,
        maximum: u64,
        error: *mut Failure,
    ) -> c_int;
    fn snow_native_resize_regions(
        w: u32,
        h: u32,
        ow: u32,
        oh: u32,
        maximum: u64,
        read: RowCallback,
        read_context: *mut c_void,
        write: RegionCallback,
        write_context: *mut c_void,
        error: *mut Failure,
    ) -> c_int;
    fn snow_webp_animation_open(
        data: *const u8,
        size: usize,
        options: *const Decode,
        desc: *mut Desc,
        error: *mut Failure,
    ) -> *mut c_void;
    fn snow_webp_animation_next(
        session: *mut c_void,
        buffers: Buffers,
        desc: *mut Desc,
        duration: *mut u32,
        error: *mut Failure,
    ) -> c_int;
    fn snow_webp_animation_reset(session: *mut c_void);
    fn snow_webp_animation_close(session: *mut c_void);
}

struct OutputBuffers {
    pixels: Vec<u8>,
    icc: Vec<u8>,
    exif: Vec<u8>,
    xmp: Vec<u8>,
    pixel_limit: u64,
    metadata_limit: u64,
    pixel_alignment: usize,
}
impl OutputBuffers {
    fn new(o: DecodeOptions) -> Self {
        Self {
            pixels: Vec::new(),
            icc: Vec::new(),
            exif: Vec::new(),
            xmp: Vec::new(),
            pixel_limit: o.max_output_bytes,
            metadata_limit: o.max_metadata_bytes,
            pixel_alignment: 1,
        }
    }
    fn ffi(&mut self) -> Buffers {
        Buffers {
            context: (self as *mut Self).cast(),
            allocate,
        }
    }
}
unsafe extern "C" fn allocate(context: *mut c_void, slot: u32, size: usize) -> *mut u8 {
    // SAFETY: C only calls synchronously with the exclusive context supplied by ffi().
    let b = unsafe { &mut *context.cast::<OutputBuffers>() };
    if slot != 0
        && size as u64
            > b.metadata_limit
                .saturating_sub((b.icc.len() + b.exif.len() + b.xmp.len()) as u64)
    {
        return std::ptr::null_mut();
    }
    let (buffer, max) = match slot {
        0 => (&mut b.pixels, b.pixel_limit),
        1 => (&mut b.icc, b.metadata_limit),
        2 => (&mut b.exif, b.metadata_limit),
        3 => (&mut b.xmp, b.metadata_limit),
        _ => return std::ptr::null_mut(),
    };
    if size as u64 > max || !buffer.is_empty() || buffer.try_reserve_exact(size).is_err() {
        return std::ptr::null_mut();
    }
    // Vec<u8> requests alignment 1, even when the system allocator happens to
    // return aligned addresses. AVIF high-depth and JXL float16 output perform
    // typed 16-bit stores, so refuse an unsuitable allocation before C sees it.
    if slot == 0 && !buffer.as_ptr().addr().is_multiple_of(b.pixel_alignment) {
        return std::ptr::null_mut();
    }
    buffer.resize(size, 0);
    buffer.as_mut_ptr()
}
fn sample(value: u32) -> Result<SampleType> {
    match value {
        0 => Ok(SampleType::U8),
        1 => Ok(SampleType::U16),
        2 => Ok(SampleType::F16),
        3 => Ok(SampleType::F32),
        _ => Err(Error::Codec("Invalid native sample description".into())),
    }
}
fn transfer(v: u32) -> TransferFunction {
    match v {
        1 => TransferFunction::Srgb,
        2 => TransferFunction::Linear,
        3 => TransferFunction::Pq,
        4 => TransferFunction::Hlg,
        _ => TransferFunction::Unknown,
    }
}
fn primaries(v: u32) -> ColorPrimaries {
    match v {
        1 => ColorPrimaries::Srgb,
        2 => ColorPrimaries::DisplayP3,
        3 => ColorPrimaries::Bt2020,
        _ => ColorPrimaries::Unknown,
    }
}
fn info(d: Desc, icc: Vec<u8>, exif: Vec<u8>, xmp: Vec<u8>) -> Result<NativeInfo> {
    if d.width == 0 || d.height == 0 || !(1..=8).contains(&d.orientation) {
        return Err(Error::Codec(
            "Invalid native image dimensions/orientation".into(),
        ));
    }
    Ok(NativeInfo {
        width: d.width,
        height: d.height,
        sample: sample(d.sample)?,
        significant_bits: d.significant_bits as u8,
        transfer: transfer(d.transfer),
        primaries: primaries(d.primaries),
        orientation: d.orientation as u8,
        hdr_reference_white_nits: d.hdr_reference_white_nits,
        has_alpha: d.has_alpha != 0,
        animated: d.animated != 0,
        frame_count: d.frame_count,
        loop_count: d.loop_count,
        icc,
        exif,
        xmp,
    })
}
fn image(d: Desc, b: OutputBuffers) -> Result<NativeImage> {
    let description = info(d, b.icc, b.exif, b.xmp)?;
    if byte_len(d.width, d.height, description.sample.bytes())? != b.pixels.len() {
        return Err(Error::Codec("Native output layout mismatch".into()));
    }
    Ok(NativeImage {
        width: d.width,
        height: d.height,
        sample: description.sample,
        pixels: b.pixels,
        icc: description.icc,
        exif: description.exif,
        xmp: description.xmp,
        significant_bits: description.significant_bits,
        transfer: description.transfer,
        primaries: description.primaries,
        orientation: description.orientation,
        hdr_reference_white_nits: description.hdr_reference_white_nits,
    })
}
fn byte_len(w: u32, h: u32, sample: usize) -> Result<usize> {
    if w == 0 || h == 0 {
        return Err(Error::InvalidInput(
            "Image dimensions must be nonzero".into(),
        ));
    }
    (w as usize)
        .checked_mul(h as usize)
        .and_then(|v| v.checked_mul(4))
        .and_then(|v| v.checked_mul(sample))
        .ok_or_else(|| Error::ResourceLimit("Image size overflow".into()))
}
fn decode_options(o: DecodeOptions) -> Result<()> {
    if o.threads == 0 || o.threads > 64 || o.max_output_bytes == 0 || o.max_memory_bytes == 0 {
        return Err(Error::InvalidInput("Invalid native decode limits".into()));
    }
    Ok(())
}
fn encode_options(format: NativeFormat, o: EncodeOptions) -> Result<()> {
    if !o.quality.is_finite()
        || !(0.0..=100.0).contains(&o.quality)
        || !o.alpha_quality.is_finite()
        || !(0.0..=100.0).contains(&o.alpha_quality)
        || !o.distance.is_finite()
        || o.distance < 0.0
        || o.threads == 0
        || o.threads > 64
        || o.max_output_bytes == 0
        || o.max_memory_bytes == 0
        || ![444, 422, 420].contains(&o.jpeg_subsampling)
    {
        return Err(Error::InvalidInput("Invalid native encode settings".into()));
    }
    if o.exact && (!o.lossless || format == NativeFormat::Jpeg) {
        return Err(Error::Unsupported(
            "Exact sample preservation requires a compatible lossless codec".into(),
        ));
    }
    if (format == NativeFormat::WebP && o.effort > 6)
        || (format == NativeFormat::Jxl && !(1..=10).contains(&o.effort))
        || (format == NativeFormat::Avif && o.speed > 10)
    {
        return Err(Error::InvalidInput(
            "Codec effort or speed is outside its supported range".into(),
        ));
    }
    Ok(())
}
pub fn inspect(format: NativeFormat, input: &[u8], options: DecodeOptions) -> Result<NativeInfo> {
    decode_options(options)?;
    let mut desc = Desc::default();
    let mut buffers = OutputBuffers::new(options);
    let mut failure = Failure::default();
    // SAFETY: all pointers remain live for this synchronous call; outputs are exclusively owned.
    let okay = unsafe {
        snow_native_decode(
            format as u32,
            input.as_ptr(),
            input.len(),
            &options.into(),
            1,
            &mut desc,
            buffers.ffi(),
            &mut failure,
        )
    };
    failure.result(okay)?;
    info(desc, buffers.icc, buffers.exif, buffers.xmp)
}
pub fn decode(format: NativeFormat, input: &[u8], options: DecodeOptions) -> Result<NativeImage> {
    decode_options(options)?;
    let mut desc = Desc::default();
    let mut buffers = OutputBuffers::new(options);
    if matches!(format, NativeFormat::Avif | NativeFormat::Jxl) {
        // These formats can return 16-bit samples. Their output sample type is
        // discovered inside the native call, before it invokes this allocator.
        buffers.pixel_alignment = std::mem::align_of::<u16>();
    }
    let mut failure = Failure::default();
    // SAFETY: same contract as inspect; allocation callback returns owned storage with checked bounds.
    let okay = unsafe {
        snow_native_decode(
            format as u32,
            input.as_ptr(),
            input.len(),
            &options.into(),
            0,
            &mut desc,
            buffers.ffi(),
            &mut failure,
        )
    };
    failure.result(okay)?;
    image(desc, buffers)
}

struct BorrowedJpegOutput<'a> {
    pixels: &'a mut [u8],
    allocated: bool,
    rejected_size: bool,
    metadata: OutputBuffers,
}
unsafe extern "C" fn allocate_borrowed_jpeg(
    context: *mut c_void,
    slot: u32,
    size: usize,
) -> *mut u8 {
    // SAFETY: decode_jpeg_into keeps this context and its exclusive destination
    // borrow alive for the synchronous JPEG call. No callback owns that borrow.
    let state = unsafe { &mut *context.cast::<BorrowedJpegOutput<'_>>() };
    if slot == 0 {
        if state.allocated || size == 0 || size != state.pixels.len() {
            state.rejected_size = true;
            return std::ptr::null_mut();
        }
        if size as u64 > state.metadata.pixel_limit {
            return std::ptr::null_mut();
        }
        state.allocated = true;
        return state.pixels.as_mut_ptr();
    }
    // SAFETY: metadata slots use the existing bounded, exclusively owned allocator;
    // slot zero was handled above and cannot allocate or free the borrowed pixels.
    unsafe {
        allocate(
            (&mut state.metadata as *mut OutputBuffers).cast(),
            slot,
            size,
        )
    }
}

/// Decode JPEG RGBA8 samples directly into caller-owned storage.
///
/// The destination must exactly match the selected native decode dimensions (including
/// reduced decoding). It is never reallocated or freed. On error its contents may be
/// partially overwritten and must not be published as a successful image.
pub fn decode_jpeg_into(
    input: &[u8],
    options: DecodeOptions,
    output: &mut [u8],
) -> Result<NativeInfo> {
    decode_options(options)?;
    if output.is_empty() {
        return Err(Error::InvalidInput(
            "JPEG destination must be nonempty".into(),
        ));
    }
    if output.len() as u64 > options.max_output_bytes {
        return Err(Error::ResourceLimit(
            "JPEG destination exceeds pixel budget".into(),
        ));
    }
    let mut state = BorrowedJpegOutput {
        pixels: output,
        allocated: false,
        rejected_size: false,
        metadata: OutputBuffers::new(options),
    };
    let mut desc = Desc::default();
    let mut failure = Failure::default();
    // SAFETY: the JPEG shim only uses synchronous callbacks, asks for the checked
    // pixel extent once, and never frees callback-owned memory. Its error jumps stay
    // entirely within C. No Rust slice is exposed until this call returns.
    let okay = unsafe {
        snow_native_decode(
            NativeFormat::Jpeg as u32,
            input.as_ptr(),
            input.len(),
            &options.into(),
            0,
            &mut desc,
            Buffers {
                context: (&mut state as *mut BorrowedJpegOutput<'_>).cast(),
                allocate: allocate_borrowed_jpeg,
            },
            &mut failure,
        )
    };
    if state.rejected_size {
        return Err(Error::InvalidInput(
            "JPEG destination length mismatch".into(),
        ));
    }
    failure.result(okay)?;
    if !state.allocated
        || desc.sample != 0
        || desc.significant_bits != 8
        || byte_len(desc.width, desc.height, 1)? != state.pixels.len()
    {
        return Err(Error::Codec("JPEG output layout mismatch".into()));
    }
    info(
        desc,
        state.metadata.icc,
        state.metadata.exif,
        state.metadata.xmp,
    )
}
struct Writer<'a> {
    callback: &'a mut dyn FnMut(&[u8]) -> Result<()>,
    error: Option<Error>,
}
unsafe extern "C" fn write(context: *mut c_void, data: *const u8, size: usize) -> c_int {
    // SAFETY: native shim provides a live byte slice during this callback only.
    let state = unsafe { &mut *context.cast::<Writer<'_>>() };
    // SAFETY: the shim provides initialized bytes valid for this synchronous callback.
    let bytes = unsafe { std::slice::from_raw_parts(data, size) };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (state.callback)(bytes))) {
        Ok(Ok(())) => 1,
        Ok(Err(e)) => {
            state.error = Some(e);
            0
        }
        Err(_) => {
            state.error = Some(Error::Codec("Output callback panicked".into()));
            0
        }
    }
}
fn desc(image: &NativeImage) -> Result<Desc> {
    if image.pixels.len() != byte_len(image.width, image.height, image.sample.bytes())?
        || !(1..=8).contains(&image.orientation)
        || image.icc.len() > u32::MAX as usize
    {
        return Err(Error::InvalidInput("Invalid native image layout".into()));
    }
    let max = match image.sample {
        SampleType::U8 => 8,
        SampleType::U16 => 16,
        SampleType::F16 => 16,
        SampleType::F32 => 32,
    };
    if image.significant_bits == 0 || image.significant_bits > max {
        return Err(Error::InvalidInput("Invalid significant bit depth".into()));
    }
    Ok(Desc {
        width: image.width,
        height: image.height,
        sample: image.sample as u32,
        significant_bits: image.significant_bits.into(),
        transfer: image.transfer as u32,
        primaries: image.primaries as u32,
        orientation: image.orientation.into(),
        hdr_reference_white_nits: image.hdr_reference_white_nits,
        has_alpha: 1,
        ..Default::default()
    })
}
pub fn encode_to_writer(
    format: NativeFormat,
    image: &NativeImage,
    options: EncodeOptions,
    output: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    encode_options(format, options)?;
    let desc = desc(image)?;
    if format == NativeFormat::Avif
        && image.sample == SampleType::U16
        && !image
            .pixels
            .as_ptr()
            .addr()
            .is_multiple_of(std::mem::align_of::<u16>())
    {
        return Err(Error::InvalidInput(
            "AVIF integer samples require 16-bit alignment".into(),
        ));
    }
    if !image.hdr_reference_white_nits.is_finite() || image.hdr_reference_white_nits < 0.0 {
        return Err(Error::InvalidInput("Invalid HDR reference white".into()));
    }
    if image.hdr_reference_white_nits > 0.0
        && (format != NativeFormat::Jxl
            || image.transfer != TransferFunction::Linear
            || image.primaries != ColorPrimaries::Srgb
            || !matches!(image.sample, SampleType::F16 | SampleType::F32))
    {
        return Err(Error::Unsupported(
            "Explicit scRGB requires floating-point linear-sRGB JXL".into(),
        ));
    }
    if options.exact {
        validate_exact_precision(&image.pixels, image.sample, image.significant_bits)?;
    }
    if options.exact && matches!(image.sample, SampleType::F16 | SampleType::F32) {
        return Err(Error::Unsupported(
            "Exact floating-point sample preservation is not guaranteed by native codecs".into(),
        ));
    }
    let mut writer = Writer {
        callback: output,
        error: None,
    };
    let mut failure = Failure::default();
    // SAFETY: checked input layout and exclusive callback context remain live for the entire call.
    let okay = unsafe {
        snow_native_encode(
            format as u32,
            &desc,
            image.pixels.as_ptr(),
            image.pixels.len(),
            image.icc.as_ptr(),
            image.icc.len(),
            &options.into(),
            write,
            (&mut writer as *mut Writer<'_>).cast(),
            &mut failure,
        )
    };
    if let Some(error) = writer.error {
        return Err(error);
    }
    failure.result(okay)
}
pub fn encode(
    format: NativeFormat,
    image: &NativeImage,
    options: EncodeOptions,
) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    encode_to_writer(format, image, options, &mut |chunk| {
        let required = output
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| Error::ResourceLimit("Encoded output overflow".into()))?;
        if required as u64 > options.max_output_bytes {
            return Err(Error::ResourceLimit("Encoded output exceeds limit".into()));
        }
        if required > output.capacity() {
            let capacity = required
                .max(output.capacity().saturating_mul(2))
                .min(usize::try_from(options.max_output_bytes).unwrap_or(usize::MAX));
            output
                .try_reserve_exact(capacity - output.len())
                .map_err(|_| Error::ResourceLimit("Encoded output allocation failed".into()))?;
        }
        output.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok(output)
}
struct ByteReader<'a> {
    callback: &'a mut dyn FnMut(u32, &mut [u8]) -> Result<()>,
    error: Option<Error>,
}
unsafe extern "C" fn read_bytes(
    context: *mut c_void,
    row: u32,
    data: *mut u8,
    size: usize,
) -> c_int {
    // SAFETY: the native shim owns a writable row of precisely size bytes for this callback.
    let state = unsafe { &mut *context.cast::<ByteReader<'_>>() };
    // SAFETY: data points to the shim-owned writable row with size initialized bytes.
    let pixels = unsafe { std::slice::from_raw_parts_mut(data, size) };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (state.callback)(row, pixels)
    })) {
        Ok(Ok(())) => 1,
        Ok(Err(e)) => {
            state.error = Some(e);
            0
        }
        Err(_) => {
            state.error = Some(Error::Codec("Input row callback panicked".into()));
            0
        }
    }
}
/// Stream opaque RGBA8 rows directly through libjpeg-turbo. Alpha must be 255 in every sample.
pub fn encode_jpeg_rows(
    width: u32,
    height: u32,
    options: EncodeOptions,
    read_row: &mut dyn FnMut(u32, &mut [u8]) -> Result<()>,
    output: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    byte_len(width, height, 1)?;
    encode_options(NativeFormat::Jpeg, options)?;
    let desc = Desc {
        width,
        height,
        sample: 0,
        significant_bits: 8,
        orientation: 1,
        transfer: 1,
        primaries: 1,
        ..Default::default()
    };
    let mut reader = ByteReader {
        callback: read_row,
        error: None,
    };
    let mut writer = Writer {
        callback: output,
        error: None,
    };
    let mut failure = Failure::default();
    // SAFETY: row/sink contexts are exclusive and synchronous; C contains all JPEG longjmps.
    let okay = unsafe {
        snow_jpeg_encode_rows(
            &desc,
            &options.into(),
            read_bytes,
            (&mut reader as *mut ByteReader<'_>).cast(),
            write,
            (&mut writer as *mut Writer<'_>).cast(),
            &mut failure,
        )
    };
    if let Some(error) = reader.error.or(writer.error) {
        return Err(error);
    }
    failure.result(okay)
}
fn transform_pixels(
    icc: &[u8],
    pixels: *mut c_void,
    count: usize,
    sample: u32,
    linear: bool,
) -> Result<()> {
    let mut failure = Failure::default();
    // SAFETY: callers provide a live, aligned, exclusive RGBA buffer of count pixels of sample type.
    let okay = unsafe {
        snow_native_transform(
            icc.as_ptr(),
            icc.len(),
            pixels,
            count,
            sample,
            linear.into(),
            &mut failure,
        )
    };
    failure.result(okay)
}
pub fn transform_rgba_f32(source_icc: &[u8], pixels: &mut [f32], linear_srgb: bool) -> Result<()> {
    if !pixels.len().is_multiple_of(4) {
        return Err(Error::InvalidInput("RGBA requires complete pixels".into()));
    }
    transform_pixels(
        source_icc,
        pixels.as_mut_ptr().cast(),
        pixels.len() / 4,
        3,
        linear_srgb,
    )
}
/// Build an RGB ICC profile from complete calibrated chromaticities and decode
/// exponents. For PNG's encoding gAMA value, callers supply its reciprocal.
pub fn create_rgb_profile(
    white: [f64; 2],
    primaries: [[f64; 2]; 3],
    decode_gamma: [f64; 3],
    max_output_bytes: u64,
) -> Result<Vec<u8>> {
    let mut buffers = OutputBuffers::new(DecodeOptions {
        max_output_bytes: 0,
        max_metadata_bytes: max_output_bytes,
        ..Default::default()
    });
    let mut failure = Failure::default();
    // SAFETY: fixed-size contiguous arrays supply the exact C array lengths;
    // the owned output callback is synchronous and enforces the metadata cap.
    let okay = unsafe {
        snow_native_rgb_profile(
            white.as_ptr(),
            primaries.as_flattened().as_ptr(),
            decode_gamma.as_ptr(),
            max_output_bytes,
            buffers.ffi(),
            &mut failure,
        )
    };
    failure.result(okay)?;
    Ok(buffers.icc)
}
pub fn transform_rgba8(source_icc: &[u8], pixels: &mut [u8]) -> Result<()> {
    if !pixels.len().is_multiple_of(4) {
        return Err(Error::InvalidInput("RGBA requires complete pixels".into()));
    }
    transform_pixels(
        source_icc,
        pixels.as_mut_ptr().cast(),
        pixels.len() / 4,
        0,
        false,
    )
}
pub fn transform_rgba16(source_icc: &[u8], pixels: &mut [u16]) -> Result<()> {
    if !pixels.len().is_multiple_of(4) {
        return Err(Error::InvalidInput("RGBA requires complete pixels".into()));
    }
    transform_pixels(
        source_icc,
        pixels.as_mut_ptr().cast(),
        pixels.len() / 4,
        1,
        false,
    )
}
pub fn resize_requirements(w: u32, h: u32, ow: u32, oh: u32) -> Result<ResizeRequirements> {
    byte_len(w, h, 4)?;
    byte_len(ow, oh, 4)?;
    let mut info = ResizeInfo::default();
    let mut failure = Failure::default();
    // SAFETY: output pointers refer to exclusive initialized stack objects.
    let okay = unsafe { snow_native_resize_query(w, h, ow, oh, &mut info, &mut failure) };
    failure.result(okay)?;
    Ok(ResizeRequirements {
        scratch_bytes: info.scratch_bytes,
        input_buffer_rows: info.input_buffer_rows,
        output_buffer_rows: info.output_buffer_rows,
        row_buffer_bytes: info.row_buffer_bytes,
    })
}
/// Resize linear-light premultiplied RGBA using bounded zimg ring buffers.
pub fn resize_rgba_f32(
    input: &[f32],
    w: u32,
    h: u32,
    ow: u32,
    oh: u32,
    max_scratch: u64,
) -> Result<Vec<f32>> {
    if byte_len(w, h, 1)? != input.len() {
        return Err(Error::InvalidInput("Resize input size mismatch".into()));
    }
    let count = byte_len(ow, oh, 1)?;
    let requirements = resize_requirements(w, h, ow, oh)?;
    if requirements
        .scratch_bytes
        .checked_add(requirements.row_buffer_bytes)
        .is_none_or(|v| v > max_scratch)
    {
        return Err(Error::ResourceLimit("Resize scratch exceeds limit".into()));
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(count)
        .map_err(|_| Error::ResourceLimit("Resize output allocation failed".into()))?;
    output.resize(count, 0.0);
    let mut failure = Failure::default();
    // SAFETY: both buffers have checked RGBA dimensions and remain disjoint throughout the call.
    let okay = unsafe {
        snow_native_resize(
            input.as_ptr(),
            w,
            h,
            output.as_mut_ptr(),
            ow,
            oh,
            max_scratch,
            &mut failure,
        )
    };
    failure.result(okay)?;
    Ok(output)
}
struct FloatReader<'a> {
    callback: &'a mut dyn FnMut(u32, &mut [f32]) -> Result<()>,
    error: Option<Error>,
}
struct FloatWriter<'a> {
    callback: &'a mut FloatRegionWriter<'a>,
    error: Option<Error>,
}
unsafe extern "C" fn read_floats(
    context: *mut c_void,
    row: u32,
    data: *mut u8,
    size: usize,
) -> c_int {
    // SAFETY: this exclusive context was passed directly to the synchronous zimg call.
    let state = unsafe { &mut *context.cast::<FloatReader<'_>>() };
    if !size.is_multiple_of(4) || !(data as usize).is_multiple_of(std::mem::align_of::<f32>()) {
        return 0;
    }
    // SAFETY: native zimg adapter provides a correctly aligned row scratch allocation.
    let pixels = unsafe { std::slice::from_raw_parts_mut(data.cast::<f32>(), size / 4) };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (state.callback)(row, pixels)
    })) {
        Ok(Ok(())) => 1,
        Ok(Err(e)) => {
            state.error = Some(e);
            0
        }
        Err(_) => {
            state.error = Some(Error::Codec("Float row callback panicked".into()));
            0
        }
    }
}
unsafe extern "C" fn write_floats(
    context: *mut c_void,
    row: u32,
    column: u32,
    data: *const u8,
    size: usize,
) -> c_int {
    // SAFETY: this exclusive context was passed directly to the synchronous zimg call.
    let state = unsafe { &mut *context.cast::<FloatWriter<'_>>() };
    if !size.is_multiple_of(4) || !(data as usize).is_multiple_of(std::mem::align_of::<f32>()) {
        return 0;
    }
    // SAFETY: output row storage is initialized by zimg and borrowed for this call only.
    let pixels = unsafe { std::slice::from_raw_parts(data.cast::<f32>(), size / 4) };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (state.callback)(row, column, pixels)
    })) {
        Ok(Ok(())) => 1,
        Ok(Err(e)) => {
            state.error = Some(e);
            0
        }
        Err(_) => {
            state.error = Some(Error::Codec("Float output callback panicked".into()));
            0
        }
    }
}
pub fn resize_regions_f32(
    w: u32,
    h: u32,
    ow: u32,
    oh: u32,
    max_scratch: u64,
    read_row: &mut dyn FnMut(u32, &mut [f32]) -> Result<()>,
    write_region: &mut FloatRegionWriter<'_>,
) -> Result<()> {
    byte_len(w, h, 4)?;
    byte_len(ow, oh, 4)?;
    let mut reader = FloatReader {
        callback: read_row,
        error: None,
    };
    let mut writer = FloatWriter {
        callback: write_region,
        error: None,
    };
    let mut failure = Failure::default();
    // SAFETY: row contexts are exclusively owned; C callbacks use aligned native scratch buffers.
    let okay = unsafe {
        snow_native_resize_regions(
            w,
            h,
            ow,
            oh,
            max_scratch,
            read_floats,
            (&mut reader as *mut FloatReader<'_>).cast(),
            write_floats,
            (&mut writer as *mut FloatWriter<'_>).cast(),
            &mut failure,
        )
    };
    if let Some(error) = reader.error.or(writer.error) {
        return Err(error);
    }
    failure.result(okay)
}

pub struct WebPAnimation {
    handle: NonNull<c_void>,
    _input: Arc<[u8]>,
    options: DecodeOptions,
    info: NativeInfo,
}
// SAFETY: the object is exclusively accessed through &mut self, never used concurrently, and
// libwebp sessions have no thread-affine state. Encoded input is immutable and outlives the decoder.
unsafe impl Send for WebPAnimation {}
impl WebPAnimation {
    pub fn open(input: Arc<[u8]>, options: DecodeOptions) -> Result<Self> {
        decode_options(options)?;
        let info = inspect(NativeFormat::WebP, &input, options)?;
        let mut desc = Desc::default();
        let mut failure = Failure::default();
        // SAFETY: input Arc remains owned until after decoder destruction.
        let pointer = unsafe {
            snow_webp_animation_open(
                input.as_ptr(),
                input.len(),
                &options.into(),
                &mut desc,
                &mut failure,
            )
        };
        let handle = match NonNull::new(pointer) {
            Some(p) => p,
            None => {
                failure.result(0)?;
                return Err(Error::Codec("Animation decoder returned null".into()));
            }
        };
        Ok(Self {
            handle,
            _input: input,
            options,
            info,
        })
    }
    pub fn info(&self) -> &NativeInfo {
        &self.info
    }
    pub fn next_frame(&mut self) -> Result<Option<NativeFrame>> {
        let mut buffers = OutputBuffers::new(self.options);
        let mut desc = Desc::default();
        let mut duration = 0;
        let mut failure = Failure::default();
        // SAFETY: decoder is exclusively borrowed; output is copied into fresh owned pixels.
        let okay = unsafe {
            snow_webp_animation_next(
                self.handle.as_ptr(),
                buffers.ffi(),
                &mut desc,
                &mut duration,
                &mut failure,
            )
        };
        if okay == 2 {
            return Ok(None);
        }
        failure.result(okay)?;
        buffers
            .icc
            .try_reserve_exact(self.info.icc.len())
            .map_err(|_| Error::ResourceLimit("Animation ICC allocation failed".into()))?;
        buffers.icc.extend_from_slice(&self.info.icc);
        buffers
            .exif
            .try_reserve_exact(self.info.exif.len())
            .map_err(|_| Error::ResourceLimit("Animation metadata allocation failed".into()))?;
        buffers.exif.extend_from_slice(&self.info.exif);
        buffers
            .xmp
            .try_reserve_exact(self.info.xmp.len())
            .map_err(|_| Error::ResourceLimit("Animation metadata allocation failed".into()))?;
        buffers.xmp.extend_from_slice(&self.info.xmp);
        desc.orientation = self.info.orientation.into();
        Ok(Some(NativeFrame {
            image: image(desc, buffers)?,
            duration_ms: duration,
        }))
    }
    pub fn reset(&mut self) {
        // SAFETY: reset owns exclusive access and input remains alive.
        unsafe { snow_webp_animation_reset(self.handle.as_ptr()) };
    }
}
impl Drop for WebPAnimation {
    fn drop(&mut self) {
        // SAFETY: handle was created once and is destroyed once, before input Arc is dropped.
        unsafe { snow_webp_animation_close(self.handle.as_ptr()) };
    }
}

/// Encode random-access rows through libjxl's chunked input interface. Callbacks may reread rows.
/// Read callbacks are serialized, but may run on codec threads, hence the Send requirement.
pub fn encode_jxl_rows(
    info: &NativeInfo,
    options: EncodeOptions,
    read_row: &mut (dyn FnMut(u32, &mut [u8]) -> Result<()> + Send),
    output: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    byte_len(info.width, info.height, info.sample.bytes())?;
    if !info.hdr_reference_white_nits.is_finite() || info.hdr_reference_white_nits < 0.0 {
        return Err(Error::InvalidInput("Invalid HDR reference white".into()));
    }
    if info.hdr_reference_white_nits > 0.0
        && (info.transfer != TransferFunction::Linear
            || info.primaries != ColorPrimaries::Srgb
            || !matches!(info.sample, SampleType::F16 | SampleType::F32))
    {
        return Err(Error::Unsupported(
            "Explicit scRGB requires floating-point linear-sRGB JXL".into(),
        ));
    }
    encode_options(NativeFormat::Jxl, options)?;
    if !(1..=8).contains(&info.orientation)
        || info.significant_bits == 0
        || info.significant_bits as usize > info.sample.bytes() * 8
    {
        return Err(Error::InvalidInput("Invalid JXL row description".into()));
    }
    if options.exact && matches!(info.sample, SampleType::F16 | SampleType::F32) {
        return Err(Error::Unsupported(
            "Exact floating-point preservation is not guaranteed".into(),
        ));
    }
    let desc = Desc {
        width: info.width,
        height: info.height,
        sample: info.sample as u32,
        significant_bits: info.significant_bits.into(),
        transfer: info.transfer as u32,
        primaries: info.primaries as u32,
        orientation: info.orientation.into(),
        hdr_reference_white_nits: info.hdr_reference_white_nits,
        has_alpha: 1,
        ..Default::default()
    };
    let mut checked_read = |row: u32, data: &mut [u8]| {
        read_row(row, data)?;
        if options.exact {
            validate_exact_precision(data, info.sample, info.significant_bits)?;
        }
        Ok(())
    };
    let mut reader = ByteReader {
        callback: &mut checked_read,
        error: None,
    };
    let mut writer = Writer {
        callback: output,
        error: None,
    };
    let mut failure = Failure::default();
    // SAFETY: dimensions/type are checked, read closure is Send, C++ serializes callback access,
    // and all callbacks finish before returning. The source context outlives native destruction.
    let okay = unsafe {
        snow_jxl_encode_rows(
            &desc,
            info.icc.as_ptr(),
            info.icc.len(),
            &options.into(),
            read_bytes,
            (&mut reader as *mut ByteReader<'_>).cast(),
            write,
            (&mut writer as *mut Writer<'_>).cast(),
            &mut failure,
        )
    };
    if let Some(error) = reader.error.or(writer.error) {
        return Err(error);
    }
    failure.result(okay)
}
fn validate_exact_precision(pixels: &[u8], sample: SampleType, bits: u8) -> Result<()> {
    if sample == SampleType::U16 && bits < 16 {
        let maximum = (1u64 << bits) - 1;
        for value in pixels.chunks_exact(2) {
            let value = u16::from_ne_bytes([value[0], value[1]]) as u64;
            let quantized = (value * maximum + 32767) / 65535;
            let expanded = (quantized * 65535 + maximum / 2) / maximum;
            if value != expanded {
                return Err(Error::Unsupported(
                    "Samples are not exactly representable at their declared significant bit depth"
                        .into(),
                ));
            }
        }
    }
    Ok(())
}
