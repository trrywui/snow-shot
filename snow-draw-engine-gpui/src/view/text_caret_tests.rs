use super::*;
use std::time::Duration;

fn advance(visual: &mut VisualTestContext, milliseconds: u64) {
    visual
        .executor()
        .advance_clock(Duration::from_millis(milliseconds));
    visual.run_until_parked();
}

#[gpui::test]
fn text_caret_blinks_and_input_restarts_the_visible_phase_without_recomposing(
    cx: &mut TestAppContext,
) {
    let (view, mut visual) = fixture(cx, CanvasDocument::default());
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.begin_text(draw::Point::new(0.0, 0.0), None, window, cx);
            view.replace_text_in_range(None, "Caret 你好😀", window, cx);
        });
    });
    paint(&mut visual);
    let generation = view.read_with(&visual, |view, _| view.generation);
    advance(&mut visual, 499);
    visual.update(|window, cx| {
        view.read_with(cx, |view, cx| assert!(view.text_caret_visible(window, cx)));
    });
    advance(&mut visual, 1);
    visual.update(|window, cx| {
        view.read_with(cx, |view, cx| {
            assert!(!view.text_caret_visible(window, cx));
            assert_eq!(view.generation, generation);
            assert_eq!(view.rendered_generation, generation);
        });
    });
    advance(&mut visual, 500);
    visual.update(|window, cx| {
        view.read_with(cx, |view, cx| assert!(view.text_caret_visible(window, cx)));
    });
    advance(&mut visual, 500);
    visual.simulate_keystrokes("left");
    visual.update(|window, cx| {
        view.read_with(cx, |view, cx| assert!(view.text_caret_visible(window, cx)));
    });
    advance(&mut visual, 300);
    visual.simulate_input("x");
    advance(&mut visual, 300);
    visual.update(|window, cx| {
        view.read_with(cx, |view, cx| assert!(view.text_caret_visible(window, cx)));
    });
    advance(&mut visual, 200);
    visual.update(|window, cx| {
        view.read_with(cx, |view, cx| assert!(!view.text_caret_visible(window, cx)));
    });
    visual.simulate_keystrokes("escape");
    advance(&mut visual, 1000);
    view.read_with(&visual, |view, _| {
        assert!(view.draft.is_none());
        assert!(view.caret_blink_task.is_none());
    });
}

#[gpui::test]
fn text_caret_suspends_for_selection_and_popup_blur_then_resumes_on_focus(cx: &mut TestAppContext) {
    let (view, mut visual) = fixture(cx, CanvasDocument::default());
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.begin_text(draw::Point::new(0.0, 0.0), None, window, cx);
            view.replace_text_in_range(None, "Select", window, cx);
            view.set_selected_text_range(0..6, window, cx);
            assert!(!view.text_caret_visible(window, cx));
            assert!(view.caret_blink_task.is_none());
        });
    });
    visual.run_until_parked();
    advance(&mut visual, 1000);
    visual.simulate_keystrokes("left");
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert!(view.text_caret_visible(window, cx));
            assert!(view.caret_blink_task.is_some());
            view.begin_text_style_popup_interaction();
        });
        window.blur(cx);
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            assert!(view.draft.is_some());
            assert!(!view.text_caret_visible(window, cx));
            assert!(view.caret_blink_task.is_none());
            view.end_text_style_popup_interaction(window, cx);
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        view.read_with(cx, |view, cx| {
            assert!(view.text_caret_visible(window, cx));
            assert!(view.caret_blink_task.is_some());
        });
        window.blur(cx);
    });
    visual.run_until_parked();
    view.read_with(&visual, |view, _| {
        assert!(view.draft.is_none());
        assert!(view.caret_blink_task.is_none());
    });
}

#[gpui::test]
fn text_caret_finishing_edit_releases_an_ongoing_native_selection(cx: &mut TestAppContext) {
    let (view, mut visual) = fixture(cx, CanvasDocument::default());
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.set_tool(draw::ActiveTool::Text, window, cx);
        });
    });
    paint(&mut visual);
    visual.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: point(px(220.0), px(160.0)),
        click_count: 1,
        ..Default::default()
    });
    visual.update(|window, cx| {
        view.read_with(cx, |view, _| {
            assert!(view.draft.is_some());
            assert!(view.selecting_text);
            assert!(window.captured_hitbox().is_some());
        });
    });
    visual.simulate_keystrokes("escape");
    visual.update(|window, cx| {
        view.read_with(cx, |view, _| {
            assert!(view.draft.is_none());
            assert!(!view.selecting_text);
            assert!(view.caret_blink_task.is_none());
            assert!(window.captured_hitbox().is_none());
        });
    });
}
