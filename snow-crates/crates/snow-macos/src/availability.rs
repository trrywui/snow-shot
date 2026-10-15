//! ScreenCaptureKit capabilities differ from the application's deployment floor.
use objc2_foundation::NSProcessInfo;
use objc2_screen_capture_kit::SCStreamConfiguration;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CaptureAvailability {
    pub screen_capture: bool,
    pub mouse_clicks: bool,
    pub hdr: bool,
}

impl CaptureAvailability {
    pub(crate) fn from_version(major: isize, minor: isize, patch: isize) -> Self {
        let version = (major, minor, patch);
        Self {
            screen_capture: version >= (14, 4, 0),
            mouse_clicks: version >= (15, 0, 0),
            hdr: version >= (15, 0, 0),
        }
    }

    pub(crate) fn current() -> Self {
        static HOST: OnceLock<CaptureAvailability> = OnceLock::new();
        *HOST.get_or_init(|| {
            let version = NSProcessInfo::processInfo().operatingSystemVersion();
            Self::from_version(
                version.majorVersion,
                version.minorVersion,
                version.patchVersion,
            )
        })
    }

    pub(crate) fn disable_mouse_clicks(self, config: &SCStreamConfiguration) {
        // macOS 14 has no click circles. Even setting this property to false
        // sends an unavailable selector, so leave its configuration untouched.
        if self.mouse_clicks {
            unsafe { config.setShowMouseClicks(false) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_floor_does_not_enable_macos15_capture_properties() {
        assert!(!CaptureAvailability::from_version(14, 3, 9).screen_capture);
        for version in [(14, 4, 0), (14, 8, 9)] {
            let available = CaptureAvailability::from_version(version.0, version.1, version.2);
            assert!(available.screen_capture);
            assert!(!available.mouse_clicks);
            assert!(!available.hdr);
        }
        for major in [15, 26] {
            let available = CaptureAvailability::from_version(major, 0, 0);
            assert!(available.screen_capture && available.mouse_clicks && available.hdr);
        }
    }

    #[test]
    fn macos14_configuration_skips_unavailable_click_selector() {
        let config = unsafe { SCStreamConfiguration::new() };
        if CaptureAvailability::current().mouse_clicks {
            unsafe { config.setShowMouseClicks(true) };
        }
        CaptureAvailability::from_version(14, 4, 0).disable_mouse_clicks(&config);
        if CaptureAvailability::current().mouse_clicks {
            // Injected macOS 14 policy must not mutate a macOS 15 property.
            assert!(unsafe { config.showMouseClicks() });
            CaptureAvailability::from_version(15, 0, 0).disable_mouse_clicks(&config);
            assert!(!unsafe { config.showMouseClicks() });
        }
    }
}
