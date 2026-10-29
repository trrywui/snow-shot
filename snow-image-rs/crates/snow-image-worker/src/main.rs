//! Dedicated codec process. Stdout is reserved for the versioned binary protocol.
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    sync::{Arc, Mutex, mpsc},
};

use snow_image_codecs as codecs;
use snow_image_core::*;
use snow_image_protocol::*;

mod backing;
mod startup;
mod stream;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Arguments {
    persistent: bool,
    address_space_limit: Option<u64>,
    output_backing: bool,
    output_handoff: bool,
}

fn arguments(args: impl Iterator<Item = String>) -> Result<Arguments> {
    let mut result = Arguments::default();
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--persistent" if !result.persistent => result.persistent = true,
            "--output-backing" if !result.output_backing => result.output_backing = true,
            "--output-handoff" if !result.output_handoff => result.output_handoff = true,
            "--address-space-limit" if result.address_space_limit.is_none() => {
                result.address_space_limit = Some(
                    args.next()
                        .ok_or_else(|| Error::InvalidInput("missing address-space limit".into()))?
                        .parse::<u64>()
                        .ok()
                        .filter(|value| *value > 0)
                        .ok_or_else(|| Error::InvalidInput("invalid address-space limit".into()))?,
                );
            }
            _ => {
                return Err(Error::InvalidInput(
                    format!("unknown or duplicate worker argument: {arg}").into(),
                ));
            }
        }
    }
    if (result.persistent && (result.output_backing || result.output_handoff))
        || (result.output_backing && result.output_handoff)
    {
        return Err(Error::InvalidInput(
            "persistent workers cannot publish mapped output".into(),
        ));
    }
    Ok(result)
}

fn apply_address_space_limit(limit: Option<u64>) -> Result<()> {
    #[cfg(unix)]
    if let Some(limit) = limit {
        let value = libc::rlim_t::try_from(limit)
            .map_err(|_| Error::InvalidInput("address-space limit overflow".into()))?;
        let limits = libc::rlimit {
            rlim_cur: value,
            rlim_max: value,
        };
        // SAFETY: setrlimit only reads this initialized, correctly aligned local rlimit.
        if unsafe { libc::setrlimit(libc::RLIMIT_AS, &limits) } != 0 {
            return Err(Error::Io(io::Error::last_os_error()));
        }
    }
    #[cfg(not(unix))]
    if limit.is_some() {
        return Err(Error::Unsupported(
            "address-space limits are configured by the parent Job Object on Windows".into(),
        ));
    }
    Ok(())
}

enum EncodedBytes {
    Owned(Vec<u8>),
    #[cfg(target_os = "macos")]
    Mapped(memmap2::Mmap),
}
impl std::ops::Deref for EncodedBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            #[cfg(target_os = "macos")]
            Self::Mapped(mapping) => mapping,
        }
    }
}
impl From<Vec<u8>> for EncodedBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Owned(bytes)
    }
}

enum Payload {
    Empty,
    Encoded {
        bytes: EncodedBytes,
        _reservation: Reservation,
    },
    Raster(FileRasterSource),
    StreamRaster(stream::StreamRaster),
}
struct Work {
    request: Request,
    payload: Result<Payload>,
    budget: Budget,
    cancellation: CancellationToken,
    // The input file remains allocated during decoding and until publication replaces
    // it. Keep its charge for the whole operation, even after the input Vec is freed.
    _backing_input_disk: Option<Reservation>,
}

#[derive(Default)]
struct ActiveRequest {
    identity: Option<(u64, u64)>,
    token: CancellationToken,
}

enum Incoming {
    Work(Box<Work>),
    Fatal(Error),
}

fn read_payload(
    reader: &mut impl Read,
    request: &Request,
    budget: &Budget,
    cancellation: &CancellationToken,
) -> Result<Payload> {
    validate_payload(request)?;
    let metadata = read_metadata(reader, request, budget)?;
    cancellation.check()?;
    match &request.operation {
        Operation::Encode {
            descriptor,
            options,
        } => {
            options.validate()?;
            Ok(Payload::Raster(
                FileRasterSource::from_reader_with_metadata(
                    descriptor.clone(),
                    reader.take(request.input_len),
                    metadata,
                    budget,
                    cancellation,
                )?,
            ))
        }
        Operation::Transform {
            descriptor,
            options,
        } => {
            options.validate()?;
            Ok(Payload::Raster(
                FileRasterSource::from_reader_with_metadata(
                    descriptor.clone(),
                    reader.take(request.input_len),
                    metadata,
                    budget,
                    cancellation,
                )?,
            ))
        }
        Operation::Capabilities
        | Operation::AnimationAdvance { .. }
        | Operation::AnimationReset { .. }
        | Operation::AnimationClose { .. } => Ok(Payload::Empty),
        _ => {
            let mut reservation = budget.reserve(ResourceKind::Input, request.input_len)?;
            let bytes = read_bytes(reader, request.input_len)?;
            reservation.resize(bytes.capacity() as u64)?;
            Ok(Payload::Encoded {
                bytes: bytes.into(),
                _reservation: reservation,
            })
        }
    }
}

fn validate_payload(request: &Request) -> Result<()> {
    request.validate()?;
    request.policy.validate()?;
    let raster = matches!(
        request.operation,
        Operation::Encode { .. } | Operation::Transform { .. }
    );
    if !raster && request.metadata.total()? != 0 {
        return Err(Error::InvalidInput(
            "only raw raster requests may carry external metadata".into(),
        ));
    }
    if matches!(
        request.operation,
        Operation::Capabilities
            | Operation::AnimationAdvance { .. }
            | Operation::AnimationReset { .. }
            | Operation::AnimationClose { .. }
    ) && request.input_len != 0
    {
        return Err(Error::InvalidInput(
            "session commands cannot carry a payload".into(),
        ));
    }
    Ok(())
}

fn read_metadata(
    reader: &mut impl Read,
    request: &Request,
    budget: &Budget,
) -> Result<SharedMetadata> {
    let metadata_charge = budget.reserve(ResourceKind::Metadata, request.metadata.total()?)?;
    let metadata = request
        .metadata
        .read(reader, budget.policy().max_metadata_bytes)?;
    SharedMetadata::from_reserved(metadata, metadata_charge)
}

fn read_backed_payload(
    file: &mut File,
    request: &Request,
    budget: &Budget,
    cancellation: &CancellationToken,
) -> Result<Payload> {
    validate_payload(request)?;
    if request.payload_storage != PayloadStorage::Backing {
        return Err(Error::InvalidInput(
            "input is not assigned to backing storage".into(),
        ));
    }
    let status = file.metadata()?;
    if !status.is_file() || status.len() != request.input_len {
        return Err(Error::InvalidInput(
            "encoded input backing length differs from declaration".into(),
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    // read_payload reserves Input before allocating the encoded Vec. This File
    // clone is dropped before Work is sent, so dispatch can safely reuse its offset
    // and truncate the publication file only after decoding has completed.
    read_payload(file, request, budget, cancellation)
}

#[cfg(target_os = "macos")]
fn read_handle_payload(
    socket: &File,
    request: &Request,
    budget: &Budget,
    cancellation: &CancellationToken,
) -> Result<Payload> {
    validate_payload(request)?;
    if request.payload_storage != PayloadStorage::InputHandle {
        return Err(Error::InvalidInput(
            "input handle storage was not declared".into(),
        ));
    }
    let length = usize::try_from(request.input_len).map_err(|_| Error::AllocationFailed)?;
    let allocation = snow_image_transport::allocation_len(length)?;
    let reservation = budget.reserve(ResourceKind::Input, allocation as u64)?;
    cancellation.check()?;
    let file = snow_image_transport::receive_input(socket, request.input_len)?;
    cancellation.check()?;
    // SAFETY: the supervisor snapshots into a private file, opens its read-only
    // capability, closes its sole writable owner and unlinks the name before
    // transfer. The transport verifies O_RDONLY, a regular unlinked file, and
    // its exact length. Neither process can change these bytes. Mapping does
    // not share a seek position with concurrent readers of this same snapshot.
    let mapping = unsafe { memmap2::MmapOptions::new().len(length).map(&file)? };
    Ok(Payload::Encoded {
        bytes: EncodedBytes::Mapped(mapping),
        _reservation: reservation,
    })
}

/// The reader keeps consuming control messages while codecs execute on the main thread.
/// A bounded queue and parent serialization keep request payloads bounded.
fn input_thread(
    sender: mpsc::SyncSender<Incoming>,
    active: Arc<Mutex<ActiveRequest>>,
    mut input_backing: Option<File>,
    mut input_handoff: Option<File>,
    persistent: bool,
) {
    #[cfg(not(target_os = "macos"))]
    let _ = &mut input_handoff;
    let mut reader = io::stdin().lock();
    let mut configuration: Option<(u64, Budget)> = None;
    let mut last_id = 0;
    loop {
        let command = match read_control::<Command>(&mut reader) {
            Ok(command) => command,
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                // Parent death must terminate native work too. All temporary storage is anonymous.
                std::process::exit(0);
            }
            Err(error) => {
                let _ = sender.send(Incoming::Fatal(error));
                return;
            }
        };
        match command {
            Command::Shutdown => std::process::exit(0),
            Command::Cancel { generation, id } => {
                let state = active
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.identity == Some((generation, id)) {
                    state.token.cancel();
                }
            }
            Command::Run(request) => {
                if let Err(error) = request.validate().and_then(|()| request.policy.validate()) {
                    let _ = sender.send(Incoming::Fatal(error));
                    return;
                }
                if !persistent && last_id != 0 {
                    let _ = sender.send(Incoming::Fatal(Error::InvalidInput(
                        "static workers accept only one operation".into(),
                    )));
                    return;
                }
                if request.id <= last_id {
                    let _ = sender.send(Incoming::Fatal(Error::InvalidInput(
                        "request identifiers must increase".into(),
                    )));
                    return;
                }
                last_id = request.id;
                let budget = if let Some((generation, budget)) = &configuration {
                    if *generation != request.generation || *budget.policy() != request.policy {
                        let _ = sender.send(Incoming::Fatal(Error::InvalidInput(
                            "worker generation or resource policy changed".into(),
                        )));
                        return;
                    }
                    budget.clone()
                } else {
                    let budget = Budget::new(request.policy.clone());
                    configuration = Some((request.generation, budget.clone()));
                    budget
                };
                let cancellation = CancellationToken::new();
                {
                    let mut state = active
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.identity = Some((request.generation, request.id));
                    state.token = cancellation.clone();
                }
                if let Operation::Encode {
                    descriptor,
                    options,
                } = &request.operation
                    && matches!(
                        options,
                        EncodeOptions::Png(_) | EncodeOptions::Jpeg(_) | EncodeOptions::Bmp(_)
                    )
                {
                    let streams = (|| {
                        validate_payload(&request)?;
                        options.validate()?;
                        let metadata = read_metadata(&mut reader, &request, &budget)?;
                        stream::channel(descriptor.clone(), metadata, &budget, cancellation.clone())
                    })();
                    let (payload, producer) = match streams {
                        Ok((source, producer)) => {
                            (Ok(Payload::StreamRaster(source)), Some(producer))
                        }
                        Err(error) => (Err(error), None),
                    };
                    if sender
                        .send(Incoming::Work(Box::new(Work {
                            request,
                            payload,
                            budget,
                            cancellation,
                            _backing_input_disk: None,
                        })))
                        .is_err()
                    {
                        return;
                    }
                    if producer.is_none_or(|producer| producer.read_from(&mut reader).is_err()) {
                        return;
                    }
                    continue;
                }
                let mut backing_input_disk = None;
                let payload = match request.payload_storage {
                    PayloadStorage::Pipe => {
                        read_payload(&mut reader, &request, &budget, &cancellation)
                    }
                    PayloadStorage::Backing => match input_backing.take() {
                        Some(mut file) if !persistent => {
                            match budget.reserve(ResourceKind::TempDisk, request.input_len) {
                                Ok(reservation) => {
                                    backing_input_disk = Some(reservation);
                                    read_backed_payload(&mut file, &request, &budget, &cancellation)
                                }
                                Err(error) => Err(error),
                            }
                        }
                        _ => Err(Error::InvalidInput(
                            "backed input requires a static publication handle".into(),
                        )),
                    },
                    PayloadStorage::InputHandle => {
                        #[cfg(target_os = "macos")]
                        {
                            match input_handoff.take() {
                                Some(socket) if !persistent => {
                                    read_handle_payload(&socket, &request, &budget, &cancellation)
                                }
                                _ => Err(Error::InvalidInput(
                                    "input handle requires a static handoff channel".into(),
                                )),
                            }
                        }
                        #[cfg(not(target_os = "macos"))]
                        Err(Error::Unsupported(
                            "input handle transport is unavailable".into(),
                        ))
                    }
                };
                let failed = payload.is_err();
                if sender
                    .send(Incoming::Work(Box::new(Work {
                        request,
                        payload,
                        budget,
                        cancellation,
                        _backing_input_disk: backing_input_disk,
                    })))
                    .is_err()
                    || failed
                {
                    return;
                }
            }
        }
    }
}

enum Output {
    #[cfg(target_os = "macos")]
    SharedImage {
        descriptor: ImageDescriptor,
        metadata: SharedMetadata,
        _pixels: Reservation,
    },
    Capabilities(Vec<CodecCapability>),
    Info {
        info: ImageInfo,
        _metadata: Reservation,
    },
    Image(Image),
    ImageBacking {
        descriptor: ImageDescriptor,
        metadata: SharedMetadata,
        _pixels: Reservation,
        _disk: Reservation,
    },
    Raster(FileRasterSource),
    Encoded {
        file: File,
        receipt: EncodeReceipt,
        _reservation: Reservation,
    },
    EncodedBacking {
        receipt: EncodeReceipt,
        _reservation: Reservation,
    },
    AnimationOpened(AnimationDescription),
    AnimationFrame(codecs::AnimationFrame),
    Done,
}

struct Sessions {
    values: HashMap<u64, codecs::AnimationSession>,
    next: u64,
    opened: bool,
}
impl Default for Sessions {
    fn default() -> Self {
        Self {
            values: HashMap::new(),
            next: 1,
            opened: false,
        }
    }
}

fn dispatch(
    request: &Request,
    payload: Payload,
    budget: &Budget,
    cancellation: &CancellationToken,
    sessions: &mut Sessions,
) -> Result<Output> {
    cancellation.check()?;
    match (&request.operation, payload) {
        (Operation::Capabilities, Payload::Empty) => {
            Ok(Output::Capabilities(codecs::capabilities()))
        }
        (
            Operation::Inspect(options),
            Payload::Encoded {
                bytes,
                _reservation,
            },
        ) => {
            let info = codecs::inspect(&bytes, options, budget, cancellation)?;
            let metadata =
                budget.reserve(ResourceKind::Metadata, info.metadata.allocation_bytes()?)?;
            Ok(Output::Info {
                info,
                _metadata: metadata,
            })
        }
        (
            Operation::Decode(options),
            Payload::Encoded {
                bytes,
                _reservation,
            },
        ) => codecs::decode(&bytes, options, budget, cancellation).map(Output::Image),
        (
            Operation::Preview {
                options,
                max_dimension,
            },
            Payload::Encoded {
                bytes,
                _reservation,
            },
        ) => {
            if *max_dimension == 0 || *max_dimension > budget.policy().preview_max_dimension {
                return Err(Error::InvalidInput("preview size exceeds policy".into()));
            }
            // Preview follows the public source orientation and color policy. Explicit frame
            // selection or metadata removal must be honored rather than silently discarded.
            if options == &DecodeOptions::default() {
                codecs::preview(
                    &bytes,
                    (*max_dimension, *max_dimension),
                    budget,
                    cancellation,
                )
                .map(Output::Image)
            } else {
                let image = codecs::decode(&bytes, options, budget, cancellation)?;
                codecs::transform(
                    &image,
                    &TransformOptions {
                        max_dimensions: Some((*max_dimension, *max_dimension)),
                        ..TransformOptions::default()
                    },
                    budget,
                    cancellation,
                )
                .map(Output::Image)
            }
        }
        (Operation::Transform { options, .. }, Payload::Raster(source)) => {
            if source.descriptor().orientation == Orientation::Normal {
                codecs::transform_source(&source, options, budget, cancellation).map(Output::Raster)
            } else {
                // Rotation requires random access. Admit the full working image first; a
                // resource-limit failure must never silently ignore its orientation.
                let descriptor = source.descriptor().clone();
                let metadata = SharedMetadata::try_copy_from(
                    source.metadata().unwrap_or(&Metadata::default()),
                    budget,
                )?;
                let mut image = MutableImage::new_shared(descriptor.clone(), metadata, budget)?;
                let row_bytes = descriptor.row_bytes()?;
                for (row, output) in image
                    .bytes_mut()
                    .chunks_exact_mut(descriptor.row_stride)
                    .enumerate()
                {
                    cancellation.check()?;
                    source.read_rows(row as u32, 1, &mut output[..row_bytes])?;
                }
                let image = codecs::transform(&image.seal(), options, budget, cancellation)?;
                let descriptor = image.descriptor().clone();
                FileRasterSource::from_reader_with_metadata(
                    descriptor,
                    image.bytes(),
                    image.shared_metadata(),
                    budget,
                    cancellation,
                )
                .map(Output::Raster)
            }
        }
        (Operation::Encode { options, .. }, Payload::Raster(source)) => {
            encode_output(&source, options, budget, cancellation)
        }
        (Operation::Encode { options, .. }, Payload::StreamRaster(source)) => {
            let output = encode_output(&source, options, budget, cancellation)?;
            source.finish()?;
            Ok(output)
        }
        (
            Operation::AnimationOpen(options),
            Payload::Encoded {
                bytes,
                _reservation,
            },
        ) => {
            if sessions.values.len() >= budget.policy().max_queued_jobs as usize {
                return Err(Error::ResourceLimit {
                    resource: ResourceKind::QueuedWork,
                    requested: sessions.values.len() as u64 + 1,
                    limit: u64::from(budget.policy().max_queued_jobs),
                });
            }
            let session = codecs::AnimationSession::open_owned(
                match bytes {
                    EncodedBytes::Owned(bytes) => bytes,
                    #[cfg(target_os = "macos")]
                    EncodedBytes::Mapped(_) => {
                        return Err(Error::InvalidInput(
                            "animation input cannot be mapped".into(),
                        ));
                    }
                },
                _reservation,
                options,
                budget,
                cancellation,
            )?;
            let id = sessions.next;
            sessions.next = id
                .checked_add(1)
                .ok_or_else(|| Error::Worker("animation session identity overflow".into()))?;
            let info = session.info();
            let description = AnimationDescription {
                session: id,
                width: info.width,
                height: info.height,
                frame_count: info.frame_count,
                loop_count: info.loop_count.unwrap_or(1),
                durations_ms: Vec::new(),
            };
            sessions.values.insert(id, session);
            sessions.opened = true;
            Ok(Output::AnimationOpened(description))
        }
        (Operation::AnimationAdvance { session }, Payload::Empty) => {
            let session = sessions
                .values
                .get_mut(session)
                .ok_or_else(|| Error::InvalidInput("unknown animation session".into()))?;
            session
                .next_frame(cancellation)
                .map(|frame| frame.map_or(Output::Done, Output::AnimationFrame))
        }
        (Operation::AnimationReset { session }, Payload::Empty) => {
            sessions
                .values
                .get_mut(session)
                .ok_or_else(|| Error::InvalidInput("unknown animation session".into()))?
                .reset(cancellation)?;
            Ok(Output::Done)
        }
        (Operation::AnimationClose { session }, Payload::Empty) => {
            sessions
                .values
                .remove(session)
                .ok_or_else(|| Error::InvalidInput("unknown animation session".into()))?;
            Ok(Output::Done)
        }
        _ => Err(Error::InvalidInput(
            "operation and payload do not match".into(),
        )),
    }
}

fn encode_output(
    source: &dyn RasterSource,
    options: &EncodeOptions,
    budget: &Budget,
    cancellation: &CancellationToken,
) -> Result<Output> {
    let mut file = tempfile::tempfile()?;
    let (receipt, reservation) =
        encode_into_file(source, options, budget, cancellation, &mut file)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(Output::Encoded {
        file,
        receipt,
        _reservation: reservation,
    })
}

fn encode_into_file(
    source: &dyn RasterSource,
    options: &EncodeOptions,
    budget: &Budget,
    cancellation: &CancellationToken,
    file: &mut File,
) -> Result<(EncodeReceipt, Reservation)> {
    let maximum = budget.policy().max_artifact_bytes;
    let mut reservation = budget.reserve(ResourceKind::TempDisk, maximum)?;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    let mut writer = CappedWriter::new(file, maximum);
    let encoded = codecs::encode(source, options, &mut writer, budget, cancellation);
    if let Some((requested, limit)) = writer.exceeded() {
        return Err(Error::ResourceLimit {
            resource: ResourceKind::EncodedOutput,
            requested,
            limit,
        });
    }
    let receipt = encoded?;
    writer.flush()?;
    if receipt.bytes != writer.written() {
        return Err(Error::Codec(
            "encoder receipt length differs from output".into(),
        ));
    }
    reservation.resize(writer.written())?;
    Ok((receipt, reservation))
}

fn dispatch_backed(
    request: &Request,
    payload: Payload,
    budget: &Budget,
    cancellation: &CancellationToken,
    sessions: &mut Sessions,
    backing: &mut File,
    pixel_backing: Option<&File>,
) -> Result<Output> {
    #[cfg(not(target_os = "macos"))]
    let _ = pixel_backing;
    if let Payload::Encoded { bytes, .. } = &payload {
        let direct = match &request.operation {
            Operation::Decode(options) => {
                codecs::DirectJpegDecode::prepare(bytes, options, None, budget, cancellation)?
            }
            Operation::Preview {
                options,
                max_dimension,
            } => codecs::DirectJpegDecode::prepare(
                bytes,
                options,
                Some(*max_dimension),
                budget,
                cancellation,
            )?,
            _ => None,
        };
        if let Some(direct) = direct {
            #[cfg(target_os = "macos")]
            if let Some(pixels) = pixel_backing {
                return decode_jpeg_shared(direct, pixels, budget, cancellation);
            }
            return decode_jpeg_backing(direct, backing, budget, cancellation);
        }
    }
    let Operation::Encode { options, .. } = &request.operation else {
        return dispatch(request, payload, budget, cancellation, sessions);
    };
    // stderr no longer aliases this private handle. A codec may write to it
    // through the capped sink while the parent waits; it cannot be mapped until
    // the process tree has exited. The shared encode helper enforces the same
    // byte cap and receipt contract as pipe-based artifact output.
    let (receipt, reservation) = match payload {
        Payload::Raster(source) => {
            encode_into_file(&source, options, budget, cancellation, backing)?
        }
        Payload::StreamRaster(source) => {
            let result = encode_into_file(&source, options, budget, cancellation, backing)?;
            source.finish()?;
            result
        }
        _ => {
            return Err(Error::InvalidInput(
                "encode operation requires a raster payload".into(),
            ));
        }
    };
    Ok(Output::EncodedBacking {
        receipt,
        _reservation: reservation,
    })
}

#[cfg(target_os = "macos")]
fn decode_jpeg_shared(
    direct: codecs::DirectJpegDecode<'_>,
    backing: &File,
    budget: &Budget,
    cancellation: &CancellationToken,
) -> Result<Output> {
    let bytes = direct.descriptor().required_bytes()?;
    let allocation = snow_image_transport::allocation_len(bytes)?;
    let pixels = budget.reserve(ResourceKind::Pixels, allocation as u64)?;
    cancellation.check()?;
    snow_image_transport::validate_pixel_backing(backing, 0)?;
    // A Darwin shm object can be sized exactly once. Never truncate it on error;
    // all kernel-owned pages disappear when the supervised process and host drop it.
    backing.set_len(bytes as u64)?;
    snow_image_transport::validate_pixel_backing(backing, bytes)?;
    // SAFETY: this unlinked object was transferred by our supervisor. The worker
    // is its sole writer, and the host waits for process-tree quiescence before
    // creating any immutable mapping. No disk allocation/writeback is involved.
    let mut mapping = unsafe { memmap2::MmapOptions::new().len(bytes).map_mut(backing)? };
    let decoded = direct.decode_into(&mut mapping, budget, cancellation);
    drop(mapping);
    let (descriptor, metadata) = decoded?;
    cancellation.check()?;
    Ok(Output::SharedImage {
        descriptor,
        metadata,
        _pixels: pixels,
    })
}

fn decode_jpeg_backing(
    direct: codecs::DirectJpegDecode<'_>,
    backing: &mut File,
    budget: &Budget,
    cancellation: &CancellationToken,
) -> Result<Output> {
    let bytes = direct.descriptor().required_bytes()?;
    let pixels = budget.reserve(ResourceKind::Pixels, bytes as u64)?;
    let disk = budget.reserve(ResourceKind::TempDisk, bytes as u64)?;
    cancellation.check()?;
    let result = (|| {
        backing.set_len(bytes as u64)?;
        // SAFETY: the supervisor owns this private file and writes input only before
        // sending Run. Its input-handle clone has closed before dispatch. This worker
        // is the sole writer until exit; no immutable pixel slices have been exposed.
        let mut mapping = unsafe { memmap2::MmapOptions::new().len(bytes).map_mut(&*backing)? };
        let decoded = direct
            .decode_into(&mut mapping, budget, cancellation)
            .and_then(|image| {
                mapping.flush()?;
                Ok(image)
            });
        // Unmap on success and failure before truncation, response, or process exit.
        // write_backed_output also checks filesystem writeback before success:
        // coherent dirty pages alone do not prove that backing storage exists.
        drop(mapping);
        let (descriptor, metadata) = decoded?;
        cancellation.check()?;
        Ok(Output::ImageBacking {
            descriptor,
            metadata,
            _pixels: pixels,
            _disk: disk,
        })
    })();
    if result.is_err() {
        // No successful descriptor is sent for partial pixels. If truncation itself
        // fails, the supervisor still destroys the private file after reaping us.
        let _ = backing.set_len(0);
    }
    result
}

fn write_output(
    writer: &mut impl Write,
    request: &Request,
    output: Result<Output>,
    budget: &Budget,
) -> Result<()> {
    let response = |reply| Response {
        version: VERSION,
        generation: request.generation,
        id: request.id,
        reply,
    };
    match output {
        #[cfg(target_os = "macos")]
        Ok(Output::SharedImage {
            descriptor,
            metadata,
            _pixels,
        }) => {
            let bytes = descriptor.required_bytes()? as u64;
            write_control(
                writer,
                &response(Reply::SharedImage {
                    descriptor,
                    metadata: MetadataLengths::from_metadata(metadata.get()),
                    bytes,
                }),
            )?;
            write_metadata(writer, metadata.get())?;
            writer.flush()?;
            drop(_pixels);
        }
        Ok(Output::Capabilities(capabilities)) => {
            write_control(writer, &response(Reply::Capabilities(capabilities)))?
        }
        Err(error) => write_control(writer, &response(Reply::Failure(error.into())))?,
        Ok(Output::Info {
            mut info,
            _metadata,
        }) => {
            let metadata = std::mem::take(&mut info.metadata);
            let lengths = MetadataLengths::from_metadata(&metadata);
            write_control(
                writer,
                &response(Reply::Info {
                    info,
                    metadata: lengths,
                }),
            )?;
            write_metadata(writer, &metadata)?;
        }
        Ok(Output::Image(image)) => {
            write_control(
                writer,
                &response(Reply::Image {
                    descriptor: image.descriptor().clone(),
                    metadata: MetadataLengths::from_metadata(image.metadata()),
                    bytes: image.bytes().len() as u64,
                }),
            )?;
            write_metadata(writer, image.metadata())?;
            writer.write_all(image.bytes())?;
        }
        Ok(Output::ImageBacking {
            descriptor,
            metadata,
            _pixels,
            _disk,
        }) => {
            let bytes = descriptor.required_bytes()? as u64;
            write_control(
                writer,
                &response(Reply::BackedImage {
                    descriptor,
                    metadata: MetadataLengths::from_metadata(metadata.get()),
                    bytes,
                }),
            )?;
            write_metadata(writer, metadata.get())?;
            writer.flush()?;
            drop((_pixels, _disk));
        }
        Ok(Output::Raster(source)) => {
            let mut descriptor = source.descriptor().clone();
            descriptor.row_stride = descriptor.row_bytes()?;
            let bytes = descriptor.required_bytes()? as u64;
            let empty_metadata = Metadata::default();
            let metadata = source.metadata().unwrap_or(&empty_metadata);
            let mut reservation =
                budget.reserve(ResourceKind::Transport, descriptor.row_stride as u64)?;
            let mut row = Vec::new();
            row.try_reserve_exact(descriptor.row_stride)
                .map_err(|_| Error::AllocationFailed)?;
            reservation.resize(row.capacity() as u64)?;
            row.resize(descriptor.row_stride, 0);
            write_control(
                writer,
                &response(Reply::Raster {
                    descriptor: descriptor.clone(),
                    metadata: MetadataLengths::from_metadata(metadata),
                    bytes,
                }),
            )?;
            write_metadata(writer, metadata)?;
            for index in 0..descriptor.height {
                source.read_rows(index, 1, &mut row)?;
                writer.write_all(&row)?;
            }
        }
        Ok(Output::Encoded {
            file,
            receipt,
            _reservation,
        }) => {
            let length = receipt.bytes;
            write_control(
                writer,
                &response(Reply::Encoded {
                    receipt,
                    bytes: length,
                }),
            )?;
            let written = io::copy(&mut file.take(length), writer)?;
            if written != length {
                return Err(Error::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "artifact was truncated",
                )));
            }
        }
        Ok(Output::EncodedBacking {
            receipt,
            _reservation,
        }) => {
            let bytes = receipt.bytes;
            write_control(writer, &response(Reply::BackedEncoded { receipt, bytes }))?;
            writer.flush()?;
            drop(_reservation);
        }
        Ok(Output::AnimationOpened(description)) => {
            write_control(writer, &response(Reply::AnimationOpened(description)))?
        }
        Ok(Output::AnimationFrame(frame)) => {
            let image = frame.image;
            write_control(
                writer,
                &response(Reply::AnimationFrame {
                    descriptor: image.descriptor().clone(),
                    metadata: MetadataLengths::from_metadata(image.metadata()),
                    bytes: image.bytes().len() as u64,
                    index: frame.index,
                    duration_ms: frame
                        .duration
                        .as_millis()
                        .try_into()
                        .map_err(|_| Error::InvalidInput("frame duration overflow".into()))?,
                }),
            )?;
            write_metadata(writer, image.metadata())?;
            writer.write_all(image.bytes())?;
        }
        Ok(Output::Done) => write_control(writer, &response(Reply::Done))?,
    }
    writer.flush()?;
    Ok(())
}

/// Populate the parent's private result file only after codec execution finishes.
/// The parent must observe process exit before exposing any mapped bytes.
fn write_backed_output(
    writer: &mut impl Write,
    request: &Request,
    output: Result<Output>,
    budget: &Budget,
    file: &mut File,
) -> Result<()> {
    let output = match output {
        Ok(output @ (Output::ImageBacking { .. } | Output::EncodedBacking { .. })) => {
            let output = backing::finish_writeback(file)
                .map(|()| output)
                .map_err(Error::Io);
            return write_output(writer, request, output, budget);
        }
        Ok(output @ (Output::Image(_) | Output::Raster(_) | Output::Encoded { .. })) => output,
        other => return write_output(writer, request, other, budget),
    };
    let prepared = (|| -> Result<(Reply, Option<SharedMetadata>, Reservation)> {
        match output {
            Output::Image(image) => {
                let bytes = image.bytes().len() as u64;
                let disk = populate_backing(file, bytes, budget, |writer| {
                    writer.write_all(image.bytes())?;
                    Ok(())
                })?;
                Ok((
                    Reply::BackedImage {
                        descriptor: image.descriptor().clone(),
                        metadata: MetadataLengths::from_metadata(image.metadata()),
                        bytes,
                    },
                    Some(image.shared_metadata()),
                    disk,
                ))
            }
            Output::Raster(source) => {
                let mut descriptor = source.descriptor().clone();
                descriptor.row_stride = descriptor.row_bytes()?;
                let bytes = descriptor.required_bytes()? as u64;
                let mut row_charge =
                    budget.reserve(ResourceKind::Transport, descriptor.row_stride as u64)?;
                let mut row = Vec::new();
                row.try_reserve_exact(descriptor.row_stride)
                    .map_err(|_| Error::AllocationFailed)?;
                row_charge.resize(row.capacity() as u64)?;
                row.resize(descriptor.row_stride, 0);
                let disk = populate_backing(file, bytes, budget, |writer| {
                    for index in 0..descriptor.height {
                        source.read_rows(index, 1, &mut row)?;
                        writer.write_all(&row)?;
                    }
                    Ok(())
                })?;
                let metadata = source.shared_metadata();
                let lengths = metadata
                    .as_ref()
                    .map_or_else(MetadataLengths::default, |value| {
                        MetadataLengths::from_metadata(value.get())
                    });
                Ok((
                    Reply::BackedRaster {
                        descriptor,
                        metadata: lengths,
                        bytes,
                    },
                    metadata,
                    disk,
                ))
            }
            Output::Encoded {
                file: encoded,
                receipt,
                _reservation,
            } => {
                let bytes = receipt.bytes;
                let disk = populate_backing(file, bytes, budget, |writer| {
                    if io::copy(&mut encoded.take(bytes), writer)? != bytes {
                        return Err(Error::Io(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "encoded result was truncated",
                        )));
                    }
                    Ok(())
                })?;
                Ok((Reply::BackedEncoded { receipt, bytes }, None, disk))
            }
            _ => unreachable!("only static owned output reaches backing publication"),
        }
    })();
    let (reply, metadata, _disk) = match prepared {
        Ok(result) => result,
        Err(error) => return write_output(writer, request, Err(error), budget),
    };
    if let Err(error) = backing::finish_writeback(file) {
        return write_output(writer, request, Err(error.into()), budget);
    }
    write_control(
        writer,
        &Response {
            version: VERSION,
            generation: request.generation,
            id: request.id,
            reply,
        },
    )?;
    if let Some(metadata) = metadata {
        write_metadata(writer, metadata.get())?;
    }
    writer.flush()?;
    Ok(())
}

fn populate_backing(
    file: &mut File,
    bytes: u64,
    budget: &Budget,
    populate: impl FnOnce(&mut CappedWriter<&mut File>) -> Result<()>,
) -> Result<Reservation> {
    let reservation = budget.reserve(ResourceKind::TempDisk, bytes)?;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    let mut writer = CappedWriter::new(&mut *file, bytes);
    populate(&mut writer)?;
    if writer.written() != bytes {
        return Err(Error::InvalidInput(
            "result backing length differs from declaration".into(),
        ));
    }
    writer.flush()?;
    if file.metadata()?.len() != bytes {
        return Err(Error::InvalidInput(
            "result backing file length differs from declaration".into(),
        ));
    }
    Ok(reservation)
}

fn run(arguments: Arguments) -> Result<()> {
    let (mut output_backing, pixel_backing, input_handoff) = if arguments.output_handoff {
        #[cfg(target_os = "macos")]
        {
            let (output, pixels, socket) = backing::take_output_handoff()?;
            (Some(output), Some(pixels), Some(socket))
        }
        #[cfg(not(target_os = "macos"))]
        return Err(Error::Unsupported(
            "shared output handoff is unavailable on this platform".into(),
        ));
    } else if arguments.output_backing {
        (Some(backing::take_output_backing()?), None, None)
    } else {
        (None, None, None)
    };
    apply_address_space_limit(arguments.address_space_limit)?;
    #[cfg(unix)]
    {
        // A streaming producer can be blocked behind native work and therefore
        // unable to observe stdin EOF. Parent identity is an independent death
        // signal; anonymous temporary files are reclaimed by process exit.
        // SAFETY: getppid takes no arguments and does not access Rust memory.
        let parent = unsafe { libc::getppid() };
        std::thread::Builder::new()
            .name("snow-image-parent".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    // SAFETY: same as the initial getppid call above.
                    if parent <= 1 || unsafe { libc::getppid() } != parent {
                        std::process::exit(0);
                    }
                }
            })?;
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let active = Arc::new(Mutex::new(ActiveRequest::default()));
    let input_backing = output_backing.as_ref().map(File::try_clone).transpose()?;
    std::thread::Builder::new()
        .name("snow-image-control".into())
        .spawn(move || {
            input_thread(
                sender,
                active,
                input_backing,
                input_handoff,
                arguments.persistent,
            )
        })?;
    let mut sessions = Sessions::default();
    let mut stdout = io::stdout().lock();
    while let Ok(incoming) = receiver.recv() {
        let work = match incoming {
            Incoming::Fatal(error) => return Err(error),
            Incoming::Work(work) => work,
        };
        let Work {
            request,
            payload,
            budget,
            cancellation,
            _backing_input_disk,
        } = *work;
        let result = payload.and_then(|payload| {
            if let Some(file) = &mut output_backing {
                dispatch_backed(
                    &request,
                    payload,
                    &budget,
                    &cancellation,
                    &mut sessions,
                    file,
                    pixel_backing.as_ref(),
                )
            } else {
                dispatch(&request, payload, &budget, &cancellation, &mut sessions)
            }
        });
        if let Some(file) = &mut output_backing {
            write_backed_output(&mut stdout, &request, result, &budget, file)?;
        } else {
            write_output(&mut stdout, &request, result, &budget)?;
        }
        if !arguments.persistent || (sessions.opened && sessions.values.is_empty()) {
            break;
        }
    }
    Ok(())
}

fn main() {
    // SAFETY: this is the first worker action, before any application file owner
    // or thread exists. Only fd 0/1/2 are intentionally inherited on Unix.
    let startup = unsafe { startup::close_unexpected_descriptors() }.map_err(Error::Io);
    if let Err(error) = startup
        .and_then(|()| arguments(std::env::args().skip(1)))
        .and_then(run)
    {
        eprintln!("snow-image-worker: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(operation: Operation, length: u64) -> Request {
        Request {
            version: VERSION,
            generation: 1,
            id: 1,
            policy: ResourcePolicy::default(),
            operation,
            input_len: length,
            payload_storage: PayloadStorage::Pipe,
            metadata: MetadataLengths::default(),
        }
    }
    #[test]
    fn arguments_reject_ambiguous_or_unbounded_limits() {
        assert!(
            arguments(
                ["--address-space-limit", "0"]
                    .into_iter()
                    .map(str::to_owned)
            )
            .is_err()
        );
        assert!(arguments(["--address-space-limit"].into_iter().map(str::to_owned)).is_err());
        assert!(
            arguments(
                ["--persistent", "--persistent"]
                    .into_iter()
                    .map(str::to_owned)
            )
            .is_err()
        );
        assert_eq!(
            arguments(
                ["--persistent", "--address-space-limit", "4096"]
                    .into_iter()
                    .map(str::to_owned)
            )
            .unwrap(),
            Arguments {
                persistent: true,
                address_space_limit: Some(4096),
                output_backing: false,
                output_handoff: false,
            }
        );
    }
    #[test]
    fn truncated_input_releases_reservations() {
        let budget = Budget::default();
        let request = request(Operation::Decode(DecodeOptions::default()), 10);
        assert!(
            read_payload(
                &mut &b"short"[..],
                &request,
                &budget,
                &CancellationToken::new()
            )
            .is_err()
        );
        assert_eq!(budget.total_used(), 0);
    }
    #[test]
    fn backed_encoded_input_is_owned_before_publication_reuses_the_file() {
        let budget = Budget::default();
        let mut request = request(Operation::Decode(DecodeOptions::default()), 5);
        request.payload_storage = PayloadStorage::Backing;
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"input").unwrap();
        // The producer leaves the file offset at EOF; the worker must seek itself.
        let payload =
            read_backed_payload(&mut file, &request, &budget, &CancellationToken::new()).unwrap();
        assert_eq!(budget.used(ResourceKind::Input), 5);
        file.set_len(0).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"output replaces input").unwrap();
        let Payload::Encoded {
            bytes,
            _reservation,
        } = payload
        else {
            panic!("expected owned encoded input");
        };
        assert_eq!(&*bytes, b"input");
        drop(bytes);
        drop(_reservation);
        assert_eq!(budget.total_used(), 0);
    }
    #[cfg(target_os = "macos")]
    fn input_handle_fixture(data: &[u8]) -> (std::os::unix::net::UnixDatagram, File, File) {
        let mut named = tempfile::NamedTempFile::new().unwrap();
        named.write_all(data).unwrap();
        named.flush().unwrap();
        let readonly = File::open(named.path()).unwrap();
        drop(named);
        let (sender, receiver) = std::os::unix::net::UnixDatagram::pair().unwrap();
        (
            sender,
            File::from(std::os::fd::OwnedFd::from(receiver)),
            readonly,
        )
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn mapped_encoded_input_retains_page_accounting_without_copying_or_seeking() {
        let budget = Budget::default();
        let (sender, socket, mut file) = input_handle_fixture(b"input");
        file.seek(SeekFrom::End(0)).unwrap();
        snow_image_transport::send_input(&sender, &file).unwrap();
        let mut request = request(Operation::Inspect(DecodeOptions::default()), 5);
        request.payload_storage = PayloadStorage::InputHandle;
        let payload =
            read_handle_payload(&socket, &request, &budget, &CancellationToken::new()).unwrap();
        assert_eq!(file.stream_position().unwrap(), 5);
        assert_eq!(
            budget.used(ResourceKind::Input),
            snow_image_transport::allocation_len(5).unwrap() as u64
        );
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
        drop((sender, socket, file));
        let Payload::Encoded {
            bytes,
            _reservation,
        } = payload
        else {
            panic!("expected mapped encoded input");
        };
        assert!(matches!(&bytes, EncodedBytes::Mapped(_)));
        assert_eq!(&*bytes, b"input");
        drop(bytes);
        drop(_reservation);
        assert_eq!(budget.total_used(), 0);
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn mapped_input_admission_and_cancellation_precede_receiving_the_capability() {
        let (sender, socket, file) = input_handle_fixture(b"input");
        snow_image_transport::send_input(&sender, &file).unwrap();
        let mut request = request(Operation::Decode(DecodeOptions::default()), 5);
        request.payload_storage = PayloadStorage::InputHandle;
        let budget = Budget::new(ResourcePolicy {
            total_memory_bytes: 1,
            ..Default::default()
        });
        assert!(matches!(
            read_handle_payload(&socket, &request, &budget, &CancellationToken::new()),
            Err(Error::ResourceLimit { .. })
        ));
        assert_eq!(budget.total_used(), 0);
        let budget = Budget::default();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            read_handle_payload(&socket, &request, &budget, &cancelled),
            Err(Error::Cancelled)
        ));
        assert_eq!(budget.total_used(), 0);
        // Neither failure consumed the queued capability. Its owners remain live
        // until normal reception or socket teardown, even after the sender closes.
        drop((sender, file));
        let payload =
            read_handle_payload(&socket, &request, &budget, &CancellationToken::new()).unwrap();
        drop(payload);
        assert_eq!(budget.total_used(), 0);
    }
    #[test]
    fn backed_input_rejects_length_changes_and_reserves_before_reading() {
        let mut request = request(Operation::Inspect(DecodeOptions::default()), 5);
        request.payload_storage = PayloadStorage::Backing;
        let budget = Budget::default();
        for length in [4, 6] {
            let mut file = tempfile::tempfile().unwrap();
            file.set_len(length).unwrap();
            assert!(
                read_backed_payload(&mut file, &request, &budget, &CancellationToken::new())
                    .is_err()
            );
            assert_eq!(budget.total_used(), 0);
        }
        let budget = Budget::new(ResourcePolicy {
            max_input_bytes: 4,
            ..ResourcePolicy::default()
        });
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"input").unwrap();
        assert!(matches!(
            read_backed_payload(&mut file, &request, &budget, &CancellationToken::new()),
            Err(Error::ResourceLimit {
                resource: ResourceKind::Input,
                ..
            })
        ));
        assert_eq!(budget.total_used(), 0);
    }
    #[test]
    fn session_commands_reject_binary_payloads() {
        let budget = Budget::default();
        let request = request(Operation::AnimationAdvance { session: 1 }, 1);
        assert!(
            read_payload(&mut &b"x"[..], &request, &budget, &CancellationToken::new()).is_err()
        );
        assert_eq!(budget.total_used(), 0);
    }
    #[test]
    fn encode_payload_is_spooled_and_released() {
        let budget = Budget::default();
        let descriptor =
            ImageDescriptor::packed(1, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
        let request = request(
            Operation::Encode {
                descriptor,
                options: EncodeOptions::default(),
            },
            4,
        );
        let payload = read_payload(
            &mut &[1, 2, 3, 4][..],
            &request,
            &budget,
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(budget.used(ResourceKind::Pixels), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 4);
        drop(payload);
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    }
    #[test]
    fn unknown_session_is_a_protocol_error() {
        let request = request(Operation::AnimationAdvance { session: 99 }, 0);
        assert!(matches!(
            dispatch(
                &request,
                Payload::Empty,
                &Budget::default(),
                &CancellationToken::new(),
                &mut Sessions::default()
            ),
            Err(Error::InvalidInput(_))
        ));
    }

    #[test]
    fn mapped_output_requires_a_disposable_worker() {
        assert!(
            arguments(
                ["--persistent", "--output-backing"]
                    .into_iter()
                    .map(str::to_owned)
            )
            .is_err()
        );
        assert!(
            arguments(
                ["--output-backing", "--output-backing"]
                    .into_iter()
                    .map(str::to_owned)
            )
            .is_err()
        );
        assert!(
            arguments(["--output-backing"].into_iter().map(str::to_owned))
                .unwrap()
                .output_backing
        );
    }

    #[test]
    fn backed_image_writes_exact_file_and_only_metadata_to_pipe() {
        let budget = Budget::default();
        let descriptor =
            ImageDescriptor::packed(2, 2, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
        let image = Image::from_vec(
            descriptor.clone(),
            vec![37; 16],
            Metadata {
                icc: Some(vec![1, 2, 3]),
                ..Metadata::default()
            },
            &budget,
        )
        .unwrap();
        let request = request(Operation::Decode(DecodeOptions::default()), 0);
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&[99; 128]).unwrap();
        let mut pipe = Vec::new();
        write_backed_output(
            &mut pipe,
            &request,
            Ok(Output::Image(image)),
            &budget,
            &mut file,
        )
        .unwrap();
        let mut reader = &pipe[..];
        let response = read_control::<Response>(&mut reader).unwrap();
        let Reply::BackedImage {
            descriptor: actual,
            metadata,
            bytes,
        } = response.reply
        else {
            panic!("expected backed image");
        };
        assert_eq!(actual, descriptor);
        assert_eq!(bytes, 16);
        assert_eq!(
            metadata.read(&mut reader, 8).unwrap().icc,
            Some(vec![1, 2, 3])
        );
        assert!(reader.is_empty());
        assert_eq!(file.metadata().unwrap().len(), 16);
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, [37; 16]);
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    }

    #[test]
    fn backed_raster_strips_padding_and_retains_exact_dimensions() {
        let budget = Budget::default();
        let mut descriptor =
            ImageDescriptor::packed(1, 2, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
        descriptor.row_stride = 6;
        let source = FileRasterSource::from_reader(
            descriptor,
            &[1, 2, 3, 4, 99, 99, 5, 6, 7, 8, 99, 99][..],
            &budget,
            &CancellationToken::new(),
        )
        .unwrap();
        let request = request(Operation::Decode(DecodeOptions::default()), 0);
        let mut file = tempfile::tempfile().unwrap();
        let mut pipe = Vec::new();
        write_backed_output(
            &mut pipe,
            &request,
            Ok(Output::Raster(source)),
            &budget,
            &mut file,
        )
        .unwrap();
        let mut reader = &pipe[..];
        let response = read_control::<Response>(&mut reader).unwrap();
        assert!(
            matches!(response.reply, Reply::BackedRaster { descriptor, bytes: 8, .. } if descriptor.row_stride == 4)
        );
        assert!(reader.is_empty());
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    }

    #[test]
    fn backed_artifact_copies_finalized_bytes_without_pipe_payload() {
        let budget = Budget::default();
        let descriptor =
            ImageDescriptor::packed(2, 2, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
        let image = MutableImage::new(descriptor, Metadata::default(), &budget)
            .unwrap()
            .seal();
        let output = encode_output(
            &image,
            &EncodeOptions::default(),
            &budget,
            &CancellationToken::new(),
        )
        .unwrap();
        drop(image);
        let request = request(Operation::Decode(DecodeOptions::default()), 0);
        let mut file = tempfile::tempfile().unwrap();
        let mut pipe = Vec::new();
        write_backed_output(&mut pipe, &request, Ok(output), &budget, &mut file).unwrap();
        let mut reader = &pipe[..];
        let response = read_control::<Response>(&mut reader).unwrap();
        let Reply::BackedEncoded { receipt, bytes } = response.reply else {
            panic!("expected backed artifact");
        };
        assert_eq!(receipt.bytes, bytes);
        assert_eq!(file.metadata().unwrap().len(), bytes);
        assert!(reader.is_empty());
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut signature = [0; 8];
        file.read_exact(&mut signature).unwrap();
        assert_eq!(&signature, b"\x89PNG\r\n\x1a\n");
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    }

    #[test]
    fn streamed_encode_writes_backing_directly_under_one_artifact_allowance() {
        let budget = Budget::new(ResourcePolicy {
            max_artifact_bytes: 1024,
            max_temp_bytes: 1024,
            ..ResourcePolicy::default()
        });
        let descriptor =
            ImageDescriptor::packed(1, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
        let request = request(
            Operation::Encode {
                descriptor: descriptor.clone(),
                options: EncodeOptions::default(),
            },
            4,
        );
        let cancellation = CancellationToken::new();
        let (source, producer) = stream::channel(
            descriptor,
            SharedMetadata::new(Metadata::default(), &budget).unwrap(),
            &budget,
            cancellation.clone(),
        )
        .unwrap();
        producer.read_from(&mut &[55; 4][..]).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        let output = dispatch_backed(
            &request,
            Payload::StreamRaster(source),
            &budget,
            &cancellation,
            &mut Sessions::default(),
            &mut file,
            None,
        )
        .unwrap();
        assert!(matches!(&output, Output::EncodedBacking { .. }));
        let length = file.metadata().unwrap().len();
        assert!(length > 8);
        assert_eq!(budget.used(ResourceKind::TempDisk), length);
        let mut pipe = Vec::new();
        write_backed_output(&mut pipe, &request, Ok(output), &budget, &mut file).unwrap();
        assert_eq!(file.metadata().unwrap().len(), length);
        let mut reader = &pipe[..];
        let response = read_control::<Response>(&mut reader).unwrap();
        assert!(matches!(response.reply, Reply::BackedEncoded { bytes, .. } if bytes == length));
        assert!(reader.is_empty());
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
        assert_eq!(budget.peak_used(ResourceKind::TempDisk), 1024);
    }

    #[test]
    fn backing_admission_and_partial_writes_never_publish_success() {
        let budget = Budget::new(ResourcePolicy {
            max_temp_bytes: 4,
            ..ResourcePolicy::default()
        });
        let descriptor =
            ImageDescriptor::packed(2, 2, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
        let image = MutableImage::new(descriptor, Metadata::default(), &budget)
            .unwrap()
            .seal();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"before").unwrap();
        let request = request(Operation::Decode(DecodeOptions::default()), 0);
        let mut pipe = Vec::new();
        write_backed_output(
            &mut pipe,
            &request,
            Ok(Output::Image(image)),
            &budget,
            &mut file,
        )
        .unwrap();
        let response = read_control::<Response>(&mut &pipe[..]).unwrap();
        assert!(matches!(
            response.reply,
            Reply::Failure(RemoteError::Limit {
                resource: ResourceKind::TempDisk,
                ..
            })
        ));
        assert_eq!(file.metadata().unwrap().len(), 6);
        assert!(
            populate_backing(&mut file, 4, &budget, |writer| {
                writer.write_all(&[0; 3])?;
                Ok(())
            })
            .is_err()
        );
        assert!(
            populate_backing(&mut file, 4, &budget, |writer| {
                writer.write_all(&[0; 5])?;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    }

    #[cfg(feature = "native-backends")]
    fn jpeg_fixture() -> Vec<u8> {
        let budget = Budget::default();
        let image = Image::from_vec(
            ImageDescriptor::packed(
                16,
                9,
                PixelFormat {
                    alpha: AlphaMode::Opaque,
                    ..PixelFormat::RGBA8
                },
                ColorEncoding::Srgb,
            )
            .unwrap(),
            (0..16 * 9 * 4)
                .map(|value| (value * 17 % 256) as u8)
                .collect(),
            Metadata::default(),
            &budget,
        )
        .unwrap();
        let mut bytes = Vec::new();
        codecs::encode(
            &image,
            &EncodeOptions::Jpeg(JpegOptions::default()),
            &mut bytes,
            &budget,
            &CancellationToken::new(),
        )
        .unwrap();
        bytes
    }

    #[cfg(feature = "native-backends")]
    #[test]
    fn direct_jpeg_backing_unmaps_before_reply_and_retains_admitted_owners() {
        let bytes = jpeg_fixture();
        let budget = Budget::default();
        let expected = codecs::decode(
            &bytes,
            &DecodeOptions::default(),
            &budget,
            &CancellationToken::new(),
        )
        .unwrap();
        let expected_pixels = expected.bytes().to_vec();
        let expected_descriptor = expected.descriptor().clone();
        drop(expected);
        for operation in [
            Operation::Decode(DecodeOptions::default()),
            Operation::Preview {
                options: DecodeOptions::default(),
                max_dimension: 2048,
            },
        ] {
            let request = request(operation, bytes.len() as u64);
            let reservation = budget
                .reserve(ResourceKind::Input, bytes.len() as u64)
                .unwrap();
            let payload = Payload::Encoded {
                bytes: bytes.clone().into(),
                _reservation: reservation,
            };
            let mut file = tempfile::tempfile().unwrap();
            file.write_all(&bytes).unwrap();
            let output = dispatch_backed(
                &request,
                payload,
                &budget,
                &CancellationToken::new(),
                &mut Sessions::default(),
                &mut file,
                None,
            )
            .unwrap();
            assert!(matches!(&output, Output::ImageBacking { .. }));
            assert_eq!(budget.used(ResourceKind::Input), 0);
            assert_eq!(
                budget.used(ResourceKind::Pixels),
                expected_pixels.len() as u64
            );
            assert_eq!(
                budget.used(ResourceKind::TempDisk),
                expected_pixels.len() as u64
            );
            file.seek(SeekFrom::Start(0)).unwrap();
            let mut actual = Vec::new();
            file.read_to_end(&mut actual).unwrap();
            assert_eq!(actual, expected_pixels);
            let mut pipe = Vec::new();
            write_backed_output(&mut pipe, &request, Ok(output), &budget, &mut file).unwrap();
            let mut reader = &pipe[..];
            let Reply::BackedImage {
                descriptor,
                metadata,
                bytes,
            } = read_control::<Response>(&mut reader).unwrap().reply
            else {
                panic!("expected mapped image")
            };
            assert_eq!(descriptor, expected_descriptor);
            assert_eq!(bytes, actual.len() as u64);
            assert_eq!(metadata.total().unwrap(), 0);
            assert!(reader.is_empty());
            assert_eq!(budget.total_used(), 0);
            assert_eq!(budget.used(ResourceKind::TempDisk), 0);
        }
    }

    #[cfg(all(feature = "native-backends", target_os = "macos"))]
    #[test]
    fn shared_jpeg_output_has_exact_samples_and_page_accounting_without_disk() {
        let bytes = jpeg_fixture();
        let budget = Budget::default();
        let expected = codecs::decode(
            &bytes,
            &DecodeOptions::default(),
            &budget,
            &CancellationToken::new(),
        )
        .unwrap()
        .bytes()
        .to_vec();
        let file = snow_image_transport::create_pixel_backing().unwrap();
        let plan = codecs::DirectJpegDecode::prepare(
            &bytes,
            &DecodeOptions::default(),
            None,
            &budget,
            &CancellationToken::new(),
        )
        .unwrap()
        .unwrap();
        let output = decode_jpeg_shared(plan, &file, &budget, &CancellationToken::new()).unwrap();
        assert!(matches!(&output, Output::SharedImage { .. }));
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
        assert_eq!(
            budget.used(ResourceKind::Pixels),
            snow_image_transport::allocation_len(expected.len()).unwrap() as u64
        );
        // SAFETY: decoding has returned and unmapped its sole mutable mapping.
        // This isolated test creates no writer or concurrent access afterward.
        let mapping =
            unsafe { memmap2::MmapOptions::new().len(expected.len()).map(&file) }.unwrap();
        assert_eq!(mapping.as_ref(), expected);
        let mut pipe = Vec::new();
        write_output(
            &mut pipe,
            &request(Operation::Decode(DecodeOptions::default()), 0),
            Ok(output),
            &budget,
        )
        .unwrap();
        let response: Response = read_control(&mut pipe.as_slice()).unwrap();
        assert!(
            matches!(response.reply, Reply::SharedImage { bytes, .. } if bytes == expected.len() as u64)
        );
        drop((mapping, file));
        assert_eq!(budget.total_used(), 0);
    }

    #[cfg(all(feature = "native-backends", target_os = "macos"))]
    #[test]
    fn shared_jpeg_admission_and_cancellation_precede_shared_allocation() {
        let bytes = jpeg_fixture();
        let budget = Budget::default();
        let file = snow_image_transport::create_pixel_backing().unwrap();
        let plan = codecs::DirectJpegDecode::prepare(
            &bytes,
            &DecodeOptions::default(),
            None,
            &budget,
            &CancellationToken::new(),
        )
        .unwrap()
        .unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            decode_jpeg_shared(plan, &file, &budget, &cancel),
            Err(Error::Cancelled)
        ));
        snow_image_transport::validate_pixel_backing(&file, 0).unwrap();
        assert_eq!(budget.total_used(), 0);
        let plan = codecs::DirectJpegDecode::prepare(
            &bytes,
            &DecodeOptions::default(),
            None,
            &budget,
            &CancellationToken::new(),
        )
        .unwrap()
        .unwrap();
        let refused = Budget::new(ResourcePolicy {
            total_memory_bytes: 1,
            ..ResourcePolicy::default()
        });
        assert!(matches!(
            decode_jpeg_shared(plan, &file, &refused, &CancellationToken::new()),
            Err(Error::ResourceLimit {
                resource: ResourceKind::Pixels,
                ..
            })
        ));
        snow_image_transport::validate_pixel_backing(&file, 0).unwrap();
        assert_eq!(refused.total_used(), 0);
    }

    #[cfg(feature = "native-backends")]
    #[test]
    fn direct_jpeg_backing_failures_release_reservations_and_never_publish_pixels() {
        let bytes = jpeg_fixture();
        let budget = Budget::default();
        let mut file = tempfile::tempfile().unwrap();
        for _ in 0..16 {
            let truncated = &bytes[..bytes.len() - 2];
            let plan = codecs::DirectJpegDecode::prepare(
                truncated,
                &DecodeOptions::default(),
                None,
                &budget,
                &CancellationToken::new(),
            )
            .unwrap()
            .unwrap();
            let result = decode_jpeg_backing(plan, &mut file, &budget, &CancellationToken::new());
            assert!(result.is_err());
            assert_eq!(file.metadata().unwrap().len(), 0);
            let mut pipe = Vec::new();
            write_backed_output(
                &mut pipe,
                &request(Operation::Decode(DecodeOptions::default()), 0),
                result,
                &budget,
                &mut file,
            )
            .unwrap();
            assert!(matches!(
                read_control::<Response>(&mut &pipe[..]).unwrap().reply,
                Reply::Failure(_)
            ));
            assert_eq!(budget.total_used(), 0);
            assert_eq!(budget.used(ResourceKind::TempDisk), 0);
        }
        let constrained = Budget::new(ResourcePolicy {
            max_temp_bytes: 1,
            ..ResourcePolicy::default()
        });
        let plan = codecs::DirectJpegDecode::prepare(
            &bytes,
            &DecodeOptions::default(),
            None,
            &constrained,
            &CancellationToken::new(),
        )
        .unwrap()
        .unwrap();
        assert!(matches!(
            decode_jpeg_backing(plan, &mut file, &constrained, &CancellationToken::new()),
            Err(Error::ResourceLimit {
                resource: ResourceKind::TempDisk,
                ..
            })
        ));
        assert_eq!(file.metadata().unwrap().len(), 0);
        assert_eq!(constrained.total_used(), 0);
        let plan = codecs::DirectJpegDecode::prepare(
            &bytes,
            &DecodeOptions::default(),
            None,
            &budget,
            &CancellationToken::new(),
        )
        .unwrap()
        .unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            decode_jpeg_backing(plan, &mut file, &budget, &cancel),
            Err(Error::Cancelled)
        ));
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
        let named = tempfile::NamedTempFile::new().unwrap();
        let mut read_only = File::open(named.path()).unwrap();
        let plan = codecs::DirectJpegDecode::prepare(
            &bytes,
            &DecodeOptions::default(),
            None,
            &budget,
            &CancellationToken::new(),
        )
        .unwrap()
        .unwrap();
        assert!(matches!(
            decode_jpeg_backing(plan, &mut read_only, &budget, &CancellationToken::new()),
            Err(Error::Io(_))
        ));
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    }
}
