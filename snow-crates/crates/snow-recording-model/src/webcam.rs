//! Portable webcam styling. Coordinates follow the other fixed-pixel recording effects.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebcamShape {
    #[default]
    Square,
    Circle,
}

/// Clockwise quarter-turns in bits 0..1, horizontal reflection before rotation in bit 2.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebcamOrientation {
    #[default]
    Identity,
    Clockwise90,
    Clockwise180,
    Clockwise270,
    Mirrored,
    MirroredClockwise90,
    MirroredClockwise180,
    MirroredClockwise270,
}

impl WebcamOrientation {
    pub fn from_raw(value: u32) -> Result<Self, String> {
        match value {
            0 => Ok(Self::Identity),
            1 => Ok(Self::Clockwise90),
            2 => Ok(Self::Clockwise180),
            3 => Ok(Self::Clockwise270),
            4 => Ok(Self::Mirrored),
            5 => Ok(Self::MirroredClockwise90),
            6 => Ok(Self::MirroredClockwise180),
            7 => Ok(Self::MirroredClockwise270),
            _ => Err("invalid webcam orientation".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebcamStyle {
    pub size: u32,
    pub shape: WebcamShape,
    /// Preferred distances from the recording region's left and bottom edges.
    pub offset_left: u32,
    pub offset_bottom: u32,
    pub orientation: WebcamOrientation,
}

impl Default for WebcamStyle {
    fn default() -> Self {
        Self {
            size: 150,
            shape: WebcamShape::Square,
            offset_left: 48,
            offset_bottom: 48,
            orientation: WebcamOrientation::Identity,
        }
    }
}

impl WebcamStyle {
    pub fn validate(self) -> Result<(), String> {
        if !(50..=500).contains(&self.size) {
            return Err("webcam size must be between 50 and 500 pixels".into());
        }
        Ok(())
    }

    /// Reduce the inset before reducing the image, retaining the complete square/circle.
    pub fn placement(self, output: (u32, u32)) -> (u32, u32, u32) {
        let side = self.size.min(output.0).min(output.1);
        let x = self.offset_left.min(output.0.saturating_sub(side));
        let bottom = self.offset_bottom.min(output.1.saturating_sub(side));
        (x, output.1.saturating_sub(bottom + side), side)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebcamMetadata {
    pub style: WebcamStyle,
    pub fps: u32,
    pub duration_ms: u64,
    /// State changes use source time, so trimming and speed changes preserve unavailable gaps.
    pub visibility: Vec<(u64, bool)>,
}
impl WebcamMetadata {
    pub fn validate(&self) -> Result<(), String> {
        self.style.validate()?;
        if !(1..=30).contains(&self.fps)
            || self.duration_ms == 0
            || self.visibility.len() > 100_000
            || self
                .visibility
                .iter()
                .any(|(at, _)| *at >= self.duration_ms)
            || self.visibility.windows(2).any(|v| v[0].0 >= v[1].0)
        {
            return Err("invalid webcam timeline".into());
        }
        Ok(())
    }
    pub fn visible_at(&self, at: u64) -> bool {
        let count = self.visibility.partition_point(|(time, _)| *time <= at);
        count != 0 && self.visibility[count - 1].1
    }
}

const MAGIC: &[u8; 9] = b"SNOWCAM\0\x03";

// Bincode is positional: decode the original layout explicitly instead of adding
// serde defaults, which cannot distinguish new fields from the old fps/timeline.
#[derive(Serialize, Deserialize)]
struct WebcamStyleV1 {
    size: u32,
    shape: WebcamShape,
}
#[derive(Serialize, Deserialize)]
struct WebcamMetadataV1 {
    style: WebcamStyleV1,
    fps: u32,
    duration_ms: u64,
    visibility: Vec<(u64, bool)>,
}
#[derive(Serialize, Deserialize)]
struct WebcamStyleV2 {
    size: u32,
    shape: WebcamShape,
    offset_left: u32,
    offset_bottom: u32,
}
#[derive(Serialize, Deserialize)]
struct WebcamMetadataV2 {
    style: WebcamStyleV2,
    fps: u32,
    duration_ms: u64,
    visibility: Vec<(u64, bool)>,
}
pub fn write_metadata(path: &std::path::Path, metadata: &WebcamMetadata) -> crate::Result<()> {
    use std::io::Write;
    metadata
        .validate()
        .map_err(crate::RecordingModelError::Decode)?;
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    file.write_all(MAGIC)?;
    bincode::serialize_into(&mut file, metadata)
        .map_err(|e| crate::RecordingModelError::Decode(e.to_string()))?;
    file.flush()?;
    Ok(())
}
pub fn read_metadata(mut input: impl std::io::Read) -> crate::Result<WebcamMetadata> {
    use bincode::Options;
    let mut magic = [0; MAGIC.len()];
    input.read_exact(&mut magic)?;
    if magic[..8] != MAGIC[..8] || !matches!(magic[8], 1..=3) {
        return Err(crate::RecordingModelError::Decode(
            "unsupported webcam metadata".into(),
        ));
    }
    let options = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(4 * 1024 * 1024);
    let metadata: WebcamMetadata = if magic[8] == 1 {
        let old: WebcamMetadataV1 = options
            .deserialize_from(input)
            .map_err(|e| crate::RecordingModelError::Decode(e.to_string()))?;
        WebcamMetadata {
            style: WebcamStyle {
                size: old.style.size,
                shape: old.style.shape,
                ..Default::default()
            },
            fps: old.fps,
            duration_ms: old.duration_ms,
            visibility: old.visibility,
        }
    } else if magic[8] == 2 {
        let old: WebcamMetadataV2 = options
            .deserialize_from(input)
            .map_err(|e| crate::RecordingModelError::Decode(e.to_string()))?;
        WebcamMetadata {
            style: WebcamStyle {
                size: old.style.size,
                shape: old.style.shape,
                offset_left: old.style.offset_left,
                offset_bottom: old.style.offset_bottom,
                orientation: WebcamOrientation::Identity,
            },
            fps: old.fps,
            duration_ms: old.duration_ms,
            visibility: old.visibility,
        }
    } else {
        options
            .deserialize_from(input)
            .map_err(|e| crate::RecordingModelError::Decode(e.to_string()))?
    };
    metadata
        .validate()
        .map_err(crate::RecordingModelError::Decode)?;
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn webcam_orientation_rejects_unknown_values() {
        for raw in 0..8 {
            assert_eq!(WebcamOrientation::from_raw(raw).unwrap() as u32, raw);
        }
        for raw in [8, 9, 255, u32::MAX] {
            assert!(WebcamOrientation::from_raw(raw).is_err());
        }
    }
    #[test]
    fn webcam_offsets_fit_without_changing_the_preferred_layout() {
        let style = WebcamStyle {
            offset_left: 600,
            offset_bottom: 400,
            ..Default::default()
        };
        assert_eq!(style.placement((1000, 800)), (600, 250, 150));
        assert_eq!(style.placement((700, 500)), (550, 0, 150));
        assert_eq!(style.placement((100, 80)), (20, 0, 80));
        assert_eq!(style.placement((1000, 800)), (600, 250, 150));
        for output in [(0, 0), (1, 1), (160, 170), (800, 600)] {
            for offset in [0, 48, 900, u32::MAX] {
                let style = WebcamStyle {
                    offset_left: offset,
                    offset_bottom: offset,
                    ..style
                };
                let (x, y, side) = style.placement(output);
                assert!(x + side <= output.0 && y + side <= output.1);
            }
        }
    }

    #[test]
    fn webcam_metadata_reads_v1_v2_and_round_trips_v3_orientation() {
        let old = WebcamMetadataV1 {
            style: WebcamStyleV1 {
                size: 220,
                shape: WebcamShape::Circle,
            },
            fps: 30,
            duration_ms: 1000,
            visibility: vec![(0, true)],
        };
        let mut bytes = b"SNOWCAM\0\x01".to_vec();
        bincode::serialize_into(&mut bytes, &old).unwrap();
        let mut metadata = read_metadata(bytes.as_slice()).unwrap();
        assert_eq!(
            (metadata.style.offset_left, metadata.style.offset_bottom),
            (48, 48)
        );
        assert_eq!(metadata.style.size, 220);
        assert_eq!(metadata.style.orientation, WebcamOrientation::Identity);
        let old_v2 = WebcamMetadataV2 {
            style: WebcamStyleV2 {
                size: 220,
                shape: WebcamShape::Circle,
                offset_left: 370,
                offset_bottom: 240,
            },
            fps: 30,
            duration_ms: 1000,
            visibility: vec![(0, true)],
        };
        let mut v2 = b"SNOWCAM\0\x02".to_vec();
        bincode::serialize_into(&mut v2, &old_v2).unwrap();
        let decoded = read_metadata(v2.as_slice()).unwrap();
        assert_eq!(decoded.style.orientation, WebcamOrientation::Identity);
        assert_eq!(
            (decoded.style.offset_left, decoded.style.offset_bottom),
            (370, 240)
        );
        metadata.style.offset_left = 370;
        metadata.style.offset_bottom = 240;
        metadata.style.orientation = WebcamOrientation::MirroredClockwise270;
        let mut current = MAGIC.to_vec();
        bincode::serialize_into(&mut current, &metadata).unwrap();
        let decoded = read_metadata(current.as_slice()).unwrap();
        assert_eq!(decoded.style, metadata.style);
        let mut invalid = current.clone();
        invalid[25..29].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_metadata(invalid.as_slice()).is_err());
        for bytes in [&bytes, &v2, &current] {
            for len in 0..bytes.len() {
                assert!(read_metadata(&bytes[..len]).is_err());
            }
        }
    }
    #[test]
    fn webcam_metadata_is_versioned_validated_and_preserves_visibility_boundaries() {
        let style = WebcamStyle::default();
        assert_eq!(
            style,
            WebcamStyle {
                size: 150,
                shape: WebcamShape::Square,
                offset_left: 48,
                offset_bottom: 48,
                orientation: WebcamOrientation::Identity,
            }
        );
        assert_eq!(style.placement((800, 600)), (48, 402, 150));
        assert_eq!(style.placement((160, 170)), (10, 0, 150));
        assert_eq!(style.placement((20, 30)), (0, 0, 20));
        let mut metadata = WebcamMetadata {
            style,
            fps: 30,
            duration_ms: 1000,
            visibility: vec![(100, true), (400, false), (800, true)],
        };
        let mut bytes = MAGIC.to_vec();
        bincode::serialize_into(&mut bytes, &metadata).unwrap();
        let restored = read_metadata(bytes.as_slice()).unwrap();
        for (at, expected) in [
            (0, false),
            (99, false),
            (100, true),
            (399, true),
            (400, false),
            (799, false),
            (800, true),
        ] {
            assert_eq!(restored.visible_at(at), expected);
        }
        bytes[8] = 4;
        assert!(read_metadata(bytes.as_slice()).is_err());
        metadata.visibility.push((800, false));
        assert!(metadata.validate().is_err());
        metadata.visibility = vec![(1000, true)];
        assert!(metadata.validate().is_err());
        metadata.visibility.clear();
        metadata.style.size = 501;
        assert!(metadata.validate().is_err());
    }
}
