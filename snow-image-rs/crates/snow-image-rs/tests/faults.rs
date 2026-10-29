//! Process-fault tests. Native cases opt in with SNOW_IMAGE_WORKER and --ignored.
#![cfg(unix)]
use std::{
    future::Future,
    io::Write,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    pin::Pin,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Waker},
    time::{Duration, Instant},
};

use snow_image_protocol::{
    self as protocol, Command as WorkerCommand, MetadataLengths, Operation, Request,
};
use snow_image_rs::*;

fn script(directory: &Path, body: &str) -> PathBuf {
    raw_script(directory, &format!("{}\n{body}", handoff_prelude()))
}
fn raw_script(directory: &Path, body: &str) -> PathBuf {
    let path = directory.join("worker.py");
    std::fs::write(
        &path,
        format!("#!/usr/bin/env python3\nimport os,sys,json,struct\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}
fn handoff_prelude() -> &'static str {
    // Model the production capability handoff while keeping existing malformed
    // worker fixtures focused on their original protocol/termination faults.
    r#"pixel_backing=None
handoff=None
if '--output-handoff' in sys.argv:
 import socket,array
 handoff=socket.socket(fileno=os.dup(2))
 data,ancillary,flags,address=handoff.recvmsg(64,socket.CMSG_SPACE(2*array.array('i').itemsize))
 if data!=b'SIMG\x01\x00\x02\x00' or flags&(socket.MSG_TRUNC|socket.MSG_CTRUNC): sys.exit(91)
 handles=array.array('i')
 for level,kind,payload in ancillary:
  if level==socket.SOL_SOCKET and kind==socket.SCM_RIGHTS: handles.frombytes(payload)
 if len(handles)!=2: sys.exit(90)
 os.dup2(handles[0],2);os.close(handles[0])
 pixel_backing=handles[1]
"#
}
fn config(worker: PathBuf) -> ServiceConfig {
    let mut config = ServiceConfig::new(worker);
    config.operation_timeout = Duration::from_secs(3);
    config
}
fn quiescent(budget: &Budget) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while budget.total_used() != 0
        || budget.used(ResourceKind::TempDisk) != 0
        || budget.used(ResourceKind::CodecOperations) != 0
    {
        assert!(
            Instant::now() < deadline,
            "resources leaked: memory={}, disk={}, operations={}",
            budget.total_used(),
            budget.used(ResourceKind::TempDisk),
            budget.used(ResourceKind::CodecOperations)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn bounded_wait(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("worker did not exit after parent EOF");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Own the deliberately orphaned worker and pipe holder even if setup or an
/// assertion fails before their identities have been published to the test.
struct ParentFixture {
    parent: Child,
}
impl ParentFixture {
    fn spawn(script: &Path) -> Self {
        Self {
            parent: Command::new(script).process_group(0).spawn().unwrap(),
        }
    }
}
impl Drop for ParentFixture {
    fn drop(&mut self) {
        // SAFETY: process_group(0) created a private group led by this fixture's
        // owned child. Its worker and holder deliberately remain in that group.
        unsafe { libc::kill(-(self.parent.id() as i32), libc::SIGKILL) };
        let _ = self.parent.wait();
    }
}

fn publish_children_marker(marker: &Path) -> String {
    // Existence is the reader's publication signal. Opening the final path
    // directly would expose an empty/truncated JSON document before write/close.
    format!(
        "with open({pending:?},'w') as marker_file:\n marker_file.write(json.dumps([w.pid,holder]))\nos.replace({pending:?},{marker:?})",
        pending = marker.with_extension("pending").to_str().unwrap(),
        marker = marker.to_str().unwrap(),
    )
}

fn input_prelude() -> &'static str {
    r#"prefix=sys.stdin.buffer.read(4)
if len(prefix)!=4: sys.exit(0)
r=json.loads(sys.stdin.buffer.read(struct.unpack('<I',prefix)[0]))['Run']
left=r['input_len']+sum(r['metadata'].values())
backed=r.get('payload_storage','Pipe')=='Backing'
input_capability=None
if r.get('payload_storage')=='InputHandle':
 import fcntl
 data,ancillary,flags,address=handoff.recvmsg(64,socket.CMSG_SPACE(array.array('i').itemsize))
 if data!=b'SINI\x01\x00\x01\x00' or flags&(socket.MSG_TRUNC|socket.MSG_CTRUNC): sys.exit(92)
 handles=array.array('i')
 for level,kind,payload in ancillary:
  if level==socket.SOL_SOCKET and kind==socket.SCM_RIGHTS: handles.frombytes(payload)
 if len(handles)!=1: sys.exit(93)
 input_capability=handles[0]
 status=os.fstat(input_capability)
 if status.st_size!=r['input_len'] or status.st_nlink!=0 or fcntl.fcntl(input_capability,fcntl.F_GETFL)&os.O_ACCMODE!=os.O_RDONLY: sys.exit(94)
if backed: os.lseek(2,0,os.SEEK_SET)
offset=0
while left:
 if input_capability is not None: b=os.pread(input_capability,min(left,65536),offset)
 else: b=os.read(2,min(left,65536)) if backed else sys.stdin.buffer.read(min(left,65536))
 if not b: sys.exit(2)
 left-=len(b)
 offset+=len(b)
if backed: os.lseek(2,0,os.SEEK_SET);os.ftruncate(2,0)
"#
}

fn backed_reply() -> String {
    let descriptor =
        ImageDescriptor::packed(1, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
    format!(
        "reply={{'version':1,'generation':r['generation'],'id':r['id'],'reply':{{'BackedImage':{{'descriptor':json.loads({:?}),'metadata':{{'icc':0,'exif':0,'xmp':0}},'bytes':4}}}}}}\nb=json.dumps(reply).encode();sys.stdout.buffer.write(struct.pack('<I',len(b))+b);sys.stdout.buffer.flush()\n",
        serde_json::to_string(&descriptor).unwrap()
    )
}

#[cfg(target_os = "macos")]
fn shared_reply() -> String {
    backed_reply().replace("BackedImage", "SharedImage")
}

#[cfg(target_os = "macos")]
fn shared_pixels() -> &'static str {
    "import mmap\npage=os.sysconf('SC_PAGE_SIZE')\nos.ftruncate(pixel_backing,page)\npixels=mmap.mmap(pixel_backing,page)\npixels[:4]=bytes([17,18,19,255])\n"
}

fn stop_service(service: ImageService) {
    let monitor = service.worker_monitor();
    drop(service);
    let deadline = Instant::now() + Duration::from_secs(5);
    while monitor.statistics().live != 0 || monitor.statistics().spawning != 0 {
        assert!(Instant::now() < deadline, "worker pool did not shut down");
        std::thread::sleep(Duration::from_millis(5));
    }
    let counts = monitor.statistics();
    assert_eq!(counts.spawned, counts.reaped);
}

fn wait_for_static_pool(service: &ImageService) -> u64 {
    let expected = u64::from(service.config().policy.max_codec_operations);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let counts = service.worker_statistics();
        if counts.live == expected && counts.spawning == 0 {
            return expected;
        }
        assert!(Instant::now() < deadline, "static pool did not become idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn assert_animation_worker_retired(service: &ImageService, static_workers: u64, input_bytes: u64) {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if service.budget().used(ResourceKind::Scratch) == 0
            && service.budget().used(ResourceKind::TempDisk) == input_bytes
            && service.worker_statistics().live == static_workers
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "idle failed animation worker retained owners: scratch={}, disk={}, live={}",
            service.budget().used(ResourceKind::Scratch),
            service.budget().used(ResourceKind::TempDisk),
            service.worker_statistics().live
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn failed_animation_open_retires_worker_before_service_or_input_drop() {
    for admission_failure in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let worker = script(
            directory.path(),
            &format!(
                "{}reply={{'version':1,'generation':r['generation'],'id':r['id'],'reply':{{'Failure':{{'Invalid':'bad animation'}}}}}}\nb=json.dumps(reply).encode();sys.stdout.buffer.write(struct.pack('<I',len(b))+b);sys.stdout.buffer.flush()\nwhile sys.stdin.buffer.read(1): pass",
                input_prelude(),
            ),
        );
        let service = ImageService::new(config(worker)).unwrap();
        let baseline = wait_for_static_pool(&service);
        let input = service.input_bytes(b"input").unwrap();
        let threads = admission_failure.then(|| {
            service
                .budget()
                .reserve(
                    ResourceKind::CodecThreads,
                    u64::from(service.config().policy.max_codec_threads),
                )
                .unwrap()
        });
        let result = service
            .open_animation(input.clone(), DecodeOptions::default())
            .wait();
        assert!(result.is_err());
        if admission_failure {
            assert!(matches!(
                result,
                Err(Error::ResourceLimit {
                    resource: ResourceKind::CodecThreads,
                    ..
                })
            ));
        }
        drop(result);
        drop(threads);
        assert_animation_worker_retired(&service, baseline, input.len());
        let budget = service.budget().clone();
        drop(input);
        stop_service(service);
        quiescent(&budget);
    }
}

#[test]
fn failed_animation_commands_retire_worker_while_session_remains_open() {
    fn finish<T>(job: Job<T>, marker: &Path, cancelled: bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !marker.exists() {
            assert!(Instant::now() < deadline, "animation command did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        if cancelled {
            job.cancel();
        }
        let result = job.wait();
        assert!(result.is_err());
        if cancelled {
            assert!(matches!(result, Err(Error::Cancelled)));
        }
        drop(result);
    }

    for reset in [false, true] {
        for cancelled in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let marker = directory.path().join("command-entered");
            let failure = if cancelled {
                "time.sleep(30)"
            } else {
                "os._exit(23)"
            };
            let worker = script(
                directory.path(),
                &format!(
                    r#"import time
while True:
 prefix=sys.stdin.buffer.read(4)
 if len(prefix)!=4: sys.exit(0)
 command=json.loads(sys.stdin.buffer.read(struct.unpack('<I',prefix)[0]))
 if 'Run' not in command: sys.exit(0)
 r=command['Run']
 left=r['input_len']+sum(r['metadata'].values())
 while left:
  data=sys.stdin.buffer.read(min(left,65536))
  if not data: sys.exit(0)
  left-=len(data)
 if 'AnimationOpen' in r['operation']:
  answer={{'AnimationOpened':{{'session':1,'width':1,'height':1,'frame_count':2,'loop_count':0,'durations_ms':[]}}}}
 else:
  open({:?},'w').close()
  {failure}
  sys.exit(0)
 reply={{'version':1,'generation':r['generation'],'id':r['id'],'reply':answer}}
 b=json.dumps(reply).encode()
 sys.stdout.buffer.write(struct.pack('<I',len(b))+b)
 sys.stdout.buffer.flush()
"#,
                    marker.to_str().unwrap(),
                ),
            );
            let service = ImageService::new(config(worker)).unwrap();
            let baseline = wait_for_static_pool(&service);
            let input = service.input_bytes(b"input").unwrap();
            let session = service
                .open_animation(input.clone(), DecodeOptions::default())
                .wait()
                .unwrap();
            if reset {
                finish(session.reset(), &marker, cancelled);
            } else {
                finish(session.next_frame(), &marker, cancelled);
            }
            // The local session and encoded source remain available for bounded recovery,
            // but a failed process must not retain its memory or temporary-disk envelope.
            assert_animation_worker_retired(&service, baseline, input.len());
            let budget = service.budget().clone();
            drop(session);
            drop(input);
            stop_service(service);
            quiescent(&budget);
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn idle_animation_footprint_retires_worker_and_recovers_without_losing_snapshots() {
    assert_idle_animation_recovers(false);
}

#[test]
fn idle_animation_exit_retires_worker_and_recovers_without_losing_snapshots() {
    assert_idle_animation_recovers(true);
}

fn assert_idle_animation_recovers(exit_while_idle: bool) {
    let directory = tempfile::tempdir().unwrap();
    let trigger = directory.path().join("fail-after-response");
    let once = directory.path().join("first-generation");
    let generations = directory.path().join("generations");
    let descriptor =
        ImageDescriptor::packed(1, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
    let descriptor_json = serde_json::to_string(&descriptor).unwrap();
    let exit_while_idle_python = if exit_while_idle { "True" } else { "False" };
    let worker = script(
        directory.path(),
        &format!(
            r#"import mmap,threading,time
retained=None
def fail_when_triggered():
 global retained
 while not os.path.exists({trigger:?}): time.sleep(0.002)
 if {exit_while_idle_python}: os._exit(23)
 retained=mmap.mmap(-1,128*1024*1024)
 # Touch every hardware page so this is physical footprint, not just address space.
 for offset in range(0,len(retained),os.sysconf('SC_PAGE_SIZE')):
  retained[offset:offset+8]=struct.pack('<Q',offset+1)
if '--persistent' in sys.argv:
 with open({generations:?},'a') as output: output.write(str(os.getpid())+'\n')
 try:
  first=os.open({once:?},os.O_CREAT|os.O_EXCL|os.O_WRONLY)
 except FileExistsError: pass
 else:
  os.close(first)
  threading.Thread(target=fail_when_triggered,daemon=True).start()
index=0
while True:
 prefix=sys.stdin.buffer.read(4)
 if len(prefix)!=4: sys.exit(0)
 command=json.loads(sys.stdin.buffer.read(struct.unpack('<I',prefix)[0]))
 if 'Run' not in command: sys.exit(0)
 r=command['Run']
 left=r['input_len']+sum(r['metadata'].values())
 while left:
  data=sys.stdin.buffer.read(min(left,65536))
  if not data: sys.exit(0)
  left-=len(data)
 pixels=b''
 if 'AnimationOpen' in r['operation']:
  index=0
  answer={{'AnimationOpened':{{'session':1,'width':1,'height':1,'frame_count':3,'loop_count':0,'durations_ms':[]}}}}
 elif 'AnimationAdvance' in r['operation'] and index<3:
  answer={{'AnimationFrame':{{'descriptor':json.loads({descriptor_json:?}),'metadata':{{'icc':0,'exif':0,'xmp':0}},'bytes':4,'index':index,'duration_ms':10}}}}
  pixels=bytes([index+1,2,3,255])
  index+=1
 elif 'AnimationReset' in r['operation']:
  index=0
  answer='Done'
 else: answer='Done'
 reply={{'version':1,'generation':r['generation'],'id':r['id'],'reply':answer}}
 b=json.dumps(reply).encode()
 sys.stdout.buffer.write(struct.pack('<I',len(b))+b+pixels)
 sys.stdout.buffer.flush()
"#,
            trigger = trigger.to_str().unwrap(),
            generations = generations.to_str().unwrap(),
            once = once.to_str().unwrap(),
        ),
    );
    let mut config = config(worker);
    config.policy.worker_memory_bytes = 64 * 1024 * 1024;
    let service = ImageService::new(config).unwrap();
    let baseline = wait_for_static_pool(&service);
    let input = service.input_bytes(b"retained animation source").unwrap();
    let session = service
        .open_animation(input.clone(), DecodeOptions::default())
        .wait()
        .unwrap();
    let snapshot = session.next_frame().wait().unwrap().unwrap();
    assert_eq!(snapshot.index, 0);
    assert_eq!(snapshot.image.bytes(), &[1, 2, 3, 255]);
    assert!(service.budget().used(ResourceKind::Scratch) > 0);
    assert_eq!(service.worker_statistics().live, baseline + 1);

    // The trigger is external to the protocol and follows completed publication.
    // No command, session drop, or service shutdown may drive the retirement.
    std::fs::write(&trigger, b"fail").unwrap();
    assert_animation_worker_retired(&service, baseline, input.len());
    let counts = service.worker_statistics();
    if !exit_while_idle {
        assert!(counts.sampled_peak_footprint_bytes > service.config().policy.worker_memory_bytes);
    }
    assert_eq!(counts.spawned - counts.reaped, baseline);
    assert_eq!(snapshot.image.bytes(), &[1, 2, 3, 255]);
    assert_eq!(service.budget().used(ResourceKind::Pixels), 4);

    // Reopening replays only the already consumed frame, then advances normally.
    let recovered = session.next_frame().wait().unwrap().unwrap();
    assert_eq!(recovered.index, 1);
    assert_eq!(recovered.image.bytes(), &[2, 2, 3, 255]);
    assert_eq!(
        std::fs::read_to_string(&generations)
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(snapshot.image.bytes(), &[1, 2, 3, 255]);
    session.reset().wait().unwrap();
    let reset = session.next_frame().wait().unwrap().unwrap();
    assert_eq!(reset.index, 0);
    assert_eq!(reset.image.bytes(), snapshot.image.bytes());

    let budget = service.budget().clone();
    drop(session);
    drop(input);
    stop_service(service);
    assert_eq!(snapshot.image.bytes(), &[1, 2, 3, 255]);
    assert_eq!(recovered.image.bytes(), &[2, 2, 3, 255]);
    drop((snapshot, recovered, reset));
    quiescent(&budget);
}

#[test]
fn early_animation_reply_cannot_bypass_input_transfer_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let replied = directory.path().join("early-reply-sent");
    let ready = directory.path().join("interpreter-ready");
    let worker = script(
        directory.path(),
        &format!(
            r#"import time
if '--persistent' not in sys.argv: open({ready:?},'a').write(str(os.getpid())+'\n')
prefix=sys.stdin.buffer.read(4)
if len(prefix)!=4: sys.exit(0)
r=json.loads(sys.stdin.buffer.read(struct.unpack('<I',prefix)[0]))['Run']
reply={{'version':1,'generation':r['generation'],'id':r['id'],'reply':{{'AnimationOpened':{{'session':1,'width':1,'height':1,'frame_count':2,'loop_count':0,'durations_ms':[]}}}}}}
b=json.dumps(reply).encode()
sys.stdout.buffer.write(struct.pack('<I',len(b))+b)
sys.stdout.buffer.flush()
open({:?},'w').close()
# Keep the input pipe open without reading the declared multi-megabyte payload.
# This fallback bounds the pre-fix regression without depending on the supervisor.
time.sleep(1)
os._exit(23)
"#,
            replied.to_str().unwrap(),
            ready = ready.to_str().unwrap(),
        ),
    );
    let mut config = config(worker);
    config.operation_timeout = Duration::from_millis(300);
    let service = ImageService::new(config).unwrap();
    let static_workers = wait_for_static_pool(&service);
    // A fresh Python script can incur interpreter startup before
    // any request is read. Wait for the prestarted fixture workers to reach the
    // script body so this deadline test measures the unfinished input transfer.
    let ready_deadline = Instant::now() + Duration::from_secs(3);
    while std::fs::read_to_string(&ready)
        .unwrap_or_default()
        .lines()
        .count()
        < static_workers as usize
    {
        assert!(
            Instant::now() < ready_deadline,
            "fake worker interpreter did not become ready"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let input = service.input_bytes(&vec![1; 2 * 1024 * 1024]).unwrap();
    let started = Instant::now();
    let result = service
        .open_animation(input.clone(), DecodeOptions::default())
        .wait();
    assert!(
        replied.exists(),
        "worker did not send an early reply: {:?}",
        result.err()
    );
    assert!(matches!(result, Err(Error::Worker(_) | Error::Io(_))));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "a premature reply bypassed the configured input-transfer deadline"
    );
    drop(result);
    assert_animation_worker_retired(&service, static_workers, input.len());
    let budget = service.budget().clone();
    drop(input);
    stop_service(service);
    quiescent(&budget);
}

#[test]
fn worker_failure_cancels_cooperative_input_without_replacing_original_error() {
    struct CooperativeRows {
        descriptor: ImageDescriptor,
        entered: PathBuf,
        fail: PathBuf,
        observed_cancel: std::sync::atomic::AtomicBool,
    }
    impl RasterSource for CooperativeRows {
        fn descriptor(&self) -> &ImageDescriptor {
            &self.descriptor
        }
        fn access(&self) -> RasterAccess {
            RasterAccess::SequentialRows
        }
        fn read_rows(&self, _: u32, _: u32, _: &mut [u8]) -> Result<()> {
            Err(Error::InvalidInput(
                "cancellable entry point required".into(),
            ))
        }
        fn read_rows_cancellable(
            &self,
            _: u32,
            _: u32,
            _: &mut [u8],
            cancel: &CancellationToken,
        ) -> Result<()> {
            std::fs::write(&self.entered, b"ready")?;
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut failed_at = None;
            while Instant::now() < deadline {
                if cancel.is_cancelled() {
                    self.observed_cancel.store(true, Ordering::Relaxed);
                    return Err(Error::Cancelled);
                }
                if self.fail.exists() {
                    let failed = failed_at.get_or_insert_with(Instant::now);
                    if failed.elapsed() >= Duration::from_secs(2) {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(Error::InvalidInput(
                "input cancellation was not relayed".into(),
            ))
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let entered = directory.path().join("producer-entered");
    let ready = directory.path().join("worker-ready");
    let fail = directory.path().join("fail-worker");
    let worker = script(
        directory.path(),
        &format!(
            "import time\nprefix=sys.stdin.buffer.read(4)\nif len(prefix)!=4: sys.exit(0)\nsys.stdin.buffer.read(struct.unpack('<I',prefix)[0])\nwhile not os.path.exists({:?}): time.sleep(0.001)\nopen({:?},'w').close()\nwhile not os.path.exists({:?}): time.sleep(0.001)\nos._exit(23)",
            entered.to_str().unwrap(),
            ready.to_str().unwrap(),
            fail.to_str().unwrap(),
        ),
    );
    let mut configuration = config(worker);
    // Interpreter startup under parallel fault tests is setup, not the failure
    // response being measured below. Keep that setup bounded independently.
    configuration.operation_timeout = Duration::from_secs(10);
    let service = ImageService::new(configuration).unwrap();
    let source = Arc::new(CooperativeRows {
        descriptor: ImageDescriptor::packed(1, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap(),
        entered,
        fail: fail.clone(),
        observed_cancel: std::sync::atomic::AtomicBool::new(false),
    });
    let job = service.encode(source.clone(), EncodeOptions::default());
    let ready_deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        assert!(
            Instant::now() < ready_deadline,
            "worker and cooperative producer did not become ready"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let started = Instant::now();
    std::fs::write(fail, b"fail now").unwrap();
    let result = job.wait();
    assert!(matches!(result, Err(Error::Worker(_) | Error::Io(_))));
    assert!(source.observed_cancel.load(Ordering::Relaxed));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "worker failure did not promptly cancel and release the cooperative producer"
    );
    drop(result);
    quiescent(service.budget());
    stop_service(service);
}

#[test]
fn backing_reply_is_not_published_until_writer_exits_and_survives_service_close() {
    let directory = tempfile::tempdir().unwrap();
    let ready = directory.path().join("ready");
    let release = directory.path().join("release");
    let worker = script(
        directory.path(),
        &format!(
            "{}import time\nos.write(2,bytes([1,1,1,1]))\n{}open({:?},'w').close()\nwhile not os.path.exists({:?}): time.sleep(0.005)\nos.lseek(2,0,os.SEEK_SET);os.write(2,bytes([2,3,4,255]));os._exit(0)",
            input_prelude(),
            backed_reply(),
            ready.to_str().unwrap(),
            release.to_str().unwrap()
        ),
    );
    let service = ImageService::new(config(worker)).unwrap();
    let budget = service.budget().clone();
    let mut job = service.decode(
        service.input_bytes(b"input").unwrap(),
        DecodeOptions::default(),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ready.exists() {
        assert!(Instant::now() < deadline, "worker did not send its reply");
        std::thread::sleep(Duration::from_millis(5));
    }
    // Reply is already available, but the same file still has a live writer.
    std::thread::sleep(Duration::from_millis(25));
    let mut context = Context::from_waker(Waker::noop());
    assert!(Pin::new(&mut job).poll(&mut context).is_pending());
    std::fs::write(release, b"finish").unwrap();
    let image = job.wait().unwrap();
    assert_eq!(image.bytes(), &[2, 3, 4, 255]);
    assert_eq!(budget.used(ResourceKind::Pixels), 4);
    assert_eq!(budget.used(ResourceKind::TempDisk), 4);
    let retained = image.clone();
    drop(image);
    stop_service(service);
    assert_eq!(retained.bytes(), &[2, 3, 4, 255]);
    drop(retained);
    quiescent(&budget);
}

#[test]
fn backing_file_length_must_match_the_reply_after_exit() {
    for length in [3, 5] {
        let directory = tempfile::tempdir().unwrap();
        let worker = script(
            directory.path(),
            &format!(
                "{}os.write(2,bytes({length}))\n{}os._exit(0)",
                input_prelude(),
                backed_reply()
            ),
        );
        let service = ImageService::new(config(worker)).unwrap();
        let result = service
            .decode(
                service.input_bytes(b"input").unwrap(),
                DecodeOptions::default(),
            )
            .wait();
        assert!(
            matches!(result, Err(Error::Worker(message)) if message.contains("file length mismatch"))
        );
        quiescent(service.budget());
        stop_service(service);
    }
}

#[test]
fn backing_crash_after_partial_write_releases_unpublished_reservations() {
    let directory = tempfile::tempdir().unwrap();
    let worker = script(
        directory.path(),
        &format!(
            "{}os.write(2,b'xx')\n{}os._exit(23)",
            input_prelude(),
            backed_reply()
        ),
    );
    let service = ImageService::new(config(worker)).unwrap();
    assert!(
        service
            .decode(
                service.input_bytes(b"input").unwrap(),
                DecodeOptions::default()
            )
            .wait()
            .is_err()
    );
    quiescent(service.budget());
    stop_service(service);
}

#[test]
fn backing_publication_terminates_a_forked_writer_that_closed_stdout() {
    let directory = tempfile::tempdir().unwrap();
    let worker = script(
        directory.path(),
        &format!(
            "{}import time\nos.write(2,bytes([17,18,19,255]))\nchild=os.fork()\nif child==0:\n os.close(0);os.close(1)\n time.sleep(30)\n os.lseek(2,0,os.SEEK_SET);os.write(2,bytes([99,99,99,99]));os._exit(0)\n{}os._exit(0)",
            input_prelude(),
            backed_reply()
        ),
    );
    let service = ImageService::new(config(worker)).unwrap();
    let start = Instant::now();
    let image = service
        .decode(
            service.input_bytes(b"input").unwrap(),
            DecodeOptions::default(),
        )
        .wait()
        .unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(image.bytes(), &[17, 18, 19, 255]);
    drop(image);
    quiescent(service.budget());
    stop_service(service);
}

#[cfg(target_os = "macos")]
#[test]
fn shared_pixels_wait_for_exit_retain_rounded_charge_and_survive_service_close() {
    let directory = tempfile::tempdir().unwrap();
    let ready = directory.path().join("ready");
    let release = directory.path().join("release");
    let worker = script(
        directory.path(),
        &format!(
            "{}import time\n{}{}open({:?},'w').close()\nwhile not os.path.exists({:?}): time.sleep(0.005)\npixels[:4]=bytes([2,3,4,255]);os._exit(0)",
            input_prelude(),
            shared_pixels(),
            shared_reply(),
            ready.to_str().unwrap(),
            release.to_str().unwrap(),
        ),
    );
    let service = ImageService::new(config(worker)).unwrap();
    let budget = service.budget().clone();
    let mut job = service.decode(
        service.input_bytes(b"input").unwrap(),
        DecodeOptions::default(),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ready.exists() {
        assert!(
            Instant::now() < deadline,
            "worker did not send shared reply"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut context = Context::from_waker(Waker::noop());
    assert!(Pin::new(&mut job).poll(&mut context).is_pending());
    std::fs::write(release, b"finish").unwrap();
    let image = job.wait().unwrap();
    let allocation = snow_image_transport::allocation_len(4).unwrap() as u64;
    assert_eq!(image.bytes(), &[2, 3, 4, 255]);
    assert_eq!(budget.used(ResourceKind::Pixels), allocation);
    assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    let retained = image.clone();
    drop(image);
    stop_service(service);
    assert_eq!(retained.bytes(), &[2, 3, 4, 255]);
    assert_eq!(budget.used(ResourceKind::Pixels), allocation);
    drop(retained);
    quiescent(&budget);
}

#[cfg(target_os = "macos")]
#[test]
fn shared_pixels_reject_wrong_allocation_and_crash_without_retaining_leases() {
    for (pages, exit_code) in [(0, 0), (2, 0), (1, 23)] {
        let directory = tempfile::tempdir().unwrap();
        let worker = script(
            directory.path(),
            &format!(
                "{}os.ftruncate(pixel_backing,os.sysconf('SC_PAGE_SIZE')*{pages})\n{}os._exit({exit_code})",
                input_prelude(),
                shared_reply(),
            ),
        );
        let service = ImageService::new(config(worker)).unwrap();
        let result = service
            .decode(
                service.input_bytes(b"input").unwrap(),
                DecodeOptions::default(),
            )
            .wait();
        assert!(result.is_err());
        drop(result);
        quiescent(service.budget());
        stop_service(service);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn shared_pixels_reap_forked_mapping_writer_before_publication() {
    let directory = tempfile::tempdir().unwrap();
    let worker = script(
        directory.path(),
        &format!(
            "{}{}import time\nchild=os.fork()\nif child==0:\n os.close(0);os.close(1);time.sleep(30)\n pixels[:4]=bytes([99,99,99,99]);os._exit(0)\n{}os._exit(0)",
            input_prelude(),
            shared_pixels(),
            shared_reply(),
        ),
    );
    let service = ImageService::new(config(worker)).unwrap();
    let start = Instant::now();
    let image = service
        .decode(
            service.input_bytes(b"input").unwrap(),
            DecodeOptions::default(),
        )
        .wait()
        .unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(image.bytes(), &[17, 18, 19, 255]);
    assert_eq!(service.budget().used(ResourceKind::TempDisk), 0);
    drop(image);
    quiescent(service.budget());
    stop_service(service);
}

#[cfg(target_os = "macos")]
#[test]
fn static_inputs_use_readonly_capabilities_across_previous_size_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let worker = script(
        directory.path(),
        &format!(
            "{}if r['payload_storage']!='InputHandle': os._exit(91)\n{}{}",
            input_prelude(),
            shared_pixels(),
            shared_reply(),
        ),
    );
    let service = ImageService::new(config(worker)).unwrap();
    for bytes in [2 * 1024 * 1024, 2 * 1024 * 1024 + 1] {
        let input = service.input_bytes(&vec![1; bytes]).unwrap();
        let image = service
            .decode(input, DecodeOptions::default())
            .wait()
            .unwrap();
        assert_eq!(image.bytes(), &[17, 18, 19, 255]);
        drop(image);
        quiescent(service.budget());
    }
    let image = service
        .preview(service.input_bytes(b"input").unwrap(), 1)
        .wait()
        .unwrap();
    assert_eq!(image.bytes(), &[17, 18, 19, 255]);
    drop(image);
    quiescent(service.budget());
    stop_service(service);
}

#[cfg(target_os = "macos")]
#[test]
fn transferred_input_retains_its_disk_owner_until_success_or_cancellation_is_reaped() {
    for cancel in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let ready = directory.path().join("input-received");
        let release = directory.path().join("release");
        let worker = script(
            directory.path(),
            &format!(
                "{}import time\nfor mutation in [lambda:os.write(input_capability,b'bad'),lambda:os.ftruncate(input_capability,0)]:\n try: mutation()\n except OSError: pass\n else: os._exit(95)\nif os.pread(input_capability,4,0)!=b'zzzz': os._exit(96)\nopen({:?},'w').close()\nwhile not os.path.exists({:?}): time.sleep(0.005)\n{}{}",
                input_prelude(),
                ready.to_str().unwrap(),
                release.to_str().unwrap(),
                shared_pixels(),
                shared_reply(),
            ),
        );
        let mut options = config(worker);
        options.policy.max_temp_bytes = 8 * 1024 * 1024;
        let service = ImageService::new(options).unwrap();
        let bytes = 65_536;
        let input = service.input_bytes(&vec![b'z'; bytes]).unwrap();
        // Move away the caller's final clone. Only the connection may retain the
        // disk lease after its input sender has finished the descriptor handoff.
        let job = service.decode(input, DecodeOptions::default());
        let deadline = Instant::now() + Duration::from_secs(3);
        while !ready.exists() {
            assert!(
                Instant::now() < deadline,
                "worker did not receive readonly input"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let input_disk = bytes as u64;
        let child_disk = (service.config().policy.max_temp_bytes - input_disk) / 2;
        assert_eq!(
            service.budget().used(ResourceKind::TempDisk),
            input_disk + child_disk
        );
        if cancel {
            job.cancel();
            assert!(matches!(job.wait(), Err(Error::Cancelled)));
        } else {
            std::fs::write(release, b"finish").unwrap();
            let image = job.wait().unwrap();
            assert_eq!(image.bytes(), &[17, 18, 19, 255]);
            assert_eq!(service.budget().used(ResourceKind::TempDisk), 0);
            drop(image);
        }
        quiescent(service.budget());
        stop_service(service);
    }
}

#[test]
fn concurrent_worker_spawns_do_not_inherit_sibling_pipe_descriptors() {
    let directory = tempfile::tempdir().unwrap();
    let worker = script(
        directory.path(),
        &format!(
            "import fcntl\nextra=[]\nfor fd in range(3,256):\n if fd==pixel_backing or (handoff is not None and fd==handoff.fileno()): continue\n try: fcntl.fcntl(fd,fcntl.F_GETFD)\n except OSError: pass\n else: extra.append(fd)\n{}if extra:\n reply={{'version':1,'generation':r['generation'],'id':r['id'],'reply':{{'Failure':{{'Invalid':'inherited unrelated descriptors: '+str(extra)}}}}}}\n b=json.dumps(reply).encode();sys.stdout.buffer.write(struct.pack('<I',len(b))+b);sys.stdout.buffer.flush();sys.exit(1)\nos.write(2,bytes([1,2,3,255]))\n{}",
            input_prelude(),
            backed_reply(),
        ),
    );
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let mut threads = Vec::new();
    for _ in 0..4 {
        let worker = worker.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            let service = ImageService::new(config(worker)).unwrap();
            for _ in 0..4 {
                let image = service
                    .decode(
                        service.input_bytes(b"input").unwrap(),
                        DecodeOptions::default(),
                    )
                    .wait()
                    .unwrap();
                assert_eq!(image.bytes(), &[1, 2, 3, 255]);
            }
            quiescent(service.budget());
            stop_service(service);
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn encoded_receipts_reject_impossible_precision_and_broken_exactness() {
    for (options, bit_depth, fidelity) in [
        (EncodeOptions::default(), 0, PixelFidelity::Exact),
        (EncodeOptions::default(), 255, PixelFidelity::Exact),
        (EncodeOptions::default(), 16, PixelFidelity::Exact),
        (
            EncodeOptions::Jpeg(JpegOptions::default()),
            16,
            PixelFidelity::Lossy,
        ),
        (
            EncodeOptions::WebP(WebPOptions::default()),
            8,
            PixelFidelity::Lossy,
        ),
        (
            EncodeOptions::Jxl(JxlOptions::default()),
            8,
            PixelFidelity::Lossy,
        ),
        (
            EncodeOptions::WebP(WebPOptions::default()),
            8,
            PixelFidelity::LosslessTransformed,
        ),
        (
            EncodeOptions::Jxl(JxlOptions::default()),
            8,
            PixelFidelity::LosslessTransformed,
        ),
        (
            EncodeOptions::Avif(AvifOptions::default()),
            8,
            PixelFidelity::Lossy,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let receipt = EncodeReceipt {
            format: options.format(),
            width: 1,
            height: 1,
            bit_depth,
            bytes: 1,
            options: options.clone(),
            fidelity,
        };
        let worker = script(
            directory.path(),
            &format!(
                "{}os.write(2,b'x')\nreply={{'version':1,'generation':r['generation'],'id':r['id'],'reply':{{'BackedEncoded':{{'receipt':json.loads({:?}),'bytes':1}}}}}}\nb=json.dumps(reply).encode();sys.stdout.buffer.write(struct.pack('<I',len(b))+b);sys.stdout.buffer.flush()",
                input_prelude(),
                serde_json::to_string(&receipt).unwrap(),
            ),
        );
        let service = ImageService::new(config(worker)).unwrap();
        let descriptor =
            ImageDescriptor::packed(1, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
        let source = MutableImage::new(descriptor, Metadata::default(), service.budget())
            .unwrap()
            .seal();
        let result = service.encode(Arc::new(source), options).wait();
        assert!(result.is_err(), "accepted invalid receipt: {receipt:?}");
        drop(result);
        let budget = service.budget().clone();
        stop_service(service);
        quiescent(&budget);
    }
}

#[test]
fn preview_reply_cannot_exceed_requested_fit() {
    let directory = tempfile::tempdir().unwrap();
    let descriptor =
        ImageDescriptor::packed(3, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
    let worker = script(
        directory.path(),
        &format!(
            "{}os.write(2,bytes(12))\nreply={{'version':1,'generation':r['generation'],'id':r['id'],'reply':{{'BackedImage':{{'descriptor':json.loads({:?}),'metadata':{{'icc':0,'exif':0,'xmp':0}},'bytes':12}}}}}}\nb=json.dumps(reply).encode();sys.stdout.buffer.write(struct.pack('<I',len(b))+b);sys.stdout.buffer.flush()",
            input_prelude(),
            serde_json::to_string(&descriptor).unwrap(),
        ),
    );
    let service = ImageService::new(config(worker)).unwrap();
    assert!(
        service
            .preview(service.input_bytes(b"input").unwrap(), 2)
            .wait()
            .is_err()
    );
    let budget = service.budget().clone();
    stop_service(service);
    quiescent(&budget);
}

fn animation_reply_worker(
    directory: &Path,
    session: u64,
    frames: u32,
    premature_end: bool,
) -> PathBuf {
    let send_frame = if premature_end { "False" } else { "True" };
    let descriptor =
        ImageDescriptor::packed(1, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
    let descriptor_json = serde_json::to_string(&descriptor).unwrap();
    script(
        directory,
        &format!(
            r#"while True:
 prefix=sys.stdin.buffer.read(4)
 if len(prefix)!=4: sys.exit(0)
 command=json.loads(sys.stdin.buffer.read(struct.unpack('<I',prefix)[0]))
 if 'Run' not in command: sys.exit(0)
 r=command['Run']
 left=r['input_len']+sum(r['metadata'].values())
 while left:
  data=sys.stdin.buffer.read(min(left,65536))
  if not data: sys.exit(0)
  left-=len(data)
 pixels=b''
 if 'AnimationOpen' in r['operation']:
  answer={{'AnimationOpened':{{'session':{session},'width':2,'height':1,'frame_count':{frames},'loop_count':0,'durations_ms':[]}}}}
 elif 'AnimationAdvance' in r['operation'] and {send_frame}:
  answer={{'AnimationFrame':{{'descriptor':json.loads({descriptor_json:?}),'metadata':{{'icc':0,'exif':0,'xmp':0}},'bytes':4,'index':0,'duration_ms':10}}}}
  pixels=bytes([1,2,3,255])
 else: answer='Done'
 reply={{'version':1,'generation':r['generation'],'id':r['id'],'reply':answer}}
 b=json.dumps(reply).encode()
 sys.stdout.buffer.write(struct.pack('<I',len(b))+b+pixels)
 sys.stdout.buffer.flush()
"#,
        ),
    )
}

#[test]
fn animation_open_rejects_zero_session_or_frame_count() {
    for (session, frames) in [(0, 1), (1, 0)] {
        let directory = tempfile::tempdir().unwrap();
        let worker = animation_reply_worker(directory.path(), session, frames, false);
        let service = ImageService::new(config(worker)).unwrap();
        let result = service
            .open_animation(
                service.input_bytes(b"input").unwrap(),
                DecodeOptions::default(),
            )
            .wait();
        assert!(
            result.is_err(),
            "accepted animation session={session}, frames={frames}"
        );
        drop(result);
        let budget = service.budget().clone();
        stop_service(service);
        quiescent(&budget);
    }
}

#[test]
fn animation_snapshot_must_match_opened_canvas() {
    let directory = tempfile::tempdir().unwrap();
    let worker = animation_reply_worker(directory.path(), 1, 1, false);
    let service = ImageService::new(config(worker)).unwrap();
    let session = service
        .open_animation(
            service.input_bytes(b"input").unwrap(),
            DecodeOptions::default(),
        )
        .wait()
        .unwrap();
    assert!(session.next_frame().wait().is_err());
    drop(session);
    let budget = service.budget().clone();
    stop_service(service);
    quiescent(&budget);
}

#[test]
fn animation_end_cannot_hide_advertised_frames() {
    let directory = tempfile::tempdir().unwrap();
    let worker = animation_reply_worker(directory.path(), 1, 2, true);
    let service = ImageService::new(config(worker)).unwrap();
    let session = service
        .open_animation(
            service.input_bytes(b"input").unwrap(),
            DecodeOptions::default(),
        )
        .wait()
        .unwrap();
    assert!(session.next_frame().wait().is_err());
    drop(session);
    let budget = service.budget().clone();
    stop_service(service);
    quiescent(&budget);
}

#[test]
fn crashed_worker_does_not_retain_reservations() {
    let directory = tempfile::tempdir().unwrap();
    let worker = script(directory.path(), "os._exit(23)");
    let service = ImageService::new(config(worker)).unwrap();
    let input = service.input_bytes(b"invalid").unwrap();
    assert!(
        service
            .decode(input, DecodeOptions::default())
            .wait()
            .is_err()
    );
    quiescent(service.budget());
}

#[test]
fn child_descendant_cannot_hold_response_pipe_after_crash() {
    let directory = tempfile::tempdir().unwrap();
    let worker = script(
        directory.path(),
        &format!(
            "{}import time\nif os.fork()==0:\n time.sleep(30)\nelse:\n os._exit(23)",
            input_prelude()
        ),
    );
    let service = ImageService::new(config(worker)).unwrap();
    let started = Instant::now();
    assert!(
        service
            .decode(
                service.input_bytes(b"input").unwrap(),
                DecodeOptions::default()
            )
            .wait()
            .is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    quiescent(service.budget());
}

#[test]
fn cancellation_reaps_uncooperative_worker_before_completion() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("ready");
    let worker = script(
        directory.path(),
        &format!(
            "{}import time\nopen({:?},'w').close()\ntime.sleep(30)",
            input_prelude(),
            marker.to_str().unwrap()
        ),
    );
    let service = ImageService::new(config(worker)).unwrap();
    let job = service.decode(
        service.input_bytes(b"input").unwrap(),
        DecodeOptions::default(),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !marker.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let start = Instant::now();
    job.cancel();
    assert!(matches!(job.wait(), Err(Error::Cancelled)));
    assert!(start.elapsed() < Duration::from_secs(2));
    quiescent(service.budget());
    assert!(service.worker_statistics().reaped >= 1);
}

#[test]
#[cfg(target_os = "macos")]
fn cancellation_deadline_survives_a_full_idle_input_pipe() {
    let directory = tempfile::tempdir().unwrap();
    let ready = directory.path().join("request-read");
    let worker = script(
        directory.path(),
        &format!(
            r#"import time
def exact(n):
 data=b''
 while len(data)<n:
  part=os.read(0,n-len(data))
  if not part: sys.exit(0)
  data+=part
 return data
prefix=exact(4)
exact(struct.unpack('<I',prefix)[0])
open({:?},'w').close()
# A single 64KiB write fills Darwin's expanded anonymous pipe. Do not consume it.
time.sleep(1)
os._exit(23)
"#,
            ready.to_str().unwrap()
        ),
    );
    let service = ImageService::new(config(worker)).unwrap();
    let descriptor =
        ImageDescriptor::packed(16384, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
    let source = MutableImage::new(descriptor, Metadata::default(), service.budget())
        .unwrap()
        .seal();
    let job = service.encode(Arc::new(source), EncodeOptions::default());
    let deadline = Instant::now() + Duration::from_secs(1);
    // Once only the response parser reservation remains, the input task finished
    // its full write and released its mutex; there is still no free pipe space.
    while !ready.exists()
        || service.budget().used(ResourceKind::Transport) != protocol::CONTROL_PARSE_ALLOWANCE
    {
        assert!(
            Instant::now() < deadline,
            "input pipe was not filled before cancellation"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let started = Instant::now();
    job.cancel();
    assert!(matches!(job.wait(), Err(Error::Cancelled)));
    assert!(started.elapsed() < Duration::from_millis(500));
    quiescent(service.budget());
    stop_service(service);
}

#[test]
fn oversized_worker_control_is_rejected_before_body_allocation() {
    let directory = tempfile::tempdir().unwrap();
    let worker = script(
        directory.path(),
        &format!(
            "{}sys.stdout.buffer.write(struct.pack('<I',{}));sys.stdout.buffer.flush()\nwhile sys.stdin.buffer.read(1): pass",
            input_prelude(),
            protocol::MAX_CONTROL_BYTES + 1
        ),
    );
    let service = ImageService::new(config(worker)).unwrap();
    assert!(
        service
            .decode(
                service.input_bytes(b"input").unwrap(),
                DecodeOptions::default()
            )
            .wait()
            .is_err()
    );
    quiescent(service.budget());
    assert!(service.budget().peak_used(ResourceKind::Pixels) < 1024);
}

#[test]
fn partial_pixel_reply_releases_unpublished_storage() {
    let directory = tempfile::tempdir().unwrap();
    let descriptor =
        ImageDescriptor::packed(4, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
    let descriptor = serde_json::to_string(&descriptor).unwrap();
    let worker = script(
        directory.path(),
        &format!(
            "{}reply={{'version':1,'generation':r['generation'],'id':r['id'],'reply':{{'Image':{{'descriptor':json.loads({:?}),'metadata':{{'icc':0,'exif':0,'xmp':0}},'bytes':16}}}}}}\nb=json.dumps(reply).encode();sys.stdout.buffer.write(struct.pack('<I',len(b))+b+b'partial');sys.stdout.buffer.flush();os._exit(0)",
            input_prelude(),
            descriptor
        ),
    );
    let service = ImageService::new(config(worker)).unwrap();
    assert!(
        service
            .decode(
                service.input_bytes(b"input").unwrap(),
                DecodeOptions::default()
            )
            .wait()
            .is_err()
    );
    quiescent(service.budget());
}

struct FailedRows {
    descriptor: ImageDescriptor,
}
impl RasterSource for FailedRows {
    fn descriptor(&self) -> &ImageDescriptor {
        &self.descriptor
    }
    fn access(&self) -> RasterAccess {
        RasterAccess::SequentialRows
    }
    fn read_rows(&self, _: u32, _: u32, _: &mut [u8]) -> Result<()> {
        Err(Error::InvalidInput(
            "injected raster producer failure".into(),
        ))
    }
}

#[test]
fn failed_raster_producer_stops_waiting_worker_promptly() {
    let directory = tempfile::tempdir().unwrap();
    let worker = script(directory.path(), "while sys.stdin.buffer.read(65536): pass");
    let service = ImageService::new(config(worker)).unwrap();
    let source = FailedRows {
        descriptor: ImageDescriptor::packed(1024, 1024, PixelFormat::RGBA8, ColorEncoding::Srgb)
            .unwrap(),
    };
    let started = Instant::now();
    let result = service
        .encode(Arc::new(source), EncodeOptions::default())
        .wait();
    assert!(
        matches!(result, Err(Error::InvalidInput(message)) if message.as_ref() == "injected raster producer failure")
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    quiescent(service.budget());
}

#[test]
fn panicking_raster_producer_reaps_worker_and_does_not_strand_the_service() {
    struct PanickingRows(ImageDescriptor);
    impl RasterSource for PanickingRows {
        fn descriptor(&self) -> &ImageDescriptor {
            &self.0
        }
        fn access(&self) -> RasterAccess {
            RasterAccess::SequentialRows
        }
        fn read_rows(&self, _: u32, _: u32, _: &mut [u8]) -> Result<()> {
            panic!("injected raster producer panic");
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let worker = script(directory.path(), "while sys.stdin.buffer.read(65536): pass");
    let service = ImageService::new(config(worker)).unwrap();
    let descriptor =
        ImageDescriptor::packed(1, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
    let started = Instant::now();
    let result = service
        .encode(
            Arc::new(PanickingRows(descriptor.clone())),
            EncodeOptions::default(),
        )
        .wait();
    assert!(matches!(result, Err(Error::Worker(_))));
    drop(result);
    assert!(started.elapsed() < Duration::from_secs(2));
    quiescent(service.budget());
    let result = service
        .encode(
            Arc::new(FailedRows { descriptor }),
            EncodeOptions::default(),
        )
        .wait();
    assert!(matches!(result, Err(Error::InvalidInput(_))));
    drop(result);
    quiescent(service.budget());
    stop_service(service);
}

#[test]
fn static_lane_reuses_input_thread_after_failure_and_panic() {
    struct RecordedRows {
        descriptor: ImageDescriptor,
        threads: std::sync::Mutex<Vec<std::thread::ThreadId>>,
    }
    impl RasterSource for RecordedRows {
        fn descriptor(&self) -> &ImageDescriptor {
            &self.descriptor
        }
        fn access(&self) -> RasterAccess {
            RasterAccess::SequentialRows
        }
        fn read_rows(&self, _: u32, _: u32, _: &mut [u8]) -> Result<()> {
            let count = {
                let mut threads = self.threads.lock().unwrap();
                threads.push(std::thread::current().id());
                threads.len()
            };
            if count == 2 {
                panic!("injected producer panic between reusable transfers");
            }
            Err(Error::InvalidInput("injected producer failure".into()))
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let worker = script(directory.path(), "while sys.stdin.buffer.read(65536): pass");
    let mut config = config(worker);
    config.policy.max_codec_operations = 1;
    let service = ImageService::new(config).unwrap();
    let rows = Arc::new(RecordedRows {
        descriptor: ImageDescriptor::packed(1, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap(),
        threads: Default::default(),
    });
    for index in 0..3 {
        let result = service
            .encode(rows.clone(), EncodeOptions::default())
            .wait();
        if index == 1 {
            assert!(matches!(result, Err(Error::Worker(_))));
        } else {
            assert!(matches!(result, Err(Error::InvalidInput(_))));
        }
        drop(result);
        quiescent(service.budget());
    }
    let threads = rows.threads.lock().unwrap();
    assert_eq!(threads.len(), 3);
    assert!(threads.iter().all(|thread| *thread == threads[0]));
    assert!(service.worker_statistics().reaped >= 3);
    stop_service(service);
}

struct PartiallyFailedRows {
    descriptor: ImageDescriptor,
    calls: AtomicUsize,
}
impl RasterSource for PartiallyFailedRows {
    fn descriptor(&self) -> &ImageDescriptor {
        &self.descriptor
    }
    fn access(&self) -> RasterAccess {
        RasterAccess::SequentialRows
    }
    fn read_rows(&self, _: u32, _: u32, destination: &mut [u8]) -> Result<()> {
        if self.calls.fetch_add(1, Ordering::Relaxed) != 0 {
            return Err(Error::InvalidInput(
                "injected partial raster failure".into(),
            ));
        }
        destination.fill(255);
        Ok(())
    }
}

#[test]
#[ignore = "requires SNOW_IMAGE_WORKER pointing to the separately built native worker"]
fn streaming_encoder_reaps_after_partial_producer_failure() {
    let worker = std::env::var_os("SNOW_IMAGE_WORKER").unwrap();
    let service = ImageService::new(config(worker.into())).unwrap();
    for options in [
        EncodeOptions::default(),
        EncodeOptions::Jpeg(JpegOptions::default()),
    ] {
        let source = PartiallyFailedRows {
            descriptor: ImageDescriptor::packed(
                1024,
                8192,
                PixelFormat::RGBA8,
                ColorEncoding::Srgb,
            )
            .unwrap(),
            calls: AtomicUsize::new(0),
        };
        let started = Instant::now();
        let result = service.encode(Arc::new(source), options).wait();
        assert!(
            matches!(result, Err(Error::InvalidInput(message)) if message.as_ref() == "injected partial raster failure")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        quiescent(service.budget());
    }
}

#[test]
#[ignore = "requires SNOW_IMAGE_WORKER pointing to the separately built native worker"]
fn streaming_encoder_rejects_unsupported_precision_without_full_input() {
    let worker = std::env::var_os("SNOW_IMAGE_WORKER").unwrap();
    let service = ImageService::new(config(worker.into())).unwrap();
    struct Source {
        descriptor: ImageDescriptor,
        rows: AtomicUsize,
    }
    impl RasterSource for Source {
        fn descriptor(&self) -> &ImageDescriptor {
            &self.descriptor
        }
        fn access(&self) -> RasterAccess {
            RasterAccess::SequentialRows
        }
        fn read_rows(&self, _: u32, rows: u32, destination: &mut [u8]) -> Result<()> {
            self.rows.fetch_add(rows as usize, Ordering::Relaxed);
            destination.fill(0);
            Ok(())
        }
    }
    let source = Arc::new(Source {
        descriptor: ImageDescriptor::packed(
            1024,
            8192,
            PixelFormat {
                sample: SampleType::F32,
                significant_bits: 32,
                ..PixelFormat::RGBA8
            },
            ColorEncoding::Srgb,
        )
        .unwrap(),
        rows: AtomicUsize::new(0),
    });
    let started = Instant::now();
    let result = service
        .encode(source.clone(), EncodeOptions::default())
        .wait();
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(source.rows.load(Ordering::Relaxed) < 8192);
    drop(result);
    quiescent(service.budget());
}

#[test]
#[ignore = "requires SNOW_IMAGE_WORKER pointing to the separately built native worker"]
fn parent_eof_during_raster_transfer_cleans_anonymous_spool() {
    let directory = tempfile::tempdir().unwrap();
    let worker = std::env::var_os("SNOW_IMAGE_WORKER").unwrap();
    let mut child = Command::new(worker)
        .env("TMPDIR", directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let descriptor =
        ImageDescriptor::packed(1024, 1024, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
    let request = Request {
        version: protocol::VERSION,
        generation: 1,
        id: 1,
        policy: ResourcePolicy::default(),
        payload_storage: protocol::PayloadStorage::Pipe,
        input_len: descriptor.required_bytes().unwrap() as u64,
        metadata: MetadataLengths::default(),
        operation: Operation::Encode {
            descriptor,
            options: EncodeOptions::default(),
        },
    };
    let mut input = child.stdin.take().unwrap();
    protocol::write_control(&mut input, &WorkerCommand::Run(request)).unwrap();
    input.write_all(&[0; 1024]).unwrap();
    drop(input);
    assert!(bounded_wait(&mut child).success());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
#[ignore = "requires SNOW_IMAGE_WORKER pointing to the separately built native worker"]
fn parent_death_terminates_worker_even_when_an_inherited_input_pipe_stays_open() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("children.json");
    let worker = std::env::var_os("SNOW_IMAGE_WORKER").unwrap();
    let descriptor =
        ImageDescriptor::packed(1024, 1024, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
    let request = serde_json::to_string(&WorkerCommand::Run(Request {
        version: protocol::VERSION,
        generation: 1,
        id: 1,
        policy: ResourcePolicy::default(),
        payload_storage: protocol::PayloadStorage::Pipe,
        input_len: descriptor.required_bytes().unwrap() as u64,
        metadata: MetadataLengths::default(),
        operation: Operation::Encode {
            descriptor,
            options: EncodeOptions::default(),
        },
    }))
    .unwrap();
    let parent_script = script(
        directory.path(),
        &format!(
            "import subprocess,time\nw=subprocess.Popen([{worker:?}],stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)\nb={request:?}.encode()\nw.stdin.write(struct.pack('<I',len(b))+b+bytes(512));w.stdin.flush()\nholder=os.fork()\nif holder==0:\n time.sleep(10);os._exit(0)\ntime.sleep(0.2)\n{publication}\ntime.sleep(30)",
            worker = worker.to_str().unwrap(),
            publication = publish_children_marker(&marker)
        ),
    );
    let mut fixture = ParentFixture::spawn(&parent_script);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !marker.exists() {
        assert!(
            fixture.parent.try_wait().unwrap().is_none(),
            "fake parent exited before publishing children"
        );
        assert!(
            Instant::now() < deadline,
            "fake parent did not start its worker"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let [worker_pid, _holder_pid]: [i32; 2] =
        serde_json::from_slice(&std::fs::read(marker).unwrap()).unwrap();
    fixture.parent.kill().unwrap();
    fixture.parent.wait().unwrap();
    let started = Instant::now();
    let terminated = loop {
        let state = Command::new("ps")
            .args(["-p", &worker_pid.to_string(), "-o", "stat="])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&state.stdout);
        if state.trim().is_empty() || state.trim().starts_with('Z') {
            break true;
        }
        if started.elapsed() > Duration::from_secs(2) {
            break false;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // The fixture owns the holder and any failed worker through its process group,
    // including assertion/unwind paths before the marker could be parsed.
    assert!(
        terminated,
        "worker survived parent death with a blocked input producer"
    );
}

#[test]
#[ignore = "requires SNOW_IMAGE_WORKER pointing to the separately built native worker"]
fn parent_death_before_worker_startup_cannot_strand_orphaned_input() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("children.json");
    let worker = std::env::var_os("SNOW_IMAGE_WORKER").unwrap();
    let stub = format!(
        "import os,signal;os.kill(os.getpid(),signal.SIGSTOP);os.execv({worker:?},[{worker:?}])",
        worker = worker.to_str().unwrap(),
    );
    let parent_script = script(
        directory.path(),
        &format!(
            "import subprocess,time\nw=subprocess.Popen([sys.executable,'-c',{stub:?}],stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)\nholder=os.fork()\nif holder==0:\n time.sleep(10);os._exit(0)\n{publication}\ntime.sleep(30)",
            publication = publish_children_marker(&marker),
        ),
    );
    let mut fixture = ParentFixture::spawn(&parent_script);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !marker.exists() {
        assert!(
            fixture.parent.try_wait().unwrap().is_none(),
            "fake parent exited before publishing children"
        );
        assert!(
            Instant::now() < deadline,
            "fake parent did not publish children"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let [worker_pid, _holder_pid]: [i32; 2] =
        serde_json::from_slice(&std::fs::read(marker).unwrap()).unwrap();
    loop {
        let state = Command::new("ps")
            .args(["-p", &worker_pid.to_string(), "-o", "stat="])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&state.stdout)
            .trim()
            .starts_with('T')
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "worker stub did not stop before exec"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    fixture.parent.kill().unwrap();
    fixture.parent.wait().unwrap();
    // The executable starts only after its original parent has been reaped. Its
    // initial getppid() is already 1, while holder still prevents stdin EOF.
    // SAFETY: worker_pid is the stopped child identity returned above.
    assert_eq!(unsafe { libc::kill(worker_pid, libc::SIGCONT) }, 0);
    let started = Instant::now();
    let terminated = loop {
        let state = Command::new("ps")
            .args(["-p", &worker_pid.to_string(), "-o", "stat="])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&state.stdout);
        if state.trim().is_empty() || state.trim().starts_with('Z') {
            break true;
        }
        if started.elapsed() > Duration::from_secs(2) {
            break false;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        terminated,
        "worker started as an orphan and remained blocked on stdin"
    );
}

fn animation_bytes(frames: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut bytes, 1, 1, &[255, 0, 0, 0, 255, 0]).unwrap();
        for index in 0..frames {
            encoder
                .write_frame(&gif::Frame {
                    width: 1,
                    height: 1,
                    delay: 1,
                    buffer: std::borrow::Cow::Owned(vec![(index % 2) as u8]),
                    ..Default::default()
                })
                .unwrap();
        }
    }
    bytes
}
fn kill_animation_worker(pid_file: &Path) {
    let pid = std::fs::read_to_string(pid_file).unwrap();
    let result = Command::new("kill")
        .args(["-KILL", pid.trim()])
        .status()
        .unwrap();
    assert!(result.success());
}
fn worker_wrapper(directory: &Path) -> (PathBuf, PathBuf) {
    let worker = std::env::var_os("SNOW_IMAGE_WORKER").unwrap();
    let worker = worker.to_str().unwrap();
    let pid_file = directory.join("animation.pid");
    let body = format!(
        "if '--persistent' in sys.argv:\n open({:?},'w').write(str(os.getpid()))\nos.execv({:?},[{:?}]+sys.argv[1:])",
        pid_file.to_str().unwrap(),
        worker,
        worker
    );
    // Pass the unopened handoff socket through exec; the real worker owns its
    // capability receive, startup descriptor closure, and stderr redirection.
    (raw_script(directory, &body), pid_file)
}

#[test]
#[ignore = "requires SNOW_IMAGE_WORKER pointing to the separately built native worker"]
fn animation_recovers_after_crash_without_invalidating_published_frames() {
    let directory = tempfile::tempdir().unwrap();
    let (worker, pid) = worker_wrapper(directory.path());
    let service = ImageService::new(config(worker)).unwrap();
    let baseline = wait_for_static_pool(&service);
    let input = service.input_bytes(&animation_bytes(4)).unwrap();
    let input_bytes = input.len();
    let session = service
        .open_animation(input, DecodeOptions::default())
        .wait()
        .unwrap();
    let snapshot = session.next_frame().wait().unwrap().unwrap();
    kill_animation_worker(&pid);
    assert_animation_worker_retired(&service, baseline, input_bytes);
    let recovered = session.next_frame().wait().unwrap().unwrap();
    assert_eq!(recovered.index, 1);
    assert_eq!(snapshot.image.bytes(), &[255, 0, 0, 255]);
    assert_eq!(recovered.image.bytes(), &[0, 255, 0, 255]);
}

#[test]
#[ignore = "requires SNOW_IMAGE_WORKER pointing to the separately built native worker"]
fn recovery_requires_explicit_reset_past_replay_limit() {
    let directory = tempfile::tempdir().unwrap();
    let (worker, pid) = worker_wrapper(directory.path());
    let service = ImageService::new(config(worker)).unwrap();
    let baseline = wait_for_static_pool(&service);
    let input = service.input_bytes(&animation_bytes(70)).unwrap();
    let input_bytes = input.len();
    let session = service
        .open_animation(input, DecodeOptions::default())
        .wait()
        .unwrap();
    for index in 0..65 {
        assert_eq!(session.next_frame().wait().unwrap().unwrap().index, index);
    }
    kill_animation_worker(&pid);
    assert_animation_worker_retired(&service, baseline, input_bytes);
    assert!(
        matches!(session.next_frame().wait(), Err(Error::Worker(message)) if message.contains("64 frames"))
    );
    session.reset().wait().unwrap();
    assert_eq!(session.next_frame().wait().unwrap().unwrap().index, 0);
}
