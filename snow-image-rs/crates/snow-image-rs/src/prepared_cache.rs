//! Optional immutable prepared pixels. Admission is shared across the whole service.
//! A cache miss keeps the already prepared file; it never recopies or recomputes pixels.

use std::{fs::File, sync::Arc};

use snow_image_core::*;

pub(crate) enum PreparedCacheResult {
    Cached(Arc<dyn RasterSource>),
    Uncached {
        descriptor: ImageDescriptor,
        metadata: SharedMetadata,
        file: File,
        disk: Reservation,
    },
}

struct CachedRaster {
    descriptor: ImageDescriptor,
    metadata: SharedMetadata,
    mapping: memmap2::Mmap,
    _prepared: Reservation,
    _disk: Reservation,
}

/// Map an optional prepared-pixel cache entry after its writer has stopped.
/// Resource admission or a mapping failure leaves the original file available.
///
/// # Safety
/// The caller must have observed the writer process tree's termination and reaped
/// its child. The file must be private and immutable for the returned owner's
/// entire lifetime: no external writable handles, mappings, or writable paths may
/// remain. The supplied disk reservation must account for that file's lifetime.
pub(crate) unsafe fn try_map_quiescent(
    descriptor: ImageDescriptor,
    metadata: SharedMetadata,
    file: File,
    disk: Reservation,
    budget: &Budget,
) -> Result<PreparedCacheResult> {
    try_map_with(descriptor, metadata, file, disk, budget, |file, length| {
        let mut options = memmap2::MmapOptions::new();
        options.len(length);
        #[cfg(windows)]
        options.no_probe_handle();
        // SAFETY: the caller guarantees a quiescent, private immutable file for
        // the resulting mapping's whole lifetime. The original handle is closed
        // when ownership transfers into CachedRaster below.
        unsafe { options.map(file) }
    })
}

fn try_map_with(
    descriptor: ImageDescriptor,
    metadata: SharedMetadata,
    file: File,
    disk: Reservation,
    budget: &Budget,
    map: impl FnOnce(&File, usize) -> std::io::Result<memmap2::Mmap>,
) -> Result<PreparedCacheResult> {
    descriptor.validate_with_policy(budget)?;
    let length = descriptor.required_bytes()?;
    if disk.resource() != ResourceKind::TempDisk || disk.amount() < length as u64 {
        return Err(Error::InvalidInput(
            "prepared backing lacks its temporary-storage reservation".into(),
        ));
    }
    let status = file.metadata()?;
    if !status.is_file() || status.len() != length as u64 {
        return Err(Error::InvalidInput(
            "prepared backing length differs from its descriptor".into(),
        ));
    }
    let reservation = match budget.reserve(ResourceKind::PreparedPixels, length as u64) {
        Ok(reservation) => reservation,
        Err(Error::ResourceLimit { .. }) => {
            return Ok(PreparedCacheResult::Uncached {
                descriptor,
                metadata,
                file,
                disk,
            });
        }
        Err(error) => return Err(error),
    };
    let mapping = match map(&file, length) {
        Ok(mapping) => mapping,
        Err(_) => {
            drop(reservation);
            return Ok(PreparedCacheResult::Uncached {
                descriptor,
                metadata,
                file,
                disk,
            });
        }
    };
    drop(file);
    Ok(PreparedCacheResult::Cached(Arc::new(CachedRaster {
        descriptor,
        metadata,
        mapping,
        _prepared: reservation,
        _disk: disk,
    })))
}

impl RasterSource for CachedRaster {
    fn descriptor(&self) -> &ImageDescriptor {
        &self.descriptor
    }
    fn access(&self) -> RasterAccess {
        RasterAccess::Regions
    }
    fn metadata(&self) -> Option<&Metadata> {
        Some(self.metadata.get())
    }
    fn read_rows(&self, first: u32, rows: u32, destination: &mut [u8]) -> Result<()> {
        let row_bytes = self.descriptor.row_bytes()?;
        if first
            .checked_add(rows)
            .is_none_or(|end| end > self.descriptor.height)
            || row_bytes.checked_mul(rows as usize) != Some(destination.len())
        {
            return Err(Error::InvalidInput(
                "invalid cached raster row request".into(),
            ));
        }
        for (index, output) in destination.chunks_exact_mut(row_bytes).enumerate() {
            let start = (first as usize + index) * self.descriptor.row_stride;
            output.copy_from_slice(&self.mapping[start..start + row_bytes]);
        }
        Ok(())
    }
    fn read_region(&self, region: Region, destination: &mut [u8]) -> Result<()> {
        if region
            .x
            .checked_add(region.width)
            .is_none_or(|end| end > self.descriptor.width)
            || region
                .y
                .checked_add(region.height)
                .is_none_or(|end| end > self.descriptor.height)
        {
            return Err(Error::InvalidInput(
                "cached raster region outside pixels".into(),
            ));
        }
        let bytes_per_pixel = self.descriptor.format.bytes_per_pixel();
        let row_bytes = (region.width as usize)
            .checked_mul(bytes_per_pixel)
            .ok_or_else(|| Error::InvalidInput("cached raster region size overflow".into()))?;
        if row_bytes.checked_mul(region.height as usize) != Some(destination.len()) {
            return Err(Error::InvalidInput(
                "cached raster region destination mismatch".into(),
            ));
        }
        if row_bytes == 0 {
            return Ok(());
        }
        for (row, output) in destination.chunks_exact_mut(row_bytes).enumerate() {
            let start = (region.y as usize + row) * self.descriptor.row_stride
                + region.x as usize * bytes_per_pixel;
            output.copy_from_slice(&self.mapping[start..start + row_bytes]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};

    fn fixture(budget: &Budget) -> (ImageDescriptor, SharedMetadata, File, Reservation) {
        let descriptor =
            ImageDescriptor::packed(2, 1, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
        let metadata = SharedMetadata::new(Metadata::default(), budget).unwrap();
        let disk = budget.reserve(ResourceKind::TempDisk, 8).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        (descriptor, metadata, file, disk)
    }

    fn map_fixture(budget: &Budget) -> PreparedCacheResult {
        let (descriptor, metadata, file, disk) = fixture(budget);
        // SAFETY: this unlinked file has a single handle, no mappings, and no writer.
        unsafe { try_map_quiescent(descriptor, metadata, file, disk, budget).unwrap() }
    }

    #[test]
    fn cached_pixels_and_metadata_follow_the_final_shared_owner() {
        let budget = Budget::default();
        let (descriptor, _, file, disk) = fixture(&budget);
        let metadata = SharedMetadata::new(
            Metadata {
                xmp: Some(vec![9, 8, 7]),
                ..Metadata::default()
            },
            &budget,
        )
        .unwrap();
        let metadata_pointer = metadata.get().xmp.as_ref().unwrap().as_ptr();
        // SAFETY: this unlinked file has a single handle and no remaining writer.
        let result =
            unsafe { try_map_quiescent(descriptor, metadata, file, disk, &budget).unwrap() };
        let PreparedCacheResult::Cached(source) = result else {
            panic!("expected cache admission")
        };
        let retained = source.clone();
        assert!(Arc::ptr_eq(&source, &retained));
        assert_eq!(source.access(), RasterAccess::Regions);
        assert_eq!(
            source.metadata().unwrap().xmp.as_ref().unwrap().as_ptr(),
            metadata_pointer
        );
        assert_eq!(budget.used(ResourceKind::PreparedPixels), 8);
        assert_eq!(budget.used(ResourceKind::Pixels), 0);
        assert_eq!(budget.used(ResourceKind::Metadata), 3);
        assert_eq!(budget.used(ResourceKind::TempDisk), 8);
        drop(source);
        let mut row = [0; 8];
        retained.read_rows(0, 1, &mut row).unwrap();
        assert_eq!(row, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(budget.used(ResourceKind::PreparedPixels), 8);
        drop(retained);
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    }

    #[test]
    fn retained_entries_exhaust_cache_then_file_backing_can_be_admitted_after_release() {
        let budget = Budget::new(ResourcePolicy {
            max_prepared_bytes: 8,
            ..ResourcePolicy::default()
        });
        let first = map_fixture(&budget);
        let second = map_fixture(&budget);
        assert!(matches!(&first, PreparedCacheResult::Cached(_)));
        let PreparedCacheResult::Uncached {
            descriptor,
            metadata,
            mut file,
            disk,
        } = second
        else {
            panic!("cache must fall back")
        };
        assert_eq!(budget.used(ResourceKind::PreparedPixels), 8);
        assert_eq!(budget.used(ResourceKind::Pixels), 0);
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = [0; 8];
        file.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
        drop(first);
        assert_eq!(budget.used(ResourceKind::PreparedPixels), 0);
        // SAFETY: fallback kept the single private file handle without creating a mapping.
        let cached =
            unsafe { try_map_quiescent(descriptor, metadata, file, disk, &budget).unwrap() };
        assert!(matches!(&cached, PreparedCacheResult::Cached(_)));
        drop(cached);
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    }

    #[test]
    fn aggregate_memory_and_disabled_cache_fall_back_without_a_pixel_charge() {
        for policy in [
            ResourcePolicy {
                total_memory_bytes: 4,
                ..ResourcePolicy::default()
            },
            ResourcePolicy {
                max_prepared_bytes: 0,
                ..ResourcePolicy::default()
            },
        ] {
            let budget = Budget::new(policy);
            let result = map_fixture(&budget);
            assert!(matches!(&result, PreparedCacheResult::Uncached { .. }));
            assert_eq!(budget.total_used(), 0);
            assert_eq!(budget.used(ResourceKind::PreparedPixels), 0);
            assert_eq!(budget.used(ResourceKind::TempDisk), 8);
            drop(result);
            assert_eq!(budget.used(ResourceKind::TempDisk), 0);
        }
    }

    #[test]
    fn mapping_failure_rolls_back_optional_admission_and_keeps_the_original_file() {
        let budget = Budget::default();
        let (descriptor, metadata, file, disk) = fixture(&budget);
        let result = try_map_with(descriptor, metadata, file, disk, &budget, |_, length| {
            assert_eq!(budget.used(ResourceKind::PreparedPixels), length as u64);
            Err(std::io::Error::other("injected mapping allocation failure"))
        })
        .unwrap();
        let PreparedCacheResult::Uncached {
            mut file,
            metadata,
            disk,
            ..
        } = result
        else {
            panic!("expected fallback")
        };
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 8);
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
        drop((file, metadata, disk));
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    }

    #[test]
    fn cached_rows_and_regions_exclude_padding_and_validate_overflow() {
        let budget = Budget::default();
        let mut descriptor =
            ImageDescriptor::packed(2, 2, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
        descriptor.row_stride = 12;
        let metadata = SharedMetadata::new(Metadata::default(), &budget).unwrap();
        let disk = budget.reserve(ResourceKind::TempDisk, 24).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&[
            1, 2, 3, 4, 5, 6, 7, 8, 99, 99, 99, 99, 9, 10, 11, 12, 13, 14, 15, 16, 99, 99, 99, 99,
        ])
        .unwrap();
        // SAFETY: only this handle owns the private unlinked file and writes have finished.
        let result =
            unsafe { try_map_quiescent(descriptor, metadata, file, disk, &budget).unwrap() };
        let PreparedCacheResult::Cached(source) = result else {
            panic!("expected cache")
        };
        let mut rows = [0; 16];
        source.read_rows(0, 2, &mut rows).unwrap();
        assert_eq!(
            rows,
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
        let mut region = [0; 8];
        source
            .read_region(
                Region {
                    x: 1,
                    y: 0,
                    width: 1,
                    height: 2,
                },
                &mut region,
            )
            .unwrap();
        assert_eq!(region, [5, 6, 7, 8, 13, 14, 15, 16]);
        assert!(source.read_rows(u32::MAX, 2, &mut []).is_err());
        assert!(source.read_rows(0, 2, &mut [0; 15]).is_err());
        assert!(
            source
                .read_region(
                    Region {
                        x: u32::MAX,
                        y: 0,
                        width: 2,
                        height: 1
                    },
                    &mut []
                )
                .is_err()
        );
        assert!(
            source
                .read_region(
                    Region {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1
                    },
                    &mut [0; 3]
                )
                .is_err()
        );
        source.read_rows(2, 0, &mut []).unwrap();
        source
            .read_region(
                Region {
                    x: 2,
                    y: 2,
                    width: 0,
                    height: 0,
                },
                &mut [],
            )
            .unwrap();
        assert_eq!(budget.used(ResourceKind::PreparedPixels), 24);
        assert_eq!(budget.used(ResourceKind::Pixels), 0);
    }

    #[test]
    fn malformed_backing_and_wrong_reservations_are_not_cache_misses() {
        for case in 0..3 {
            let budget = Budget::default();
            let (descriptor, metadata, file, mut disk) = fixture(&budget);
            if case == 0 {
                file.set_len(7).unwrap();
            }
            if case == 1 {
                disk.resize(7).unwrap();
            }
            if case == 2 {
                drop(disk);
                disk = budget.reserve(ResourceKind::Scratch, 8).unwrap();
            }
            assert!(matches!(
                // SAFETY: no alias can mutate this private file after the deliberate setup above.
                unsafe { try_map_quiescent(descriptor, metadata, file, disk, &budget) },
                Err(Error::InvalidInput(_))
            ));
            assert_eq!(budget.total_used(), 0);
            assert_eq!(budget.used(ResourceKind::TempDisk), 0);
        }
    }

    #[test]
    fn default_cache_limit_keeps_large_prepared_rasters_on_disk() {
        let budget = Budget::default();
        assert_eq!(budget.policy().max_prepared_bytes, 64 * 1024 * 1024);
        let descriptor =
            ImageDescriptor::packed(4096, 4097, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap();
        let bytes = descriptor.required_bytes().unwrap() as u64;
        let metadata = SharedMetadata::new(Metadata::default(), &budget).unwrap();
        let disk = budget.reserve(ResourceKind::TempDisk, bytes).unwrap();
        let file = tempfile::tempfile().unwrap();
        file.set_len(bytes).unwrap();
        // SAFETY: this sparse file is private, unlinked, and has no other handle or mapping.
        let result =
            unsafe { try_map_quiescent(descriptor, metadata, file, disk, &budget).unwrap() };
        assert!(matches!(&result, PreparedCacheResult::Uncached { .. }));
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), bytes);
        drop(result);
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    }
}
