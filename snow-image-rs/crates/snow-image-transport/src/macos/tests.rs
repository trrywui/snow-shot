use super::*;
use std::{
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    process::{Command, Stdio},
    sync::Mutex,
};

static SERIAL: Mutex<()> = Mutex::new(());
const CHILD: &str = "SNOW_IMAGE_TRANSPORT_CHILD";
const INPUT_CHILD: &str = "SNOW_IMAGE_TRANSPORT_INPUT_CHILD";
const INPUT_CONTENTS: &[u8] = b"immutable encoded input";

fn socket_file(socket: UnixDatagram) -> File {
    File::from(OwnedFd::from(socket))
}

fn descriptor_count() -> usize {
    std::fs::read_dir("/dev/fd").unwrap().count()
}

fn frozen_input(contents: &[u8]) -> File {
    let mut temporary = tempfile::NamedTempFile::new().unwrap();
    temporary.write_all(contents).unwrap();
    let input = File::open(temporary.path()).unwrap();
    drop(temporary);
    assert_eq!(input.metadata().unwrap().nlink(), 0);
    input
}

#[test]
fn input_transfer_follows_startup_and_retains_an_immutable_mapping() {
    let _serial = SERIAL.lock().unwrap();
    let baseline = descriptor_count();
    for _ in 0..16 {
        let output = tempfile::tempfile().unwrap();
        let pixels = create_pixel_backing().unwrap();
        let mut input = frozen_input(INPUT_CONTENTS);
        // A shared file cursor must not affect mapping-based input consumption.
        input.seek(SeekFrom::End(0)).unwrap();
        let (send, receive) = UnixDatagram::pair().unwrap();
        let receive = socket_file(receive);
        send_backing(&send, &output, &pixels).unwrap();
        send_input(&send, &input).unwrap();
        drop(input);
        let received_backing = receive_backing(&receive).unwrap();
        let received = receive_input(&receive, INPUT_CONTENTS.len() as u64).unwrap();
        // SAFETY: F_GETFD only reads flags on the live received File.
        let flags = unsafe { libc::fcntl(received.as_raw_fd(), libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
        assert_eq!(received.metadata().unwrap().nlink(), 0);
        // SAFETY: frozen_input closed the only writer and unlinked its private file.
        let mapping = unsafe { memmap2::MmapOptions::new().map(&received).unwrap() };
        drop((received, received_backing, receive, send, output, pixels));
        assert_eq!(&*mapping, INPUT_CONTENTS);
        drop(mapping);
        assert_eq!(descriptor_count(), baseline);
    }
}

#[test]
fn input_sender_rejects_writable_named_empty_and_nonregular_files() {
    let _serial = SERIAL.lock().unwrap();
    let mut named = tempfile::NamedTempFile::new().unwrap();
    named.write_all(INPUT_CONTENTS).unwrap();
    let readonly_named = File::open(named.path()).unwrap();
    let writeonly = OpenOptions::new().write(true).open(named.path()).unwrap();
    let (send, receive) = UnixDatagram::pair().unwrap();
    let empty = frozen_input(b"");
    let directory = File::open(std::env::temp_dir()).unwrap();
    let writable = named.as_file().try_clone().unwrap();
    for rejected in [&readonly_named, &writeonly, &writable, &empty, &directory] {
        assert!(send_input(&send, rejected).is_err());
    }
    drop(named);
    assert!(send_input(&send, &writeonly).is_err());
    assert!(send_input(&send, &writable).is_err());
    drop((writeonly, writable));
    send_input(&send, &readonly_named).unwrap();
    let input = receive_input(&socket_file(receive), INPUT_CONTENTS.len() as u64).unwrap();
    assert_eq!(input.metadata().unwrap().nlink(), 0);
}

#[test]
fn malformed_input_handoffs_close_all_handles_and_validate_length() {
    let _serial = SERIAL.lock().unwrap();
    let input = frozen_input(INPUT_CONTENTS);
    let empty = frozen_input(b"");
    let mut writable = tempfile::tempfile().unwrap();
    writable.write_all(INPUT_CONTENTS).unwrap();
    let mut named = tempfile::NamedTempFile::new().unwrap();
    named.write_all(INPUT_CONTENTS).unwrap();
    let readonly_named = File::open(named.path()).unwrap();
    let directory = File::open(std::env::temp_dir()).unwrap();
    let (send, receive) = UnixDatagram::pair().unwrap();
    let receive = socket_file(receive);
    let baseline = descriptor_count();
    for _ in 0..16 {
        for (payload, handles) in [
            (MESSAGE.as_slice(), vec![input.as_raw_fd()]),
            (
                b"SINI\x01\x00\x01\x00extra".as_slice(),
                vec![input.as_raw_fd()],
            ),
            (INPUT_MESSAGE.as_slice(), vec![]),
            (INPUT_MESSAGE.as_slice(), vec![input.as_raw_fd(); 2]),
            (INPUT_MESSAGE.as_slice(), vec![input.as_raw_fd(); 254]),
            (INPUT_MESSAGE.as_slice(), vec![writable.as_raw_fd()]),
            (INPUT_MESSAGE.as_slice(), vec![readonly_named.as_raw_fd()]),
            (INPUT_MESSAGE.as_slice(), vec![empty.as_raw_fd()]),
            (INPUT_MESSAGE.as_slice(), vec![directory.as_raw_fd()]),
        ] {
            send_descriptors(send.as_raw_fd(), payload, &handles).unwrap();
            assert!(receive_input(&receive, INPUT_CONTENTS.len() as u64).is_err());
            assert_eq!(descriptor_count(), baseline);
        }
        for expected in [
            0,
            INPUT_CONTENTS.len() as u64 - 1,
            INPUT_CONTENTS.len() as u64 + 1,
            u64::MAX,
        ] {
            send_input(&send, &input).unwrap();
            assert!(receive_input(&receive, expected).is_err());
            assert_eq!(descriptor_count(), baseline);
        }
    }
}

#[test]
fn input_handoff_wait_is_bounded() {
    let _serial = SERIAL.lock().unwrap();
    let (send, receive) = UnixDatagram::pair().unwrap();
    let start = Instant::now();
    let error = receive_input_with_timeout(&socket_file(receive), 1, Duration::from_millis(10))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_millis(500));
    drop(send);
}

#[test]
fn child_reads_frozen_input() {
    if std::env::var_os(INPUT_CHILD).is_none() {
        return;
    }
    // SAFETY: duplicate inherited fd2; the new descriptor is owned exactly once.
    let descriptor = unsafe { libc::fcntl(2, libc::F_DUPFD_CLOEXEC, 3) };
    assert!(descriptor >= 0);
    // SAFETY: successful descriptor duplication gives this File unique ownership.
    let socket = unsafe { File::from_raw_fd(descriptor) };
    let _backing = receive_backing(&socket).unwrap();
    let input = receive_input(&socket, INPUT_CONTENTS.len() as u64).unwrap();
    // SAFETY: the parent closed its only writer and unlinked this private snapshot.
    let mapping = unsafe { memmap2::MmapOptions::new().map(&input).unwrap() };
    drop(input);
    assert_eq!(&*mapping, INPUT_CONTENTS);
}

#[test]
fn child_input_handoffs_leave_no_retained_descriptors() {
    let _serial = SERIAL.lock().unwrap();
    let baseline = descriptor_count();
    for _ in 0..8 {
        let output = tempfile::tempfile().unwrap();
        let pixels = create_pixel_backing().unwrap();
        let input = frozen_input(INPUT_CONTENTS);
        let (send, receive) = UnixDatagram::pair().unwrap();
        send_backing(&send, &output, &pixels).unwrap();
        send_input(&send, &input).unwrap();
        drop(input);
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "macos::tests::child_reads_frozen_input"])
            .env(INPUT_CHILD, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(OwnedFd::from(receive)))
            .status()
            .unwrap();
        assert!(status.success());
        drop((output, pixels, send));
        assert_eq!(descriptor_count(), baseline);
    }
}

#[test]
fn checked_rounding_and_object_type() {
    let _serial = SERIAL.lock().unwrap();
    assert_eq!(allocation_len(0).unwrap(), 0);
    let page = allocation_len(1).unwrap();
    assert!(page.is_power_of_two());
    assert_eq!(allocation_len(page).unwrap(), page);
    assert_eq!(allocation_len(page + 1).unwrap(), page * 2);
    assert!(allocation_len(usize::MAX).is_err());
    assert!(allocation_len(isize::MAX as usize).is_err());
    let file = tempfile::tempfile().unwrap();
    assert!(validate_pixel_backing(&file, 0).is_err());
    let pixels = create_pixel_backing().unwrap();
    validate_pixel_backing(&pixels, 0).unwrap();
    pixels.set_len(7).unwrap();
    validate_pixel_backing(&pixels, 7).unwrap();
    assert!(validate_pixel_backing(&pixels, page + 1).is_err());
    assert!(validate_pixel_backing(&pixels, 0).is_err());
    assert!(pixels.set_len(0).is_err());
}

#[test]
fn paired_transfer_preserves_contents_and_cloexec() {
    let _serial = SERIAL.lock().unwrap();
    let mut output = tempfile::tempfile().unwrap();
    output.write_all(b"private encoded input").unwrap();
    let pixels = create_pixel_backing().unwrap();
    let (send, receive) = UnixDatagram::pair().unwrap();
    send_backing(&send, &output, &pixels).unwrap();
    let (mut received_output, received_pixels) = receive_backing(&socket_file(receive)).unwrap();
    received_output.seek(SeekFrom::Start(0)).unwrap();
    let mut contents = String::new();
    received_output.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "private encoded input");
    for descriptor in [received_output.as_raw_fd(), received_pixels.as_raw_fd()] {
        // SAFETY: this read-only query uses a descriptor whose File owner is live.
        let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
    }
    received_pixels.set_len(10).unwrap();
    validate_pixel_backing(&pixels, 10).unwrap();
    // SAFETY: this test is the only writer, and both object owners remain live.
    let mut writable = unsafe {
        memmap2::MmapOptions::new()
            .len(10)
            .map_mut(&received_pixels)
            .unwrap()
    };
    writable.copy_from_slice(b"pixel data");
    drop(writable);
    drop(received_pixels);
    // SAFETY: the only writer and writable mapping were dropped above.
    let readonly = unsafe { memmap2::MmapOptions::new().len(10).map(&pixels).unwrap() };
    drop(pixels);
    assert_eq!(&*readonly, b"pixel data");
}

#[test]
fn malformed_handoffs_release_all_delivered_handles() {
    let _serial = SERIAL.lock().unwrap();
    let output = tempfile::tempfile().unwrap();
    let pixels = create_pixel_backing().unwrap();
    let (send, receive) = UnixDatagram::pair().unwrap();
    let receive = socket_file(receive);
    let baseline = descriptor_count();
    for _ in 0..32 {
        for (payload, handles) in [
            (
                b"wrongver".as_slice(),
                vec![output.as_raw_fd(), pixels.as_raw_fd()],
            ),
            (
                b"SIMG\x01\x00\x02\x00extra".as_slice(),
                vec![output.as_raw_fd(), pixels.as_raw_fd()],
            ),
            (&MESSAGE, vec![output.as_raw_fd()]),
            (
                &MESSAGE,
                vec![output.as_raw_fd(), pixels.as_raw_fd(), pixels.as_raw_fd()],
            ),
            (&MESSAGE, vec![pixels.as_raw_fd(), output.as_raw_fd()]),
            (&MESSAGE, vec![output.as_raw_fd(), output.as_raw_fd()]),
            (&MESSAGE, vec![]),
        ] {
            send_descriptors(send.as_raw_fd(), payload, &handles).unwrap();
            assert!(receive_backing(&receive).is_err());
            assert_eq!(descriptor_count(), baseline);
        }
    }
}

#[test]
fn oversized_ancillary_data_releases_every_kernel_delivered_handle() {
    let _serial = SERIAL.lock().unwrap();
    let output = tempfile::tempfile().unwrap();
    let (send, receive) = UnixDatagram::pair().unwrap();
    let receive = socket_file(receive);
    let baseline = descriptor_count();
    for count in [64, 254].into_iter().cycle().take(16) {
        //64 reproduces the original truncated-control leak;254 reaches the
        //maximum64-bit Darwin send control payload under MCLBYTES=2048.
        let descriptor_storage = [output.as_raw_fd(); 254];
        let descriptors = &descriptor_storage[..count];
        let mut control = [0_usize; 130];
        let mut vector = libc::iovec {
            iov_base: MESSAGE.as_ptr().cast_mut().cast(),
            iov_len: MESSAGE.len(),
        };
        // SAFETY: the zeroed message is populated with correctly sized live buffers.
        let mut message: libc::msghdr = unsafe { zeroed() };
        message.msg_iov = &mut vector;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        // SAFETY: the constant array size cannot overflow ancillary arithmetic.
        message.msg_controllen = unsafe { libc::CMSG_SPACE(size_of_val(descriptors) as u32) };
        // SAFETY: the usize array is sufficiently aligned and larger than the
        // header plus64descriptor integers; sendmsg only borrows original handles.
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
            assert_eq!(
                libc::sendmsg(send.as_raw_fd(), &message, 0),
                MESSAGE.len() as isize
            );
        }
        assert!(receive_backing(&receive).is_err());
        assert_eq!(descriptor_count(), baseline);
    }
}

#[test]
fn absent_sender_does_not_strand_startup() {
    let _serial = SERIAL.lock().unwrap();
    let (send, receive) = UnixDatagram::pair().unwrap();
    // A connected but silent sender must not prevent bounded startup cleanup.
    let start = Instant::now();
    let error = receive_with_timeout(&socket_file(receive), Duration::from_millis(10)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_millis(500));
    drop(send);
}

#[test]
fn child_writes_shared_pixels() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    // SAFETY: duplicate inherited fd2; the new descriptor is owned exactly once.
    let descriptor = unsafe { libc::fcntl(2, libc::F_DUPFD_CLOEXEC, 3) };
    assert!(descriptor >= 0);
    // SAFETY: the successful duplication returned a new descriptor.
    let socket = unsafe { File::from_raw_fd(descriptor) };
    let (_, pixels) = receive_backing(&socket).unwrap();
    pixels.set_len(5000).unwrap();
    // SAFETY: this subprocess is the sole writer before its observed exit.
    let mut mapping = unsafe {
        memmap2::MmapOptions::new()
            .len(5000)
            .map_mut(&pixels)
            .unwrap()
    };
    for (index, byte) in mapping.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    drop(mapping);
}

#[test]
fn retained_pixels_survive_writer_exit_and_handle_closure() {
    let _serial = SERIAL.lock().unwrap();
    let baseline = descriptor_count();
    for _ in 0..8 {
        let output = tempfile::tempfile().unwrap();
        let pixels = create_pixel_backing().unwrap();
        let (send, receive) = UnixDatagram::pair().unwrap();
        send_backing(&send, &output, &pixels).unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "macos::tests::child_writes_shared_pixels"])
            .env(CHILD, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(OwnedFd::from(receive)))
            .status()
            .unwrap();
        assert!(status.success());
        validate_pixel_backing(&pixels, 5000).unwrap();
        // SAFETY: status() observed/reaped the sole writer; this private object
        // has no remaining writable views and will never be modified again.
        let mapping = unsafe { memmap2::MmapOptions::new().len(5000).map(&pixels).unwrap() };
        drop((pixels, output, send));
        assert!(
            mapping
                .iter()
                .enumerate()
                .all(|(index, byte)| *byte == (index % 251) as u8)
        );
        drop(mapping);
        assert_eq!(descriptor_count(), baseline);
    }
}
