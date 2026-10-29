use super::*;

#[test]
fn visual_only_layers_never_supply_reconstruction_donors() {
    let source = RgbaImage::from_fn(100, 60, |x, _| {
        image::Rgba(if x < 50 {
            [50, 100, 150, 255]
        } else {
            [240, 10, 20, 255]
        })
    });
    let mut mask = vec![0; 6000];
    for y in 25..35 {
        for x in 40..45 {
            mask[y * 100 + x] = 255;
        }
    }
    let donors: Vec<_> = (0..6000)
        .map(|i| if i % 100 < 50 { 255 } else { 0 })
        .collect();
    let result =
        reconstruct_with_donors(&source, &mask, Some(&donors), &Default::default()).unwrap();
    assert_eq!(result.get_pixel(42, 30).0, [50, 100, 150, 255]);
    assert_eq!(result.get_pixel(75, 30), source.get_pixel(75, 30));
}
use image::Rgba;
fn mask(w: u32, h: u32, x: u32, y: u32, width: u32, height: u32) -> Vec<u8> {
    (0..w * h)
        .map(|i| {
            if i % w >= x && i % w < x + width && i / w >= y && i / w < y + height {
                255
            } else {
                0
            }
        })
        .collect()
}
fn erase(clean: &RgbaImage, mask: &[u8]) -> RgbaImage {
    let mut src = clean.clone();
    for (i, p) in src.pixels_mut().enumerate() {
        if mask[i] > 0 && p[3] > 0 {
            p[0] = 255;
            p[1] = 0;
            p[2] = 255;
        }
    }
    reconstruct(&src, mask, &Default::default()).unwrap()
}
fn exact_known(clean: &RgbaImage, out: &RgbaImage, mask: &[u8]) {
    for (i, (a, b)) in clean.pixels().zip(out.pixels()).enumerate() {
        assert_eq!(a[3], b[3], "alpha {i}");
        if mask[i] == 0 || a[3] == 0 {
            assert_eq!(a, b, "known {i}");
        }
    }
}
#[test]
fn flat_affine_edge_and_alpha_are_preserved() {
    for (w, h, x, y, mw, mh) in [(160, 120, 44, 28, 72, 64), (32, 40, 0, 0, 27, 35)] {
        let src = RgbaImage::from_fn(w, h, |x, y| {
            Rgba([
                (50 + x / 4 + y / 8) as u8,
                (100 + x / 8) as u8,
                (150 + y / 4) as u8,
                if x == w - 1 { 120 } else { 255 },
            ])
        });
        let mask = mask(w, h, x, y, mw, mh);
        let out = erase(&src, &mask);
        exact_known(&src, &out, &mask);
        let mut sum = 0;
        let mut count = 0;
        for (i, (a, b)) in src.pixels().zip(out.pixels()).enumerate() {
            if mask[i] > 0 {
                for c in 0..3 {
                    sum += a[c].abs_diff(b[c]) as usize;
                    count += 1;
                }
            }
        }
        assert!(
            (sum as f64 / count as f64) < 1.5,
            "affine MAE {}",
            sum as f64 / count as f64
        );
    }
}
#[test]
fn periodic_checker_is_exact_and_masked_colors_cannot_leak() {
    let src = RgbaImage::from_fn(160, 112, |x, y| {
        let n = if (x / 8 + y / 8) % 2 == 0 { 24 } else { 0 };
        Rgba([140 + n, 160 + n, 180 + n, 255])
    });
    let mask = mask(160, 112, 48, 32, 64, 48);
    let out = erase(&src, &mask);
    assert_eq!(src, out);
    let mut other = src.clone();
    for (i, p) in other.pixels_mut().enumerate() {
        if mask[i] > 0 {
            *p = Rgba([0, 255, 20, 255]);
        }
    }
    assert_eq!(
        out,
        reconstruct(&other, &mask, &Default::default()).unwrap()
    );
}
fn noise(x: u32, y: u32) -> i32 {
    let h = (x.wrapping_mul(73856093)) ^ (y.wrapping_mul(19349663));
    ((h ^ (h >> 13)) % 25) as i32 - 12
}
#[test]
fn guided_texture_preserves_bands_and_avoids_remote_panel() {
    for vertical in [false, true] {
        let src = RgbaImage::from_fn(192, 160, |x, y| {
            if y >= 138 {
                return if (x / 7 + y / 5) % 2 == 0 {
                    Rgba([250, 240, 15, 255])
                } else {
                    Rgba([5, 5, 5, 255])
                };
            }
            let coordinate = if vertical { x } else { y };
            let base = if coordinate < 60 {
                [55, 80, 110]
            } else if coordinate < 90 {
                [205, 65, 90]
            } else {
                [65, 150, 40]
            };
            Rgba(
                [0, 1, 2]
                    .map(|c| (base[c] + noise(x, y)).clamp(0, 255) as u8)
                    .into_iter()
                    .chain([255])
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap(),
            )
        });
        let mask = mask(192, 160, 62, 26, 68, 94);
        let out = erase(&src, &mask);
        exact_known(&src, &out, &mask);
        let mut wrong = 0;
        let mut total = 0;
        for y in 26_u32..120 {
            for x in 62..130 {
                let coordinate = if vertical { x } else { y };
                if coordinate.abs_diff(60) < 4 || coordinate.abs_diff(90) < 4 {
                    continue;
                }
                let a = src.get_pixel(x, y);
                let b = out.get_pixel(x, y);
                let error = (0..3).map(|c| a[c].abs_diff(b[c]) as u32).sum::<u32>();
                wrong += usize::from(error > 90);
                total += 1;
            }
        }
        assert!(
            (wrong as f64 / total as f64) < 0.02,
            "bands vertical={vertical}, wrong={wrong}/{total}"
        );
    }
}
#[test]
fn diagonal_edges_and_texture_diversity() {
    let src = RgbaImage::from_fn(144, 144, |x, y| {
        let n = (((x * 17 + y * 31) ^ (x * y)) % 17) as i32 - 8;
        let b = if x + y < 144 {
            [65, 100, 170]
        } else {
            [170, 150, 50]
        };
        Rgba([(b[0] + n) as u8, (b[1] + n) as u8, (b[2] + n) as u8, 255])
    });
    let m = mask(144, 144, 42, 40, 60, 64);
    let out = erase(&src, &m);
    exact_known(&src, &out, &m);
    let mut wrong = 0;
    let mut total = 0;
    for y in 40_u32..104 {
        for x in 42..102 {
            if (x + y).abs_diff(144) < 8 {
                continue;
            }
            wrong += usize::from(src.get_pixel(x, y)[0].abs_diff(out.get_pixel(x, y)[0]) > 40);
            total += 1;
        }
    }
    assert!(
        (wrong as f64 / total as f64) < 0.02,
        "diagonal {wrong}/{total}"
    );
    let mut random = 0x1845a233_u32;
    let src = RgbaImage::from_fn(128, 112, |_, _| {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        let n = (random % 65) as i32 - 32;
        Rgba([(120 + n) as u8, (150 + n) as u8, (90 + n) as u8, 255])
    });
    let m = mask(128, 112, 44, 36, 40, 40);
    let out = erase(&src, &m);
    let values: Vec<_> = out
        .pixels()
        .zip(&m)
        .filter_map(|(p, m)| (*m > 0).then_some(f64::from(p[0])))
        .collect();
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
    assert!(variance > 40.0, "texture variance {variance}");
}
#[test]
fn cancellation_gaps_and_missing_donors() {
    let src = RgbaImage::from_pixel(32, 32, Rgba([70, 130, 190, 255]));
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(matches!(
        reconstruct(&src, &vec![255; 1024], &cancel),
        Err(CanvasError::Cancelled)
    ));
    assert!(reconstruct(&src, &vec![255; 1024], &Default::default()).is_err());
    let mut src = src;
    for y in 0..32 {
        src.put_pixel(20, y, Rgba([255, 0, 0, 0]));
    }
    let m = mask(32, 32, 5, 5, 18, 16);
    let out = erase(&src, &m);
    exact_known(&src, &out, &m);
}

#[test]
fn reconstruction_budget_and_mid_operation_cancellation_are_enforced() {
    let source = RgbaImage::from_fn(512, 512, |x, y| {
        Rgba([((x * 17 + y * 13) % 251) as u8, 120, 80, 255])
    });
    let m = mask(512, 512, 128, 128, 256, 256);
    assert!(matches!(
        reconstruct(&source, &m, &Cancellation::after_checks(600)),
        Err(CanvasError::Cancelled)
    ));
    let source = RgbaImage::from_pixel(2048, 2048, Rgba([80, 120, 160, 255]));
    let m = mask(2048, 2048, 100, 100, 1800, 1800);
    assert!(matches!(
        reconstruct(&source, &m, &Default::default()),
        Err(CanvasError::ResourceLimit)
    ));
}

#[test]
fn profiling_identifies_periodic_and_multiscale_work_without_changing_results() {
    for periodic in [true, false] {
        let source = RgbaImage::from_fn(96, 80, |x, y| {
            let n = if periodic {
                ((x / 4 + y / 4) % 2 * 24) as i32
            } else {
                noise(x, y) * 2
            };
            Rgba([(120 + n) as u8, (150 + n) as u8, (90 + n) as u8, 255])
        });
        let mask = mask(96, 80, 36, 28, 24, 24);
        let expected = reconstruct(&source, &mask, &Default::default()).unwrap();
        let mut profile = profile::SmartEraseProfile::default();
        let actual = reconstruct_profiled(
            &source,
            &mask,
            None,
            &Default::default(),
            &mut Profiler {
                data: Some(&mut profile),
            },
        )
        .unwrap();
        assert_eq!(actual, expected);
        exact_known(&source, &actual, &mask);
        assert_eq!(profile.masked_pixels, 24 * 24);
        assert_eq!(
            profile.solver,
            Some(if periodic {
                SmartEraseSolver::Periodic
            } else {
                SmartEraseSolver::Multiscale
            })
        );
        if periodic {
            assert!(profile.levels.is_empty());
        } else {
            assert!(!profile.levels.is_empty());
            let fine = profile.levels.last().unwrap();
            assert_eq!(fine.size, [96, 80]);
            assert_eq!(fine.masked_pixels, 24 * 24);
            assert!(fine.donor_patches > 0);
            assert!(fine.passes >= 2);
        }
    }
}

#[test]
fn profiling_counts_only_donors_inside_the_working_grid() {
    let source = RgbaImage::from_pixel(2048, 32, Rgba([70, 120, 180, 255]));
    let mask = mask(2048, 32, 8, 8, 16, 16);
    let mut profile = profile::SmartEraseProfile::default();
    reconstruct_profiled(
        &source,
        &mask,
        None,
        &Default::default(),
        &mut Profiler {
            data: Some(&mut profile),
        },
    )
    .unwrap();
    assert!(profile.working_pixels < 2048 * 32);
    assert_eq!(
        profile.donor_pixels + profile.masked_pixels,
        profile.working_pixels
    );
}
