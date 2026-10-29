use crate::{
    CanvasDocument, CanvasError, CanvasSession, RetainedScene, SourceImage, engine as draw,
};

/// Resolve host text measurements before capturing a noninteractive snapshot.
/// Interactive views retain their own shaping cache between frames.
pub fn prepare_text_metrics(
    document: &mut CanvasDocument,
    session: &mut CanvasSession,
) -> Result<(), CanvasError> {
    crate::text_layout::synchronize(
        &mut crate::vector::VectorContext::default(),
        document,
        session.viewport(),
    )?;
    session.flush(document)?;
    Ok(())
}

/// Immutable, Send-able document export snapshot with enough lower-layer content
/// for the ordered effect pipeline. Transient selection overlays are never exported.
#[derive(Clone)]
pub struct CanvasSnapshot {
    pub(crate) scene: RetainedScene,
    pub(crate) background: Option<SourceImage>,
    pub(crate) resources: u64,
    pub(crate) viewport: draw::FrameView,
    pub(crate) presentation: crate::CanvasPresentation,
}
impl CanvasSnapshot {
    /// Capture the retained document scene without constructing another viewport.
    pub fn capture(
        document: &CanvasDocument,
        session: &CanvasSession,
    ) -> Result<Self, CanvasError> {
        let scene = session.snapshot_scene(document)?;
        Ok(Self {
            viewport: scene.frame(),
            scene,
            background: document.background().cloned(),
            resources: document.resource_revision(),
            presentation: Default::default(),
        })
    }

    pub(crate) fn source_only(source: SourceImage) -> Self {
        let [width, height] = source.dimensions();
        let origin = source.origin();
        let units = source.units_per_pixel();
        let viewport = draw::FrameView {
            surface: draw::SurfaceSize { width, height },
            camera: draw::Camera {
                zoom: 1.0 / units,
                center: draw::Point::new(
                    origin.x + f64::from(width) * units / 2.0,
                    origin.y + f64::from(height) * units / 2.0,
                ),
            },
            ..Default::default()
        };
        Self {
            scene: RetainedScene::default().with_frame(viewport),
            resources: source.pixels_identity(),
            background: Some(source),
            viewport,
            presentation: crate::CanvasPresentation {
                clear_background: false,
                ..Default::default()
            },
        }
    }
    pub fn viewport(&self) -> draw::FrameView {
        self.viewport
    }
    /// Choose an export camera without allocating an output-sized viewport.
    pub fn with_region(
        mut self,
        region: crate::CanvasRect,
        output_size: [u32; 2],
    ) -> Result<Self, CanvasError> {
        region.validate()?;
        crate::document::validate_raster(output_size[0], output_size[1])?;
        if region.width <= 0.0 || region.height <= 0.0 {
            return Err(draw::ErrorCode::InvalidArgument.into());
        }
        let zoom = f64::from(output_size[0]) / region.width;
        if !zoom.is_finite() || (region.height * zoom - f64::from(output_size[1])).abs() > 1.0 {
            return Err(draw::ErrorCode::InvalidArgument.into());
        }
        self.viewport = draw::FrameView {
            surface: draw::SurfaceSize {
                width: output_size[0],
                height: output_size[1],
            },
            camera: draw::Camera {
                center: draw::Point::new(
                    region.x + f64::from(output_size[0]) / (2.0 * zoom),
                    region.y + f64::from(output_size[1]) / (2.0 * zoom),
                ),
                zoom,
            },
            ..self.viewport
        };
        Ok(self)
    }
    pub fn with_presentation(
        mut self,
        presentation: crate::CanvasPresentation,
    ) -> Result<Self, CanvasError> {
        presentation.validate()?;
        self.presentation = presentation;
        Ok(self)
    }
}
