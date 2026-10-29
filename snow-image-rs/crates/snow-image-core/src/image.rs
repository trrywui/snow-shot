use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::{Budget, CancellationToken, Error, Reservation, ResourceKind, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ChannelOrder {
    Gray,
    GrayAlpha,
    Rgb,
    Bgr,
    Rgba,
    Bgra,
}
impl ChannelOrder {
    pub fn count(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::GrayAlpha => 2,
            Self::Rgb | Self::Bgr => 3,
            Self::Rgba | Self::Bgra => 4,
        }
    }
    pub fn has_alpha(self) -> bool {
        matches!(self, Self::GrayAlpha | Self::Rgba | Self::Bgra)
    }
}
/// Interleaved storage samples. Multibyte values use native byte order; F16 and
/// F32 use IEEE binary16 and binary32. Integer U16 color values span 0..65535,
/// including when `PixelFormat::significant_bits` records a lower source precision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SampleType {
    U8,
    U16,
    F16,
    F32,
}
impl SampleType {
    pub fn bytes(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U16 | Self::F16 => 2,
            Self::F32 => 4,
        }
    }
    pub fn bits(self) -> u8 {
        (self.bytes() * 8) as u8
    }
    pub fn is_float(self) -> bool {
        matches!(self, Self::F16 | Self::F32)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AlphaMode {
    /// Pixels are fully opaque. A physically present alpha channel is unused;
    /// exporters normalize it to full opacity, including capture buffers whose
    /// unused alpha bytes are zero. Those bytes are not accepted alpha samples.
    Opaque,
    /// Color channels are independent of the meaningful alpha sample.
    Straight,
    /// Color channels have already been multiplied by the alpha sample.
    Premultiplied,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PixelFormat {
    pub channels: ChannelOrder,
    pub sample: SampleType,
    pub alpha: AlphaMode,
    pub significant_bits: u8,
}
impl PixelFormat {
    pub const RGBA8: Self = Self {
        channels: ChannelOrder::Rgba,
        sample: SampleType::U8,
        alpha: AlphaMode::Straight,
        significant_bits: 8,
    };
    pub const BGRA8: Self = Self {
        channels: ChannelOrder::Bgra,
        ..Self::RGBA8
    };
    pub const RGB8: Self = Self {
        channels: ChannelOrder::Rgb,
        alpha: AlphaMode::Opaque,
        ..Self::RGBA8
    };
    pub fn bytes_per_pixel(self) -> usize {
        self.channels.count() * self.sample.bytes()
    }
    pub fn validate(self) -> Result<()> {
        if self.significant_bits == 0
            || self.significant_bits > self.sample.bits()
            || (self.sample.is_float() && self.significant_bits != self.sample.bits())
        {
            return Err(Error::InvalidInput(
                "invalid significant bit depth for sample type".into(),
            ));
        }
        if !self.channels.has_alpha() && self.alpha != AlphaMode::Opaque {
            return Err(Error::InvalidInput(
                "alpha mode requires an alpha channel".into(),
            ));
        }
        Ok(())
    }
}

/// CIE xy chromaticities, retained as f64 to roundtrip PNG's integer/100000 tags.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RgbChromaticities {
    pub white: [f64; 2],
    pub red: [f64; 2],
    pub green: [f64; 2],
    pub blue: [f64; 2],
}
impl RgbChromaticities {
    pub fn validate(&self) -> Result<()> {
        for [x, y] in [self.white, self.red, self.green, self.blue] {
            if !x.is_finite() || !y.is_finite() || x < 0.0 || y <= 0.0 || x + y > 1.0 + 1e-9 {
                return Err(Error::InvalidInput("invalid CIE xy chromaticities".into()));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ColorEncoding {
    Srgb,
    LinearSrgb,
    DisplayP3,
    ScRgb {
        reference_white_nits: f32,
    },
    Bt2020Pq,
    /// Reference white is the luminance of a 75% HLG signal (diffuse white).
    /// Processing uses system gamma 1.2, as for the typical 1000-nit reference display.
    Bt2020Hlg {
        reference_white_nits: f32,
    },
    /// ICC profile bytes are in the accompanying Metadata.
    Icc,
    /// Source PNG gAMA/cHRM description. Gamma is the encoding exponent
    /// (typically 0.45455), so the linear-light decoding exponent is its reciprocal.
    /// An absent component is unknown, never an implicit sRGB assumption.
    CalibratedRgb {
        gamma: Option<f64>,
        chromaticities: Option<RgbChromaticities>,
    },
    /// Source BMP calibrated RGB values. Endpoints are XYZ values stored as
    /// signed 2.30 fixed point; channel gamma values are unsigned 16.16 decoding
    /// exponents. Zero values are retained as explicitly incomplete descriptions.
    CalibratedBmp {
        endpoints: [[f64; 3]; 3],
        gamma: [f64; 3],
    },
    Unknown,
}
impl ColorEncoding {
    pub fn validate(&self) -> Result<()> {
        if let Self::ScRgb {
            reference_white_nits,
        }
        | Self::Bt2020Hlg {
            reference_white_nits,
        } = self
            && (!reference_white_nits.is_finite() || *reference_white_nits <= 0.0)
        {
            return Err(Error::InvalidInput(
                "reference white must be finite and positive".into(),
            ));
        }
        match self {
            Self::CalibratedRgb {
                gamma,
                chromaticities,
            } => {
                if gamma.is_none() && chromaticities.is_none() {
                    return Err(Error::InvalidInput(
                        "calibrated RGB requires source color information".into(),
                    ));
                }
                if gamma.is_some_and(|gamma| !gamma.is_finite() || gamma <= 0.0) {
                    return Err(Error::InvalidInput(
                        "PNG gamma must be finite and positive".into(),
                    ));
                }
                if let Some(chromaticities) = chromaticities {
                    chromaticities.validate()?;
                }
            }
            Self::CalibratedBmp { endpoints, gamma }
                if endpoints.iter().flatten().any(|value| !value.is_finite())
                    || gamma.iter().any(|value| !value.is_finite() || *value < 0.0) =>
            {
                return Err(Error::InvalidInput(
                    "invalid calibrated BMP color description".into(),
                ));
            }
            _ => (),
        }
        Ok(())
    }
    pub fn is_hdr(&self) -> bool {
        matches!(
            self,
            Self::ScRgb { .. } | Self::Bt2020Pq | Self::Bt2020Hlg { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Orientation {
    #[default]
    Normal,
    FlipHorizontal,
    Rotate180,
    FlipVertical,
    Transpose,
    Rotate90,
    Transverse,
    Rotate270,
}
impl Orientation {
    pub fn swaps_dimensions(self) -> bool {
        matches!(
            self,
            Self::Transpose | Self::Rotate90 | Self::Transverse | Self::Rotate270
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageDescriptor {
    pub width: u32,
    pub height: u32,
    pub row_stride: usize,
    pub format: PixelFormat,
    pub color: ColorEncoding,
    pub orientation: Orientation,
}
impl ImageDescriptor {
    pub fn packed(
        width: u32,
        height: u32,
        format: PixelFormat,
        color: ColorEncoding,
    ) -> Result<Self> {
        let row_stride = (width as usize)
            .checked_mul(format.bytes_per_pixel())
            .ok_or_else(|| Error::InvalidInput("row size overflow".into()))?;
        let value = Self {
            width,
            height,
            row_stride,
            format,
            color,
            orientation: Orientation::Normal,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn row_bytes(&self) -> Result<usize> {
        (self.width as usize)
            .checked_mul(self.format.bytes_per_pixel())
            .ok_or_else(|| Error::InvalidInput("row size overflow".into()))
    }
    pub fn required_bytes(&self) -> Result<usize> {
        self.row_stride
            .checked_mul(self.height as usize)
            .ok_or_else(|| Error::InvalidInput("raster size overflow".into()))
    }
    pub fn validate(&self) -> Result<()> {
        self.format.validate()?;
        self.color.validate()?;
        if self.width == 0 || self.height == 0 {
            return Err(Error::InvalidInput(
                "image dimensions must be nonzero".into(),
            ));
        }
        if self.row_stride < self.row_bytes()? {
            return Err(Error::InvalidInput(
                "row stride is shorter than a pixel row".into(),
            ));
        }
        if !self.row_stride.is_multiple_of(self.format.sample.bytes()) {
            return Err(Error::InvalidInput(
                "row stride is not aligned to a sample".into(),
            ));
        }
        self.required_bytes()?;
        Ok(())
    }
    pub fn validate_with_policy(&self, budget: &Budget) -> Result<()> {
        self.validate()?;
        let bytes = self.required_bytes()? as u64;
        if bytes > budget.policy().max_raster_bytes {
            return Err(Error::ResourceLimit {
                resource: ResourceKind::Pixels,
                requested: bytes,
                limit: budget.policy().max_raster_bytes,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub icc: Option<Vec<u8>>,
    pub exif: Option<Vec<u8>>,
    pub xmp: Option<Vec<u8>>,
}
impl Metadata {
    pub fn byte_len(&self) -> Result<u64> {
        self.icc
            .iter()
            .chain(self.exif.iter())
            .chain(self.xmp.iter())
            .try_fold(0u64, |total, bytes| {
                total
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(|| Error::InvalidInput("metadata size overflow".into()))
            })
    }
    pub fn allocation_bytes(&self) -> Result<u64> {
        self.icc
            .iter()
            .chain(self.exif.iter())
            .chain(self.xmp.iter())
            .try_fold(0u64, |total, bytes| {
                total
                    .checked_add(bytes.capacity() as u64)
                    .ok_or_else(|| Error::InvalidInput("metadata capacity overflow".into()))
            })
    }
}

/// Immutable metadata with one resource charge shared across images and sessions.
#[derive(Clone, Debug)]
pub struct SharedMetadata(Arc<MetadataStorage>);
#[derive(Debug)]
struct MetadataStorage {
    value: Metadata,
    _reservation: Reservation,
}
impl SharedMetadata {
    /// Adopts externally allocated metadata and accounts for its actual capacity.
    pub fn new(value: Metadata, budget: &Budget) -> Result<Self> {
        let reservation = budget.reserve(ResourceKind::Metadata, value.allocation_bytes()?)?;
        Ok(Self(Arc::new(MetadataStorage {
            value,
            _reservation: reservation,
        })))
    }
    /// Reserves the copy before allocating; cloning SharedMetadata itself never copies bytes.
    pub fn try_copy_from(value: &Metadata, budget: &Budget) -> Result<Self> {
        let mut reservation = budget.reserve(ResourceKind::Metadata, value.byte_len()?)?;
        fn copy(bytes: Option<&Vec<u8>>) -> Result<Option<Vec<u8>>> {
            bytes
                .map(|bytes| {
                    let mut result = Vec::new();
                    result
                        .try_reserve_exact(bytes.len())
                        .map_err(|_| Error::AllocationFailed)?;
                    result.extend_from_slice(bytes);
                    Ok(result)
                })
                .transpose()
        }
        let value = Metadata {
            icc: copy(value.icc.as_ref())?,
            exif: copy(value.exif.as_ref())?,
            xmp: copy(value.xmp.as_ref())?,
        };
        reservation.resize(value.allocation_bytes()?)?;
        Ok(Self(Arc::new(MetadataStorage {
            value,
            _reservation: reservation,
        })))
    }
    pub fn from_reserved(value: Metadata, mut reservation: Reservation) -> Result<Self> {
        if reservation.resource() != ResourceKind::Metadata {
            return Err(Error::InvalidInput(
                "metadata requires a metadata reservation".into(),
            ));
        }
        reservation.resize(value.allocation_bytes()?)?;
        Ok(Self(Arc::new(MetadataStorage {
            value,
            _reservation: reservation,
        })))
    }
    pub fn get(&self) -> &Metadata {
        &self.0.value
    }
}

/// Optional analysis produced from precisely the immutable pixels that own it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageAnalysis {
    pub has_transparency: bool,
    pub peak_linear_luminance: Option<f32>,
}

#[derive(Debug)]
enum PixelStorage {
    Owned(Vec<u8>),
    External(Box<dyn OwnedPixelStorage>),
}
impl AsRef<[u8]> for PixelStorage {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::External(storage) => storage.as_ref().as_ref(),
        }
    }
}

/// Ownership of externally allocated immutable pixel bytes, such as a sealed mapping.
///
/// Every `as_ref()` call must return the same byte sequence and length for the entire
/// lifetime of this owner. The owner must retain all backing resources. For mapped
/// storage, its constructor must ensure no process or handle can modify or truncate
/// the backing while a Rust slice exists. Merely opening a writable file read-only
/// does not establish that invariant. The core performs no unsafe mapping itself.
///
/// Passing this owner never copies its pixels; copying to mutable storage is an
/// explicit, budgeted [`Image::try_copy`] operation. Any storage capacity beyond
/// the visible slice must also be covered by the supplied Pixels reservation.
pub trait OwnedPixelStorage: AsRef<[u8]> + Send + Sync + std::fmt::Debug {}
impl<T: AsRef<[u8]> + Send + Sync + std::fmt::Debug> OwnedPixelStorage for T {}

#[derive(Debug)]
struct ImageStorage {
    descriptor: ImageDescriptor,
    pixels: PixelStorage,
    metadata: SharedMetadata,
    analysis: Option<ImageAnalysis>,
    _pixels_reservation: Reservation,
}
/// Cheaply cloneable, immutable image ownership. Pixel allocation is never copied implicitly.
#[derive(Clone, Debug)]
pub struct Image(Arc<ImageStorage>);
impl Image {
    pub fn from_vec(
        descriptor: ImageDescriptor,
        pixels: Vec<u8>,
        metadata: Metadata,
        budget: &Budget,
    ) -> Result<Self> {
        Self::from_shared_metadata(
            descriptor,
            pixels,
            SharedMetadata::new(metadata, budget)?,
            budget,
        )
    }
    pub fn from_shared_metadata(
        descriptor: ImageDescriptor,
        pixels: Vec<u8>,
        metadata: SharedMetadata,
        budget: &Budget,
    ) -> Result<Self> {
        let pixels_reservation = budget.reserve(ResourceKind::Pixels, pixels.capacity() as u64)?;
        Self::from_reserved(descriptor, pixels, metadata, pixels_reservation)
    }
    /// Transfers an already reserved Vec allocation without a second charge or an
    /// interval in which allocated pixels have no owning reservation.
    pub fn from_reserved(
        descriptor: ImageDescriptor,
        pixels: Vec<u8>,
        metadata: SharedMetadata,
        pixels_reservation: Reservation,
    ) -> Result<Self> {
        validate_pixel_reservation(&descriptor, &pixels_reservation, pixels.capacity())?;
        if pixels.len() != descriptor.required_bytes()? {
            return Err(Error::InvalidInput(
                "pixel buffer length differs from descriptor".into(),
            ));
        }
        validate_metadata(&descriptor, metadata.get())?;
        Ok(Self(Arc::new(ImageStorage {
            descriptor,
            pixels: PixelStorage::Owned(pixels),
            metadata,
            analysis: None,
            _pixels_reservation: pixels_reservation,
        })))
    }
    /// Adopts an immutable external owner without copying pixels. The reservation
    /// must already cover its entire allocation and is retained through the final
    /// image clone. See [`OwnedPixelStorage`] for mapping and immutability requirements.
    pub fn from_owned_storage(
        descriptor: ImageDescriptor,
        storage: impl OwnedPixelStorage + 'static,
        metadata: SharedMetadata,
        pixels_reservation: Reservation,
    ) -> Result<Self> {
        let length = storage.as_ref().len();
        validate_pixel_reservation(&descriptor, &pixels_reservation, length)?;
        if length != descriptor.required_bytes()? {
            return Err(Error::InvalidInput(
                "external pixel storage length differs from descriptor".into(),
            ));
        }
        validate_metadata(&descriptor, metadata.get())?;
        Ok(Self(Arc::new(ImageStorage {
            descriptor,
            pixels: PixelStorage::External(Box::new(storage)),
            metadata,
            analysis: None,
            _pixels_reservation: pixels_reservation,
        })))
    }
    pub fn descriptor(&self) -> &ImageDescriptor {
        &self.0.descriptor
    }
    pub fn bytes(&self) -> &[u8] {
        self.0.pixels.as_ref()
    }
    pub fn metadata(&self) -> &Metadata {
        self.0.metadata.get()
    }
    pub fn shared_metadata(&self) -> SharedMetadata {
        self.0.metadata.clone()
    }
    pub fn analysis(&self) -> Option<&ImageAnalysis> {
        self.0.analysis.as_ref()
    }
    pub fn view(&self) -> ImageView<'_> {
        ImageView {
            descriptor: self.descriptor(),
            bytes: self.bytes(),
            metadata: self.metadata(),
        }
    }
    /// Explicitly allocates a mutable pixel copy and shares immutable metadata.
    pub fn try_copy(&self, budget: &Budget) -> Result<MutableImage> {
        let mut copied =
            MutableImage::new_shared(self.descriptor().clone(), self.shared_metadata(), budget)?;
        copied.bytes_mut().copy_from_slice(self.bytes());
        Ok(copied)
    }
}

fn validate_pixel_reservation(
    descriptor: &ImageDescriptor,
    reservation: &Reservation,
    capacity: usize,
) -> Result<()> {
    if reservation.resource() != ResourceKind::Pixels {
        return Err(Error::InvalidInput(
            "pixel ownership requires a Pixels reservation".into(),
        ));
    }
    descriptor.validate_with_policy(reservation.budget())?;
    if reservation.amount() < capacity as u64 {
        return Err(Error::InvalidInput(
            "pixel reservation does not cover storage capacity".into(),
        ));
    }
    Ok(())
}

fn validate_metadata(descriptor: &ImageDescriptor, metadata: &Metadata) -> Result<()> {
    if descriptor.color == ColorEncoding::Icc && metadata.icc.as_ref().is_none_or(Vec::is_empty) {
        return Err(Error::InvalidInput(
            "ICC color encoding requires a profile".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct ImageView<'a> {
    descriptor: &'a ImageDescriptor,
    bytes: &'a [u8],
    metadata: &'a Metadata,
}
impl<'a> ImageView<'a> {
    pub fn descriptor(&self) -> &'a ImageDescriptor {
        self.descriptor
    }
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
    pub fn metadata(&self) -> &'a Metadata {
        self.metadata
    }
    pub fn row(&self, row: u32) -> Result<&'a [u8]> {
        if row >= self.descriptor.height {
            return Err(Error::InvalidInput("row outside raster".into()));
        }
        let start = row as usize * self.descriptor.row_stride;
        Ok(&self.bytes[start..start + self.descriptor.row_bytes()?])
    }
}

#[derive(Debug)]
pub struct MutableImage {
    storage: MutableStorage,
}
#[derive(Debug)]
struct MutableStorage {
    descriptor: ImageDescriptor,
    pixels: Vec<u8>,
    metadata: SharedMetadata,
    pixels_reservation: Reservation,
}
impl MutableStorage {
    fn seal(self, analysis: Option<ImageAnalysis>) -> Image {
        Image(Arc::new(ImageStorage {
            descriptor: self.descriptor,
            pixels: PixelStorage::Owned(self.pixels),
            metadata: self.metadata,
            analysis,
            _pixels_reservation: self.pixels_reservation,
        }))
    }
}
impl MutableImage {
    pub fn new(descriptor: ImageDescriptor, metadata: Metadata, budget: &Budget) -> Result<Self> {
        Self::new_shared(descriptor, SharedMetadata::new(metadata, budget)?, budget)
    }
    pub fn new_shared(
        descriptor: ImageDescriptor,
        metadata: SharedMetadata,
        budget: &Budget,
    ) -> Result<Self> {
        descriptor.validate_with_policy(budget)?;
        validate_metadata(&descriptor, metadata.get())?;
        let length = descriptor.required_bytes()?;
        let mut pixels_reservation = budget.reserve(ResourceKind::Pixels, length as u64)?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(length)
            .map_err(|_| Error::AllocationFailed)?;
        pixels_reservation.resize(pixels.capacity() as u64)?;
        pixels.resize(length, 0);
        Ok(Self {
            storage: MutableStorage {
                descriptor,
                pixels,
                metadata,
                pixels_reservation,
            },
        })
    }
    pub fn descriptor(&self) -> &ImageDescriptor {
        &self.storage.descriptor
    }
    pub fn bytes(&self) -> &[u8] {
        &self.storage.pixels
    }
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.storage.pixels
    }
    pub fn metadata(&self) -> &Metadata {
        self.storage.metadata.get()
    }
    pub fn shared_metadata(&self) -> SharedMetadata {
        self.storage.metadata.clone()
    }
    pub fn seal(self) -> Image {
        self.storage.seal(None)
    }
    /// Analysis is computed while borrowing the final pixels, then pixels are sealed.
    pub fn seal_with_analysis(
        self,
        analyze: impl FnOnce(&ImageView<'_>) -> ImageAnalysis,
    ) -> Image {
        let storage = self.storage;
        let analysis = analyze(&ImageView {
            descriptor: &storage.descriptor,
            bytes: &storage.pixels,
            metadata: storage.metadata.get(),
        });
        storage.seal(Some(analysis))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterAccess {
    SequentialRows,
    RandomRows,
    Regions,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Row reads pack active samples into `dst`; row padding is never copied.
/// Implementations advertising SequentialRows may reject backward reads.
pub trait RasterSource: Send + Sync {
    fn descriptor(&self) -> &ImageDescriptor;
    fn access(&self) -> RasterAccess;
    fn metadata(&self) -> Option<&Metadata> {
        None
    }
    fn read_rows(&self, first_row: u32, rows: u32, dst: &mut [u8]) -> Result<()>;
    /// Custom producers must return promptly and check this token during expensive work.
    /// A worker can be terminated, but an arbitrary blocked host callback cannot be stopped.
    fn read_rows_cancellable(
        &self,
        first_row: u32,
        rows: u32,
        dst: &mut [u8],
        cancellation: &CancellationToken,
    ) -> Result<()> {
        cancellation.check()?;
        self.read_rows(first_row, rows, dst)?;
        cancellation.check()
    }
    fn read_region(&self, _region: Region, _dst: &mut [u8]) -> Result<()> {
        Err(Error::Unsupported(
            "region access is unavailable for this raster".into(),
        ))
    }
}

fn validate_rows(
    descriptor: &ImageDescriptor,
    first: u32,
    rows: u32,
    length: usize,
) -> Result<usize> {
    if first
        .checked_add(rows)
        .is_none_or(|end| end > descriptor.height)
    {
        return Err(Error::InvalidInput("rows outside raster".into()));
    }
    let row_bytes = descriptor.row_bytes()?;
    if row_bytes.checked_mul(rows as usize) != Some(length) {
        return Err(Error::InvalidInput(
            "row destination length differs from requested rows".into(),
        ));
    }
    Ok(row_bytes)
}
impl RasterSource for Image {
    fn descriptor(&self) -> &ImageDescriptor {
        self.descriptor()
    }
    fn access(&self) -> RasterAccess {
        RasterAccess::Regions
    }
    fn metadata(&self) -> Option<&Metadata> {
        Some(self.metadata())
    }
    fn read_rows(&self, first_row: u32, rows: u32, dst: &mut [u8]) -> Result<()> {
        let row_bytes = validate_rows(self.descriptor(), first_row, rows, dst.len())?;
        for (offset, output) in dst.chunks_exact_mut(row_bytes).enumerate() {
            output.copy_from_slice(self.view().row(first_row + offset as u32)?);
        }
        Ok(())
    }
    fn read_region(&self, region: Region, dst: &mut [u8]) -> Result<()> {
        let descriptor = self.descriptor();
        if region
            .x
            .checked_add(region.width)
            .is_none_or(|end| end > descriptor.width)
            || region
                .y
                .checked_add(region.height)
                .is_none_or(|end| end > descriptor.height)
        {
            return Err(Error::InvalidInput("region outside raster".into()));
        }
        let bpp = descriptor.format.bytes_per_pixel();
        let row_bytes = (region.width as usize)
            .checked_mul(bpp)
            .ok_or_else(|| Error::InvalidInput("region size overflow".into()))?;
        if row_bytes.checked_mul(region.height as usize) != Some(dst.len()) {
            return Err(Error::InvalidInput(
                "region destination length differs from region".into(),
            ));
        }
        if row_bytes == 0 {
            return Ok(());
        }
        for (row, output) in dst.chunks_exact_mut(row_bytes).enumerate() {
            let start = (region.y as usize + row) * descriptor.row_stride + region.x as usize * bpp;
            output.copy_from_slice(&self.bytes()[start..start + row_bytes]);
        }
        Ok(())
    }
}

/// A seekable writer without a file handle, used only while a private raster is built.
pub trait RasterWrite: Write + Seek {}
impl<T: Write + Seek> RasterWrite for T {}

struct BoundedRasterWriter<'a> {
    file: &'a mut std::fs::File,
    maximum: u64,
    position: u64,
}
impl Write for BoundedRasterWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.maximum.saturating_sub(self.position) {
            return Err(std::io::Error::other("raster write exceeds declared size"));
        }
        let written = self.file.write(bytes)?;
        self.position += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
impl Seek for BoundedRasterWriter<'_> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let position = match position {
            SeekFrom::Start(value) => Some(value),
            SeekFrom::Current(value) => self.position.checked_add_signed(value),
            SeekFrom::End(value) => self.maximum.checked_add_signed(value),
        }
        .filter(|value| *value <= self.maximum)
        .ok_or_else(|| std::io::Error::other("raster seek exceeds declared size"))?;
        self.file.seek(SeekFrom::Start(position))?;
        self.position = position;
        Ok(position)
    }
}

/// Immutable private disk snapshot for logical rasters larger than the working budget.
/// No writable handle or path escapes this type; its disk reservation lasts until drop.
#[derive(Debug)]
pub struct FileRasterSource {
    descriptor: ImageDescriptor,
    metadata: Option<SharedMetadata>,
    reader: Mutex<std::fs::File>,
    _disk_reservation: Reservation,
}
impl FileRasterSource {
    pub fn shared_metadata(&self) -> Option<SharedMetadata> {
        self.metadata.clone()
    }
    /// Build a raster directly into private storage without an extra file copy.
    /// The producer cannot retain a writable handle after this function returns.
    pub fn create(
        descriptor: ImageDescriptor,
        metadata: SharedMetadata,
        budget: &Budget,
        cancellation: &CancellationToken,
        render: impl FnOnce(&mut dyn RasterWrite) -> Result<()>,
    ) -> Result<Self> {
        descriptor.validate_with_policy(budget)?;
        validate_metadata(&descriptor, metadata.get())?;
        let size = descriptor.required_bytes()? as u64;
        let disk = budget.reserve(ResourceKind::TempDisk, size)?;
        cancellation.check()?;
        let mut file = tempfile::tempfile()?;
        render(&mut BoundedRasterWriter {
            file: &mut file,
            maximum: size,
            position: 0,
        })?;
        cancellation.check()?;
        file.flush()?;
        if file.metadata()?.len() != size {
            return Err(Error::InvalidInput(
                "raster producer did not write declared size".into(),
            ));
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(Self {
            descriptor,
            metadata: Some(metadata),
            reader: Mutex::new(file),
            _disk_reservation: disk,
        })
    }
    pub fn from_reader(
        descriptor: ImageDescriptor,
        reader: impl Read,
        budget: &Budget,
        cancellation: &CancellationToken,
    ) -> Result<Self> {
        Self::from_reader_impl(descriptor, reader, None, budget, cancellation)
    }
    pub fn from_reader_with_metadata(
        descriptor: ImageDescriptor,
        reader: impl Read,
        metadata: SharedMetadata,
        budget: &Budget,
        cancellation: &CancellationToken,
    ) -> Result<Self> {
        Self::from_reader_impl(descriptor, reader, Some(metadata), budget, cancellation)
    }
    fn from_reader_impl(
        descriptor: ImageDescriptor,
        mut reader: impl Read,
        metadata: Option<SharedMetadata>,
        budget: &Budget,
        cancellation: &CancellationToken,
    ) -> Result<Self> {
        descriptor.validate_with_policy(budget)?;
        validate_metadata(
            &descriptor,
            metadata
                .as_ref()
                .map(SharedMetadata::get)
                .unwrap_or(&Metadata::default()),
        )?;
        let size = descriptor.required_bytes()? as u64;
        let disk = budget.reserve(ResourceKind::TempDisk, size)?;
        let mut temporary = tempfile::tempfile()?;
        let mut remaining = size;
        let mut block = [0u8; 64 * 1024];
        while remaining > 0 {
            cancellation.check()?;
            let count = remaining.min(block.len() as u64) as usize;
            reader.read_exact(&mut block[..count])?;
            temporary.write_all(&block[..count])?;
            remaining -= count as u64;
        }
        temporary.flush()?;
        temporary.seek(SeekFrom::Start(0))?;
        Ok(Self {
            descriptor,
            metadata,
            reader: Mutex::new(temporary),
            _disk_reservation: disk,
        })
    }
}
impl RasterSource for FileRasterSource {
    fn descriptor(&self) -> &ImageDescriptor {
        &self.descriptor
    }
    fn access(&self) -> RasterAccess {
        RasterAccess::RandomRows
    }
    fn metadata(&self) -> Option<&Metadata> {
        self.metadata.as_ref().map(SharedMetadata::get)
    }
    fn read_rows(&self, first_row: u32, rows: u32, dst: &mut [u8]) -> Result<()> {
        let row_bytes = validate_rows(&self.descriptor, first_row, rows, dst.len())?;
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| Error::Io(std::io::Error::other("raster reader lock poisoned")))?;
        for (index, output) in dst.chunks_exact_mut(row_bytes).enumerate() {
            let offset = (first_row as u64 + index as u64) * self.descriptor.row_stride as u64;
            reader.seek(SeekFrom::Start(offset))?;
            reader.read_exact(output)?;
        }
        Ok(())
    }
}
