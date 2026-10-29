//! Immutable host rendering inputs. Custom GPUI hosts publish snapshots instead of
//! borrowing a painter callback across UI and background threads.
use crate::{CanvasError, SourceImage, engine::Point};
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl CanvasRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Result<Self, CanvasError> {
        let result = Self {
            x,
            y,
            width,
            height,
        };
        result.validate()?;
        Ok(result)
    }
    pub(crate) fn validate(self) -> Result<(), CanvasError> {
        if [
            self.x,
            self.y,
            self.width,
            self.height,
            self.x + self.width,
            self.y + self.height,
        ]
        .into_iter()
        .any(|v| !v.is_finite() || v.abs() > 1.0e9)
            || self.width < 0.0
            || self.height < 0.0
        {
            return Err(crate::engine::ErrorCode::InvalidArgument.into());
        }
        Ok(())
    }
    pub fn contains(self, point: Point<f64>) -> bool {
        point.x >= self.x
            && point.y >= self.y
            && point.x < self.x + self.width
            && point.y < self.y + self.height
    }
}

/// One immutable pristine layer, in paint order. Coverage is independent of alpha.
#[derive(Clone)]
pub struct ImageSourceLayer {
    pub image: SourceImage,
    /// Optional independent X/Y scaling, matching a host image's canvas rectangle.
    pub canvas_rect: Option<CanvasRect>,
    pub coverage: Option<CanvasRect>,
    pub smart_erase_source: bool,
}
impl ImageSourceLayer {
    pub fn new(image: SourceImage) -> Self {
        Self {
            image,
            canvas_rect: None,
            coverage: None,
            smart_erase_source: true,
        }
    }
    pub(crate) fn placement(&self) -> CanvasRect {
        self.canvas_rect.unwrap_or(CanvasRect {
            x: self.image.origin().x,
            y: self.image.origin().y,
            width: f64::from(self.image.width()) * self.image.units_per_pixel(),
            height: f64::from(self.image.height()) * self.image.units_per_pixel(),
        })
    }
}

/// Per-view host stages, deliberately outside engine history and persistence.
/// `background_override` replaces capture painting while pristine source pixels
/// remain available to restore, magnifier and reconstruction. Before/after stages
/// are source-over; transparent output composites over a GPUI-owned backdrop.
#[derive(Clone)]
pub struct CanvasPresentation {
    pub before: Option<SourceImage>,
    pub after: Option<SourceImage>,
    pub background_override: Option<SourceImage>,
    /// Host coverage for document content. Zero restores the pristine/pre-content
    /// presentation; one retains content. Placement uses canvas coordinates.
    pub content_mask: Option<SourceImage>,
    /// Source-over tint after the pristine background and before host/content.
    pub background_tint: Option<crate::engine::ColorRgba8>,
    pub content_visible: bool,
    pub clear_background: bool,
    pub content_clip: Option<CanvasRect>,
    pub watermark_area: Option<CanvasRect>,
    pub spotlight_area: Option<CanvasRect>,
}
impl Default for CanvasPresentation {
    fn default() -> Self {
        Self {
            before: None,
            after: None,
            background_override: None,
            content_mask: None,
            background_tint: None,
            content_visible: true,
            clear_background: true,
            content_clip: None,
            watermark_area: None,
            spotlight_area: None,
        }
    }
}
impl CanvasPresentation {
    pub(crate) fn validate(&self) -> Result<(), CanvasError> {
        for area in [self.content_clip, self.watermark_area, self.spotlight_area]
            .into_iter()
            .flatten()
        {
            area.validate()?;
        }
        Ok(())
    }
    pub(crate) fn cache_key(&self) -> u64 {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        (self.content_visible, self.clear_background).hash(&mut hash);
        self.background_tint
            .map(|c| (c.r, c.g, c.b, c.a))
            .hash(&mut hash);
        for area in [self.content_clip, self.watermark_area, self.spotlight_area] {
            area.is_some().hash(&mut hash);
            if let Some(r) = area {
                for v in [r.x, r.y, r.width, r.height] {
                    v.to_bits().hash(&mut hash);
                }
            }
        }
        for image in [
            &self.before,
            &self.after,
            &self.background_override,
            &self.content_mask,
        ] {
            image.is_some().hash(&mut hash);
            if let Some(image) = image {
                image.pixels_identity().hash(&mut hash);
                for v in [image.origin().x, image.origin().y, image.units_per_pixel()] {
                    v.to_bits().hash(&mut hash);
                }
            }
        }
        hash.finish()
    }
    pub(crate) fn is_default(&self) -> bool {
        self.cache_key() == Self::default().cache_key()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ExportTile {
    /// Pixel rectangle in the synchronized snapshot's output grid.
    pub source: [u32; 4],
    pub destination: [u32; 2],
}

/// Reconstruction policy for new Smart Erase work. Completed patches remain
/// reusable. CPU execution requires CPU-readable eligible donors; rendering
/// continues to use the window's GPU device in either mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SmartEraseExecution {
    /// Preserve cheap CPU models and use the qualified bounded CPU path for tiny
    /// raster masks; unresolved larger/layered inputs prefer GPU reconstruction.
    #[default]
    Automatic,
    Cpu,
    /// Bypass automatic texture crossover routing. Cheap affine/periodic CPU
    /// models remain enabled; unavailable GPU capabilities retain CPU fallback.
    GpuPreferred,
}

#[derive(Clone)]
pub struct CanvasViewOptions {
    pub presentation: CanvasPresentation,
    pub interaction_enabled: bool,
    pub wheel_zoom_enabled: bool,
    pub delete_text_after_cursor: bool,
    pub text_editing_bounds: Option<CanvasRect>,
    pub smart_erase_execution: SmartEraseExecution,
}
impl Default for CanvasViewOptions {
    fn default() -> Self {
        Self {
            presentation: Default::default(),
            interaction_enabled: true,
            wheel_zoom_enabled: true,
            delete_text_after_cursor: true,
            text_editing_bounds: None,
            smart_erase_execution: SmartEraseExecution::Automatic,
        }
    }
}
