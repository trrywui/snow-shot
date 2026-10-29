use std::{
    fs::File,
    io,
    mem::{size_of, zeroed},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{fs::MetadataExt, net::UnixDatagram},
    },
    time::{Duration, Instant},
};

const MESSAGE: [u8; 8] = *b"SIMG\x01\0\x02\0";
const INPUT_MESSAGE: [u8; 8] = *b"SINI\x01\0\x01\0";
// Darwin externalizes every SCM_RIGHTS descriptor before truncating control
// bytes, and does not close entries whose numbers are omitted by truncation.
// Receive the complete bounded kernel maximum instead: XNU uipc_usrreq.c sets
// UIPC_MAX_CMSG_FD=512 and sockargs caps each control mbuf at MCLBYTES. The
// Each protocol message still requires its exact handle count. Small receive
// buffers would leak descriptors delivered by a malformed message.
const MAX_RECEIVED_HANDLES: usize = 512;
// SAFETY: this constant size cannot overflow the ancillary alignment arithmetic.
const CONTROL_BYTES: usize =
    unsafe { libc::CMSG_SPACE((MAX_RECEIVED_HANDLES * size_of::<RawFd>()) as u32) as usize };
const STARTUP_TIMEOUT: Duration = Duration::from_secs(1);

#[repr(C, align(8))]
struct Control([u8; CONTROL_BYTES]);

#[repr(C)]
struct ProcessFileInfo {
    open_flags: u32,
    status: u32,
    offset: i64,
    kind: i32,
    guard_flags: u32,
}

// Layout from macOS SDK sys/proc_info.h: proc_fileinfo + pshm_info.
#[repr(C)]
struct SharedMemoryInfo {
    file: ProcessFileInfo,
    stat: libc::vinfo_stat,
    mapped_address: u64,
    name: [libc::c_char; libc::MAXPATHLEN as usize],
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn cloexec(descriptor: RawFd) -> io::Result<()> {
    // SAFETY: both operations only access flags of a live borrowed descriptor.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: this changes only flags on the same live borrowed descriptor.
    if unsafe { libc::fcntl(descriptor, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn writable(file: &File) -> io::Result<()> {
    // SAFETY: F_GETFL reads flags of this owned live descriptor.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & libc::O_ACCMODE != libc::O_RDWR {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "backing requires read/write access",
        ));
    }
    Ok(())
}

fn readonly_input_len(file: &File) -> io::Result<u64> {
    // SAFETY: F_GETFL reads flags of this owned live descriptor.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & libc::O_ACCMODE != libc::O_RDONLY || flags & libc::O_EVTONLY != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "input handoff requires read-only access",
        ));
    }
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() || metadata.len() == 0 {
        return Err(invalid("input handoff requires a nonempty regular file"));
    }
    if metadata.nlink() != 0 {
        return Err(invalid("input handoff requires an unlinked private file"));
    }
    Ok(metadata.len())
}

/// Round a pixel allocation to the actual host VM page size, checking overflow.
/// Reserve this amount before sizing the shared object and retain it with pixels.
pub fn allocation_len(bytes: usize) -> io::Result<usize> {
    // SAFETY: sysconf has no pointer arguments and does not acquire ownership.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page = usize::try_from(page).map_err(|_| invalid("invalid VM page size"))?;
    if !page.is_power_of_two() {
        return Err(invalid("invalid VM page size"));
    }
    let rounded = bytes
        .checked_add(page - 1)
        .map(|length| length & !(page - 1))
        .filter(|length| *length <= isize::MAX as usize)
        .ok_or_else(|| invalid("pixel allocation exceeds address space"))?;
    Ok(rounded)
}

/// Create an unallocated, immediately unlinked POSIX shared-memory object.
///
/// Do not call `set_len(0)`: Darwin permits only one successful sizing operation,
/// including zero-sized calls. The eventual writer sizes it once after admission.
pub fn create_pixel_backing() -> io::Result<File> {
    for _ in 0..16 {
        let mut random = [0_u8; 12];
        // SAFETY: random is a writable buffer smaller than getentropy's 256-byte cap.
        if unsafe { libc::getentropy(random.as_mut_ptr().cast(), random.len()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut name = [0_u8; 31];
        name[..6].copy_from_slice(b"/simg-");
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for (index, value) in random.iter().enumerate() {
            name[6 + 2 * index] = HEX[(value >> 4) as usize];
            name[7 + 2 * index] = HEX[(value & 15) as usize];
        }
        // SAFETY: name has one trailing NUL, valid flags, and an explicit mode.
        let descriptor = unsafe {
            libc::shm_open(
                name.as_ptr().cast(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600,
            )
        };
        if descriptor < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::AlreadyExists {
                continue;
            }
            return Err(error);
        }
        // SAFETY: shm_open returned a new uniquely owned descriptor.
        let file = unsafe { File::from_raw_fd(descriptor) };
        loop {
            // SAFETY: the same still-valid NUL-terminated name identifies our object.
            if unsafe { libc::shm_unlink(name.as_ptr().cast()) } == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        cloexec(descriptor)?;
        return Ok(file);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique shared-memory object",
    ))
}

/// Verify the kernel object type and its page-rounded size.
///
/// `bytes == 0` requires an unallocated object. This does not establish writer
/// quiescence: the host must prove worker/tree termination before exposing pixels.
pub fn validate_pixel_backing(file: &File, bytes: usize) -> io::Result<()> {
    let expected = allocation_len(bytes)?;
    // SAFETY: all-zero bits initialize this POD SDK structure, including reserved fields.
    let mut info: SharedMemoryInfo = unsafe { zeroed() };
    // SAFETY: flavor 5 is PROC_PIDFDPSHMINFO. Kernel checks the descriptor is POSIX
    // shared memory before filling this correctly sized, exclusively borrowed buffer.
    let received = unsafe {
        libc::proc_pidfdinfo(
            libc::getpid(),
            file.as_raw_fd(),
            5,
            (&mut info as *mut SharedMemoryInfo).cast(),
            size_of::<SharedMemoryInfo>() as i32,
        )
    };
    if received != size_of::<SharedMemoryInfo>() as i32 {
        return Err(invalid("pixel backing is not a POSIX shared-memory object"));
    }
    if file.metadata()?.len() != expected as u64 {
        return Err(invalid("pixel backing allocation differs from descriptor"));
    }
    Ok(())
}

fn validate_pair(output: &File, pixels: &File) -> io::Result<()> {
    if !output.metadata()?.file_type().is_file() {
        return Err(invalid("artifact backing must be a regular file"));
    }
    writable(output)?;
    writable(pixels)?;
    validate_pixel_backing(pixels, 0)
}

/// Transfer the two backing objects in one fixed, versioned datagram.
/// Queue before spawning the worker so parent death cannot strand its startup.
pub fn send_backing(socket: &UnixDatagram, output: &File, pixels: &File) -> io::Result<()> {
    validate_pair(output, pixels)?;
    send_descriptors(
        socket.as_raw_fd(),
        &MESSAGE,
        &[output.as_raw_fd(), pixels.as_raw_fd()],
    )
}

/// Send one read-only, unlinked nonempty regular file after the startup handoff.
///
/// The caller must freeze a private input snapshot and close all writable aliases
/// before sending it. Access-mode validation cannot establish that invariant.
/// Sending duplicates the handle; resource ownership remains with the caller.
pub fn send_input(socket: &UnixDatagram, file: &File) -> io::Result<()> {
    readonly_input_len(file)?;
    send_descriptors(socket.as_raw_fd(), &INPUT_MESSAGE, &[file.as_raw_fd()])
}

/// Receive one immutable encoded-input handle with an exact declared byte length.
///
/// The worker must reserve input storage before mapping and retain that reservation
/// with the mapping. This checks type, access mode, link count, and size, but relies on the host
/// having closed every writable alias to its private snapshot before handoff.
/// All handles from rejected messages are closed, and the receive wait is bounded.
pub fn receive_input(socket: &File, expected_len: u64) -> io::Result<File> {
    receive_input_with_timeout(socket, expected_len, STARTUP_TIMEOUT)
}

fn receive_input_with_timeout(
    socket: &File,
    expected_len: u64,
    timeout: Duration,
) -> io::Result<File> {
    let [input] = receive_descriptors(socket, &INPUT_MESSAGE, timeout)?;
    if readonly_input_len(&input)? != expected_len {
        return Err(invalid("input handoff size differs from declared length"));
    }
    Ok(input)
}

fn send_descriptors(socket: RawFd, payload: &[u8], descriptors: &[RawFd]) -> io::Result<()> {
    if descriptors.len() > MAX_RECEIVED_HANDLES {
        return Err(invalid("too many backing handles"));
    }
    let mut control = Control([0; CONTROL_BYTES]);
    let mut vector = libc::iovec {
        iov_base: payload.as_ptr().cast_mut().cast(),
        iov_len: payload.len(),
    };
    // SAFETY: zeroed msghdr is a valid empty message, populated below with live buffers.
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    if !descriptors.is_empty() {
        message.msg_control = control.0.as_mut_ptr().cast();
        // SAFETY: descriptor count is bounded by MAX_RECEIVED_HANDLES above.
        message.msg_controllen = unsafe { libc::CMSG_SPACE(size_of_val(descriptors) as u32) };
        // SAFETY: aligned control has enough bytes for the header plus all borrowed
        // descriptor integers. sendmsg copies them without transferring our ownership.
        unsafe {
            let header = message.msg_control.cast::<libc::cmsghdr>();
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(size_of_val(descriptors) as u32);
            std::ptr::copy_nonoverlapping(
                descriptors.as_ptr().cast::<u8>(),
                libc::CMSG_DATA(header),
                size_of_val(descriptors),
            );
        }
    }
    loop {
        // SAFETY: every message pointer refers to a live buffer for this synchronous call.
        let sent = unsafe { libc::sendmsg(socket, &message, libc::MSG_NOSIGNAL) };
        if sent >= 0 {
            return if sent as usize == payload.len() {
                Ok(())
            } else {
                Err(invalid("partial backing datagram"))
            };
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Receive exactly two handles at worker startup, before creating threads.
/// Every received descriptor is owned and marked CLOEXEC before validation;
/// malformed/truncated messages close all delivered handles. Startup wait is bounded.
pub fn receive_backing(socket: &File) -> io::Result<(File, File)> {
    receive_with_timeout(socket, STARTUP_TIMEOUT)
}

fn receive_with_timeout(socket: &File, timeout: Duration) -> io::Result<(File, File)> {
    let [output, pixels] = receive_descriptors(socket, &MESSAGE, timeout)?;
    validate_pair(&output, &pixels)?;
    Ok((output, pixels))
}

fn receive_descriptors<const N: usize>(
    socket: &File,
    expected_message: &[u8; 8],
    timeout: Duration,
) -> io::Result<[File; N]> {
    let started = Instant::now();
    let mut payload = [0_u8; MESSAGE.len()];
    let mut control = Control([0; CONTROL_BYTES]);
    let mut vector = libc::iovec {
        iov_base: payload.as_mut_ptr().cast(),
        iov_len: payload.len(),
    };
    // SAFETY: zeroed msghdr is valid and all buffer fields are populated before recvmsg.
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = control.0.as_mut_ptr().cast();
    let received = loop {
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "backing handoff timed out",
            ));
        }
        let mut poll = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll receives one writable descriptor record, borrowed for the call.
        let ready = unsafe {
            libc::poll(
                &mut poll,
                1,
                remaining.as_millis().max(1).min(i32::MAX as u128) as i32,
            )
        };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready == 0 {
            continue;
        }
        message.msg_controllen = CONTROL_BYTES as u32;
        message.msg_flags = 0;
        // SAFETY: writable payload/control buffers are live and sized in message.
        let received =
            unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, libc::MSG_DONTWAIT) };
        if received >= 0 {
            break received as usize;
        }
        let error = io::Error::last_os_error();
        if !matches!(
            error.kind(),
            io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
        ) {
            return Err(error);
        }
    };

    let mut handles: [Option<OwnedFd>; MAX_RECEIVED_HANDLES] = std::array::from_fn(|_| None);
    let mut count = 0;
    let mut valid = true;
    let mut flag_error = None;
    let available = (message.msg_controllen as usize).min(CONTROL_BYTES);
    let mut offset = 0;
    // SAFETY: zero payload cannot overflow ancillary alignment arithmetic.
    let header_bytes = unsafe { libc::CMSG_LEN(0) as usize };
    while available.saturating_sub(offset) >= size_of::<libc::cmsghdr>() {
        // SAFETY: the checked range contains a complete POD header; unaligned read
        // avoids relying on an untrusted ancillary offset for alignment.
        let header = unsafe {
            std::ptr::read_unaligned(control.0.as_ptr().add(offset).cast::<libc::cmsghdr>())
        };
        let length = header.cmsg_len as usize;
        if length < header_bytes {
            valid = false;
            break;
        }
        // A truncated header can retain its original length. Always adopt the
        // visible whole descriptors before rejecting it. The kernel-sized buffer
        // above prevents undisclosed descriptors on supported Darwin kernels.
        let complete = length <= available - offset;
        valid &= complete;
        let data = &control.0[offset + header_bytes..offset + length.min(available - offset)];
        if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_RIGHTS {
            valid &= data.len().is_multiple_of(size_of::<RawFd>());
            for bytes in data.chunks_exact(size_of::<RawFd>()) {
                let descriptor =
                    RawFd::from_ne_bytes(bytes.try_into().expect("descriptor-sized chunk"));
                if descriptor < 0 {
                    valid = false;
                    continue;
                }
                // SAFETY: SCM_RIGHTS gives this process a new owned descriptor for
                // each integer; adopting every delivered descriptor prevents leaks.
                let owned = unsafe { OwnedFd::from_raw_fd(descriptor) };
                if let Err(error) = cloexec(descriptor) {
                    flag_error = Some(error);
                }
                if count < handles.len() {
                    handles[count] = Some(owned);
                    count += 1;
                } else {
                    valid = false;
                    drop(owned);
                }
            }
        } else {
            valid = false;
        }
        if !complete {
            break;
        }
        // SAFETY: data lies inside the fixed CONTROL_BYTES-sized buffer.
        offset += unsafe { libc::CMSG_SPACE(data.len() as u32) as usize };
    }
    if let Some(error) = flag_error {
        return Err(error);
    }
    if !valid
        || received != MESSAGE.len()
        || payload != *expected_message
        || count != N
        || message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0
    {
        return Err(invalid("malformed descriptor handoff"));
    }
    Ok(std::array::from_fn(|index| {
        File::from(handles[index].take().expect("validated handle count"))
    }))
}

#[cfg(test)]
mod tests;
