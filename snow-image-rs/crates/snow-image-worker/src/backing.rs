//! Transfer the inherited publication file out of stderr before starting codecs.
//!
//! This runs once, before worker threads or codec calls. The returned file owns an
//! independent handle; OS stderr and the shared MSVC CRT descriptor are redirected
//! to the null device so native diagnostics cannot alias the publication writer.
use std::{fs::File, io};

pub(crate) fn take_output_backing() -> io::Result<File> {
    platform::take()
}

#[cfg(target_os = "macos")]
pub(crate) fn take_output_handoff() -> io::Result<(File, File, File)> {
    use std::os::fd::{AsRawFd, FromRawFd};
    // SAFETY: duplicate inherited fd 2 without borrowing its ownership. This is
    // called once at startup before threads, codecs, or other descriptors exist.
    let descriptor = unsafe { libc::fcntl(libc::STDERR_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl created an exclusively owned descriptor.
    let socket = unsafe { File::from_raw_fd(descriptor) };
    let handles = snow_image_transport::receive_backing(&socket)?;
    validate_regular(&handles.0)?;
    let null = std::fs::OpenOptions::new().write(true).open("/dev/null")?;
    // SAFETY: both descriptors are live. Replacing fd 2 closes its socket alias;
    // the received publication and pixel handles retain independent ownership.
    if unsafe { libc::dup2(null.as_raw_fd(), libc::STDERR_FILENO) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // Keep the socket for a single later read-only input capability. The parent
    // queues it before Run; input reception has its own finite deadline.
    Ok((handles.0, handles.1, socket))
}

/// Observe delayed allocation/writeback failures before publishing a result.
///
/// Temporary transport does not need hardware-cache durability. On Apple,
/// `File::sync_data` uses F_FULLFSYNC, so use the narrower fsync contract here.
/// An mmap flush alone can succeed on APFS even when fsync reports ENOSPC.
pub(crate) fn finish_writeback(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        loop {
            // SAFETY: fsync operates on this borrowed live file descriptor and
            // neither acquires ownership nor accesses Rust memory.
            if unsafe { libc::fsync(file.as_raw_fd()) } == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
    #[cfg(not(unix))]
    file.sync_data()
}

fn validate_regular(file: &File) -> io::Result<()> {
    if !file.metadata()?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "worker output backing must be a regular file",
        ));
    }
    Ok(())
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::{
        fs::OpenOptions,
        os::fd::{AsRawFd, FromRawFd},
    };

    pub(super) fn take() -> io::Result<File> {
        // SAFETY: duplicate the inherited descriptor without borrowing its ownership.
        // CLOEXEC prevents a subsequently spawned process from inheriting the writer.
        let descriptor = unsafe { libc::fcntl(libc::STDERR_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fcntl returned a new descriptor exclusively owned by this function.
        let backing = unsafe { File::from_raw_fd(descriptor) };
        validate_regular(&backing)?;
        // SAFETY: F_GETFL only reads the open descriptor's status flags.
        let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if flags & libc::O_ACCMODE == libc::O_RDONLY {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "worker output backing is read-only",
            ));
        }
        let null = OpenOptions::new().write(true).open("/dev/null")?;
        // SAFETY: both descriptors are live. dup2 atomically replaces only fd 2;
        // the duplicate publication descriptor remains owned by `backing`.
        if unsafe { libc::dup2(null.as_raw_fd(), libc::STDERR_FILENO) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(backing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Seek, SeekFrom, Write},
        process::{Command, Stdio},
    };

    const CHILD_MODE: &str = "SNOW_IMAGE_BACKING_TEST_CHILD";

    #[test]
    fn redirect_child() {
        let Ok(mode) = std::env::var(CHILD_MODE) else {
            return;
        };
        if mode == "reject-pipe" {
            assert!(take_output_backing().is_err());
            return;
        }
        let mut file = take_output_backing().unwrap();
        file.write_all(b"sealed pixels").unwrap();
        file.flush().unwrap();
        std::io::stderr()
            .write_all(b"Rust stderr after publication")
            .unwrap();
        let message = b"native stderr after publication";
        #[cfg(unix)]
        {
            // SAFETY: fd 2 is redirected and the pointer covers exactly message.len() bytes.
            let written = unsafe { libc::write(2, message.as_ptr().cast(), message.len()) };
            assert_eq!(written, message.len() as isize);
        }
        #[cfg(windows)]
        {
            unsafe extern "C" {
                fn _write(descriptor: i32, data: *const std::ffi::c_void, size: u32) -> i32;
            }
            // SAFETY: the CRT fd 2 is redirected and message remains live for the call.
            let written = unsafe { _write(2, message.as_ptr().cast(), message.len() as u32) };
            assert_eq!(written, message.len() as i32);
        }
    }

    #[test]
    fn native_and_rust_diagnostics_do_not_alias_publication_file() {
        let mut file = tempfile::tempfile().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "backing::tests::redirect_child", "--nocapture"])
            .env(CHILD_MODE, "redirect")
            .stdout(Stdio::null())
            .stderr(Stdio::from(file.try_clone().unwrap()))
            .status()
            .unwrap();
        assert!(status.success(), "redirection child failed");
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"sealed pixels");
    }

    #[test]
    fn output_backing_rejects_pipe_before_mutation() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "backing::tests::redirect_child", "--nocapture"])
            .env(CHILD_MODE, "reject-pipe")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status()
            .unwrap();
        assert!(status.success());
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        fs::OpenOptions,
        os::windows::io::{FromRawHandle, IntoRawHandle},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, INVALID_HANDLE_VALUE},
        System::{
            Console::{GetStdHandle, STD_ERROR_HANDLE, SetStdHandle},
            Threading::GetCurrentProcess,
        },
    };

    // The native prefix and Rust both use the dynamic MSVC CRT (/MD). These
    // declarations therefore operate on the same descriptor table as codecs.
    unsafe extern "C" {
        fn _open_osfhandle(handle: isize, flags: i32) -> i32;
        fn _get_osfhandle(descriptor: i32) -> isize;
        fn _dup2(source: i32, destination: i32) -> i32;
        fn _close(descriptor: i32) -> i32;
    }
    const O_WRONLY: i32 = 0x0001;
    const O_BINARY: i32 = 0x8000;
    const O_NOINHERIT: i32 = 0x0080;

    struct CrtDescriptor(i32);
    impl Drop for CrtDescriptor {
        fn drop(&mut self) {
            // SAFETY: this descriptor was returned by _open_osfhandle and has a
            // unique owner. The permanent standard descriptor 2 is never wrapped.
            unsafe {
                _close(self.0);
            }
        }
    }

    pub(super) fn take() -> io::Result<File> {
        // SAFETY: these calls only read process-owned standard-handle state.
        let original = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        if original.is_null() || original == INVALID_HANDLE_VALUE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "worker output backing handle is missing",
            ));
        }
        // SAFETY: this returns the current process pseudo-handle without taking ownership.
        let process = unsafe { GetCurrentProcess() };
        let mut duplicate = std::ptr::null_mut();
        // SAFETY: current process pseudo-handles and the inherited stderr handle
        // are valid; the initialized output receives a distinct noninheritable handle.
        if unsafe {
            DuplicateHandle(
                process,
                original,
                process,
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: DuplicateHandle transferred unique ownership of this handle.
        let backing = unsafe { File::from_raw_handle(duplicate) };
        validate_regular(&backing)?;
        let null = OpenOptions::new().write(true).open("NUL")?;
        let raw_null = null.into_raw_handle();
        // SAFETY: raw_null is uniquely owned. On success the CRT takes ownership;
        // on failure ownership is restored to File below.
        let descriptor =
            unsafe { _open_osfhandle(raw_null as isize, O_WRONLY | O_BINARY | O_NOINHERIT) };
        if descriptor < 0 {
            // SAFETY: _open_osfhandle failed and did not adopt the handle.
            drop(unsafe { File::from_raw_handle(raw_null) });
            return Err(io::Error::other(
                "could not create null stderr CRT descriptor",
            ));
        }
        // SAFETY: fd 2 is the CRT's standard-error slot established by process startup.
        let previous_crt_handle = unsafe { _get_osfhandle(2) };
        if descriptor != 2 {
            let source = CrtDescriptor(descriptor);
            // SAFETY: the source is owned and valid. _dup2 duplicates it into fd 2,
            // closing fd 2's previous handle; the duplicate outlives source's Drop.
            if unsafe { _dup2(source.0, 2) } != 0 {
                return Err(io::Error::other("could not redirect CRT stderr"));
            }
        }
        // SAFETY: fd 2 now owns the null-device handle. SetStdHandle borrows that
        // live handle, with ownership retained by the CRT until process exit.
        let null_stderr = unsafe { _get_osfhandle(2) };
        if null_stderr == -1 {
            return Err(io::Error::other("null CRT stderr handle is missing"));
        }
        // SAFETY: fd 2 owns this live null handle until CRT process teardown.
        if unsafe { SetStdHandle(STD_ERROR_HANDLE, null_stderr as _) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if original as isize != previous_crt_handle {
            // SAFETY: a separately inherited OS stderr handle was not owned by
            // CRT fd 2. After both redirections it has no standard-stream owner;
            // the returned backing file already owns its independent duplicate.
            unsafe {
                CloseHandle(original);
            }
        }
        Ok(backing)
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::*;
    pub(super) fn take() -> io::Result<File> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "output handles are unsupported on this target",
        ))
    }
}
