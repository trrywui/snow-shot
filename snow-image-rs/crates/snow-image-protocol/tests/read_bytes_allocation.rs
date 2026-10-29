//! Verify the pinned standard library's bounded spare-capacity read contract.
//! Production protocol code remains unsafe-free. This separate binary forwards
//! all allocation to System except explicitly refused growth/initial allocation.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    io::{self, Read, Write},
    ptr,
};

use snow_image_core::Error;
use snow_image_protocol::read_bytes;

#[derive(Clone, Copy, Default, Debug)]
struct Counts {
    allocations: usize,
    reallocations: usize,
    bytes: usize,
    refused: usize,
}
thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static REFUSE_INITIAL: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<Counts> = const { Cell::new(Counts { allocations: 0, reallocations: 0, bytes: 0, refused: 0 }) };
}

struct GuardedAllocator;
// SAFETY: allocations and deallocations preserve their original System layout.
// Returning null for a selected allocation is the documented allocator failure
// mechanism. Thread-local counters are initialized before the guarded window.
unsafe impl GlobalAlloc for GuardedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let refused = ACTIVE.try_with(|active| active.get()).unwrap_or(false)
            && COUNTS.with(|counts| {
                let mut value = counts.get();
                value.allocations += 1;
                value.bytes += layout.size();
                let refused = REFUSE_INITIAL.with(Cell::get) || value.allocations > 1;
                value.refused += usize::from(refused);
                counts.set(value);
                refused
            });
        if refused {
            return ptr::null_mut();
        }
        // SAFETY: caller's valid layout is passed directly to System.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: this routes through our guarded allocation with the same layout.
        let pointer = unsafe { self.alloc(layout) };
        if !pointer.is_null() {
            // SAFETY: the returned allocation owns exactly this writable layout.
            unsafe { pointer.write_bytes(0, layout.size()) };
        }
        pointer
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if ACTIVE.try_with(|active| active.get()).unwrap_or(false) {
            COUNTS.with(|counts| {
                let mut value = counts.get();
                value.reallocations += 1;
                value.refused += 1;
                counts.set(value);
            });
            return ptr::null_mut();
        }
        // SAFETY: the caller upholds System's pointer/layout/reallocation contract.
        unsafe { System.realloc(pointer, layout, size) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: every non-null allocation came from System with this layout.
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: GuardedAllocator = GuardedAllocator;

struct Window;
impl Window {
    fn enter(refuse: bool) -> Self {
        COUNTS.with(|counts| counts.set(Counts::default()));
        REFUSE_INITIAL.with(|flag| flag.set(refuse));
        ACTIVE.with(|flag| assert!(!flag.replace(true)));
        Self
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        ACTIVE.with(|flag| flag.set(false));
    }
}

struct ShortReader<'a> {
    bytes: &'a [u8],
    position: usize,
    maximum: usize,
    calls: usize,
}
impl Read for ShortReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.calls.is_multiple_of(3) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let count = output
            .len()
            .min(self.maximum)
            .min(self.bytes.len() - self.position);
        output[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
    fn read_to_end(&mut self, _output: &mut Vec<u8>) -> io::Result<usize> {
        panic!("Take must not dispatch to an unbounded custom read_to_end");
    }
}

#[test]
fn bounded_reads_allocate_once_never_grow_or_cross_payload_boundaries() {
    let source: Vec<u8> = (0..=2 * 1024 * 1024)
        .map(|index| (index % 251) as u8)
        .collect();
    for length in [
        0,
        1,
        7,
        31,
        32,
        33,
        8191,
        8192,
        8193,
        65_536,
        2 * 1024 * 1024,
    ] {
        for short in [false, true] {
            let mut reader = ShortReader {
                bytes: &source,
                position: 0,
                maximum: if short { 7 } else { usize::MAX },
                calls: 0,
            };
            let result = {
                let _window = Window::enter(false);
                read_bytes(&mut reader, length as u64)
            };
            let counts = COUNTS.with(Cell::get);
            let bytes = result.unwrap();
            assert_eq!(bytes, source[..length]);
            assert_eq!(bytes.capacity(), length);
            assert_eq!(reader.position, length);
            assert_eq!(
                counts.allocations,
                usize::from(length != 0),
                "{length}, short={short}: {counts:?}"
            );
            assert_eq!(counts.bytes, length);
            assert_eq!(counts.reallocations, 0);
            assert_eq!(counts.refused, 0);
            if length == 0 {
                assert_eq!(reader.calls, 0);
            }
        }
        // A slice implements read_buf directly, covering the non-initializing
        // spare-capacity branch as well as ShortReader's default read_buf above.
        let mut slice = &source[..];
        let result = {
            let _window = Window::enter(false);
            read_bytes(&mut slice, length as u64)
        };
        let counts = COUNTS.with(Cell::get);
        assert_eq!(result.unwrap(), source[..length]);
        assert_eq!(slice, &source[length..]);
        assert_eq!(counts.allocations, usize::from(length != 0));
        assert_eq!(counts.reallocations, 0);
        assert_eq!(counts.refused, 0);
    }

    let mut truncated = &source[..17];
    let result = {
        let _window = Window::enter(false);
        read_bytes(&mut truncated, 32)
    };
    assert!(
        matches!(result, Err(Error::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof)
    );
    let counts = COUNTS.with(Cell::get);
    assert_eq!(counts.allocations, 1);
    assert_eq!(counts.bytes, 32);
    assert_eq!(counts.reallocations, 0);
    assert_eq!(counts.refused, 0);

    let mut untouched = &source[..];
    let result = {
        let _window = Window::enter(true);
        read_bytes(&mut untouched, 1024)
    };
    assert!(matches!(result, Err(Error::AllocationFailed)));
    assert_eq!(untouched.len(), source.len());
    assert_eq!(COUNTS.with(Cell::get).refused, 1);

    // The real IPC reader forwards read_buf without initializing spare capacity.
    // A sentinel after the payload must remain available to the next command.
    let (mut pipe, mut writer) = io::pipe().unwrap();
    let producer = std::thread::spawn(move || {
        writer.write_all(&[0x5a; 65_536]).unwrap();
        writer.write_all(&[0xa5]).unwrap();
    });
    let result = {
        let _window = Window::enter(false);
        read_bytes(&mut pipe, 65_536)
    };
    let counts = COUNTS.with(Cell::get);
    assert_eq!(result.unwrap(), [0x5a; 65_536]);
    assert_eq!(counts.allocations, 1);
    assert_eq!(counts.bytes, 65_536);
    assert_eq!(counts.reallocations, 0);
    assert_eq!(counts.refused, 0);
    let mut sentinel = [0];
    pipe.read_exact(&mut sentinel).unwrap();
    assert_eq!(sentinel, [0xa5]);
    producer.join().unwrap();
}
