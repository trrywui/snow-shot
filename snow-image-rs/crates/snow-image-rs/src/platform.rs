//! OS containment is deliberately separate from library buffer accounting.

use snow_image_core::{ResourcePolicy, Result};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Detect deferred allocation/writeback failure before dropping the snapshot's
/// sole writer. Private input needs stable filesystem content, not a hardware
/// cache flush across power failure (Darwin F_FULLFSYNC).
#[cfg(target_os = "macos")]
pub(crate) fn flush_snapshot(file: &std::fs::File) -> Result<()> {
    use std::os::fd::AsRawFd;
    loop {
        // SAFETY: fsync borrows a live owned file descriptor for this call only.
        if unsafe { libc::fsync(file.as_raw_fd()) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error.into());
        }
    }
}

/// A wakeup hint only: callers still use Child::try_wait/wait for exit status and
/// wait_tree_terminated for publication. This object never reaps a process.
#[cfg(target_os = "macos")]
pub(crate) struct ExitNotification {
    queue: Option<std::os::fd::OwnedFd>,
    process_id: usize,
    notified: std::sync::atomic::AtomicBool,
}

#[cfg(target_os = "macos")]
impl ExitNotification {
    /// Call under the process-wide spawn lock: kqueue and CLOEXEC are separate
    /// syscalls, just like Darwin's pipe/socket creation.
    pub(crate) fn new(child: &Child) -> Result<Self> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::sync::atomic::AtomicBool;

        // SAFETY: kqueue has no arguments and returns a new owned descriptor.
        let descriptor = unsafe { libc::kqueue() };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: the successful call returned a new descriptor owned only here.
        let queue = unsafe { OwnedFd::from_raw_fd(descriptor) };
        // SAFETY: both fcntl operations access flags on our live private descriptor.
        let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
        if flags < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: queue remains owned and F_SETFD changes only descriptor flags.
        if unsafe { libc::fcntl(descriptor, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let process_id = child.id() as usize;
        let change = libc::kevent {
            ident: process_id,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: one fully initialized change is borrowed by the synchronous
        // call; no event output is requested and timeout forbids waiting.
        let registered = unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                &change,
                1,
                std::ptr::null_mut(),
                0,
                &timeout,
            )
        };
        if registered < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(Self {
                    queue: None,
                    process_id,
                    notified: AtomicBool::new(true),
                });
            }
            return Err(error.into());
        }
        Ok(Self {
            queue: Some(queue),
            process_id,
            notified: AtomicBool::new(false),
        })
    }

    /// Wait for an exit hint or the supplied maximum interval. EINTR never
    /// restarts the original timeout; subsequent waits after a hint are immediate.
    pub(crate) fn wait(&self, timeout: Duration) -> Result<()> {
        use std::os::fd::AsRawFd;
        use std::sync::atomic::Ordering;

        if self.notified.load(Ordering::Acquire) {
            return Ok(());
        }
        let Some(queue) = &self.queue else {
            return Ok(());
        };
        let started = Instant::now();
        loop {
            let remaining = timeout.saturating_sub(started.elapsed());
            let interval = libc::timespec {
                tv_sec: remaining.as_secs().min(libc::time_t::MAX as u64) as libc::time_t,
                tv_nsec: remaining.subsec_nanos().into(),
            };
            // SAFETY: all-zero bits initialize this plain event output structure.
            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            // SAFETY: the queue is live, there is no change list, and the single
            // output slot and finite timeout are borrowed for this call only.
            let ready = unsafe {
                libc::kevent(
                    queue.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    &mut event,
                    1,
                    &interval,
                )
            };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    if started.elapsed() >= timeout {
                        return Ok(());
                    }
                    continue;
                }
                return Err(error.into());
            }
            if ready == 0 {
                return Ok(());
            }
            if event.flags & libc::EV_ERROR != 0 {
                let code = i32::try_from(event.data).unwrap_or(libc::EIO);
                return Err(std::io::Error::from_raw_os_error(code).into());
            }
            if event.ident != self.process_id
                || event.filter != libc::EVFILT_PROC
                || event.fflags & libc::NOTE_EXIT == 0
            {
                return Err(std::io::Error::other("unexpected worker exit notification").into());
            }
            self.notified.store(true, Ordering::Release);
            return Ok(());
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) struct ExitNotification;

#[cfg(not(target_os = "macos"))]
impl ExitNotification {
    pub(crate) fn new(_child: &Child) -> Result<Self> {
        Ok(Self)
    }
    pub(crate) fn wait(&self, timeout: Duration) -> Result<()> {
        std::thread::sleep(timeout.min(Duration::from_millis(10)));
        Ok(())
    }
}

#[cfg(not(windows))]
pub(crate) struct Containment;

#[cfg(not(windows))]
pub(crate) fn configure(command: &mut Command, policy: &ResourcePolicy) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .arg("--address-space-limit")
        .arg(policy.worker_address_space_bytes.to_string());
}

#[cfg(unix)]
pub(crate) fn terminate_tree(child: &mut Child, _containment: &Containment) {
    // SAFETY: this child was assigned its own process group before exec. A negative
    // PID targets that group, preventing inherited pipes from surviving a crash.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
}

/// Observe that the worker process group has no live members after termination.
///
/// The supported worker and codec stack never creates a new session or moves
/// descendants out of its assigned process group. This is lifecycle containment,
/// not an OS sandbox for deliberately hostile worker executables.
#[cfg(unix)]
pub(crate) fn wait_tree_terminated(
    child: &Child,
    _containment: &Containment,
    timeout: Duration,
) -> Result<()> {
    let started = Instant::now();
    loop {
        // SAFETY: signal 0 only probes the process group assigned before exec;
        // it does not send a signal or acquire ownership of another process.
        let present = unsafe { libc::kill(-(child.id() as libc::pid_t), 0) };
        if present < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            #[cfg(target_os = "macos")]
            if error.raw_os_error() == Some(libc::EPERM) {
                // Darwin can return EPERM for a group containing only dying or
                // zombie processes. Require separate process-state evidence.
                if !macos_group_has_live_processes(child.id())? {
                    return Ok(());
                }
            } else if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error.into());
            }
            #[cfg(not(target_os = "macos"))]
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error.into());
            }
        }
        if started.elapsed() >= timeout {
            return Err(snow_image_core::Error::Worker(
                "worker process-group termination was not observed before the deadline".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(2).min(timeout.saturating_sub(started.elapsed())));
    }
}

#[cfg(target_os = "macos")]
fn macos_group_has_live_processes(group: u32) -> std::io::Result<bool> {
    // PROC_PGRP_ONLY is defined as 2 by the public macOS sys/proc_info.h API.
    const PROC_PGRP_ONLY: u32 = 2;
    // A full buffer is inconclusive, never success. This keeps cleanup's own
    // memory fixed even if a faulty replacement worker forks many processes.
    let mut pids = [0i32; 256];
    let capacity = std::mem::size_of_val(&pids);
    // SAFETY: libproc writes at most capacity bytes to this initialized PID array.
    let size = unsafe {
        libc::proc_listpids(
            PROC_PGRP_ONLY,
            group,
            pids.as_mut_ptr().cast(),
            capacity as i32,
        )
    };
    if size < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let size = size as usize;
    if size >= capacity {
        return Ok(true);
    }
    if !size.is_multiple_of(std::mem::size_of::<i32>()) {
        return Err(std::io::Error::other(
            "invalid process-group enumeration size",
        ));
    }
    for pid in pids[..size / std::mem::size_of::<i32>()]
        .iter()
        .copied()
        .filter(|pid| *pid > 0)
    {
        let mut state = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        // SAFETY: PROC_PIDTBSDINFO selects exactly proc_bsdinfo, whose full size
        // is supplied. No state is read until that entire structure is returned.
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                state.as_mut_ptr().cast(),
                std::mem::size_of::<libc::proc_bsdinfo>() as i32,
            )
        };
        if written == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                continue; // exited between enumeration and the state query
            }
            return Err(error);
        }
        if written as usize != std::mem::size_of::<libc::proc_bsdinfo>() {
            return Err(std::io::Error::other(
                "incomplete process-group member state",
            ));
        }
        // SAFETY: the successful call initialized the complete proc_bsdinfo above.
        let state = unsafe { state.assume_init() };
        if state.pbi_pgid == group && state.pbi_status != libc::SZOMB {
            return Ok(true);
        }
        // A zombie has completed process teardown and owns no writable address
        // space or descriptors. It can safely await collection by its new parent.
    }
    Ok(false)
}

#[cfg(not(windows))]
pub(crate) fn attach(_child: &mut Child, _policy: &ResourcePolicy) -> Result<Containment> {
    Ok(Containment)
}

#[cfg(target_os = "macos")]
pub(crate) fn footprint(child: &Child) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: proc_pid_rusage writes the v2 structure selected by RUSAGE_INFO_V2.
    let result = unsafe {
        libc::proc_pid_rusage(
            child.id() as i32,
            libc::RUSAGE_INFO_V2,
            info.as_mut_ptr().cast(),
        )
    };
    if result != 0 {
        return None;
    }
    // SAFETY: the successful call initialized the complete v2 structure.
    Some(unsafe { info.assume_init() }.ri_phys_footprint)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn footprint(_child: &Child) -> Option<u64> {
    None
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::os::windows::{io::AsRawHandle, process::CommandExt};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
                JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
                JobObjectExtendedLimitInformation, QueryInformationJobObject,
                SetInformationJobObject, TerminateJobObject,
            },
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
            Threading::{
                CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
            },
        },
    };

    pub(crate) struct Containment(HANDLE);
    // SAFETY: the owned kernel handle is not borrowed and CloseHandle is thread independent.
    unsafe impl Send for Containment {}
    impl Drop for Containment {
        fn drop(&mut self) {
            // SAFETY: this is our live, uniquely owned Job Object handle.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    pub(crate) fn configure(command: &mut Command, _policy: &ResourcePolicy) {
        command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
    }
    pub(crate) fn attach(child: &mut Child, policy: &ResourcePolicy) -> Result<Containment> {
        // SAFETY: null attributes select a private, non-inherited unnamed job.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let owner = Containment(job);
        // SAFETY: zero is a valid initial value for this POD Windows structure.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_JOB_MEMORY;
        // The selected codecs use threads, never child processes. Forbid child
        // creation so no descendant can retain a writable publication handle.
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
        limits.ProcessMemoryLimit = usize::try_from(policy.worker_memory_bytes).map_err(|_| {
            snow_image_core::Error::InvalidInput("worker commit limit exceeds address space".into())
        })?;
        limits.JobMemoryLimit = limits.ProcessMemoryLimit;
        // SAFETY: structures and the child process handle remain valid for both synchronous calls.
        if unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
            || unsafe { AssignProcessToJobObject(job, child.as_raw_handle().cast()) } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        // The child was created suspended and has exactly its initial thread. Enumerate that
        // thread only after the memory/job lifetime rules have been installed.
        // SAFETY: the snapshot call has no borrowed arguments.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: THREADENTRY32 is a POD structure and dwSize is set before use.
        let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        // SAFETY: snapshot and entry are valid for enumeration.
        let mut found = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        let mut resumed = false;
        while found {
            if entry.th32OwnerProcessID == child.id() {
                // SAFETY: requesting a non-inherited handle to a thread identified by the snapshot.
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if !thread.is_null() {
                    // SAFETY: this handle has THREAD_SUSPEND_RESUME rights.
                    resumed = unsafe { ResumeThread(thread) } != u32::MAX;
                    // SAFETY: the opened handle is uniquely owned here.
                    unsafe {
                        CloseHandle(thread);
                    }
                }
                break;
            }
            // SAFETY: entry/snapshot remain valid until the loop completes.
            found = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
        }
        // SAFETY: snapshot is a uniquely owned live handle.
        unsafe {
            CloseHandle(snapshot);
        }
        if !resumed {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(owner)
    }
    pub(crate) fn footprint(child: &Child) -> Option<u64> {
        // SAFETY: counters is POD and the API writes within cb bytes.
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        counters.cb = std::mem::size_of_val(&counters) as u32;
        // SAFETY: child owns the process handle; the output structure is correctly sized.
        if unsafe { GetProcessMemoryInfo(child.as_raw_handle().cast(), &mut counters, counters.cb) }
            == 0
        {
            None
        } else {
            Some(counters.WorkingSetSize as u64)
        }
    }
    pub(crate) fn terminate_tree(child: &mut Child, containment: &Containment) {
        // SAFETY: the parent owns this live job and all its contained descendants.
        unsafe {
            TerminateJobObject(containment.0, 1);
        }
        let _ = child.kill();
    }

    /// A terminated Job Object can still contain processes while termination
    /// completes. Only an observed zero active-process count permits publication.
    pub(crate) fn wait_tree_terminated(
        _child: &Child,
        containment: &Containment,
        timeout: Duration,
    ) -> Result<()> {
        let started = Instant::now();
        loop {
            // SAFETY: zero is a valid initial value for this Windows POD structure.
            let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
                unsafe { std::mem::zeroed() };
            // SAFETY: the private Job Object remains owned; the output pointer and
            // byte count describe a complete live accounting structure.
            if unsafe {
                QueryInformationJobObject(
                    containment.0,
                    JobObjectBasicAccountingInformation,
                    (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    std::mem::size_of_val(&accounting) as u32,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            if accounting.ActiveProcesses == 0 {
                return Ok(());
            }
            if started.elapsed() >= timeout {
                return Err(snow_image_core::Error::Worker(
                    "worker Job Object termination was not observed before the deadline".into(),
                ));
            }
            std::thread::sleep(
                Duration::from_millis(2).min(timeout.saturating_sub(started.elapsed())),
            );
        }
    }
}

#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(all(test, target_os = "macos"))]
mod exit_notification_tests {
    use super::*;
    use std::{
        io::Write,
        os::fd::AsRawFd,
        process::Stdio,
        sync::{Arc, Mutex, mpsc},
    };

    static SERIAL: Mutex<()> = Mutex::new(());

    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn controlled_child() -> ChildGuard {
        ChildGuard(
            Command::new("/bin/sh")
                .args(["-c", "read -r line"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    #[test]
    fn times_out_while_child_is_alive_and_sets_cloexec() {
        let _serial = SERIAL.lock().unwrap();
        let mut child = controlled_child();
        let notification = ExitNotification::new(&child.0).unwrap();
        let descriptor = notification.queue.as_ref().unwrap().as_raw_fd();
        // SAFETY: this read-only query accesses the live queue's descriptor flags.
        let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
        let started = Instant::now();
        notification.wait(Duration::from_millis(20)).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(15));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(child.0.try_wait().unwrap().is_none());
    }

    #[test]
    fn exit_wakes_waiter_without_reaping_or_waiting_for_timeout() {
        let _serial = SERIAL.lock().unwrap();
        let mut child = controlled_child();
        let notification = Arc::new(ExitNotification::new(&child.0).unwrap());
        let waiting = notification.clone();
        let (ready_send, ready_receive) = mpsc::channel();
        let (done_send, done_receive) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            ready_send.send(()).unwrap();
            done_send
                .send(waiting.wait(Duration::from_secs(2)))
                .unwrap();
        });
        ready_receive.recv_timeout(Duration::from_secs(1)).unwrap();
        child.0.stdin.take().unwrap().write_all(b"exit\n").unwrap();
        done_receive
            .recv_timeout(Duration::from_millis(500))
            .unwrap()
            .unwrap();
        waiter.join().unwrap();
        // The notification did not consume exit status; the ordinary Child owner
        // can still reap it and obtain the expected shell result.
        assert!(child.0.wait().unwrap().success());
        let started = Instant::now();
        notification.wait(Duration::from_secs(2)).unwrap();
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn already_reaped_registration_wakes_and_repeated_queues_close() {
        let _serial = SERIAL.lock().unwrap();
        let baseline = std::fs::read_dir("/dev/fd").unwrap().count();
        for _ in 0..16 {
            let mut child = controlled_child();
            child.0.stdin.take().unwrap().write_all(b"exit\n").unwrap();
            assert!(child.0.wait().unwrap().success());
            let notification = ExitNotification::new(&child.0).unwrap();
            let started = Instant::now();
            notification.wait(Duration::from_secs(2)).unwrap();
            assert!(started.elapsed() < Duration::from_millis(100));
            drop(notification);
            drop(child);
            assert_eq!(std::fs::read_dir("/dev/fd").unwrap().count(), baseline);
        }
    }
}
