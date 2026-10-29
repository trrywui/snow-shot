mod connection_overlay;
mod element;
mod host_controls;
mod overlay_geometry;
mod selection_guides;
mod selection_input;
#[cfg(test)]
mod selection_input_tests;
mod selection_overlay;
pub use host_controls::{
    CanvasCommandKeyResolver, CanvasCrosshair, CanvasCrosshairFactory, CanvasCursorLayer,
};
#[cfg(target_os = "windows")]
mod smart_erase;
#[cfg(test)]
mod tests;
mod text_caret;
mod text_input;
mod text_overlay;

use std::sync::Arc;

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, KeyDownEvent, KeyUpEvent,
    LayoutId, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder,
    Pixels, Point, ScrollDelta, ScrollEvent, SharedString, Style, Subscription, Task,
    UTF16Selection, Window, div, fill, point, prelude::*, px, relative, rgb, rgba,
};
use snow_draw_engine as draw;
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    CanvasDocument, CanvasError, CanvasSession, SourceImage,
    text_buffer::TextBuffer,
    vector::{TextLayout, VectorContext},
};

/// Semantic notifications for a containing toolbar or future Snow Shot shell.
pub enum CanvasEvent {
    /// A primary double-click which neither the editor nor text input consumed.
    UnhandledDoubleClick {
        position: Point<Pixels>,
    },
    Changed,
    Error(SharedString),
    Rendered,
}

struct TextDraft {
    buffer: TextBuffer,
    target: draw::TextCommitTarget,
    center: draw::Point<f64>,
    rotation: f64,
    style: draw::TextStyle,
    width: f64,
    auto_resize: bool,
    layout: TextLayout,
    changed: bool,
    minimum_grapheme_width: f64,
    ink_reserve: f64,
    preferred_x: Option<f64>,
    caret_affinity: Option<(usize, usize, f64)>,
}

impl Drop for CanvasView {
    fn drop(&mut self) {
        self.smart_cancel.cancel();
    }
}

#[derive(Clone, Copy, PartialEq)]
struct ViewRenderKey {
    scene: draw::SceneRevision,
    decoration: draw::DecorationRevision,
    frame: draw::FrameView,
    resources: u64,
    scale: u32,
    presentation: u64,
}

/// Retained native canvas. Domain state, transient viewport state and render caches have
/// distinct owners. Display composition is coalesced at frame preparation so pixels and
/// editor overlays always consume the same scene. Export workers own separate renderers.
pub struct CanvasView {
    command_key_resolver: Option<CanvasCommandKeyResolver>,
    cursor_layers: [Option<CursorStyle>; 2],
    crosshair_factory: Option<CanvasCrosshairFactory>,
    crosshair_cache: Option<(u32, Option<CanvasCrosshair>)>,
    show_dirty_regions: bool,
    clear_renderer_pending: bool,
    options: crate::CanvasViewOptions,
    text_style_popup_depth: u32,
    document: CanvasDocument,
    session: CanvasSession,
    logical_frame_ready: bool,
    logical_failure: bool,
    focus: FocusHandle,
    bounds: Bounds<Pixels>,
    scale: f32,
    #[cfg(target_os = "windows")]
    gpu_canvas: Option<crate::GpuCanvas>,
    #[cfg(target_os = "windows")]
    gpu_smart_erase: Option<crate::GpuSmartErase>,
    #[cfg(target_os = "windows")]
    gpu_donors: Option<crate::GpuDonorAssembler>,
    #[cfg(target_os = "windows")]
    gpu_frame: Option<crate::GpuCanvasFrame>,
    image_frame: Option<draw::FrameView>,
    image_key: Option<ViewRenderKey>,
    image_scene: Option<crate::RetainedScene>,
    image_presentation: Option<crate::CanvasPresentation>,
    smart_service: crate::SmartEraseService,
    metrics: VectorContext,
    clipboard_task: Option<Task<()>>,
    gpu_wait_task: Option<Task<()>>,
    smart_task: Option<Task<()>>,
    smart_cancel: crate::CanvasCancellation,
    smart_requested_generation: Option<u64>,
    smart_warning: Option<String>,
    generation: u64,
    rendered_generation: u64,
    requested_key: Option<ViewRenderKey>,
    draft: Option<TextDraft>,
    caret_visible: bool,
    caret_blink_task: Option<Task<()>>,
    text_selection_overlay: text_overlay::TextSelectionOverlay,
    selecting_text: bool,
    pan: Option<(Point<Pixels>, draw::Camera)>,
    space_down: bool,
    space_key: Option<String>,
    cursor: CursorStyle,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CanvasEvent> for CanvasView {}
impl Focusable for CanvasView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CanvasView {
    pub fn new(
        mut document: CanvasDocument,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self, CanvasError> {
        let mut session = CanvasSession::new(&mut document)?;
        session.set_deferred_composition(true);
        let focus = cx.focus_handle().tab_stop(true);
        let subscriptions = vec![
            cx.on_release_in(window, |this, window, cx| {
                this.smart_cancel.cancel();
                this.caret_blink_task = None;
                this.text_selection_overlay.clear_published(window);
                #[cfg(target_os = "windows")]
                {
                    this.gpu_frame = None;
                    this.gpu_canvas = None;
                }
                if this.selecting_text || this.document.engine().has_pointer_capture() {
                    window.release_pointer();
                }
                crate::cleanup::retire_send_resources(
                    (
                        std::mem::take(&mut this.smart_service),
                        this.draft.take().map(|draft| draft.layout),
                    ),
                    cx,
                );
                crate::retire_canvas_document(std::mem::take(&mut this.document), cx);
            }),
            cx.on_blur(&focus, window, |this, window, cx| {
                this.stop_text_caret(cx);
                if this.text_style_popup_depth == 0 {
                    this.finish_text(true, window, cx);
                }
                this.cancel_gesture(window, cx);
            }),
            cx.on_focus(&focus, window, |this, window, cx| {
                this.reset_text_caret(window, cx);
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.stop_text_caret(cx);
                    this.cancel_gesture(window, cx);
                } else {
                    this.reset_text_caret(window, cx);
                }
            }),
        ];
        Ok(Self {
            command_key_resolver: None,
            cursor_layers: [None, None],
            crosshair_factory: None,
            crosshair_cache: None,
            show_dirty_regions: false,
            clear_renderer_pending: false,
            options: Default::default(),
            text_style_popup_depth: 0,
            document,
            session,
            logical_frame_ready: false,
            logical_failure: false,
            focus,
            bounds: Bounds::default(),
            scale: window.scale_factor(),
            #[cfg(target_os = "windows")]
            gpu_canvas: None,
            #[cfg(target_os = "windows")]
            gpu_smart_erase: None,
            #[cfg(target_os = "windows")]
            gpu_donors: None,
            #[cfg(target_os = "windows")]
            gpu_frame: None,
            image_frame: None,
            image_key: None,
            image_scene: None,
            image_presentation: None,
            smart_service: crate::SmartEraseService::default(),
            metrics: VectorContext::default(),
            clipboard_task: None,
            gpu_wait_task: None,
            smart_task: None,
            smart_cancel: Default::default(),
            smart_requested_generation: None,
            smart_warning: None,
            generation: 0,
            rendered_generation: 0,
            requested_key: None,
            draft: None,
            caret_visible: true,
            caret_blink_task: None,
            text_selection_overlay: Default::default(),
            selecting_text: false,
            pan: None,
            space_down: false,
            space_key: None,
            cursor: CursorStyle::Crosshair,
            error: None,
            _subscriptions: subscriptions,
        })
    }

    pub fn document(&self) -> &CanvasDocument {
        &self.document
    }

    pub fn options(&self) -> &crate::CanvasViewOptions {
        &self.options
    }
    pub fn set_options(
        &mut self,
        options: crate::CanvasViewOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), CanvasError> {
        options.presentation.validate()?;
        if let Some(area) = options.text_editing_bounds {
            area.validate()?;
        }
        if !options.interaction_enabled {
            self.finish_text(true, window, cx);
            self.cancel_gesture(window, cx);
        }
        self.options = options;
        self.invalidate_render();
        if self.draft.is_some() {
            self.update_draft(window, cx);
        } else {
            self.changed(window, cx);
        }
        Ok(())
    }
    pub fn begin_text_style_popup_interaction(&mut self) {
        self.text_style_popup_depth = self.text_style_popup_depth.saturating_add(1);
    }
    pub fn end_text_style_popup_interaction(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text_style_popup_depth = self.text_style_popup_depth.saturating_sub(1);
        if self.text_style_popup_depth == 0 && self.draft.is_some() {
            self.focus.focus(window, cx);
        }
    }
    pub fn set_active_text_style(
        &mut self,
        style: draw::TextStyle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), CanvasError> {
        self.document.command(|engine| {
            engine.set_viewport_text_creation_style(
                self.session.viewport(),
                style.clone(),
                snow_draw_engine_editor::TEXT_STYLE_ALL_PROPERTIES,
            )
        })?;
        if let Some(draft) = &mut self.draft {
            draft.changed |= draft.style != style;
            draft.style = style;
            self.update_draft(window, cx);
        } else {
            self.changed(window, cx);
        }
        Ok(())
    }
    pub fn session(&self) -> &CanvasSession {
        &self.session
    }
    /// Latest logical content bounds, useful to coordinate a containing native host.
    pub fn surface_bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }
    pub fn snapshot(&self) -> Result<crate::CanvasSnapshot, CanvasError> {
        crate::CanvasSnapshot::capture(&self.document, &self.session)?
            .with_presentation(self.options.presentation.clone())
    }
    pub fn smart_erase_snapshot(&self) -> crate::SmartEraseSnapshot {
        self.smart_service.snapshot()
    }
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }
    pub fn is_rendering(&self) -> bool {
        self.smart_task.is_some()
            || (!self.logical_failure
                && (!self.logical_frame_ready || self.rendered_generation != self.generation))
    }
    #[doc(hidden)]
    pub fn render_diagnostics(&self) -> String {
        format!(
            "generation={} rendered={} pending={} bounds={:?} frame={:?} error={:?}",
            self.generation,
            self.rendered_generation,
            self.is_rendering(),
            self.bounds,
            self.session.scene().frame(),
            self.error
        )
    }
    pub fn has_text_draft(&self) -> bool {
        self.draft.is_some()
    }

    /// Copies a strict, overlay-free PNG of the current viewport asynchronously.
    pub fn copy_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.clipboard_task.is_some() {
            return;
        }
        self.finish_text(true, window, cx);
        self.cancel_gesture(window, cx);
        let snapshot = match self.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let scale = window.scale_factor();
        let smart_erase = self.smart_service.snapshot();
        let mut job = match cx
            .gpu_service()
            .map_err(|error| CanvasError::Gpu(error.to_string()))
            .and_then(|service| {
                crate::GpuExportJob::bytes(
                    service,
                    snapshot,
                    scale,
                    smart_erase,
                    crate::GpuExportFormat::Png,
                    Default::default(),
                )
            }) {
            Ok(job) => job,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.clipboard_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = loop {
                if let std::task::Poll::Ready(result) = job.poll() {
                    break result;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(2))
                    .await;
            };
            let _ = this.update_in(cx, |this, _, cx| {
                this.clipboard_task = None;
                match result {
                    Ok(bytes) => cx.write_to_clipboard(ClipboardItem::new_image(
                        &gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes),
                    )),
                    Err(error) => this.fail(error, cx),
                }
            });
        }));
    }

    pub fn set_tool(
        &mut self,
        tool: draw::ActiveTool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_text(true, window, cx);
        self.cancel_gesture(window, cx);
        match self.session.set_tool(&mut self.document, tool) {
            Ok(_) => {
                self.cursor = if tool == draw::ActiveTool::Select {
                    CursorStyle::Arrow
                } else if tool == draw::ActiveTool::Text {
                    CursorStyle::IBeam
                } else {
                    CursorStyle::Crosshair
                };
                self.changed(window, cx);
            }
            Err(error) => self.fail(error, cx),
        }
        self.focus.focus(window, cx);
    }

    pub fn command(
        &mut self,
        command: impl FnOnce(
            &mut draw::Engine,
            draw::ViewportId,
        ) -> Result<draw::MutationResult, draw::ErrorCode>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_text(true, window, cx);
        self.cancel_gesture(window, cx);
        match self.session.mutate(&mut self.document, command) {
            Ok(_) => self.changed(window, cx),
            Err(error) => self.fail(error, cx),
        }
    }

    /// Apply a nonstructural tool-style command during pointer input. Unlike
    /// `command`, this preserves the active gesture and never commits text.
    /// Hosts must use `command` for document, history or selection operations.
    pub fn update_tool_style(
        &mut self,
        command: impl FnOnce(
            &mut draw::Engine,
            draw::ViewportId,
        ) -> Result<draw::MutationResult, draw::ErrorCode>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.options.interaction_enabled || self.draft.is_some() {
            return false;
        }
        match self.session.mutate(&mut self.document, command) {
            Ok(_) => {
                self.changed(window, cx);
                true
            }
            Err(error) => {
                self.fail(error, cx);
                false
            }
        }
    }

    pub fn replace_document(
        &mut self,
        mut document: CanvasDocument,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), CanvasError> {
        self.cancel_gesture(window, cx);
        let mut session = CanvasSession::new(&mut document)?;
        session.set_deferred_composition(true);
        if self.bounds.size.width > px(0.0) && self.bounds.size.height > px(0.0) {
            session.set_surface(
                &mut document,
                f32::from(self.bounds.size.width).round().max(1.0) as u32,
                f32::from(self.bounds.size.height).round().max(1.0) as u32,
            )?;
        }
        self.invalidate_render();
        let previous_document = std::mem::replace(&mut self.document, document);
        crate::retire_canvas_document(previous_document, cx);
        #[cfg(target_os = "windows")]
        {
            self.gpu_frame = None;
            self.gpu_canvas = None;
            if let Some(donors) = &mut self.gpu_donors {
                donors.clear_cache();
            }
        }
        self.image_frame = None;
        self.image_key = None;
        self.image_scene = None;
        self.image_presentation = None;
        self.session = session;
        self.draft = None;
        self.requested_key = None;
        self.error = None;
        self.fit(window, cx);
        Ok(())
    }

    pub fn set_background(
        &mut self,
        image: SourceImage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_text(true, window, cx);
        self.cancel_gesture(window, cx);
        if let Err(error) = self.document.replace_background(Some(image)) {
            self.fail(error, cx);
            return;
        }
        self.invalidate_render();
        self.fit(window, cx);
    }

    pub fn fit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let surface = self.session.scene().frame().surface;
        let (center, width, height) = if let Some(image) = self.document.background() {
            let width = f64::from(image.width()) * image.units_per_pixel();
            let height = f64::from(image.height()) * image.units_per_pixel();
            (
                draw::Point::new(
                    image.origin().x + width / 2.0,
                    image.origin().y + height / 2.0,
                ),
                width,
                height,
            )
        } else {
            (draw::Point::new(480.0, 320.0), 960.0, 640.0)
        };
        let camera = draw::Camera {
            center,
            zoom: (f64::from(surface.width) / width * 0.92)
                .min(f64::from(surface.height) / height * 0.92)
                .clamp(1.0e-6, 8.0),
        };
        if let Err(error) = self.session.set_camera(&mut self.document, camera) {
            self.fail(error, cx);
        }
        self.changed(window, cx);
    }

    pub fn zoom(&mut self, factor: f64, window: &mut Window, cx: &mut Context<Self>) {
        let mut camera = self.session.scene().frame().camera;
        camera.zoom = (camera.zoom * factor).clamp(1.0e-6, 8.0);
        if let Err(error) = self.session.set_camera(&mut self.document, camera) {
            self.fail(error, cx);
        }
        self.changed(window, cx);
    }

    fn fail(&mut self, error: impl std::fmt::Display, cx: &mut Context<Self>) {
        let error: SharedString = error.to_string().into();
        self.error = Some(error.clone());
        cx.emit(CanvasEvent::Error(error));
        cx.notify();
    }

    fn changed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.logical_frame_ready = false;
        self.logical_failure = false;
        cx.emit(CanvasEvent::Changed);
        cx.notify();
    }

    fn prepare_logical_frame(&mut self) -> Result<(), CanvasError> {
        if self.logical_frame_ready {
            return Ok(());
        }
        self.sync_text_measurements()?;
        self.session.flush(&self.document)?;
        self.logical_frame_ready = true;
        let cursor = self.session.scene().cursor().unwrap_or_default();
        let key = ViewRenderKey {
            scene: cursor.scene_revision,
            decoration: cursor.decoration_revision,
            frame: self.session.scene().frame(),
            resources: self.document.resource_revision(),
            scale: self.scale.to_bits(),
            presentation: self.options.presentation.cache_key(),
        };
        if self.requested_key != Some(key) {
            self.requested_key = Some(key);
            self.generation = self.generation.wrapping_add(1);
        }
        Ok(())
    }

    fn invalidate_render(&mut self) {
        self.smart_cancel.cancel();
        self.smart_warning = None;
        self.generation = self.generation.wrapping_add(1);
        self.requested_key = None;
    }

    fn prepare_display_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "windows")]
        if let Ok(service) = window.gpu_service()
            && self.gpu_frame.as_ref().is_some_and(|frame| {
                frame.tiles.iter().any(|tile| {
                    tile.texture.service_id() != service.service_id()
                        || tile.texture.epoch() != service.epoch()
                })
            })
        {
            self.gpu_canvas = None;
            self.rendered_generation = self.generation.wrapping_sub(1);
        }
        if self.logical_failure {
            return;
        }
        if let Err(error) = self.prepare_logical_frame() {
            self.logical_failure = true;
            self.fail(error, cx);
            return;
        }
        #[cfg(target_os = "windows")]
        if self
            .gpu_canvas
            .as_mut()
            .is_some_and(|canvas| matches!(canvas.is_ready(), Ok(false)))
            || matches!(self.text_selection_overlay.is_ready(), Ok(false))
        {
            if self.gpu_wait_task.is_none() {
                // Completion events only schedule another readiness check. An
                // unavailable wake retains timer polling; the renderer itself
                // still reports device/retirement failures on the next check.
                #[cfg(target_os = "windows")]
                let wake = self
                    .gpu_canvas
                    .as_mut()
                    .and_then(|canvas| canvas.completion_wake().ok().flatten());
                self.gpu_wait_task = Some(cx.spawn_in(window, async move |this, cx| {
                    #[cfg(target_os = "windows")]
                    if let Some(wake) = wake {
                        wake.await;
                    } else {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(1))
                            .await;
                    }
                    #[cfg(not(target_os = "windows"))]
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1))
                        .await;
                    let _ = this.update_in(cx, |this, _, cx| {
                        this.gpu_wait_task = None;
                        cx.notify();
                    });
                }));
            }
            return;
        }
        if self.logical_frame_ready
            && self.rendered_generation == self.generation
            && self.image_key == self.requested_key
        {
            match text_overlay::prepare_publication(self, window) {
                Ok(publication) => self.text_selection_overlay.publish(publication),
                Err(error) => self.fail(error, cx),
            }
            return;
        }
        if !self.logical_frame_ready
            || self.rendered_generation == self.generation
            || self.bounds.size.width <= px(0.0)
            || self.bounds.size.height <= px(0.0)
        {
            return;
        }
        #[cfg(target_os = "windows")]
        let result = (|| {
            let overlay = text_overlay::prepare_publication(self, window)?;
            let service = window
                .gpu_service()
                .map_err(|e| CanvasError::Gpu(e.to_string()))?;
            if self
                .gpu_canvas
                .as_ref()
                .is_none_or(|canvas| canvas.service().service_id() != service.service_id())
            {
                self.gpu_canvas = Some(
                    crate::GpuCanvas::new(service).map_err(|e| CanvasError::Gpu(e.to_string()))?,
                );
            }
            let canvas = self.gpu_canvas.as_mut().unwrap();
            if std::mem::take(&mut self.clear_renderer_pending) {
                canvas.clear();
            }
            canvas
                .prepare(
                    self.session.scene(),
                    self.document.background(),
                    self.document.resource_revision(),
                    self.scale,
                    &self.options.presentation,
                    &self.smart_service.snapshot(),
                    &Default::default(),
                )
                .map(|frame| (frame, overlay))
                .map_err(|e| CanvasError::Gpu(e.to_string()))
        })();
        #[cfg(not(target_os = "windows"))]
        let result: Result<((), text_overlay::TextOverlayPublication), CanvasError> = Err(
            CanvasError::Unsupported("GPU canvas requires the Windows backend"),
        );
        // Preparation failures keep both content and controls from the last publication.
        self.rendered_generation = self.generation;
        match result {
            Ok((_frame, overlay)) => {
                #[cfg(target_os = "windows")]
                {
                    self.gpu_frame = Some(_frame);
                }
                self.text_selection_overlay.publish(overlay);
                self.image_frame = Some(self.session.scene().frame());
                self.image_key = self.requested_key;
                self.image_scene = Some(self.session.scene().clone());
                self.image_presentation = Some(self.options.presentation.clone());
                self.error = None;
                cx.emit(CanvasEvent::Rendered);
                self.prepare_smart_erase(window, cx);
            }
            Err(error) => self.fail(error, cx),
        }
    }

    fn display_scene(&self) -> &crate::RetainedScene {
        if self.logical_frame_ready && self.image_key == self.requested_key {
            self.session.scene()
        } else {
            // A failed preparation/composition retains one coherent completed
            // frame, including its controls, rather than mixing two revisions.
            self.image_scene.as_ref().unwrap_or(self.session.scene())
        }
    }

    fn display_presentation(&self) -> &crate::CanvasPresentation {
        self.image_presentation
            .as_ref()
            .unwrap_or(&self.options.presentation)
    }

    fn prepare_smart_erase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seed = self.smart_service.snapshot();
        #[cfg(target_os = "windows")]
        let seed = window
            .gpu_service()
            .map(|service| seed.for_device(&service))
            .unwrap_or(seed);
        #[cfg(target_os = "windows")]
        let filters = self
            .gpu_canvas
            .as_ref()
            .map_or_else(Vec::new, |canvas| canvas.smart_filters());
        #[cfg(not(target_os = "windows"))]
        let filters: Vec<draw::FilterDisplayItem> = Vec::new();
        let pending = filters.iter().any(|filter| {
            self.document
                .background()
                .is_some_and(|source| seed.patch(filter, source).is_none())
        });
        if !pending {
            return;
        }
        if self.smart_requested_generation == Some(self.generation) {
            if let Some(warning) = self.smart_warning.clone() {
                self.fail(warning, cx);
            }
            return;
        }
        if self.smart_task.is_some() {
            return;
        }
        let snapshot = match self.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let generation = self.generation;
        self.smart_requested_generation = Some(generation);
        self.smart_warning = None;
        let job = crate::smart_erase_service::SmartEraseJob::new(
            generation,
            snapshot.background,
            filters,
            seed,
        );
        let cancel = crate::CanvasCancellation::default();
        self.smart_cancel = cancel.clone();
        #[cfg(target_os = "windows")]
        if let Ok(service) = window.gpu_service()
            && service.capabilities().compute_shaders
            && self.options.smart_erase_execution != crate::SmartEraseExecution::Cpu
        {
            self.start_gpu_smart_task(window, cx, job, service, generation, cancel);
            return;
        }
        let worker_cancel = cancel.clone();
        self.smart_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    job.run(&worker_cancel)
                        .map(|output| (output.snapshot, output.warnings.into_iter().next()))
                })
                .await;
            let _ = this.update_in(cx, |this, _, cx| {
                this.smart_task = None;
                if cancel.check().is_ok() {
                    match result {
                        Ok((results, warning)) => {
                            // Results are keyed by source identity and geometry;
                            // they never replace current display pixels/overlays.
                            this.smart_service.seed(&results);
                            if generation == this.generation {
                                this.smart_warning = warning;
                            }
                        }
                        Err(error) if generation == this.generation => {
                            this.smart_warning = Some(error.to_string());
                        }
                        Err(_) => {}
                    }
                }
                // Recompose the current scene with the completed cache or start
                // the latest coalesced solve. Never publish the worker's raster.
                this.rendered_generation = this.generation.wrapping_sub(1);
                cx.notify();
            });
        }));
    }

    fn cancel_gesture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.invalidate_render();
        self.pan = None;
        self.space_down = false;
        self.space_key = None;
        self.selecting_text = false;
        window.release_pointer();
        match self
            .session
            .input(&mut self.document, draw::InputEvent::FocusLost)
        {
            Ok(_) => {}
            Err(error) => self.fail(error, cx),
        }
        self.changed(window, cx);
    }

    fn input(
        &mut self,
        event: draw::InputEvent,
        hitbox: &Hitbox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input_with_consumption(event, hitbox, window, cx);
    }

    fn input_with_consumption(
        &mut self,
        event: draw::InputEvent,
        hitbox: &Hitbox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.session.input(&mut self.document, event) {
            Ok(update) => {
                let consumed = update.interaction.consumed;
                match update.interaction.capture {
                    draw::PointerCaptureCommand::Capture(_) => window.capture_pointer(hitbox.id),
                    draw::PointerCaptureCommand::Release => window.release_pointer(),
                    _ => {}
                }
                if let draw::CursorCommand::Set(cursor) = update.interaction.cursor {
                    self.cursor = native_cursor(cursor);
                }
                self.changed(window, cx);
                consumed
            }
            Err(error) => {
                window.release_pointer();
                self.fail(error, cx);
                false
            }
        }
    }

    fn sync_text_measurements(&mut self) -> Result<(), CanvasError> {
        crate::text_layout::synchronize(
            &mut self.metrics,
            &mut self.document,
            self.session.viewport(),
        )
    }
}

impl Render for CanvasView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("snow-canvas")
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .track_focus(&self.focus)
            .key_context("SnowCanvas")
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .child(element::CanvasElement { view: cx.entity() })
    }
}

fn native_cursor(cursor: draw::CursorStyle) -> CursorStyle {
    match cursor {
        draw::CursorStyle::Text => CursorStyle::IBeam,
        draw::CursorStyle::Crosshair
        | draw::CursorStyle::CornerRadius
        | draw::CursorStyle::Stroke
        | draw::CursorStyle::Eraser => CursorStyle::Crosshair,
        draw::CursorStyle::Grab => CursorStyle::OpenHand,
        draw::CursorStyle::Grabbing | draw::CursorStyle::Move => CursorStyle::ClosedHand,
        draw::CursorStyle::ResizeHorizontal => CursorStyle::ResizeLeftRight,
        draw::CursorStyle::ResizeVertical => CursorStyle::ResizeUpDown,
        draw::CursorStyle::ResizeNwSe => CursorStyle::ResizeUpLeftDownRight,
        draw::CursorStyle::ResizeNeSw => CursorStyle::ResizeUpRightDownLeft,
        _ => CursorStyle::Arrow,
    }
}
