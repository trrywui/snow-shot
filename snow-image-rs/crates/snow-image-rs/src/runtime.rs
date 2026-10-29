//! Supervised jobs. Only bounded protocol parsing and owned storage live in the host.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::{Job, platform};
use protocol::{MetadataLengths, Operation, Reply, Request, Response};
use snow_image_core::*;
use snow_image_protocol as protocol;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
// On macOS std creates pipes/socket pairs before setting CLOEXEC with fcntl.
// Another simultaneous spawn can inherit a handle in that gap and retain a
// writer after its intended owner is reaped. Cover capability creation as well
// as command.spawn, since output handoff creates its socket pair beforehand.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

/// Process telemetry, separate from deterministic reservation accounting.
#[derive(Clone, Copy, Debug, Default)]
pub struct WorkerStatistics {
    pub spawned: u64,
    pub reaped: u64,
    pub live: u64,
    pub spawning: u64,
    /// Largest observed footprint of one worker; sampling can miss transient peaks.
    pub sampled_peak_footprint_bytes: u64,
}

#[derive(Debug, Default)]
struct WorkerCounters {
    spawned: AtomicU64,
    reaped: AtomicU64,
    live: AtomicU64,
    spawning: AtomicU64,
    footprint: AtomicU64,
}

/// A diagnostic observer that does not keep the service or its workers alive.
#[derive(Clone, Debug)]
pub struct WorkerMonitor(Arc<WorkerCounters>);
impl WorkerMonitor {
    pub fn statistics(&self) -> WorkerStatistics {
        WorkerStatistics {
            spawned: self.0.spawned.load(Ordering::Relaxed),
            reaped: self.0.reaped.load(Ordering::Relaxed),
            live: self.0.live.load(Ordering::Relaxed),
            spawning: self.0.spawning.load(Ordering::Relaxed),
            sampled_peak_footprint_bytes: self.0.footprint.load(Ordering::Relaxed),
        }
    }
}

struct SpawnGuard(Arc<WorkerCounters>);
impl Drop for SpawnGuard {
    fn drop(&mut self) {
        self.0.spawning.fetch_sub(1, Ordering::Release);
    }
}
fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Debug)]
pub struct ServiceConfig {
    pub worker_path: PathBuf,
    pub policy: ResourcePolicy,
    pub operation_timeout: Duration,
    pub cancellation_grace: Duration,
    counters: Arc<WorkerCounters>,
}
impl ServiceConfig {
    pub fn new(worker_path: impl Into<PathBuf>) -> Self {
        Self {
            worker_path: worker_path.into(),
            policy: ResourcePolicy::default(),
            operation_timeout: Duration::from_secs(300),
            cancellation_grace: Duration::from_millis(100),
            counters: Arc::new(WorkerCounters::default()),
        }
    }
}

struct PrivateFile(File);
impl PrivateFile {
    fn new() -> std::io::Result<Self> {
        Ok(Self(tempfile::tempfile()?))
    }
    fn as_file_mut(&mut self) -> &mut File {
        &mut self.0
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

#[cfg(target_os = "macos")]
fn freeze_input_snapshot(file: tempfile::NamedTempFile) -> Result<PrivateFile> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    platform::flush_snapshot(file.as_file())?;
    let original = file.as_file().metadata()?;
    let readonly = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(file.path())?;
    let reopened = readonly.metadata()?;
    if (original.dev(), original.ino(), original.len())
        != (reopened.dev(), reopened.ino(), reopened.len())
    {
        return Err(Error::Io(std::io::Error::other(
            "input snapshot identity changed",
        )));
    }
    // Drop the sole writable owner and unlink its private name before the
    // read-only capability can escape. SCM_RIGHTS cannot upgrade its access mode.
    file.close()?;
    if readonly.metadata()?.nlink() != 0 {
        return Err(Error::Io(std::io::Error::other(
            "input snapshot is still named",
        )));
    }
    Ok(PrivateFile(readonly))
}

#[derive(Debug)]
struct MappedPixels {
    mapping: memmap2::Mmap,
    _disk: Reservation,
}
impl AsRef<[u8]> for MappedPixels {
    fn as_ref(&self) -> &[u8] {
        &self.mapping
    }
}

struct BackedRaster {
    descriptor: ImageDescriptor,
    metadata: SharedMetadata,
    file: Mutex<PrivateFile>,
    _disk: Reservation,
}
impl RasterSource for BackedRaster {
    fn descriptor(&self) -> &ImageDescriptor {
        &self.descriptor
    }
    fn access(&self) -> RasterAccess {
        RasterAccess::RandomRows
    }
    fn metadata(&self) -> Option<&Metadata> {
        Some(self.metadata.get())
    }
    fn read_rows(&self, first: u32, rows: u32, dst: &mut [u8]) -> Result<()> {
        let row_bytes = self.descriptor.row_bytes()?;
        if first
            .checked_add(rows)
            .is_none_or(|end| end > self.descriptor.height)
            || row_bytes.checked_mul(rows as usize) != Some(dst.len())
        {
            return Err(Error::InvalidInput(
                "invalid backed raster row request".into(),
            ));
        }
        let mut file = lock(&self.file);
        let file = file.as_file_mut();
        file.seek(SeekFrom::Start(
            first as u64 * self.descriptor.row_stride as u64,
        ))?;
        if row_bytes == self.descriptor.row_stride {
            file.read_exact(dst)?;
        } else {
            for (index, row) in dst.chunks_exact_mut(row_bytes).enumerate() {
                file.seek(SeekFrom::Start(
                    (first as u64 + index as u64) * self.descriptor.row_stride as u64,
                ))?;
                file.read_exact(row)?;
            }
        }
        Ok(())
    }
}

struct InputStorage {
    file: Mutex<PrivateFile>,
    length: u64,
    _disk: Reservation,
}
/// Immutable private snapshot. Later changes to the original file cannot alter an image job.
#[derive(Clone)]
pub struct EncodedInput(Arc<InputStorage>);
impl EncodedInput {
    pub fn len(&self) -> u64 {
        self.0.length
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

struct InspectionStorage {
    info: ImageInfo,
    _metadata: Reservation,
}

struct CapabilityStorage {
    values: Vec<CodecCapability>,
    _metadata: Reservation,
}
/// Immutable capabilities from the linked worker, with retained metadata accounting.
#[derive(Clone)]
pub struct Capabilities(Arc<CapabilityStorage>);
impl std::ops::Deref for Capabilities {
    type Target = [CodecCapability];
    fn deref(&self) -> &Self::Target {
        &self.0.values
    }
}
impl Capabilities {
    fn from_values(values: Vec<CodecCapability>, budget: &Budget) -> Result<Self> {
        if values.len() > 8
            || values.iter().enumerate().any(|(index, value)| {
                values[..index]
                    .iter()
                    .any(|previous| previous.format == value.format)
                    || value.input_sample_types.len() > 4
                    || value
                        .input_sample_types
                        .iter()
                        .enumerate()
                        .any(|(index, sample)| value.input_sample_types[..index].contains(sample))
            })
        {
            return Err(Error::Worker("invalid capability table".into()));
        }
        let bytes = values.capacity() as u64 * std::mem::size_of::<CodecCapability>() as u64
            + values
                .iter()
                .map(|value| {
                    value.input_sample_types.capacity() as u64
                        * std::mem::size_of::<SampleType>() as u64
                })
                .sum::<u64>();
        let metadata = budget.reserve(ResourceKind::Metadata, bytes)?;
        Ok(Self(Arc::new(CapabilityStorage {
            values,
            _metadata: metadata,
        })))
    }
}
#[derive(Clone)]
pub struct Inspection(Arc<InspectionStorage>);
impl Inspection {
    pub fn info(&self) -> &ImageInfo {
        &self.0.info
    }
}
impl std::ops::Deref for Inspection {
    type Target = ImageInfo;
    fn deref(&self) -> &Self::Target {
        self.info()
    }
}

struct ArtifactStorage {
    file: Mutex<PrivateFile>,
    receipt: EncodeReceipt,
    _disk: Reservation,
    budget: Budget,
}
#[derive(Clone)]
pub struct EncodedArtifact(Arc<ArtifactStorage>);
/// A bounded memory representation whose accounting lasts as long as its bytes.
pub struct EncodedBytes {
    bytes: Vec<u8>,
    _reservation: Reservation,
}
impl AsRef<[u8]> for EncodedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
impl std::ops::Deref for EncodedBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}
#[derive(Clone, Copy, Debug)]
pub enum WriteMode {
    CreateNew,
    Replace,
}
impl EncodedArtifact {
    pub fn receipt(&self) -> &EncodeReceipt {
        &self.0.receipt
    }
    pub fn read_bytes(&self, maximum: u64) -> Result<EncodedBytes> {
        let size = self.0.receipt.bytes;
        if size > maximum {
            return Err(Error::ResourceLimit {
                resource: ResourceKind::EncodedOutput,
                requested: size,
                limit: maximum,
            });
        }
        let mut reservation = self.0.budget.reserve(ResourceKind::EncodedOutput, size)?;
        let mut file = lock(&self.0.file);
        file.as_file_mut().seek(SeekFrom::Start(0))?;
        let bytes = protocol::read_bytes(file.as_file_mut(), size)?;
        reservation.resize(bytes.capacity() as u64)?;
        Ok(EncodedBytes {
            bytes,
            _reservation: reservation,
        })
    }
    /// Atomically publish in the destination directory only after a complete copy and flush.
    pub fn save(&self, destination: impl AsRef<Path>, mode: WriteMode) -> Result<()> {
        let destination = destination.as_ref();
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let _disk = self
            .0
            .budget
            .reserve(ResourceKind::TempDisk, self.0.receipt.bytes)?;
        let mut staged = tempfile::NamedTempFile::new_in(parent)?;
        let mut source = lock(&self.0.file);
        source.as_file_mut().seek(SeekFrom::Start(0))?;
        let copied = std::io::copy(source.as_file_mut(), staged.as_file_mut())?;
        if copied != self.0.receipt.bytes {
            return Err(Error::Worker("artifact length changed".into()));
        }
        staged.as_file_mut().flush()?;
        staged.as_file().sync_all()?;
        match mode {
            WriteMode::CreateNew => {
                staged
                    .persist_noclobber(destination)
                    .map_err(|e| Error::Io(e.error))?;
            }
            WriteMode::Replace => {
                staged
                    .persist(destination)
                    .map_err(|e| Error::Io(e.error))?;
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct PreparedTransform {
    source: Arc<dyn RasterSource>,
}
impl PreparedTransform {
    pub fn source(&self) -> Arc<dyn RasterSource> {
        self.source.clone()
    }
}
impl RasterSource for PreparedTransform {
    fn descriptor(&self) -> &ImageDescriptor {
        self.source.descriptor()
    }
    fn access(&self) -> RasterAccess {
        self.source.access()
    }
    fn read_rows(&self, first: u32, count: u32, dst: &mut [u8]) -> Result<()> {
        self.source.read_rows(first, count, dst)
    }
    fn read_region(&self, region: Region, dst: &mut [u8]) -> Result<()> {
        self.source.read_region(region, dst)
    }
    fn metadata(&self) -> Option<&Metadata> {
        self.source.metadata()
    }
}

#[derive(Default)]
struct StaticLane {
    // Drop the process owner before reusable idle executors at lane shutdown.
    ready: Option<Connection>,
    idle_io: Option<IoPair>,
}
type Work = Box<dyn FnOnce(&mut StaticLane) + Send + 'static>;
pub(crate) struct Engine {
    pub(crate) config: Arc<ServiceConfig>,
    pub(crate) budget: Budget,
    sender: Option<mpsc::SyncSender<Work>>,
    threads: Vec<JoinHandle<()>>,
    pub(crate) shutdown: CancellationToken,
    pub(crate) animation: crate::animation::AnimationPool,
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.animation.shutdown();
        self.sender.take();
        let threads = std::mem::take(&mut self.threads);
        // Cleanup owns the supervisors until every child has been killed/reaped. It never
        // blocks the UI thread dropping its final service handle.
        let _ = std::thread::Builder::new()
            .name("snow-image-reaper".into())
            .spawn(move || {
                for thread in threads {
                    let _ = thread.join();
                }
            });
    }
}

#[derive(Clone)]
pub struct ImageService(pub(crate) Arc<Engine>);
impl ImageService {
    pub fn new(mut config: ServiceConfig) -> Result<Self> {
        config.policy.validate()?;
        config.counters = Arc::new(WorkerCounters::default());
        if config.operation_timeout.is_zero() {
            return Err(Error::InvalidInput(
                "operation timeout must be nonzero".into(),
            ));
        }
        config.worker_path = config.worker_path.canonicalize()?;
        if !config.worker_path.is_file() {
            return Err(Error::InvalidInput(
                "worker must be an executable file".into(),
            ));
        }
        let budget = Budget::new(config.policy.clone());
        let config = Arc::new(config);
        let (sender, receiver) = mpsc::sync_channel::<Work>(config.policy.max_queued_jobs as usize);
        let receiver = Arc::new(Mutex::new(receiver));
        let shutdown = CancellationToken::new();
        let mut threads = Vec::new();
        for index in 0..config.policy.max_codec_operations {
            let receiver = receiver.clone();
            let config = config.clone();
            let budget = budget.clone();
            let shutdown = shutdown.clone();
            threads.push(
                std::thread::Builder::new()
                    .name(format!("snow-image-supervisor-{index}"))
                    .spawn(move || {
                        // Idle workers have no job allocation allowance. Their first request
                        // receives an admitted envelope, and the process is never reused.
                        let mut lane = StaticLane {
                            ready: Connection::spawn(
                                config.clone(),
                                budget.clone(),
                                shutdown.clone(),
                                false,
                            )
                            .ok(),
                            idle_io: None,
                        };
                        loop {
                            let work = lock(&receiver).recv();
                            match work {
                                Ok(work) => work(&mut lane),
                                Err(_) => break,
                            }
                            if !shutdown.is_cancelled() && lane.ready.is_none() {
                                lane.ready = Connection::spawn_with_io(
                                    config.clone(),
                                    budget.clone(),
                                    shutdown.clone(),
                                    false,
                                    lane.idle_io.take(),
                                )
                                .ok();
                            }
                        }
                    })?,
            );
        }
        let animation =
            crate::animation::AnimationPool::new(config.clone(), budget.clone(), shutdown.clone())?;
        Ok(Self(Arc::new(Engine {
            config,
            budget,
            sender: Some(sender),
            threads,
            shutdown,
            animation,
        })))
    }
    pub fn budget(&self) -> &Budget {
        &self.0.budget
    }
    pub fn config(&self) -> &ServiceConfig {
        &self.0.config
    }
    pub fn worker_statistics(&self) -> WorkerStatistics {
        self.worker_monitor().statistics()
    }
    pub fn worker_monitor(&self) -> WorkerMonitor {
        WorkerMonitor(self.0.config.counters.clone())
    }
    pub fn input_bytes(&self, bytes: &[u8]) -> Result<EncodedInput> {
        self.input_reader(&mut &bytes[..], bytes.len() as u64)
    }
    pub fn input_file(&self, path: impl AsRef<Path>) -> Result<EncodedInput> {
        let mut file = File::open(path)?;
        let length = file.metadata()?.len();
        self.input_reader(&mut file, length)
    }
    fn input_reader(&self, reader: &mut impl Read, length: u64) -> Result<EncodedInput> {
        if length == 0 {
            return Err(Error::InvalidInput("empty encoded input".into()));
        }
        if length > self.0.config.policy.max_input_bytes {
            return Err(Error::ResourceLimit {
                resource: ResourceKind::Input,
                requested: length,
                limit: self.0.config.policy.max_input_bytes,
            });
        }
        let disk = self.0.budget.reserve(ResourceKind::TempDisk, length)?;
        let _scratch = self
            .0
            .budget
            .reserve(ResourceKind::Transport, protocol::TRANSFER_BYTES as u64)?;
        #[cfg(target_os = "macos")]
        let mut file = tempfile::NamedTempFile::new()?;
        #[cfg(not(target_os = "macos"))]
        let mut file = PrivateFile::new()?;
        let mut buffer = [0u8; protocol::TRANSFER_BYTES];
        copy_exact(
            reader,
            file.as_file_mut(),
            length,
            &mut buffer,
            &CancellationToken::new(),
        )?;
        file.flush()?;
        #[cfg(target_os = "macos")]
        let file = freeze_input_snapshot(file)?;
        Ok(EncodedInput(Arc::new(InputStorage {
            file: Mutex::new(file),
            length,
            _disk: disk,
        })))
    }
    pub fn capabilities(&self) -> Job<Capabilities> {
        self.submit(Operation::Capabilities, Payload::Empty, |out| match out {
            Outcome::Capabilities(values) => Ok(values),
            _ => Err(unexpected_reply()),
        })
    }
    pub fn inspect(&self, input: EncodedInput, options: DecodeOptions) -> Job<Inspection> {
        self.submit(
            Operation::Inspect(options),
            Payload::Encoded(input),
            |out| match out {
                Outcome::Info(info) => Ok(info),
                _ => Err(unexpected_reply()),
            },
        )
    }
    pub fn decode(&self, input: EncodedInput, options: DecodeOptions) -> Job<Image> {
        self.submit(
            Operation::Decode(options),
            Payload::Encoded(input),
            take_image,
        )
    }
    pub fn preview(&self, input: EncodedInput, max_dimension: u32) -> Job<Image> {
        self.submit(
            Operation::Preview {
                options: DecodeOptions::default(),
                max_dimension,
            },
            Payload::Encoded(input),
            take_image,
        )
    }
    pub fn encode(
        &self,
        source: Arc<dyn RasterSource>,
        options: EncodeOptions,
    ) -> Job<EncodedArtifact> {
        let result = raster_payload(source, &self.0.budget);
        match result {
            Ok((descriptor, payload)) => self.submit(
                Operation::Encode {
                    descriptor,
                    options,
                },
                payload,
                |out| match out {
                    Outcome::Encoded(artifact) => Ok(artifact),
                    _ => Err(unexpected_reply()),
                },
            ),
            Err(error) => failed_job(error),
        }
    }
    pub fn prepare_transform(
        &self,
        source: Arc<dyn RasterSource>,
        options: TransformOptions,
    ) -> Job<PreparedTransform> {
        match raster_payload(source, &self.0.budget) {
            Ok((descriptor, payload)) => self.submit(
                Operation::Transform {
                    descriptor,
                    options,
                },
                payload,
                |out| match out {
                    Outcome::Image(image) => Ok(PreparedTransform {
                        source: Arc::new(image),
                    }),
                    Outcome::Raster(source) => Ok(PreparedTransform { source }),
                    _ => Err(unexpected_reply()),
                },
            ),
            Err(error) => failed_job(error),
        }
    }
    fn submit<T: Send + 'static>(
        &self,
        operation: Operation,
        payload: Payload,
        convert: impl FnOnce(Outcome) -> Result<T> + Send + 'static,
    ) -> Job<T> {
        let (job, completion) = Job::pair();
        let queued = match self.0.budget.reserve(ResourceKind::QueuedWork, 1) {
            Ok(value) => value,
            Err(error) => {
                completion.finish(Err(error));
                return job;
            }
        };
        let config = self.0.config.clone();
        let budget = self.0.budget.clone();
        let shutdown = self.0.shutdown.clone();
        let cancellation = job.cancellation_token();
        let work: Work = Box::new(move |ready| {
            drop(queued);
            let result = cancellation
                .check()
                .and_then(|()| shutdown.check())
                .and_then(|()| {
                    run_static(
                        ready,
                        &config,
                        &budget,
                        operation,
                        payload,
                        &cancellation,
                        &shutdown,
                    )
                })
                .and_then(convert);
            completion.finish(result);
        });
        match self
            .0
            .sender
            .as_ref()
            .expect("live engine has sender")
            .try_send(work)
        {
            Ok(()) => job,
            Err(mpsc::TrySendError::Full(work)) | Err(mpsc::TrySendError::Disconnected(work)) => {
                // Complete the same job through its normal cancellation path without spawning
                // unbounded supervisor threads when the bounded queue refuses admission.
                job.cancel();
                work(&mut StaticLane::default());
                job
            }
        }
    }
}

fn failed_job<T>(error: Error) -> Job<T> {
    let (job, completion) = Job::pair();
    completion.finish(Err(error));
    job
}
fn unexpected_reply() -> Error {
    Error::Worker("worker returned an unexpected response kind".into())
}
fn take_image(outcome: Outcome) -> Result<Image> {
    match outcome {
        Outcome::Image(image) => Ok(image),
        _ => Err(unexpected_reply()),
    }
}

pub(crate) enum Payload {
    Empty,
    Encoded(EncodedInput),
    Raster {
        source: Arc<dyn RasterSource>,
        descriptor: ImageDescriptor,
        metadata: SharedMetadata,
    },
}
impl Payload {
    fn length(&self) -> Result<u64> {
        match self {
            Self::Empty => Ok(0),
            Self::Encoded(input) => Ok(input.len()),
            Self::Raster { descriptor, .. } => Ok(descriptor.required_bytes()? as u64),
        }
    }
    fn metadata(&self) -> Option<&Metadata> {
        match self {
            Self::Raster { metadata, .. } => Some(metadata.get()),
            _ => None,
        }
    }
}
fn raster_payload(
    source: Arc<dyn RasterSource>,
    budget: &Budget,
) -> Result<(ImageDescriptor, Payload)> {
    source.descriptor().validate_with_policy(budget)?;
    let mut descriptor = source.descriptor().clone();
    descriptor.row_stride = descriptor.row_bytes()?;
    let metadata =
        SharedMetadata::try_copy_from(source.metadata().unwrap_or(&Metadata::default()), budget)?;
    Ok((
        descriptor.clone(),
        Payload::Raster {
            source,
            descriptor,
            metadata,
        },
    ))
}

pub(crate) enum Outcome {
    Capabilities(Capabilities),
    Info(Inspection),
    Image(Image),
    Raster(Arc<dyn RasterSource>),
    Encoded(EncodedArtifact),
    Backed(BackedOutput),
    AnimationOpened(protocol::AnimationDescription),
    Frame {
        image: Image,
        index: u32,
        duration_ms: u64,
    },
    Done,
}
pub(crate) enum BackedOutput {
    #[cfg(target_os = "macos")]
    SharedImage {
        descriptor: ImageDescriptor,
        metadata: SharedMetadata,
        bytes: usize,
        pixels: Reservation,
    },
    Image {
        descriptor: ImageDescriptor,
        metadata: SharedMetadata,
        bytes: u64,
        pixels: Reservation,
        disk: Reservation,
    },
    Raster {
        descriptor: ImageDescriptor,
        metadata: SharedMetadata,
        bytes: u64,
        disk: Reservation,
    },
    Encoded {
        receipt: EncodeReceipt,
        disk: Reservation,
    },
}
struct Process {
    child: Child,
    exit_notification: platform::ExitNotification,
    _containment: platform::Containment,
    reaped: bool,
    counters: Arc<WorkerCounters>,
}

type IoTask<T> = Box<dyn FnOnce() -> Result<T> + Send>;

/// One reusable transfer thread with one admitted task and one completion at a time.
/// Idle executors hold no codec buffers or resource reservations. Static supervisor
/// lanes retain them across disposable workers; animation connections reuse them.
struct IoExecutor<T: Send + 'static> {
    tasks: Option<mpsc::SyncSender<IoTask<T>>>,
    completed: mpsc::Receiver<Result<T>>,
    thread: Option<JoinHandle<()>>,
    cancellation: Option<CancellationToken>,
}
impl<T: Send + 'static> IoExecutor<T> {
    fn new(name: &str) -> Result<Self> {
        let (tasks, receiver) = mpsc::sync_channel::<IoTask<T>>(1);
        let (sender, completed) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                while let Ok(task) = receiver.recv() {
                    let result = catch_unwind(AssertUnwindSafe(task))
                        .unwrap_or_else(|_| Err(Error::Worker("I/O task panicked".into())));
                    if sender.send(result).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            tasks: Some(tasks),
            completed,
            thread: Some(thread),
            cancellation: None,
        })
    }
    fn submit(
        &mut self,
        cancellation: &CancellationToken,
        task: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<()> {
        if self.cancellation.is_some() {
            return Err(Error::Worker(
                "I/O executor already has a pending task".into(),
            ));
        }
        self.tasks
            .as_ref()
            .ok_or_else(|| Error::Worker("I/O executor has stopped".into()))?
            .try_send(Box::new(task))
            .map_err(|_| Error::Worker("I/O executor rejected a task".into()))?;
        self.cancellation = Some(cancellation.clone());
        Ok(())
    }
    fn try_recv(&mut self) -> std::result::Result<Result<T>, mpsc::TryRecvError> {
        let result = self.completed.try_recv();
        if !matches!(result, Err(mpsc::TryRecvError::Empty)) {
            self.cancellation = None;
        }
        result
    }
    fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::result::Result<Result<T>, mpsc::RecvTimeoutError> {
        let result = self.completed.recv_timeout(timeout);
        if !matches!(result, Err(mpsc::RecvTimeoutError::Timeout)) {
            self.cancellation = None;
        }
        result
    }
    fn recv(&mut self) -> Result<T> {
        let result = self
            .completed
            .recv()
            .unwrap_or_else(|_| Err(Error::Worker("I/O executor stopped".into())));
        self.cancellation = None;
        result
    }
    fn reusable(&self) -> bool {
        self.cancellation.is_none()
            && self
                .thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
    }
}
impl<T: Send + 'static> Drop for IoExecutor<T> {
    fn drop(&mut self) {
        if let Some(cancellation) = &self.cancellation {
            cancellation.cancel();
        }
        // Connection drops Process first, closing the other pipe ends before an
        // outstanding transfer is joined. Custom producers must honor cancellation.
        self.tasks.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct IoPair {
    input: IoExecutor<()>,
    output: IoExecutor<(std::process::ChildStdout, Result<Outcome>)>,
}
impl IoPair {
    fn new() -> Result<Self> {
        Ok(Self {
            input: IoExecutor::new("snow-image-input")?,
            output: IoExecutor::new("snow-image-output")?,
        })
    }
    fn reusable(&self) -> bool {
        self.input.reusable() && self.output.reusable()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if !self.reaped {
            platform::terminate_tree(&mut self.child, &self._containment);
            let _ = self.child.wait();
            self.reaped = true;
        }
        // Connection declares this field before its envelopes and publication file.
        // Keep those owners alive until the OS confirms there can be no writer left.
        // This runs on a supervisor/reaper thread, never the service caller's thread.
        while platform::wait_tree_terminated(
            &self.child,
            &self._containment,
            Duration::from_millis(100),
        )
        .is_err()
        {
            platform::terminate_tree(&mut self.child, &self._containment);
            std::thread::sleep(Duration::from_millis(10));
        }
        self.counters.reaped.fetch_add(1, Ordering::Relaxed);
        self.counters.live.fetch_sub(1, Ordering::Relaxed);
    }
}

fn run_static(
    lane: &mut StaticLane,
    config: &ServiceConfig,
    budget: &Budget,
    operation: Operation,
    payload: Payload,
    cancellation: &CancellationToken,
    shutdown: &CancellationToken,
) -> Result<Outcome> {
    let mut connection = match lane.ready.take() {
        Some(connection) => connection,
        None => Connection::spawn_with_io(
            Arc::new(config.clone()),
            budget.clone(),
            shutdown.clone(),
            false,
            lane.idle_io.take(),
        )?,
    };
    let result = connection.call(operation, payload, cancellation, None);
    // Completed tasks own no child handles, mappings, payloads or reservations.
    // Move only that idle pair out; the old process and all result owners still
    // follow the normal termination, tree-proof and accounting path before return.
    lane.idle_io = connection.take_idle_io();
    drop(connection);
    result
}

/// The process owner retains all reservations until kill/wait has completed.
pub(crate) struct Connection {
    process: Process,
    io: Option<IoPair>,
    stdin: Arc<Mutex<ChildStdin>>,
    stdout: Option<std::process::ChildStdout>,
    output_backing: Option<PrivateFile>,
    #[cfg(target_os = "macos")]
    pixel_backing: Option<File>,
    #[cfg(target_os = "macos")]
    input_handoff: Option<Arc<std::os::unix::net::UnixDatagram>>,
    // A transferred input descriptor retains its disk owner through process-tree
    // quiescence, even when the caller and the input task drop their clones.
    #[cfg(target_os = "macos")]
    input_owner: Option<EncodedInput>,
    config: Arc<ServiceConfig>,
    policy: ResourcePolicy,
    budget: Budget,
    shutdown: CancellationToken,
    generation: u64,
    persistent: bool,
    healthy: bool,
    child_memory: Option<Reservation>,
    child_disk: Option<Reservation>,
}

impl Connection {
    pub(crate) fn spawn(
        config: Arc<ServiceConfig>,
        budget: Budget,
        shutdown: CancellationToken,
        persistent: bool,
    ) -> Result<Self> {
        Self::spawn_with_io(config, budget, shutdown, persistent, None)
    }
    fn spawn_with_io(
        config: Arc<ServiceConfig>,
        budget: Budget,
        shutdown: CancellationToken,
        persistent: bool,
        io: Option<IoPair>,
    ) -> Result<Self> {
        config.counters.spawning.fetch_add(1, Ordering::AcqRel);
        let _spawn = SpawnGuard(config.counters.clone());
        shutdown.check()?;
        let mut policy = config.policy.clone();
        policy.max_codec_threads = (policy.max_codec_threads / policy.max_codec_operations).max(1);
        let (child_memory, child_disk) = if persistent {
            let (memory, disk) = admit_child(&budget, &mut policy, 3)?;
            (Some(memory), Some(disk))
        } else {
            (None, None)
        };
        let spawn_serial = lock(&SPAWN_LOCK);
        let mut command = Command::new(&config.worker_path);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if persistent {
            command.arg("--persistent");
        }
        #[cfg(target_os = "macos")]
        let mut pixel_backing = None;
        #[cfg(target_os = "macos")]
        let mut input_handoff = None;
        let output_backing = if persistent {
            None
        } else {
            let file = PrivateFile::new()?;
            #[cfg(target_os = "macos")]
            {
                use std::os::{fd::OwnedFd, unix::net::UnixDatagram};
                let pixels = snow_image_transport::create_pixel_backing()?;
                let (parent, child) = UnixDatagram::pair()?;
                // Queue both capabilities before the child can start. Datagram
                // receive has no EOF when its peer disappears, so sending after
                // spawn would leave a parent-death window during startup.
                snow_image_transport::send_backing(&parent, &file.0, &pixels)?;
                command
                    .arg("--output-handoff")
                    .stderr(Stdio::from(OwnedFd::from(child)));
                pixel_backing = Some(pixels);
                input_handoff = Some(Arc::new(parent));
            }
            #[cfg(not(target_os = "macos"))]
            command
                .arg("--output-backing")
                .stderr(Stdio::from(file.0.try_clone()?));
            Some(file)
        };
        platform::configure(&mut command, &policy);
        let mut child = command.spawn()?;
        let exit_notification = match platform::ExitNotification::new(&child) {
            Ok(notification) => notification,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        drop(spawn_serial);
        let containment = match platform::attach(&mut child, &policy) {
            Ok(containment) => containment,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let mut process = Process {
            child,
            exit_notification,
            _containment: containment,
            reaped: false,
            counters: config.counters.clone(),
        };
        process.counters.spawned.fetch_add(1, Ordering::Relaxed);
        process.counters.live.fetch_add(1, Ordering::Relaxed);
        let stdin = Arc::new(Mutex::new(
            process
                .child
                .stdin
                .take()
                .ok_or_else(|| Error::Worker("worker stdin missing".into()))?,
        ));
        let stdout = process
            .child
            .stdout
            .take()
            .ok_or_else(|| Error::Worker("worker stdout missing".into()))?;
        let io = match io {
            Some(io) => io,
            None => IoPair::new()?,
        };
        Ok(Self {
            process,
            io: Some(io),
            stdin,
            stdout: Some(stdout),
            output_backing,
            #[cfg(target_os = "macos")]
            pixel_backing,
            #[cfg(target_os = "macos")]
            input_handoff,
            #[cfg(target_os = "macos")]
            input_owner: None,
            config,
            policy,
            budget,
            shutdown,
            generation: next_id(),
            persistent,
            healthy: true,
            child_memory,
            child_disk,
        })
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn healthy(&self) -> bool {
        self.healthy
    }
    pub(crate) fn check_idle(&mut self) -> Result<()> {
        if self.process.child.try_wait()?.is_some() {
            self.healthy = false;
            // Retirement still drops Process first: its reap/tree proof must precede
            // release of the connection's reservations and transferred handles.
            return Err(Error::Worker("idle worker exited".into()));
        }
        self.check_footprint()
    }
    pub(crate) fn check_footprint(&self) -> Result<()> {
        if let Some(bytes) = platform::footprint(&self.process.child) {
            self.config
                .counters
                .footprint
                .fetch_max(bytes, Ordering::Relaxed);
            if bytes > self.policy.worker_memory_bytes {
                return Err(Error::ResourceLimit {
                    resource: ResourceKind::Scratch,
                    requested: bytes,
                    limit: self.policy.worker_memory_bytes,
                });
            }
        }
        Ok(())
    }
    fn take_idle_io(&mut self) -> Option<IoPair> {
        if self.io.as_ref().is_some_and(IoPair::reusable) {
            self.io.take()
        } else {
            None
        }
    }
    fn input(&mut self) -> &mut IoExecutor<()> {
        &mut self
            .io
            .as_mut()
            .expect("connection owns I/O executors")
            .input
    }
    fn output(&mut self) -> &mut IoExecutor<(std::process::ChildStdout, Result<Outcome>)> {
        &mut self
            .io
            .as_mut()
            .expect("connection owns I/O executors")
            .output
    }
    fn stop(&mut self) {
        self.healthy = false;
        if !self.process.reaped {
            platform::terminate_tree(&mut self.process.child, &self.process._containment);
            let _ = self.process.child.wait();
            self.process.reaped = true;
        }
    }
    pub(crate) fn call(
        &mut self,
        operation: Operation,
        payload: Payload,
        cancellation: &CancellationToken,
        cached_metadata: Option<SharedMetadata>,
    ) -> Result<Outcome> {
        cancellation.check()?;
        self.shutdown.check()?;
        if !self.healthy {
            return Err(Error::Worker("worker generation has retired".into()));
        }
        let started = Instant::now();
        let _active = loop {
            match self.budget.reserve(ResourceKind::CodecOperations, 1) {
                Ok(reservation) => break reservation,
                Err(Error::ResourceLimit { .. }) => {
                    cancellation.check()?;
                    self.shutdown.check()?;
                    if started.elapsed() >= self.config.operation_timeout {
                        return Err(Error::Worker(
                            "operation admission deadline exceeded".into(),
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error),
            }
        };
        let _threads = self.budget.reserve(
            ResourceKind::CodecThreads,
            self.policy.max_codec_threads as u64,
        )?;
        if !self.persistent {
            let (memory, disk) = admit_child(&self.budget, &mut self.policy, 2)?;
            self.child_memory = Some(memory);
            self.child_disk = Some(disk);
        }
        let id = next_id();
        let generation = self.generation;
        // A macOS static worker receives the frozen input's read-only capability;
        // its own admitted mapping does not copy or change the shared file cursor.
        #[cfg(target_os = "macos")]
        let handle_input = self.input_handoff.is_some()
            && matches!(
                &operation,
                Operation::Inspect(_) | Operation::Decode(_) | Operation::Preview { .. }
            )
            && matches!(&payload, Payload::Encoded(_));
        #[cfg(not(target_os = "macos"))]
        let handle_input = false;
        let backed_input =
            !self.persistent && !handle_input && matches!(payload, Payload::Encoded(_));
        let request = Request {
            version: protocol::VERSION,
            generation,
            id,
            policy: self.policy.clone(),
            operation,
            input_len: payload.length()?,
            payload_storage: if handle_input {
                protocol::PayloadStorage::InputHandle
            } else if backed_input {
                protocol::PayloadStorage::Backing
            } else {
                protocol::PayloadStorage::Pipe
            },
            metadata: payload
                .metadata()
                .map_or(MetadataLengths::default(), MetadataLengths::from_metadata),
        };
        request.validate()?;
        #[cfg(target_os = "macos")]
        if handle_input && let Payload::Encoded(input) = &payload {
            self.input_owner = Some(input.clone());
        }
        let stdout = self
            .stdout
            .take()
            .ok_or_else(|| Error::Worker("worker stdout missing".into()))?;
        let writer_stdin = self.stdin.clone();
        // The producer must also stop after worker failure or timeout. Keep this
        // signal separate from user cancellation so an internal transfer abort
        // cannot replace the original worker/codec error with Cancelled.
        let transfer_cancel = CancellationToken::new();
        let writer_cancel = transfer_cancel.clone();
        let writer_budget = self.budget.clone();
        let writer_request = request.clone();
        let input_transfer = if backed_input {
            if request.input_len > self.policy.max_temp_bytes {
                return Err(Error::ResourceLimit {
                    resource: ResourceKind::TempDisk,
                    requested: request.input_len,
                    limit: self.policy.max_temp_bytes,
                });
            }
            InputTransfer::Backing(
                self.output_backing
                    .as_ref()
                    .ok_or_else(|| Error::Worker("input backing missing".into()))?
                    .0
                    .try_clone()?,
            )
        } else {
            #[cfg(target_os = "macos")]
            if handle_input {
                InputTransfer::Handle(
                    self.input_handoff
                        .as_ref()
                        .expect("handoff checked above")
                        .clone(),
                )
            } else {
                InputTransfer::Pipe
            }
            #[cfg(not(target_os = "macos"))]
            InputTransfer::Pipe
        };
        if let Err(error) = self.input().submit(&transfer_cancel, move || {
            send_payload(
                &mut lock(&writer_stdin),
                &writer_request,
                payload,
                &writer_budget,
                &writer_cancel,
                input_transfer,
            )
        }) {
            self.stop();
            return Err(error);
        }
        let reader_budget = self.budget.clone();
        let backed = !self.persistent;
        #[cfg(target_os = "macos")]
        let shared = self.pixel_backing.is_some();
        #[cfg(not(target_os = "macos"))]
        let shared = false;
        if let Err(error) = self.output().submit(&transfer_cancel, move || {
            let mut stdout = stdout;
            let result = receive_response(
                &mut stdout,
                &request,
                &reader_budget,
                cached_metadata,
                backed,
                shared,
            );
            Ok((stdout, result))
        }) {
            transfer_cancel.cancel();
            self.stop();
            let _ = self.input().recv();
            return Err(error);
        }
        let mut cancelling = None;
        let mut outcome = None;
        let mut response_ready = None;
        let mut failure = None;
        let mut sent = None;
        let mut cancel_write_pending = false;
        loop {
            if cancellation.check().is_err() || self.shutdown.check().is_err() {
                transfer_cancel.cancel();
                if cancelling.is_none() {
                    cancelling = Some(Instant::now());
                }
                if cancelling.is_some_and(|time| time.elapsed() >= self.config.cancellation_grace) {
                    failure = Some(Error::Cancelled);
                    break;
                }
            }
            if started.elapsed() >= self.config.operation_timeout {
                failure = Some(Error::Worker("operation deadline exceeded".into()));
                break;
            }
            if let Err(error) = self.check_footprint() {
                failure = Some(error);
                break;
            }
            if outcome.is_none() {
                match self.output().recv_timeout(Duration::from_millis(10)) {
                    Ok(result) => {
                        outcome = Some(match result {
                            Ok((stdout, result)) => {
                                self.stdout = Some(stdout);
                                result
                            }
                            Err(error) => Err(error),
                        });
                        response_ready = Some(Instant::now());
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        failure = Some(Error::Worker("worker response reader stopped".into()));
                        break;
                    }
                }
            }
            // A producer can fail before completing its declared payload. Observe that
            // failure while the child is waiting for input, rather than waiting for the
            // operation deadline with an open, incomplete pipe.
            if sent.is_none() {
                match self.input().try_recv() {
                    Ok(Ok(())) => sent = Some(Ok(())),
                    Ok(Err(error)) => {
                        // This completion was consumed; the original error is kept
                        // separately while both transfer owners are drained below.
                        sent = Some(Ok(()));
                        failure = Some(error);
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => (),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        sent = Some(Ok(()));
                        failure = Some(Error::Worker("input sender panicked".into()));
                        break;
                    }
                }
            }
            if cancelling.is_some() && sent.is_some() && !cancel_write_pending {
                let stdin = self.stdin.clone();
                // Even a tiny control message can block when a malformed worker
                // leaves the pipe full. Keep this write on the bounded input
                // executor so the supervisor can still enforce the grace period.
                if self
                    .input()
                    .submit(&transfer_cancel, move || {
                        protocol::write_control(
                            &mut *lock(&stdin),
                            &protocol::Command::Cancel { generation, id },
                        )
                    })
                    .is_ok()
                {
                    cancel_write_pending = true;
                }
            }
            match self.process.child.try_wait() {
                Ok(Some(status)) => {
                    platform::terminate_tree(&mut self.process.child, &self.process._containment);
                    self.process.reaped = true;
                    self.healthy = false;
                    if !status.success() {
                        failure =
                            Some(Error::Worker(format!("worker exited with {status}").into()));
                    }
                    if outcome.is_none() {
                        outcome = Some(match self.output().recv() {
                            Ok((stdout, result)) => {
                                self.stdout = Some(stdout);
                                result
                            }
                            Err(error) => Err(error),
                        });
                    }
                    break;
                }
                Ok(None) => (),
                Err(error) => {
                    failure = Some(error.into());
                    break;
                }
            }
            if outcome.as_ref().is_some_and(Result::is_err) {
                break;
            }
            if self.persistent && outcome.is_some() && sent.is_some() {
                // A short command may allocate after the sample preceding the
                // response wait. Check its retained footprint before accepting
                // the result and leaving the worker alive between commands.
                if let Err(error) = self.check_footprint() {
                    failure = Some(error);
                }
                break;
            }
            if outcome.is_some() {
                let remaining = self
                    .config
                    .operation_timeout
                    .saturating_sub(started.elapsed());
                let remaining = cancelling.map_or(remaining, |time: Instant| {
                    remaining.min(
                        self.config
                            .cancellation_grace
                            .saturating_sub(time.elapsed()),
                    )
                });
                let interval = if response_ready
                    .is_some_and(|time: Instant| time.elapsed() < Duration::from_millis(5))
                {
                    Duration::from_micros(100)
                } else {
                    Duration::from_millis(2)
                };
                if !self.persistent && sent.is_some() {
                    // Exit notification is only a bounded wakeup. The next loop
                    // still checks cancellation/footprint and uses Child::try_wait
                    // for reaping before the separate process-tree proof.
                    #[cfg(target_os = "macos")]
                    let interval = Duration::from_millis(2);
                    if let Err(error) = self.process.exit_notification.wait(interval.min(remaining))
                    {
                        failure = Some(error);
                        break;
                    }
                    continue;
                }
                // A premature acknowledgement can arrive before input finishes.
                // Keep that sender supervised; its completion is not a proc event.
                std::thread::sleep(interval.min(remaining));
            }
        }
        if failure.is_some()
            || cancelling.is_some()
            || outcome.as_ref().is_some_and(Result::is_err)
            || !self.persistent
        {
            transfer_cancel.cancel();
            self.stop();
        }
        let sent = sent.unwrap_or_else(|| self.input().recv());
        if cancel_write_pending {
            // stop() has reaped the worker, so a blocked cooperative control
            // write is now released before its stdin owner can be dropped.
            let _ = self.input().recv();
        }
        if outcome.is_none() {
            outcome = Some(match self.output().recv() {
                Ok((stdout, result)) => {
                    self.stdout = Some(stdout);
                    result
                }
                Err(error) => Err(error),
            });
        }
        cancellation.check()?;
        self.shutdown.check()?;
        if let Some(error) = failure {
            return Err(error);
        }
        let outcome =
            outcome.ok_or_else(|| Error::Worker("worker produced no result".into()))??;
        sent?;
        match outcome {
            Outcome::Backed(backing) => self.publish_backing(backing),
            other => Ok(other),
        }
    }
    fn publish_backing(&mut self, backing: BackedOutput) -> Result<Outcome> {
        if !self.process.reaped || self.persistent {
            return Err(Error::Worker("publication writer is still alive".into()));
        }
        platform::wait_tree_terminated(
            &self.process.child,
            &self.process._containment,
            Duration::from_secs(1),
        )?;
        // Only host-owned result reservations remain necessary after both transfer
        // tasks have completed and the OS confirms that every writer is gone.
        self.child_memory.take();
        self.child_disk.take();
        #[cfg(target_os = "macos")]
        if let BackedOutput::SharedImage {
            descriptor,
            metadata,
            bytes,
            pixels,
        } = backing
        {
            let file = self
                .pixel_backing
                .take()
                .ok_or_else(|| Error::Worker("shared publication backing missing".into()))?;
            snow_image_transport::validate_pixel_backing(&file, bytes)?;
            // SAFETY: the unlinked shared-memory object is private to the host
            // and this worker tree. All writer processes are gone, both I/O
            // tasks completed, and no writable handles escape. Mapping only the
            // validated payload keeps alignment padding out of published pixels.
            let mapping = unsafe { memmap2::MmapOptions::new().len(bytes).map(&file)? };
            drop(file);
            return Ok(Outcome::Image(Image::from_owned_storage(
                descriptor, mapping, metadata, pixels,
            )?));
        }
        let file = self
            .output_backing
            .take()
            .ok_or_else(|| Error::Worker("publication file missing".into()))?;
        let bytes = match &backing {
            #[cfg(target_os = "macos")]
            BackedOutput::SharedImage { .. } => unreachable!("shared image published above"),
            BackedOutput::Image { bytes, .. } | BackedOutput::Raster { bytes, .. } => *bytes,
            BackedOutput::Encoded { receipt, .. } => receipt.bytes,
        };
        let status = file.0.metadata()?;
        if !status.is_file() || status.len() != bytes {
            return Err(Error::Worker("publication file length mismatch".into()));
        }
        match backing {
            #[cfg(target_os = "macos")]
            BackedOutput::SharedImage { .. } => unreachable!("shared image published above"),
            BackedOutput::Image {
                descriptor,
                metadata,
                bytes,
                pixels,
                disk,
            } => {
                let length = usize::try_from(bytes).map_err(|_| {
                    Error::InvalidInput("image mapping exceeds address space".into())
                })?;
                let mut options = memmap2::MmapOptions::new();
                options.len(length);
                #[cfg(windows)]
                options.no_probe_handle();
                // SAFETY: this private temporary file is unlinked on Unix or opened
                // without sharing on Windows. Its writer has been reaped and the OS
                // has confirmed its process group/Job is empty. No writable handle
                // escapes. The original host handle is closed after creating this
                // read-only mapping; memmap2 may retain a private lifetime handle.
                let mapping = unsafe { options.map(&file.0)? };
                drop(file);
                Ok(Outcome::Image(Image::from_owned_storage(
                    descriptor,
                    MappedPixels {
                        mapping,
                        _disk: disk,
                    },
                    metadata,
                    pixels,
                )?))
            }
            BackedOutput::Raster {
                descriptor,
                metadata,
                disk,
                ..
            } => {
                // SAFETY: the process tree is quiescent, both I/O tasks completed,
                // and this private file has no escaping writable handles or paths.
                let cache = unsafe {
                    crate::prepared_cache::try_map_quiescent(
                        descriptor,
                        metadata,
                        file.0,
                        disk,
                        &self.budget,
                    )?
                };
                match cache {
                    crate::prepared_cache::PreparedCacheResult::Cached(source) => {
                        Ok(Outcome::Raster(source))
                    }
                    crate::prepared_cache::PreparedCacheResult::Uncached {
                        descriptor,
                        metadata,
                        file,
                        disk,
                    } => Ok(Outcome::Raster(Arc::new(BackedRaster {
                        descriptor,
                        metadata,
                        file: Mutex::new(PrivateFile(file)),
                        _disk: disk,
                    }))),
                }
            }
            BackedOutput::Encoded { receipt, disk } => Ok(Outcome::Encoded(EncodedArtifact(
                Arc::new(ArtifactStorage {
                    file: Mutex::new(file),
                    receipt,
                    _disk: disk,
                    budget: self.budget.clone(),
                }),
            ))),
        }
    }
}

fn admit_child(
    budget: &Budget,
    policy: &mut ResourcePolicy,
    divisor: u64,
) -> Result<(Reservation, Reservation)> {
    let free = budget
        .policy()
        .total_memory_bytes
        .saturating_sub(budget.total_used());
    let allowance = budget.policy().working_bytes_per_job.min(free / divisor);
    if allowance < 1024 * 1024 {
        return Err(Error::ResourceLimit {
            resource: ResourceKind::Scratch,
            requested: 1024 * 1024,
            limit: allowance,
        });
    }
    let memory = budget.reserve(ResourceKind::Scratch, allowance)?;
    let disk_bytes = budget
        .policy()
        .max_temp_bytes
        .saturating_sub(budget.used(ResourceKind::TempDisk))
        / divisor;
    let disk = budget.reserve(ResourceKind::TempDisk, disk_bytes)?;
    policy.total_memory_bytes = allowance;
    policy.working_bytes_per_job = allowance;
    policy.max_temp_bytes = disk_bytes;
    Ok((memory, disk))
}

enum InputTransfer {
    Pipe,
    Backing(File),
    #[cfg(target_os = "macos")]
    Handle(Arc<std::os::unix::net::UnixDatagram>),
}

fn send_payload(
    writer: &mut ChildStdin,
    request: &Request,
    payload: Payload,
    budget: &Budget,
    cancel: &CancellationToken,
    transfer: InputTransfer,
) -> Result<()> {
    #[cfg(target_os = "macos")]
    if let InputTransfer::Handle(socket) = transfer {
        let Payload::Encoded(input) = payload else {
            return Err(Error::InvalidInput(
                "only encoded input can use a transferred handle".into(),
            ));
        };
        cancel.check()?;
        snow_image_transport::send_input(&socket, &lock(&input.0.file).0)?;
        cancel.check()?;
        protocol::write_control(writer, &protocol::Command::Run(request.clone()))?;
        return Ok(());
    }
    if let InputTransfer::Backing(mut backing) = transfer {
        let Payload::Encoded(input) = payload else {
            return Err(Error::InvalidInput(
                "only encoded input can use static backing".into(),
            ));
        };
        const BUFFER_BYTES: usize = 1024 * 1024;
        let mut reservation = budget.reserve(ResourceKind::Transport, BUFFER_BYTES as u64)?;
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(BUFFER_BYTES)
            .map_err(|_| Error::AllocationFailed)?;
        reservation.resize(buffer.capacity() as u64)?;
        buffer.resize(BUFFER_BYTES, 0);
        let mut file = lock(&input.0.file);
        file.as_file_mut().seek(SeekFrom::Start(0))?;
        backing.set_len(0)?;
        backing.seek(SeekFrom::Start(0))?;
        copy_exact(
            file.as_file_mut(),
            &mut backing,
            input.len(),
            &mut buffer,
            cancel,
        )?;
        backing.flush()?;
        cancel.check()?;
        // The worker may read/replace the file only after receiving this command.
        // This function never writes to backing after sending Run.
        drop(backing);
        protocol::write_control(writer, &protocol::Command::Run(request.clone()))?;
        return Ok(());
    }
    protocol::write_control(writer, &protocol::Command::Run(request.clone()))?;
    if let Some(metadata) = payload.metadata() {
        protocol::write_metadata(writer, metadata)?;
    }
    match payload {
        Payload::Empty => (),
        Payload::Encoded(input) => {
            let _scratch =
                budget.reserve(ResourceKind::Transport, protocol::TRANSFER_BYTES as u64)?;
            let mut buffer = [0; protocol::TRANSFER_BYTES];
            let mut file = lock(&input.0.file);
            file.as_file_mut().seek(SeekFrom::Start(0))?;
            copy_exact(file.as_file_mut(), writer, input.len(), &mut buffer, cancel)?;
        }
        Payload::Raster {
            source, descriptor, ..
        } => {
            let row_bytes = descriptor.row_bytes()?;
            let rows = (protocol::TRANSFER_BYTES / row_bytes)
                .max(1)
                .min(descriptor.height as usize);
            let length = row_bytes.checked_mul(rows).ok_or(Error::AllocationFailed)?;
            let mut scratch = budget.reserve(ResourceKind::Transport, length as u64)?;
            let mut buffer = Vec::new();
            buffer
                .try_reserve_exact(length)
                .map_err(|_| Error::AllocationFailed)?;
            scratch.resize(buffer.capacity() as u64)?;
            buffer.resize(length, 0);
            let mut first = 0;
            while first < descriptor.height {
                cancel.check()?;
                let count = (descriptor.height - first).min(rows as u32);
                let bytes = &mut buffer[..count as usize * row_bytes];
                source.read_rows_cancellable(first, count, bytes, cancel)?;
                writer.write_all(bytes)?;
                first += count;
            }
        }
    }
    writer.flush()?;
    Ok(())
}

fn receive_response(
    mut reader: impl Read,
    request: &Request,
    budget: &Budget,
    cached_metadata: Option<SharedMetadata>,
    backed: bool,
    shared: bool,
) -> Result<Outcome> {
    let _control = budget.reserve(ResourceKind::Transport, protocol::CONTROL_PARSE_ALLOWANCE)?;
    let response: Response = protocol::read_control(&mut reader)?;
    if response.version != protocol::VERSION
        || response.id != request.id
        || response.generation != request.generation
    {
        return Err(Error::Worker("worker response identity mismatch".into()));
    }
    let expected = matches!(response.reply, Reply::Failure(_))
        || matches!(
            (&request.operation, &response.reply),
            (Operation::Capabilities, Reply::Capabilities(_))
                | (Operation::Inspect(_), Reply::Info { .. })
                | (
                    Operation::Decode(_) | Operation::Preview { .. },
                    Reply::Image { .. } | Reply::BackedImage { .. } | Reply::SharedImage { .. }
                )
                | (
                    Operation::Transform { .. },
                    Reply::Image { .. }
                        | Reply::Raster { .. }
                        | Reply::BackedImage { .. }
                        | Reply::BackedRaster { .. }
                )
                | (
                    Operation::Encode { .. },
                    Reply::Encoded { .. } | Reply::BackedEncoded { .. }
                )
                | (Operation::AnimationOpen(_), Reply::AnimationOpened(_))
                | (
                    Operation::AnimationAdvance { .. },
                    Reply::AnimationFrame { .. } | Reply::Done
                )
                | (
                    Operation::AnimationReset { .. } | Operation::AnimationClose { .. },
                    Reply::Done
                )
        );
    if !expected {
        return Err(unexpected_reply());
    }
    if !backed
        && matches!(
            response.reply,
            Reply::BackedImage { .. } | Reply::BackedRaster { .. } | Reply::BackedEncoded { .. }
        )
    {
        return Err(Error::Worker(
            "persistent worker cannot publish a backing file".into(),
        ));
    }
    match response.reply {
        Reply::SharedImage {
            descriptor,
            metadata,
            bytes,
        } => {
            if !backed || !shared {
                return Err(Error::Worker(
                    "shared publication backing unavailable".into(),
                ));
            }
            validate_returned_descriptor(&descriptor, request, budget)?;
            if descriptor.required_bytes()? as u64 != bytes {
                return Err(Error::Worker("shared image reply length mismatch".into()));
            }
            #[cfg(target_os = "macos")]
            {
                let bytes = usize::try_from(bytes)
                    .map_err(|_| Error::Worker("shared image exceeds address space".into()))?;
                let allocated = snow_image_transport::allocation_len(bytes)?;
                let pixels = budget.reserve(ResourceKind::Pixels, allocated as u64)?;
                let metadata = receive_metadata(&mut reader, metadata, budget, cached_metadata)?;
                Ok(Outcome::Backed(BackedOutput::SharedImage {
                    descriptor,
                    metadata,
                    bytes,
                    pixels,
                }))
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = metadata;
                Err(Error::Worker(
                    "shared publication is unsupported on this platform".into(),
                ))
            }
        }
        Reply::BackedImage {
            descriptor,
            metadata,
            bytes,
        } => {
            validate_returned_descriptor(&descriptor, request, budget)?;
            if descriptor.required_bytes()? as u64 != bytes {
                return Err(Error::Worker("image reply length mismatch".into()));
            }
            let metadata = receive_metadata(&mut reader, metadata, budget, cached_metadata)?;
            Ok(Outcome::Backed(BackedOutput::Image {
                descriptor,
                metadata,
                bytes,
                pixels: budget.reserve(ResourceKind::Pixels, bytes)?,
                disk: budget.reserve(ResourceKind::TempDisk, bytes)?,
            }))
        }
        Reply::BackedRaster {
            descriptor,
            metadata,
            bytes,
        } => {
            validate_returned_descriptor(&descriptor, request, budget)?;
            if descriptor.required_bytes()? as u64 != bytes {
                return Err(Error::Worker("raster reply length mismatch".into()));
            }
            let metadata = receive_metadata(&mut reader, metadata, budget, cached_metadata)?;
            Ok(Outcome::Backed(BackedOutput::Raster {
                descriptor,
                metadata,
                bytes,
                disk: budget.reserve(ResourceKind::TempDisk, bytes)?,
            }))
        }
        Reply::BackedEncoded { receipt, bytes } => {
            validate_receipt(&receipt, bytes, request, budget)?;
            Ok(Outcome::Backed(BackedOutput::Encoded {
                receipt,
                disk: budget.reserve(ResourceKind::TempDisk, bytes)?,
            }))
        }
        Reply::Capabilities(values) => Ok(Outcome::Capabilities(Capabilities::from_values(
            values, budget,
        )?)),
        Reply::Failure(error) => Err(error.into_accounted_error(budget)),
        Reply::Info { mut info, metadata } => {
            if info.metadata.byte_len()? != 0 {
                return Err(Error::Worker(
                    "metadata must use bounded binary transport".into(),
                ));
            }
            if info.width == 0
                || info.height == 0
                || info.frame_count == 0
                || !matches!(info.bit_depth, 1..=16 | 32)
                || info.frame_count > budget.policy().max_animation_frames
            {
                return Err(Error::Worker(
                    "invalid inspected dimensions or frame count".into(),
                ));
            }
            info.color.validate()?;
            let mut reservation = budget.reserve(ResourceKind::Metadata, metadata.total()?)?;
            info.metadata = metadata.read(&mut reader, budget.policy().max_metadata_bytes)?;
            reservation.resize(info.metadata.allocation_bytes()?)?;
            Ok(Outcome::Info(Inspection(Arc::new(InspectionStorage {
                info,
                _metadata: reservation,
            }))))
        }
        Reply::Image {
            descriptor,
            metadata,
            bytes,
        } => {
            validate_returned_descriptor(&descriptor, request, budget)?;
            if descriptor.required_bytes()? as u64 != bytes {
                return Err(Error::Worker("image reply length mismatch".into()));
            }
            let metadata = receive_metadata(&mut reader, metadata, budget, cached_metadata)?;
            let mut image = MutableImage::new_shared(descriptor, metadata, budget)?;
            reader.read_exact(image.bytes_mut())?;
            Ok(Outcome::Image(image.seal()))
        }
        Reply::AnimationOpened(description) => {
            if description.session == 0
                || description.frame_count == 0
                || description.frame_count > budget.policy().max_animation_frames
                || !description.durations_ms.is_empty()
                || description.width == 0
                || description.height == 0
            {
                return Err(Error::Worker("invalid animation description".into()));
            }
            Ok(Outcome::AnimationOpened(description))
        }
        Reply::AnimationFrame {
            descriptor,
            metadata,
            bytes,
            index,
            duration_ms,
        } => {
            validate_returned_descriptor(&descriptor, request, budget)?;
            if descriptor.required_bytes()? as u64 != bytes
                || index >= budget.policy().max_animation_frames
            {
                return Err(Error::Worker("invalid animation frame reply".into()));
            }
            let metadata = receive_metadata(&mut reader, metadata, budget, cached_metadata)?;
            let mut image = MutableImage::new_shared(descriptor, metadata, budget)?;
            reader.read_exact(image.bytes_mut())?;
            Ok(Outcome::Frame {
                image: image.seal(),
                index,
                duration_ms,
            })
        }
        Reply::Done => Ok(Outcome::Done),
        Reply::Raster {
            descriptor,
            metadata,
            bytes,
        } => {
            validate_returned_descriptor(&descriptor, request, budget)?;
            if descriptor.required_bytes()? as u64 != bytes {
                return Err(Error::Worker("raster reply length mismatch".into()));
            }
            let reservation = budget.reserve(ResourceKind::Metadata, metadata.total()?)?;
            let metadata = metadata.read(&mut reader, budget.policy().max_metadata_bytes)?;
            let metadata = SharedMetadata::from_reserved(metadata, reservation)?;
            let raster = FileRasterSource::from_reader_with_metadata(
                descriptor,
                &mut reader,
                metadata,
                budget,
                &CancellationToken::new(),
            )?;
            Ok(Outcome::Raster(Arc::new(raster)))
        }
        Reply::Encoded { receipt, bytes } => {
            validate_receipt(&receipt, bytes, request, budget)?;
            let disk = budget.reserve(ResourceKind::TempDisk, bytes)?;
            let mut file = PrivateFile::new()?;
            let _scratch =
                budget.reserve(ResourceKind::Transport, protocol::TRANSFER_BYTES as u64)?;
            let mut buffer = [0; protocol::TRANSFER_BYTES];
            copy_exact(
                &mut reader,
                file.as_file_mut(),
                bytes,
                &mut buffer,
                &CancellationToken::new(),
            )?;
            file.flush()?;
            Ok(Outcome::Encoded(EncodedArtifact(Arc::new(
                ArtifactStorage {
                    file: Mutex::new(file),
                    receipt,
                    _disk: disk,
                    budget: budget.clone(),
                },
            ))))
        }
    }
}

fn validate_returned_descriptor(
    descriptor: &ImageDescriptor,
    request: &Request,
    budget: &Budget,
) -> Result<()> {
    descriptor.validate_with_policy(budget)?;
    match &request.operation {
        Operation::Preview { max_dimension, .. } => {
            let maximum = (*max_dimension).min(budget.policy().preview_max_dimension);
            if descriptor.width > maximum
                || descriptor.height > maximum
                || descriptor.orientation != Orientation::Normal
            {
                return Err(Error::Worker(
                    "preview exceeds requested bounds or orientation".into(),
                ));
            }
        }
        Operation::Decode(options)
            if options.apply_orientation && descriptor.orientation != Orientation::Normal =>
        {
            return Err(Error::Worker(
                "decoded image orientation was not applied".into(),
            ));
        }
        Operation::Transform {
            descriptor: source,
            options,
        } => {
            let (width, height) =
                if options.apply_orientation && source.orientation.swaps_dimensions() {
                    (source.height, source.width)
                } else {
                    (source.width, source.height)
                };
            let target = match options.max_dimensions {
                Some((w, h)) if width > w || height > h => {
                    if u64::from(w) * u64::from(height) <= u64::from(h) * u64::from(width) {
                        (
                            w,
                            ((u64::from(height) * u64::from(w) / u64::from(width)) as u32).max(1),
                        )
                    } else {
                        (
                            ((u64::from(width) * u64::from(h) / u64::from(height)) as u32).max(1),
                            h,
                        )
                    }
                }
                _ => (width, height),
            };
            let orientation = if options.apply_orientation {
                Orientation::Normal
            } else {
                source.orientation
            };
            if (descriptor.width, descriptor.height) != target
                || descriptor.orientation != orientation
            {
                return Err(Error::Worker(
                    "transform output differs from requested geometry".into(),
                ));
            }
        }
        _ => (),
    }
    Ok(())
}

fn validate_receipt(
    receipt: &EncodeReceipt,
    bytes: u64,
    request: &Request,
    budget: &Budget,
) -> Result<()> {
    receipt.options.validate()?;
    if receipt.bytes != bytes || receipt.format != receipt.options.format() {
        return Err(Error::Worker("invalid encoded artifact receipt".into()));
    }
    let Operation::Encode {
        descriptor,
        options,
    } = &request.operation
    else {
        return Err(unexpected_reply());
    };
    if receipt.width != descriptor.width
        || receipt.height != descriptor.height
        || &receipt.options != options
    {
        return Err(Error::Worker(
            "encoded artifact does not match requested dimensions/options".into(),
        ));
    }
    let depth = match options {
        EncodeOptions::Png(_) => (descriptor.format.sample.bytes() * 8) as u8,
        EncodeOptions::Jxl(_) => descriptor.format.significant_bits,
        EncodeOptions::Avif(options) => options.bit_depth,
        _ => 8,
    };
    let fidelity = match options {
        EncodeOptions::Png(_) => PixelFidelity::Exact,
        EncodeOptions::Bmp(options)
            if descriptor.format.channels.has_alpha() && !options.preserve_alpha =>
        {
            PixelFidelity::LosslessTransformed
        }
        EncodeOptions::Bmp(_) => PixelFidelity::Exact,
        EncodeOptions::WebP(options) if options.exact => PixelFidelity::Exact,
        EncodeOptions::WebP(options) if options.lossless => PixelFidelity::LosslessTransformed,
        EncodeOptions::Jxl(options) if options.exact => PixelFidelity::Exact,
        EncodeOptions::Jxl(options) if options.distance == 0.0 => {
            PixelFidelity::LosslessTransformed
        }
        EncodeOptions::Avif(options) if options.exact => PixelFidelity::Exact,
        _ => PixelFidelity::Lossy,
    };
    if bytes == 0 || receipt.bit_depth != depth || receipt.fidelity != fidelity {
        return Err(Error::Worker(
            "encoded artifact precision or fidelity differs from request".into(),
        ));
    }
    if bytes > budget.policy().max_artifact_bytes {
        return Err(Error::ResourceLimit {
            resource: ResourceKind::EncodedOutput,
            requested: bytes,
            limit: budget.policy().max_artifact_bytes,
        });
    }
    Ok(())
}

fn copy_exact(
    reader: &mut impl Read,
    writer: &mut impl Write,
    mut length: u64,
    buffer: &mut [u8],
    cancellation: &CancellationToken,
) -> Result<()> {
    while length != 0 {
        cancellation.check()?;
        let count = length.min(buffer.len() as u64) as usize;
        reader.read_exact(&mut buffer[..count])?;
        writer.write_all(&buffer[..count])?;
        length -= count as u64;
    }
    Ok(())
}

fn receive_metadata(
    reader: &mut impl Read,
    lengths: MetadataLengths,
    budget: &Budget,
    cached: Option<SharedMetadata>,
) -> Result<SharedMetadata> {
    // The incoming copy is transport scratch. An unchanged profile then reuses the
    // session owner instead of allocating a second retained ICC/XMP object per frame.
    let _transport = budget.reserve(ResourceKind::Transport, lengths.total()?)?;
    let metadata = lengths.read(reader, budget.policy().max_metadata_bytes)?;
    if let Some(shared) = cached
        && shared.get() == &metadata
    {
        return Ok(shared);
    }
    SharedMetadata::new(metadata, budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn frozen_input_is_unlinked_readonly_and_has_no_writable_owner() {
        use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"immutable input").unwrap();
        let path = file.path().to_owned();
        let mut frozen = freeze_input_snapshot(file).unwrap();
        assert!(!path.exists());
        assert_eq!(frozen.0.metadata().unwrap().nlink(), 0);
        // SAFETY: F_GETFL only reads access flags on this owned live descriptor.
        let access = unsafe { libc::fcntl(frozen.0.as_raw_fd(), libc::F_GETFL) };
        assert_eq!(access & libc::O_ACCMODE, libc::O_RDONLY);
        assert!(frozen.0.write_all(b"mutation").is_err());
        assert!(frozen.0.set_len(0).is_err());
        let mut bytes = Vec::new();
        frozen.0.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"immutable input");
    }

    #[test]
    fn io_executor_recovers_from_task_panic_and_releases_task_owners() {
        let budget = Budget::default();
        let reservation = budget.reserve(ResourceKind::Transport, 4096).unwrap();
        let mut executor = IoExecutor::<u32>::new("snow-image-test-io").unwrap();
        let cancel = CancellationToken::new();
        executor
            .submit(&cancel, move || {
                let _reservation = reservation;
                panic!("injected transfer panic");
            })
            .unwrap();
        assert!(matches!(executor.recv(), Err(Error::Worker(_))));
        assert_eq!(budget.total_used(), 0);
        executor.submit(&cancel, || Ok(42)).unwrap();
        assert_eq!(executor.recv().unwrap(), 42);
        assert!(!cancel.is_cancelled());
    }

    #[test]
    fn io_executor_admits_one_task_and_drop_cancels_pending_work() {
        let mut executor = IoExecutor::<()>::new("snow-image-test-io").unwrap();
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        executor
            .submit(&cancel, move || {
                while !task_cancel.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(Error::Cancelled)
            })
            .unwrap();
        assert!(executor.submit(&cancel, || Ok(())).is_err());
        drop(executor);
        assert!(cancel.is_cancelled());
    }

    fn png_capability() -> CodecCapability {
        CodecCapability {
            format: Format::Png,
            decode: true,
            encode: true,
            animation_decode: true,
            input_sample_types: vec![SampleType::U8, SampleType::U16],
            supports_alpha: true,
            max_width: None,
            max_height: None,
        }
    }

    #[test]
    fn capability_metadata_lives_until_final_owner_and_rejects_duplicate_formats() {
        let budget = Budget::default();
        let capabilities = Capabilities::from_values(vec![png_capability()], &budget).unwrap();
        let shared = capabilities.clone();
        let charge = budget.used(ResourceKind::Metadata);
        assert!(charge >= std::mem::size_of::<CodecCapability>() as u64 + 2);
        drop(capabilities);
        assert_eq!(shared[0].format, Format::Png);
        assert_eq!(budget.used(ResourceKind::Metadata), charge);
        drop(shared);
        assert_eq!(budget.total_used(), 0);
        assert!(
            Capabilities::from_values(vec![png_capability(), png_capability()], &budget).is_err()
        );
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn remote_error_keeps_bounded_metadata_after_control_parser_is_gone() {
        let budget = Budget::default();
        let request = Request {
            version: protocol::VERSION,
            generation: 4,
            id: 9,
            policy: budget.policy().clone(),
            operation: Operation::Capabilities,
            input_len: 0,
            payload_storage: protocol::PayloadStorage::Pipe,
            metadata: MetadataLengths::default(),
        };
        let mut wire = Vec::new();
        protocol::write_control(
            &mut wire,
            &Response {
                version: protocol::VERSION,
                generation: 4,
                id: 9,
                reply: Reply::Failure(protocol::RemoteError::Unsupported("a".repeat(8192))),
            },
        )
        .unwrap();
        let error = match receive_response(&wire[..], &request, &budget, None, false, false) {
            Err(error) => error,
            Ok(_) => panic!("expected remote failure"),
        };
        assert!(
            matches!(&error, Error::Unsupported(message) if message.len() == MAX_DIAGNOSTIC_BYTES)
        );
        assert_eq!(budget.used(ResourceKind::Transport), 0);
        assert_eq!(
            budget.used(ResourceKind::Metadata),
            MAX_DIAGNOSTIC_BYTES as u64
        );
        drop(error);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn shared_reply_requires_configured_static_backing_and_exact_payload() {
        for (backed, shared, bytes) in [(false, false, 4), (true, false, 4), (true, true, 3)] {
            let budget = Budget::default();
            let request = Request {
                version: protocol::VERSION,
                generation: 4,
                id: 9,
                policy: budget.policy().clone(),
                operation: Operation::Decode(DecodeOptions::default()),
                input_len: 1,
                payload_storage: protocol::PayloadStorage::Pipe,
                metadata: MetadataLengths::default(),
            };
            let mut wire = Vec::new();
            protocol::write_control(
                &mut wire,
                &Response {
                    version: protocol::VERSION,
                    generation: 4,
                    id: 9,
                    reply: Reply::SharedImage {
                        descriptor: ImageDescriptor::packed(
                            1,
                            1,
                            PixelFormat::RGBA8,
                            ColorEncoding::Srgb,
                        )
                        .unwrap(),
                        metadata: MetadataLengths::default(),
                        bytes,
                    },
                },
            )
            .unwrap();
            assert!(matches!(
                receive_response(&wire[..], &request, &budget, None, backed, shared),
                Err(Error::Worker(_))
            ));
            assert_eq!(budget.total_used(), 0);
            assert_eq!(budget.used(ResourceKind::TempDisk), 0);
        }
    }
}
