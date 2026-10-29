//! Discard descriptors that are not part of the worker's inherited protocol.
//!
//! On macOS, creating a pipe and setting CLOEXEC are separate system calls.
//! Serializing our own spawns cannot exclude unrelated application threads from
//! that window. Only stdin, stdout and stderr belong to the worker protocol.

use std::io;

/// Close every inherited descriptor other than the three standard streams.
///
/// # Safety
/// Call once at process startup, before opening files or starting threads. There
/// must be no Rust owners of descriptors >= 3 and no concurrent descriptor users.
pub(crate) unsafe fn close_unexpected_descriptors() -> io::Result<()> {
    #[cfg(unix)]
    {
        // Enumerate the actual table instead of using RLIMIT_NOFILE: an inherited
        // descriptor can exceed a subsequently lowered soft limit. Collect and
        // drop ReadDir before closing, so its own descriptor is no longer owned.
        let descriptors = std::fs::read_dir("/dev/fd")?
            .map(|entry| {
                let entry = entry?;
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<libc::c_int>().ok())
                    .ok_or_else(|| io::Error::other("invalid descriptor directory entry"))
            })
            .collect::<io::Result<Vec<_>>>()?;
        for descriptor in descriptors
            .into_iter()
            .filter(|descriptor| *descriptor >= 3)
        {
            loop {
                // SAFETY: the caller guarantees exclusive startup access and no
                // live Rust owners. The enumeration's descriptor is already closed.
                if unsafe { libc::close(descriptor) } == 0 {
                    break;
                }
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EBADF) {
                    break;
                }
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
                // No thread can reuse a descriptor here, so retry is safe even
                // on systems where an interrupted close already closed it.
            }
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs::File,
        io::{Read, Seek, SeekFrom, Write},
        os::{fd::AsRawFd, unix::process::CommandExt},
        process::{Command, Stdio},
    };

    const CHILD: &str = "SNOW_IMAGE_DESCRIPTOR_TEST_CHILD";
    const LEAKED_FD: libc::c_int = 192;

    #[test]
    fn descriptor_child() {
        if std::env::var_os(CHILD).is_none() {
            return;
        }
        // SAFETY: F_GETFD queries a descriptor without taking ownership.
        assert!(unsafe { libc::fcntl(LEAKED_FD, libc::F_GETFD) } >= 0);
        // SAFETY: this isolated subprocess has no owned nonstandard descriptors;
        // the intentionally inherited raw descriptor has no Rust owner.
        unsafe { close_unexpected_descriptors() }.unwrap();
        // SAFETY: same read-only query as above.
        assert_eq!(unsafe { libc::fcntl(LEAKED_FD, libc::F_GETFD) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
        std::io::stderr()
            .write_all(b"standard streams retained")
            .unwrap();
    }

    #[test]
    fn inherited_descriptors_above_soft_limit_are_closed() {
        let source = File::open("/dev/null").unwrap();
        let source_fd = source.as_raw_fd();
        let mut result = tempfile::tempfile().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "startup::tests::descriptor_child", "--nocapture"])
            .env(CHILD, "1")
            .stdout(Stdio::null())
            .stderr(Stdio::from(result.try_clone().unwrap()));
        // SAFETY: this closure performs only async-signal-safe descriptor and
        // resource-limit syscalls; the captured source remains open until spawn.
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(source_fd, LEAKED_FD) < 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut limit = std::mem::zeroed::<libc::rlimit>();
                if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
                    return Err(io::Error::last_os_error());
                }
                limit.rlim_cur = 64;
                if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let status = command.status().unwrap();
        result.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        result.read_to_end(&mut bytes).unwrap();
        assert!(
            status.success(),
            "descriptor child failed: {}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(bytes, b"standard streams retained");
    }
}
