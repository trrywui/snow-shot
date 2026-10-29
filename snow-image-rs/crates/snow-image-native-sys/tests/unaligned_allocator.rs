#![cfg(feature = "native-backends")]

use snow_image_native_sys::*;
use std::alloc::{GlobalAlloc, Layout, System};

/// Honor every requested alignment while deliberately avoiding accidental extra
/// alignment for byte allocations. This models a valid non-System allocator.
struct ByteSkewAllocator;
fn byte_layout(layout: Layout) -> Option<Layout> {
    Layout::from_size_align(layout.size().checked_add(1)?, 2).ok()
}
// SAFETY: each allocation meets its requested alignment and each deallocation
// recovers the exact pointer/layout originally passed to System.
unsafe impl GlobalAlloc for ByteSkewAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() != 1 {
            // SAFETY: this branch forwards the unchanged valid allocation layout.
            return unsafe { System.alloc(layout) };
        }
        let Some(backing) = byte_layout(layout) else {
            return std::ptr::null_mut();
        };
        // SAFETY: backing includes one extra byte and requires an even base pointer.
        let base = unsafe { System.alloc(backing) };
        if base.is_null() {
            base
        } else {
            // SAFETY: backing.size() is layout.size() + 1, preserving the whole requested extent.
            unsafe { base.add(1) }
        }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if layout.align() == 1 {
            // SAFETY: alloc shifted this allocation by exactly one byte.
            unsafe { System.dealloc(pointer.sub(1), byte_layout(layout).unwrap()) };
        } else {
            // SAFETY: this branch was allocated with this unchanged layout.
            unsafe { System.dealloc(pointer, layout) };
        }
    }
}
#[global_allocator]
static ALLOCATOR: ByteSkewAllocator = ByteSkewAllocator;

fn image(sample: SampleType, pixels: Vec<u8>, bits: u8) -> NativeImage {
    assert_eq!(pixels.as_ptr().addr() % 2, 1);
    NativeImage {
        width: 1,
        height: 1,
        sample,
        pixels,
        icc: Vec::new(),
        exif: Vec::new(),
        xmp: Vec::new(),
        significant_bits: bits,
        transfer: TransferFunction::Srgb,
        primaries: ColorPrimaries::Srgb,
        orientation: 1,
        hdr_reference_white_nits: 0.0,
    }
}

#[test]
fn avif_u16_input_rejects_valid_but_unaligned_byte_storage() {
    let source = image(
        SampleType::U16,
        [0u16, 65535, 65535, 65535]
            .into_iter()
            .flat_map(u16::to_ne_bytes)
            .collect(),
        10,
    );
    assert!(matches!(
        encode(NativeFormat::Avif, &source, EncodeOptions { bit_depth: 10, ..Default::default() }),
        Err(Error::InvalidInput(message)) if message.contains("alignment")
    ));
}

#[test]
fn high_depth_decode_refuses_unaligned_allocator_output() {
    let source = image(SampleType::U8, vec![20, 90, 200, 255], 8);
    let encoded = encode(
        NativeFormat::Avif,
        &source,
        EncodeOptions {
            bit_depth: 10,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        decode(NativeFormat::Avif, &encoded, DecodeOptions::default()),
        Err(Error::ResourceLimit(_))
    ));
    let source = image(
        SampleType::F16,
        [0x3800u16, 0x3400, 0x3000, 0x3c00]
            .into_iter()
            .flat_map(u16::to_ne_bytes)
            .collect(),
        16,
    );
    // Chunked JXL input copies byte samples into aligned native blocks, so its
    // input remains supported. The returned float16 storage needs alignment.
    let encoded = encode(
        NativeFormat::Jxl,
        &source,
        EncodeOptions {
            effort: 1,
            lossless: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        decode(NativeFormat::Jxl, &encoded, DecodeOptions::default()),
        Err(Error::ResourceLimit(_))
    ));
}

#[test]
fn jpeg_u8_borrowed_and_owned_destinations_need_no_extra_alignment() {
    let source = image(SampleType::U8, vec![20, 90, 200, 255], 8);
    let encoded = encode(NativeFormat::Jpeg, &source, EncodeOptions::default()).unwrap();
    let owned = decode(NativeFormat::Jpeg, &encoded, DecodeOptions::default()).unwrap();
    let mut output = vec![0; 4];
    assert_eq!(output.as_ptr().addr() % 2, 1);
    decode_jpeg_into(&encoded, DecodeOptions::default(), &mut output).unwrap();
    assert_eq!(output, owned.pixels);
}
