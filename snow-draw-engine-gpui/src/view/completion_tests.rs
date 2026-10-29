//! The host may complete a screenshot only after text/label editing declines.
use super::*;
use gpui::PlatformInput;
use std::{cell::Cell, rc::Rc};

#[gpui::test]
fn completion_priority_keeps_bound_labels_and_active_text_drafts(cx: &mut TestAppContext) {
    let mut document = CanvasDocument::default();
    document
        .command(|e| {
            e.apply_annotation_json(
                br#"{"version":1,"operations":[{"type":"arrow","points":[[100,220],[300,220]]}]}"#,
            )
        })
        .unwrap();
    let (view, mut visual) = fixture(cx, document);
    let unhandled = Rc::new(Cell::new(0));
    let observed = unhandled.clone();
    visual.update(|_, cx| {
        cx.subscribe(&view, move |_, event, _| {
            if matches!(event, CanvasEvent::UnhandledDoubleClick { .. }) {
                observed.set(observed.get() + 1);
            }
        })
        .detach();
    });
    let press = |visual: &mut VisualTestContext, position: Point<Pixels>| {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    click_count: 2,
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    click_count: 2,
                    ..Default::default()
                }),
                cx,
            );
        });
        visual.run_until_parked();
    };
    let arrow = view.read_with(&visual, |v, _| {
        let frame = v.session.scene().frame();
        let p = draw::canvas_to_view(draw::Point::new(200., 220.), &frame.camera, frame.surface);
        v.surface_bounds().origin + point(px(p.x as f32), px(p.y as f32))
    });
    press(&mut visual, arrow);
    view.read_with(&visual, |v, _| {
        assert!(matches!(
            v.draft.as_ref().unwrap().target,
            draw::TextCommitTarget::NewArrow(_)
        ))
    });
    assert_eq!(unhandled.get(), 0);
    // A double-click outside the current editor must neither commit nor escape
    // to the screenshot's completion notification.
    press(&mut visual, point(px(25.), px(25.)));
    view.read_with(&visual, |v, _| assert!(v.has_text_draft()));
    assert_eq!(unhandled.get(), 0);
    visual.update(|window, cx| view.update(cx, |v, cx| v.finish_text(false, window, cx)));
    press(&mut visual, point(px(25.), px(25.)));
    assert_eq!(unhandled.get(), 1);
}
