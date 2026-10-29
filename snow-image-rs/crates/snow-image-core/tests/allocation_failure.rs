//! An isolated test executable: production ownership code still forbids unsafe Rust.
//!
//! Only the exact pixel Vec layout is refused, once, inside the scoped window.
//! Test-harness allocations and metadata/Arc owners continue to use System.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    ptr,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use snow_image_core::{
    Budget, ColorEncoding, Error, ImageDescriptor, Metadata, MutableImage, PixelFormat,
    ResourceKind,
};

const PIXEL_BYTES: usize = 1024 * 1024;
static REFUSE_PIXEL_ALLOCATION: AtomicBool = AtomicBool::new(false);
static REFUSALS: AtomicUsize = AtomicUsize::new(0);

struct RefusingAllocator;

// SAFETY: successful allocations and all deallocations are delegated unchanged
// to System. Returning null for the selected allocation is GlobalAlloc's defined
// allocation-failure behavior; no foreign or dangling allocation is returned.
unsafe impl GlobalAlloc for RefusingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() == PIXEL_BYTES
            && layout.align() == 1
            && REFUSE_PIXEL_ALLOCATION
                .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            REFUSALS.fetch_add(1, Ordering::SeqCst);
            return ptr::null_mut();
        }
        // SAFETY: the caller supplied a valid allocation Layout, forwarded intact.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: every non-null allocation came from System with this layout.
        // The GlobalAlloc caller upholds its ordinary deallocation preconditions.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: RefusingAllocator = RefusingAllocator;

struct RefusalWindow;
impl RefusalWindow {
    fn enter() -> Self {
        assert!(!REFUSE_PIXEL_ALLOCATION.swap(true, Ordering::SeqCst));
        Self
    }
}
impl Drop for RefusalWindow {
    fn drop(&mut self) {
        REFUSE_PIXEL_ALLOCATION.store(false, Ordering::SeqCst);
    }
}

#[test]
fn actual_pixel_allocation_failure_releases_all_admitted_owners() {
    let budget = Budget::default();
    let descriptor =
        ImageDescriptor::packed(512, 512, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
    assert_eq!(descriptor.required_bytes().unwrap(), PIXEL_BYTES);
    let metadata = Metadata {
        exif: Some(vec![0x55; 31]),
        ..Metadata::default()
    };
    let result = {
        let _window = RefusalWindow::enter();
        MutableImage::new(descriptor.clone(), metadata, &budget)
    };
    assert!(matches!(result, Err(Error::AllocationFailed)));
    assert_eq!(REFUSALS.load(Ordering::SeqCst), 1);
    assert_eq!(budget.peak_used(ResourceKind::Pixels), PIXEL_BYTES as u64);
    assert_eq!(budget.peak_used(ResourceKind::Metadata), 31);
    assert_eq!(budget.used(ResourceKind::Pixels), 0);
    assert_eq!(budget.used(ResourceKind::Metadata), 0);
    assert_eq!(budget.total_used(), 0);

    // Refusal is scoped and one-shot. The same request succeeds after the error.
    let image = MutableImage::new(descriptor, Metadata::default(), &budget).unwrap();
    assert_eq!(image.bytes().len(), PIXEL_BYTES);
    assert_eq!(budget.used(ResourceKind::Pixels), PIXEL_BYTES as u64);
    drop(image);
    assert_eq!(budget.total_used(), 0);
}
