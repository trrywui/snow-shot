//! Private, length-delimited worker protocol. Pixel and metadata bytes never enter JSON.
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use snow_image_core::*;
use std::io::{Read, Write};

pub const VERSION: u32 = 1;
pub const MAX_CONTROL_BYTES: usize = 1024 * 1024;
/// Conservative parsing envelope for this fixed schema, including the input
/// buffer and deserialized collections. In particular, a JSON array of short
/// integers can expand into a much larger Vec<u64> before semantic validation.
/// Revisit this bound whenever a control type gains a new collection variant.
pub const CONTROL_PARSE_ALLOWANCE: u64 = (MAX_CONTROL_BYTES * 16) as u64;
pub const TRANSFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct MetadataLengths {
    pub icc: u64,
    pub exif: u64,
    pub xmp: u64,
}

impl MetadataLengths {
    pub fn total(self) -> Result<u64> {
        self.icc
            .checked_add(self.exif)
            .and_then(|n| n.checked_add(self.xmp))
            .ok_or_else(|| Error::InvalidInput("metadata length overflow".into()))
    }
    pub fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            icc: metadata.icc.as_ref().map_or(0, |v| v.len() as u64),
            exif: metadata.exif.as_ref().map_or(0, |v| v.len() as u64),
            xmp: metadata.xmp.as_ref().map_or(0, |v| v.len() as u64),
        }
    }
    pub fn read(self, reader: &mut impl Read, maximum: u64) -> Result<Metadata> {
        if self.total()? > maximum {
            return Err(Error::ResourceLimit {
                resource: ResourceKind::Metadata,
                requested: self.total()?,
                limit: maximum,
            });
        }
        fn part(reader: &mut impl Read, length: u64) -> Result<Option<Vec<u8>>> {
            if length == 0 {
                return Ok(None);
            }
            Ok(Some(read_bytes(reader, length)?))
        }
        Ok(Metadata {
            icc: part(reader, self.icc)?,
            exif: part(reader, self.exif)?,
            xmp: part(reader, self.xmp)?,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Operation {
    Capabilities,
    Inspect(DecodeOptions),
    Decode(DecodeOptions),
    Preview {
        options: DecodeOptions,
        max_dimension: u32,
    },
    Encode {
        descriptor: ImageDescriptor,
        options: EncodeOptions,
    },
    Transform {
        descriptor: ImageDescriptor,
        options: TransformOptions,
    },
    AnimationOpen(DecodeOptions),
    AnimationAdvance {
        session: u64,
    },
    AnimationReset {
        session: u64,
    },
    AnimationClose {
        session: u64,
    },
}

/// Control messages remain on the pipe. Static encoded input can reuse the
/// publication file or arrive as a separate immutable, read-only OS capability.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum PayloadStorage {
    #[default]
    Pipe,
    Backing,
    InputHandle,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    pub generation: u64,
    pub id: u64,
    pub policy: ResourcePolicy,
    pub operation: Operation,
    pub input_len: u64,
    #[serde(default)]
    pub payload_storage: PayloadStorage,
    pub metadata: MetadataLengths,
}

impl Request {
    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION || self.generation == 0 || self.id == 0 {
            return Err(Error::InvalidInput(
                "invalid worker protocol identity".into(),
            ));
        }
        if matches!(
            self.payload_storage,
            PayloadStorage::Backing | PayloadStorage::InputHandle
        ) && (!matches!(
            self.operation,
            Operation::Inspect(_) | Operation::Decode(_) | Operation::Preview { .. }
        ) || self.input_len == 0
            || self.metadata.total()? != 0)
        {
            return Err(Error::InvalidInput(
                "handle-backed input requires a nonempty static encoded image without external metadata"
                    .into(),
            ));
        }
        let maximum = match &self.operation {
            Operation::Encode { descriptor, .. } | Operation::Transform { descriptor, .. } => {
                descriptor.validate()?;
                if descriptor.required_bytes()? as u64 != self.input_len {
                    return Err(Error::InvalidInput("raster payload length mismatch".into()));
                }
                self.policy.max_raster_bytes
            }
            _ => self.policy.max_input_bytes,
        };
        if self.input_len > maximum {
            return Err(Error::ResourceLimit {
                resource: ResourceKind::Input,
                requested: self.input_len,
                limit: maximum,
            });
        }
        if self.metadata.total()? > self.policy.max_metadata_bytes {
            return Err(Error::ResourceLimit {
                resource: ResourceKind::Metadata,
                requested: self.metadata.total()?,
                limit: self.policy.max_metadata_bytes,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(
    clippy::large_enum_variant,
    reason = "Run owns one fixed-size descriptor; boxing adds a separate allocation to every bounded control request"
)]
pub enum Command {
    Run(Request),
    Cancel { generation: u64, id: u64 },
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnimationDescription {
    pub session: u64,
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
    pub loop_count: u32,
    pub durations_ms: Vec<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Reply {
    SharedImage {
        descriptor: ImageDescriptor,
        metadata: MetadataLengths,
        bytes: u64,
    },
    BackedImage {
        descriptor: ImageDescriptor,
        metadata: MetadataLengths,
        bytes: u64,
    },
    BackedRaster {
        descriptor: ImageDescriptor,
        metadata: MetadataLengths,
        bytes: u64,
    },
    BackedEncoded {
        receipt: EncodeReceipt,
        bytes: u64,
    },
    Capabilities(Vec<CodecCapability>),
    Info {
        info: ImageInfo,
        metadata: MetadataLengths,
    },
    Image {
        descriptor: ImageDescriptor,
        metadata: MetadataLengths,
        bytes: u64,
    },
    Raster {
        descriptor: ImageDescriptor,
        metadata: MetadataLengths,
        bytes: u64,
    },
    Encoded {
        receipt: EncodeReceipt,
        bytes: u64,
    },
    AnimationOpened(AnimationDescription),
    AnimationFrame {
        descriptor: ImageDescriptor,
        metadata: MetadataLengths,
        bytes: u64,
        index: u32,
        duration_ms: u64,
    },
    Done,
    Failure(RemoteError),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub version: u32,
    pub generation: u64,
    pub id: u64,
    pub reply: Reply,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteError {
    Cancelled,
    BackendLimit(String),
    Limit {
        resource: ResourceKind,
        requested: u64,
        limit: u64,
    },
    Invalid(String),
    Unsupported(String),
    Failed(String),
}

impl From<Error> for RemoteError {
    fn from(error: Error) -> Self {
        match error {
            Error::Cancelled => Self::Cancelled,
            Error::BackendResourceLimit(message) => Self::BackendLimit(message.into_string()),
            Error::ResourceLimit {
                resource,
                requested,
                limit,
            } => Self::Limit {
                resource,
                requested,
                limit,
            },
            Error::InvalidInput(message) => Self::Invalid(message.into_string()),
            Error::Unsupported(message) => Self::Unsupported(message.into_string()),
            other => Self::Failed(other.to_string()),
        }
    }
}

impl From<RemoteError> for Error {
    fn from(error: RemoteError) -> Self {
        match error {
            RemoteError::Cancelled => Self::Cancelled,
            RemoteError::BackendLimit(message) => Self::BackendResourceLimit(message.into()),
            RemoteError::Limit {
                resource,
                requested,
                limit,
            } => Self::ResourceLimit {
                resource,
                requested,
                limit,
            },
            RemoteError::Invalid(message) => Self::InvalidInput(message.into()),
            RemoteError::Unsupported(message) => Self::Unsupported(message.into()),
            RemoteError::Failed(message) => Self::Worker(message.into()),
        }
    }
}

impl RemoteError {
    /// Keep diagnostic bytes admitted for as long as the returned error is owned.
    pub fn into_accounted_error(self, budget: &Budget) -> Error {
        let result = match self {
            Self::BackendLimit(message) => {
                Diagnostic::from_remote(&message, budget).map(Error::BackendResourceLimit)
            }
            Self::Invalid(message) => {
                Diagnostic::from_remote(&message, budget).map(Error::InvalidInput)
            }
            Self::Unsupported(message) => {
                Diagnostic::from_remote(&message, budget).map(Error::Unsupported)
            }
            Self::Failed(message) => Diagnostic::from_remote(&message, budget).map(Error::Worker),
            other => return other.into(),
        };
        match result {
            Ok(error) | Err(error) => error,
        }
    }
}

pub fn write_control(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let mut bytes = Vec::new();
    {
        let mut capped = CappedWriter::new(&mut bytes, MAX_CONTROL_BYTES as u64);
        serde_json::to_writer(&mut capped, value)
            .map_err(|e| Error::InvalidInput(e.to_string().into()))?;
    }
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

pub fn read_control<T: DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix)?;
    let length = u32::from_le_bytes(prefix) as usize;
    if length == 0 || length > MAX_CONTROL_BYTES {
        return Err(Error::InvalidInput(
            "worker control frame exceeds limit".into(),
        ));
    }
    let bytes = read_bytes(reader, length as u64)?;
    serde_json::from_slice(&bytes).map_err(|e| Error::InvalidInput(e.to_string().into()))
}

pub fn read_bytes(reader: &mut impl Read, length: u64) -> Result<Vec<u8>> {
    let length = usize::try_from(length).map_err(|_| Error::AllocationFailed)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| Error::AllocationFailed)?;
    // Rust 1.97.1's Take uses default_read_to_end, which fills spare capacity
    // through read_buf. With the exact bound already reserved, its full-buffer
    // EOF probe sees Take's exhausted limit without growing or reading beyond
    // this payload. The isolated allocation test guards this toolchain contract.
    reader.take(length as u64).read_to_end(&mut bytes)?;
    if bytes.len() != length {
        return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
    }
    Ok(bytes)
}

pub fn write_metadata(writer: &mut impl Write, metadata: &Metadata) -> Result<()> {
    for bytes in [&metadata.icc, &metadata.exif, &metadata.xmp]
        .into_iter()
        .flatten()
    {
        writer.write_all(bytes)?;
    }
    Ok(())
}

pub struct CappedWriter<W> {
    inner: W,
    maximum: u64,
    written: u64,
    exceeded: Option<u64>,
}
impl<W> CappedWriter<W> {
    pub fn new(inner: W, maximum: u64) -> Self {
        Self {
            inner,
            maximum,
            written: 0,
            exceeded: None,
        }
    }
    pub fn written(&self) -> u64 {
        self.written
    }
    pub fn exceeded(&self) -> Option<(u64, u64)> {
        self.exceeded.map(|requested| (requested, self.maximum))
    }
    pub fn into_inner(self) -> W {
        self.inner
    }
}
impl<W: Write> Write for CappedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.maximum.saturating_sub(self.written) {
            self.exceeded = Some(self.written.saturating_add(bytes.len() as u64));
            return Err(std::io::Error::other("output byte limit exceeded"));
        }
        let written = self.inner.write(bytes)?;
        self.written += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(operation: Operation) -> Request {
        Request {
            version: VERSION,
            generation: 1,
            id: 1,
            policy: ResourcePolicy::default(),
            operation,
            input_len: 1,
            payload_storage: PayloadStorage::Backing,
            metadata: MetadataLengths::default(),
        }
    }
    #[test]
    fn backing_transport_is_limited_to_static_encoded_input() {
        for storage in [PayloadStorage::Backing, PayloadStorage::InputHandle] {
            for operation in [
                Operation::Inspect(DecodeOptions::default()),
                Operation::Decode(DecodeOptions::default()),
                Operation::Preview {
                    options: DecodeOptions::default(),
                    max_dimension: 10,
                },
            ] {
                let mut value = request(operation);
                value.payload_storage = storage;
                assert!(value.validate().is_ok());
            }
            for operation in [
                Operation::AnimationOpen(DecodeOptions::default()),
                Operation::AnimationAdvance { session: 1 },
                Operation::Capabilities,
            ] {
                let mut value = request(operation);
                value.payload_storage = storage;
                assert!(value.validate().is_err());
            }
            let mut value = request(Operation::Decode(DecodeOptions::default()));
            value.payload_storage = storage;
            value.input_len = 0;
            assert!(value.validate().is_err());
            value.input_len = 1;
            value.metadata.icc = 1;
            assert!(value.validate().is_err());
        }
    }
    #[test]
    fn initial_version_one_messages_default_to_pipe_storage() {
        let mut value =
            serde_json::to_value(request(Operation::Decode(DecodeOptions::default()))).unwrap();
        value.as_object_mut().unwrap().remove("payload_storage");
        let restored: Request = serde_json::from_value(value).unwrap();
        assert_eq!(restored.payload_storage, PayloadStorage::Pipe);
        assert!(restored.validate().is_ok());
    }
    #[test]
    fn rejects_oversized_control_before_reading_body() {
        let bytes = ((MAX_CONTROL_BYTES + 1) as u32).to_le_bytes();
        assert!(read_control::<Command>(&mut &bytes[..]).is_err());
    }
    #[test]
    fn caps_outputs_without_partial_over_limit_write() {
        let mut writer = CappedWriter::new(Vec::new(), 3);
        writer.write_all(b"ab").unwrap();
        assert!(writer.write_all(b"cd").is_err());
        assert_eq!(writer.exceeded(), Some((4, 3)));
        assert_eq!(writer.into_inner(), b"ab");
    }
    #[test]
    fn rejects_metadata_overflow_and_truncation() {
        assert!(
            MetadataLengths {
                icc: u64::MAX,
                exif: 1,
                xmp: 0
            }
            .total()
            .is_err()
        );
        assert!(
            MetadataLengths {
                icc: 4,
                exif: 0,
                xmp: 0
            }
            .read(&mut &b"x"[..], 4)
            .is_err()
        );
    }
}
