//! Native format policy and ownership of the device configuration lock.
use objc2::rc::Retained;
use objc2_av_foundation::AVCaptureDevice;

pub(super) trait ConfigurableDevice {
    fn lock(&self) -> Result<(), String>;
    fn unlock(&self);
}

impl ConfigurableDevice for Retained<AVCaptureDevice> {
    fn lock(&self) -> Result<(), String> {
        unsafe { self.lockForConfiguration() }.map_err(|error| error.to_string())
    }
    fn unlock(&self) {
        unsafe { self.unlockForConfiguration() };
    }
}

/// macOS may change an unlocked input's format when its session starts or another
/// application uses the device. Keep the lock until the capture session stops.
pub(super) struct ConfigurationLock<D: ConfigurableDevice>(pub D);
impl<D: ConfigurableDevice> ConfigurationLock<D> {
    pub fn new(device: D) -> Result<Self, String> {
        device.lock()?;
        Ok(Self(device))
    }
}
impl<D: ConfigurableDevice> Drop for ConfigurationLock<D> {
    fn drop(&mut self) {
        self.0.unlock();
    }
}

pub(super) struct FormatPreference {
    pub fps: f64,
    pub score: (bool, u64, bool, u32, u64),
}

/// Prefer the requested cadence, then the smallest native frame that avoids
/// upscaling. If every frame is too small, retain as much camera detail as possible.
pub(super) fn preference(
    dimensions: (i32, i32),
    size: u32,
    fps: u32,
    range: (f64, f64),
) -> Option<FormatPreference> {
    let (width, height) = dimensions;
    let (minimum, maximum) = range;
    if width <= 0
        || height <= 0
        || width > 8192
        || height > 8192
        || !minimum.is_finite()
        || !maximum.is_finite()
        || minimum <= 0.0
        || maximum < minimum
    {
        return None;
    }
    let requested = f64::from(fps);
    let rate = requested.clamp(minimum, maximum);
    let side = width.min(height) as u32;
    let undersized = side < size;
    let area = width as u64 * height as u64;
    Some(FormatPreference {
        fps: rate,
        score: (
            rate > requested,
            ((rate - requested).abs() * 1_000_000.0).round() as u64,
            undersized,
            if undersized { u32::MAX - side } else { 0 },
            area,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    struct Device(Rc<Cell<bool>>);
    impl ConfigurableDevice for Device {
        fn lock(&self) -> Result<(), String> {
            if self.0.replace(true) {
                Err("busy".into())
            } else {
                Ok(())
            }
        }
        fn unlock(&self) {
            assert!(self.0.replace(false));
        }
    }

    #[test]
    fn configuration_stays_exclusive_until_owner_retires_including_startup_failure() {
        let locked = Rc::new(Cell::new(false));
        let owner = ConfigurationLock::new(Device(locked.clone())).unwrap();
        assert!(ConfigurationLock::new(Device(locked.clone())).is_err());
        assert!(
            locked.get(),
            "failed acquisition cannot unlock another owner"
        );
        drop(owner);
        assert!(!locked.get());
        let start = || -> Result<(), String> {
            let _owner = ConfigurationLock::new(Device(locked.clone()))?;
            assert!(
                locked.get(),
                "session startup must retain the configuration"
            );
            Err("session could not start".into())
        };
        assert!(start().is_err());
        assert!(!locked.get(), "failed startup releases the configuration");
        drop(ConfigurationLock::new(Device(locked.clone())).unwrap());
        assert!(!locked.get());
    }

    #[test]
    fn native_format_policy_preserves_cadence_then_bounds_capture_and_upscaling() {
        let score = |dimensions, range| preference(dimensions, 500, 30, range).unwrap().score;
        assert!(score((640, 480), (30.0, 30.0)) < score((640, 480), (15.0, 15.0)));
        assert!(score((640, 480), (15.0, 15.0)) < score((640, 480), (60.0, 60.0)));
        assert!(score((800, 600), (30.0, 30.0)) < score((1920, 1080), (30.0, 60.0)));
        assert!(score((800, 600), (30.0, 30.0)) < score((640, 480), (30.0, 30.0)));
        assert!(score((640, 480), (30.0, 30.0)) < score((320, 240), (30.0, 30.0)));
        assert!(score((640, 480), (30.0, 30.0)) < score((1920, 200), (30.0, 30.0)));
        assert_eq!(
            preference((640, 480), 150, 1, (5.0, 30.0)).unwrap().fps,
            5.0
        );
        assert_eq!(
            preference((640, 480), 150, 30, (29.97, 29.97)).unwrap().fps,
            29.97
        );
        for dimensions in [(0, 1), (-1, 480), (8193, 480)] {
            assert!(preference(dimensions, 150, 30, (1.0, 30.0)).is_none());
        }
        for range in [(0.0, 30.0), (30.0, 15.0), (f64::NAN, 30.0)] {
            assert!(preference((640, 480), 150, 30, range).is_none());
        }
    }
}
