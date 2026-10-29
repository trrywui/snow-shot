use super::*;
use std::time::Duration;

const CARET_BLINK_PHASE: Duration = Duration::from_millis(500);

impl CanvasView {
    fn text_caret_can_blink(&self, window: &Window) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|draft| draft.buffer.selection().is_empty())
            && self.focus.is_focused(window)
            && window.is_window_active()
    }

    pub(super) fn text_caret_visible(&self, window: &Window, _: &App) -> bool {
        self.caret_visible
            && self.text_selection_overlay.has_published_caret()
            && self.focus.is_focused(window)
            && window.is_window_active()
    }

    /// Editing and caret movement start a fresh visible phase. Holding the task
    /// here ties its cancellation to this editing session and view lifetime.
    pub(super) fn reset_text_caret(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.caret_blink_task = None;
        self.caret_visible = true;
        if self.text_caret_can_blink(window) {
            self.caret_blink_task = Some(cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(CARET_BLINK_PHASE).await;
                    let keep_blinking = this.update_in(cx, |this, window, cx| {
                        if !this.text_caret_can_blink(window) {
                            return false;
                        }
                        this.caret_visible = !this.caret_visible;
                        // A caret phase changes only the native overlay. It must
                        // never invalidate the document's retained raster.
                        cx.notify();
                        true
                    });
                    if !matches!(keep_blinking, Ok(true)) {
                        break;
                    }
                }
            }));
        }
        cx.notify();
    }

    pub(super) fn stop_text_caret(&mut self, cx: &mut Context<Self>) {
        self.caret_blink_task = None;
        self.caret_visible = true;
        cx.notify();
    }
}
