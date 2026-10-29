//! Ownership, resource accounting, and image contracts for Snow Image.
//!
//! Pixel samples wider than eight bits use native byte order. An [`Image`] is immutable:
//! cloning it shares storage and retains its reservations until the last owner is dropped.
#![forbid(unsafe_code)]

mod budget;
mod error;
mod image;
mod options;

pub use budget::*;
pub use error::*;
pub use image::*;
pub use options::*;

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    fn descriptor() -> ImageDescriptor {
        ImageDescriptor::packed(2, 2, PixelFormat::RGBA8, ColorEncoding::Srgb).unwrap()
    }

    #[test]
    fn reservations_follow_shared_pixel_ownership() {
        let budget = Budget::default();
        let mut mutable = MutableImage::new(descriptor(), Metadata::default(), &budget).unwrap();
        mutable.bytes_mut()[0] = 42;
        let image = mutable.seal();
        let cloned = image.clone();
        assert_eq!(budget.used(ResourceKind::Pixels), 16);
        assert_eq!(image.bytes().as_ptr(), cloned.bytes().as_ptr());
        drop(image);
        assert_eq!(cloned.bytes()[0], 42);
        assert_eq!(budget.used(ResourceKind::Pixels), 16);
        drop(cloned);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn external_pixel_owner_and_reservation_follow_final_clone() {
        #[derive(Debug)]
        struct External {
            bytes: Box<[u8]>,
            drops: Arc<AtomicUsize>,
        }
        impl AsRef<[u8]> for External {
            fn as_ref(&self) -> &[u8] {
                &self.bytes
            }
        }
        impl Drop for External {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::Relaxed);
            }
        }
        let budget = Budget::default();
        let reservation = budget.reserve(ResourceKind::Pixels, 16).unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let owner = External {
            bytes: vec![43; 16].into_boxed_slice(),
            drops: drops.clone(),
        };
        let original_pointer = owner.as_ref().as_ptr();
        let image = Image::from_owned_storage(
            descriptor(),
            owner,
            SharedMetadata::new(Metadata::default(), &budget).unwrap(),
            reservation,
        )
        .unwrap();
        let cloned = image.clone();
        drop(image);
        assert_eq!(cloned.bytes().as_ptr(), original_pointer);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        assert_eq!(budget.used(ResourceKind::Pixels), 16);
        let mut copy = cloned.try_copy(&budget).unwrap();
        copy.bytes_mut()[0] = 99;
        assert_eq!(cloned.bytes()[0], 43);
        assert_ne!(copy.bytes().as_ptr(), original_pointer);
        drop(cloned);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        assert_eq!(budget.used(ResourceKind::Pixels), 16);
        drop(copy);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn externally_reserved_pixels_validate_kind_capacity_and_length() {
        let budget = Budget::default();
        for (length, kind, amount) in [
            (15, ResourceKind::Pixels, 16),
            (16, ResourceKind::Scratch, 16),
            (16, ResourceKind::Pixels, 15),
        ] {
            let reservation = budget.reserve(kind, amount).unwrap();
            assert!(
                Image::from_owned_storage(
                    descriptor(),
                    vec![1; length].into_boxed_slice(),
                    SharedMetadata::new(Metadata::default(), &budget).unwrap(),
                    reservation
                )
                .is_err()
            );
            assert_eq!(budget.total_used(), 0);
        }
        let mut bytes = Vec::with_capacity(32);
        bytes.resize(16, 0);
        let reservation = budget.reserve(ResourceKind::Pixels, 16).unwrap();
        assert!(
            Image::from_reserved(
                descriptor(),
                bytes,
                SharedMetadata::new(Metadata::default(), &budget).unwrap(),
                reservation
            )
            .is_err()
        );
        assert_eq!(budget.total_used(), 0);
        let reservation = budget.reserve(ResourceKind::Pixels, 16).unwrap();
        let image = Image::from_reserved(
            descriptor(),
            vec![42; 16],
            SharedMetadata::new(Metadata::default(), &budget).unwrap(),
            reservation,
        )
        .unwrap();
        assert_eq!(budget.used(ResourceKind::Pixels), 16);
        drop(image);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn calibrated_color_retains_precise_source_tags_and_unknown_components() {
        let source_gamma = f64::from(u32::MAX) / 100_000.0;
        let color = ColorEncoding::CalibratedRgb {
            gamma: Some(source_gamma),
            chromaticities: None,
        };
        color.validate().unwrap();
        assert!(!color.is_hdr());
        let ColorEncoding::CalibratedRgb {
            gamma: Some(gamma), ..
        } = color
        else {
            unreachable!()
        };
        assert_eq!((gamma * 100_000.0).round() as u32, u32::MAX);
        assert!(
            ColorEncoding::CalibratedRgb {
                gamma: Some(0.0),
                chromaticities: None
            }
            .validate()
            .is_err()
        );
        assert!(
            ColorEncoding::CalibratedRgb {
                gamma: None,
                chromaticities: None
            }
            .validate()
            .is_err()
        );
        let incomplete_bmp = ColorEncoding::CalibratedBmp {
            endpoints: [[0.0; 3]; 3],
            gamma: [0.0; 3],
        };
        incomplete_bmp.validate().unwrap();
        assert!(!incomplete_bmp.is_hdr());
        let mut chromaticities = RgbChromaticities {
            white: [0.3127, 0.329],
            red: [0.64, 0.33],
            green: [0.3, 0.6],
            blue: [0.15, 0.06],
        };
        chromaticities.validate().unwrap();
        chromaticities.blue = [0.5, 0.6];
        assert!(chromaticities.validate().is_err());
        assert!(
            ColorEncoding::CalibratedBmp {
                endpoints: [[0.0; 3]; 3],
                gamma: [f64::NAN, 1.0, 1.0]
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn metadata_capacity_is_charged_once_and_released() {
        let budget = Budget::default();
        let mut profile = Vec::with_capacity(32);
        profile.extend([1, 2, 3, 4]);
        let metadata = Metadata {
            icc: Some(profile),
            ..Metadata::default()
        };
        let image = MutableImage::new(descriptor(), metadata, &budget)
            .unwrap()
            .seal();
        assert_eq!(budget.used(ResourceKind::Metadata), 32);
        let clone = image.clone();
        drop(image);
        assert_eq!(budget.used(ResourceKind::Metadata), 32);
        drop(clone);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn validation_rejects_overflow_invalid_stride_and_sample_bits() {
        let mut desc = descriptor();
        desc.row_stride = usize::MAX;
        assert!(desc.required_bytes().is_err());
        desc.row_stride = 4;
        assert!(desc.validate().is_err());
        desc.row_stride = 8;
        desc.format.significant_bits = 9;
        assert!(desc.validate().is_err());
        desc.format.sample = SampleType::F16;
        desc.format.significant_bits = 10;
        assert!(desc.validate().is_err());
        desc.format.significant_bits = 16;
        desc.row_stride = 17;
        assert!(desc.validate().is_err());
        desc.row_stride = 16;
        desc.width = 0;
        assert!(desc.validate().is_err());
    }

    #[test]
    fn padded_rows_and_regions_never_publish_padding() {
        let budget = Budget::default();
        let mut desc = descriptor();
        desc.row_stride = 12;
        let pixels = vec![
            1, 2, 3, 4, 5, 6, 7, 8, 99, 99, 99, 99, 11, 12, 13, 14, 15, 16, 17, 18, 99, 99, 99, 99,
        ];
        let image = Image::from_vec(desc, pixels, Metadata::default(), &budget).unwrap();
        let mut packed = [0; 16];
        image.read_rows(0, 2, &mut packed).unwrap();
        assert_eq!(
            packed,
            [1, 2, 3, 4, 5, 6, 7, 8, 11, 12, 13, 14, 15, 16, 17, 18]
        );
        let mut region = [0; 8];
        image
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
        assert_eq!(region, [5, 6, 7, 8, 15, 16, 17, 18]);
        assert!(image.read_rows(u32::MAX, 2, &mut packed).is_err());
        assert!(image.read_rows(0, 1, &mut packed).is_err());
        assert!(image.view().row(2).is_err());
    }

    #[test]
    fn failed_admission_rolls_back_all_reservations() {
        let budget = Budget::new(ResourcePolicy {
            max_metadata_bytes: 0,
            ..ResourcePolicy::default()
        });
        let metadata = Metadata {
            xmp: Some(vec![1]),
            ..Metadata::default()
        };
        assert!(MutableImage::new(descriptor(), metadata, &budget).is_err());
        assert_eq!(budget.total_used(), 0);
        let malformed = vec![0; 15];
        assert!(Image::from_vec(descriptor(), malformed, Metadata::default(), &budget).is_err());
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn aggregate_and_job_limits_are_independent() {
        let budget = Budget::new(ResourcePolicy {
            total_memory_bytes: 100,
            working_bytes_per_job: 60,
            ..ResourcePolicy::default()
        });
        let first = budget.job();
        let second = budget.job();
        let a = first.reserve(ResourceKind::Scratch, 55).unwrap();
        assert!(first.reserve(ResourceKind::Pixels, 6).is_err());
        let b = second.reserve(ResourceKind::Scratch, 45).unwrap();
        assert_eq!(budget.total_used(), 100);
        assert!(second.reserve(ResourceKind::Scratch, 1).is_err());
        drop(a);
        assert_eq!(first.used(), 0);
        drop(b);
        assert_eq!(second.used(), 0);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn resizing_reservations_is_transactional_and_overflow_safe() {
        let budget = Budget::new(ResourcePolicy {
            total_memory_bytes: 10,
            working_bytes_per_job: 8,
            ..ResourcePolicy::default()
        });
        let job = budget.job();
        let mut reservation = job.reserve(ResourceKind::Pixels, 5).unwrap();
        assert!(reservation.resize(9).is_err());
        assert_eq!(job.used(), 5);
        assert_eq!(budget.total_used(), 5);
        reservation.resize(7).unwrap();
        assert_eq!(budget.total_used(), 7);
        reservation.resize(2).unwrap();
        assert_eq!(job.used(), 2);
        assert!(budget.reserve(ResourceKind::Pixels, u64::MAX).is_err());
        drop(reservation);
        assert_eq!(job.used(), 0);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn temporary_rasters_use_disk_not_pixel_budget() {
        let budget = Budget::new(ResourcePolicy {
            total_memory_bytes: 1,
            ..ResourcePolicy::default()
        });
        let source = FileRasterSource::from_reader(
            descriptor(),
            Cursor::new([7; 16]),
            &budget,
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.used(ResourceKind::TempDisk), 16);
        let mut row = [0; 8];
        source.read_rows(1, 1, &mut row).unwrap();
        assert_eq!(row, [7; 8]);
        drop(source);
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    }

    #[test]
    fn cancelled_and_truncated_snapshots_release_disk_budget() {
        let budget = Budget::default();
        let token = CancellationToken::new();
        let clone = token.clone();
        token.cancel();
        assert!(matches!(clone.check(), Err(Error::Cancelled)));
        assert!(
            FileRasterSource::from_reader(descriptor(), Cursor::new([7; 16]), &budget, &token)
                .is_err()
        );
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
        assert!(
            FileRasterSource::from_reader(
                descriptor(),
                Cursor::new([7; 15]),
                &budget,
                &CancellationToken::new()
            )
            .is_err()
        );
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
    }

    #[test]
    fn exactness_options_are_not_silently_weakened() {
        assert!(
            EncodeOptions::WebP(WebPOptions {
                lossless: false,
                exact: true,
                ..WebPOptions::default()
            })
            .validate()
            .is_err()
        );
        assert!(
            EncodeOptions::Jxl(JxlOptions {
                distance: 1.0,
                exact: true,
                ..JxlOptions::default()
            })
            .validate()
            .is_err()
        );
        assert!(
            EncodeOptions::Jxl(JxlOptions {
                distance: f32::NAN,
                ..JxlOptions::default()
            })
            .validate()
            .is_err()
        );
        assert!(
            EncodeOptions::Avif(AvifOptions {
                bit_depth: 9,
                ..AvifOptions::default()
            })
            .validate()
            .is_err()
        );
        assert!(EncodeOptions::default().validate().is_ok());
    }

    #[test]
    fn floating_point_storage_does_not_imply_hdr() {
        let format = PixelFormat {
            sample: SampleType::F32,
            significant_bits: 32,
            ..PixelFormat::RGBA8
        };
        let image = ImageDescriptor::packed(1, 1, format, ColorEncoding::LinearSrgb).unwrap();
        assert!(!image.color.is_hdr());
        assert!(
            ColorEncoding::ScRgb {
                reference_white_nits: 80.0
            }
            .is_hdr()
        );
        assert!(
            ColorEncoding::ScRgb {
                reference_white_nits: f32::INFINITY
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn concurrent_accounting_never_admits_more_than_global_limit() {
        let budget = Budget::new(ResourcePolicy {
            total_memory_bytes: 64,
            ..ResourcePolicy::default()
        });
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let budget = budget.clone();
                scope.spawn(move || {
                    for _ in 0..100 {
                        if let Ok(mut reservation) = budget.reserve(ResourceKind::Pixels, 8) {
                            assert!(budget.total_used() <= 64);
                            let _ = reservation.resize(16);
                            assert!(budget.total_used() <= 64);
                        }
                    }
                });
            }
        });
        assert_eq!(budget.total_used(), 0);
    }
    #[test]
    fn explicit_pixel_copy_shares_accounted_metadata() {
        let budget = Budget::default();
        let metadata = Metadata {
            icc: Some(vec![1, 2, 3]),
            ..Metadata::default()
        };
        let source = MutableImage::new(descriptor(), metadata, &budget)
            .unwrap()
            .seal();
        let copy = source.try_copy(&budget).unwrap().seal();
        assert_ne!(source.bytes().as_ptr(), copy.bytes().as_ptr());
        assert_eq!(
            source.metadata().icc.as_ref().unwrap().as_ptr(),
            copy.metadata().icc.as_ref().unwrap().as_ptr()
        );
        assert_eq!(budget.used(ResourceKind::Metadata), 3);
        assert_eq!(budget.used(ResourceKind::Pixels), 32);
        drop(source);
        assert_eq!(budget.used(ResourceKind::Metadata), 3);
        drop(copy);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn private_file_producer_cannot_exceed_declared_size() {
        let budget = Budget::default();
        let metadata = SharedMetadata::new(Metadata::default(), &budget).unwrap();
        let result = FileRasterSource::create(
            descriptor(),
            metadata.clone(),
            &budget,
            &CancellationToken::new(),
            |writer| {
                writer.write_all(&[1; 17])?;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(budget.used(ResourceKind::TempDisk), 0);
        let source = FileRasterSource::create(
            descriptor(),
            metadata,
            &budget,
            &CancellationToken::new(),
            |writer| {
                writer.seek(std::io::SeekFrom::Start(8))?;
                writer.write_all(&[2; 8])?;
                writer.seek(std::io::SeekFrom::Start(0))?;
                writer.write_all(&[1; 8])?;
                assert!(writer.seek(std::io::SeekFrom::Start(17)).is_err());
                Ok(())
            },
        )
        .unwrap();
        let mut bytes = [0; 16];
        source.read_rows(0, 2, &mut bytes).unwrap();
        assert_eq!(bytes, [1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2]);
    }

    #[test]
    fn policies_require_bounded_execution() {
        assert!(ResourcePolicy::default().validate().is_ok());
        assert!(
            ResourcePolicy {
                max_codec_operations: 3,
                ..ResourcePolicy::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            ResourcePolicy {
                working_bytes_per_job: 0,
                ..ResourcePolicy::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            ResourcePolicy {
                max_metadata_bytes: 0,
                ..ResourcePolicy::default()
            }
            .validate()
            .is_ok()
        );
    }
    #[test]
    fn reservation_high_water_marks_survive_releases() {
        let budget = Budget::default();
        let first = budget.reserve(ResourceKind::Pixels, 10).unwrap();
        let second = budget.reserve(ResourceKind::Metadata, 5).unwrap();
        assert_eq!(budget.peak_total_used(), 15);
        drop(first);
        drop(second);
        assert_eq!(budget.total_used(), 0);
        assert_eq!(budget.peak_total_used(), 15);
        assert_eq!(budget.peak_used(ResourceKind::Pixels), 10);
        let _disk = budget.reserve(ResourceKind::TempDisk, 100).unwrap();
        assert_eq!(budget.peak_total_used(), 15);
        assert_eq!(budget.peak_used(ResourceKind::TempDisk), 100);
    }
    #[test]
    fn cancellable_rows_check_before_and_after_producer() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Producer {
            descriptor: ImageDescriptor,
            token: CancellationToken,
            calls: AtomicUsize,
        }
        impl RasterSource for Producer {
            fn descriptor(&self) -> &ImageDescriptor {
                &self.descriptor
            }
            fn access(&self) -> RasterAccess {
                RasterAccess::SequentialRows
            }
            fn read_rows(&self, _: u32, _: u32, dst: &mut [u8]) -> Result<()> {
                self.calls.fetch_add(1, Ordering::Relaxed);
                self.token.cancel();
                dst.fill(1);
                Ok(())
            }
        }
        let token = CancellationToken::new();
        let source = Producer {
            descriptor: descriptor(),
            token: token.clone(),
            calls: AtomicUsize::new(0),
        };
        assert!(matches!(
            source.read_rows_cancellable(0, 1, &mut [0; 8], &token),
            Err(Error::Cancelled)
        ));
        assert_eq!(source.calls.load(Ordering::Relaxed), 1);
        assert!(matches!(
            source.read_rows_cancellable(0, 1, &mut [0; 8], &token),
            Err(Error::Cancelled)
        ));
        assert_eq!(source.calls.load(Ordering::Relaxed), 1);
    }
}
