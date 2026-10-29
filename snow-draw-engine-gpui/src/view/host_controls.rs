use super::*;
use std::rc::Rc;

/// Resolve command-key identities only. Text/IME contents are never passed through it.
pub type CanvasCommandKeyResolver = Rc<dyn Fn(&gpui::Keystroke) -> Option<SharedString>>;
/// Host overrides take precedence over the tool override and engine semantic cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasCursorLayer {
    CanvasTool,
    Host,
}
/// Native GPUI primitives decorate the platform crosshair. GPUI0.3.9 has no custom
/// bitmap/hidden cursor API, so its native crosshair remains the center/fallback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasCrosshair {
    pub radius: f32,
    pub gap: f32,
    pub width: f32,
    pub color: Hsla,
}
impl Default for CanvasCrosshair {
    fn default() -> Self {
        Self {
            radius: 18.0,
            gap: 5.0,
            width: 1.0,
            color: rgb(0x2864a8).into(),
        }
    }
}
impl CanvasCrosshair {
    fn valid(self) -> bool {
        [
            self.radius,
            self.gap,
            self.width,
            self.color.h,
            self.color.s,
            self.color.l,
            self.color.a,
        ]
        .into_iter()
        .all(f32::is_finite)
            && (1.0..=256.0).contains(&self.radius)
            && (0.0..self.radius).contains(&self.gap)
            && (0.25..=16.0).contains(&self.width)
    }
}
pub type CanvasCrosshairFactory = Rc<dyn Fn(f32) -> Option<CanvasCrosshair>>;

impl CanvasView {
    pub fn set_command_key_resolver(&mut self, resolver: Option<CanvasCommandKeyResolver>) {
        self.command_key_resolver = resolver;
    }
    pub(super) fn command_key(&self, key: &gpui::Keystroke, cx: &App) -> SharedString {
        // Use the pinned runtime's printable-key classification, including IME
        // input, rather than treating unknown named/function keys as text.
        let printable = key.key.is_empty()
            || key
                .clone()
                .with_simulated_ime()
                .key_char
                .is_some_and(|text| !text.chars().any(char::is_control));
        if self.draft.is_some() && !key.modifiers.secondary(cx) && printable {
            return key.key.clone().into();
        }
        self.command_key_resolver
            .as_ref()
            .and_then(|resolver| resolver(key))
            .unwrap_or_else(|| key.key.clone().into())
    }
    pub fn set_cursor_for_layer(
        &mut self,
        layer: CanvasCursorLayer,
        cursor: CursorStyle,
        cx: &mut Context<Self>,
    ) {
        self.cursor_layers[match layer {
            CanvasCursorLayer::CanvasTool => 0,
            CanvasCursorLayer::Host => 1,
        }] = Some(cursor);
        cx.notify();
    }
    pub fn clear_cursor_for_layer(&mut self, layer: CanvasCursorLayer, cx: &mut Context<Self>) {
        self.cursor_layers[match layer {
            CanvasCursorLayer::CanvasTool => 0,
            CanvasCursorLayer::Host => 1,
        }] = None;
        cx.notify();
    }
    pub fn set_crosshair_factory(
        &mut self,
        factory: Option<CanvasCrosshairFactory>,
        cx: &mut Context<Self>,
    ) {
        self.crosshair_factory = factory;
        self.crosshair_cache = None;
        cx.notify();
    }
    pub(super) fn effective_cursor(&self) -> CursorStyle {
        self.cursor_layers[1]
            .or(self.cursor_layers[0])
            .unwrap_or_else(|| {
                if self.draft.is_some() {
                    CursorStyle::IBeam
                } else {
                    self.cursor
                }
            })
    }
    pub(super) fn update_crosshair(&mut self, scale: f32, cx: &mut Context<Self>) {
        if self
            .crosshair_cache
            .as_ref()
            .is_some_and(|(old, _)| *old == scale.to_bits())
        {
            return;
        }
        let candidate = self
            .crosshair_factory
            .as_ref()
            .and_then(|factory| factory(scale));
        let crosshair = candidate.filter(|value| value.valid());
        self.crosshair_cache = Some((scale.to_bits(), crosshair));
        if candidate.is_some() && crosshair.is_none() {
            self.fail("Invalid custom crosshair dimensions or color", cx);
        }
    }
    /// Invalidates display caches for the next frame without composing in the
    /// host callback. The last completed image stays available until that paint.
    pub fn clear_render_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.invalidate_render();
        self.clear_renderer_pending = true;
        self.changed(window, cx);
    }
    pub fn set_show_dirty_regions(&mut self, show: bool, cx: &mut Context<Self>) {
        self.show_dirty_regions = show;
        cx.notify();
    }
    pub fn show_dirty_regions(&self) -> bool {
        self.show_dirty_regions
    }
}

pub(super) fn paint_crosshair(
    value: CanvasCrosshair,
    position: Point<Pixels>,
    window: &mut Window,
) {
    for (a, b) in [
        ((-value.radius, 0.0), (-value.gap, 0.0)),
        ((value.gap, 0.0), (value.radius, 0.0)),
        ((0.0, -value.radius), (0.0, -value.gap)),
        ((0.0, value.gap), (0.0, value.radius)),
    ] {
        let horizontal = a.1 == b.1;
        let offset = if horizontal {
            point(px(a.0), px(a.1 - value.width / 2.0))
        } else {
            point(px(a.0 - value.width / 2.0), px(a.1))
        };
        let size = if horizontal {
            gpui::size(px(b.0 - a.0), px(value.width))
        } else {
            gpui::size(px(value.width), px(b.1 - a.1))
        };
        window.paint_quad(fill(Bounds::new(position + offset, size), value.color));
    }
}
