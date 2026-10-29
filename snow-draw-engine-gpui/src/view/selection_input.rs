use super::draw;

/// Keep existing editor affordances interactive beyond the host's content clip.
/// Distances are logical pixels, matching the Qt host and shared editor hit geometry.
pub(super) fn hits_selection_overlay(
    overlays: &[draw::OverlayDisplayItem],
    world: draw::Point<f64>,
    zoom: f64,
) -> bool {
    let mut text_actual_frame = None;
    for overlay in overlays.iter().rev() {
        let draw::OverlayDisplayItem::Rectangle(item) = overlay else {
            continue;
        };
        if item.kind == draw::UiShapeKind::TextActualFrame {
            text_actual_frame = Some(item);
            continue;
        }
        let local = local_view_point(item, world, zoom);
        let half_width = (item.width * zoom).max(0.0) / 2.0;
        let half_height = (item.height * zoom).max(0.0) / 2.0;
        if matches!(
            item.kind,
            draw::UiShapeKind::SelectionResizeHandle
                | draw::UiShapeKind::SelectionRotationHandle
                | draw::UiShapeKind::SelectionCornerRadiusHandle
                | draw::UiShapeKind::ArrowEndpointHandle
                | draw::UiShapeKind::ArrowFocusHandle
                | draw::UiShapeKind::ArrowSegmentHandle
                | draw::UiShapeKind::MagnifierMoveHandle
                | draw::UiShapeKind::SpeechBubbleAddTailHandle
                | draw::UiShapeKind::SpeechBubbleTailBaseHandle
                | draw::UiShapeKind::SpeechBubbleTailTipHandle
                | draw::UiShapeKind::ShapeTextHandle
        ) {
            let extra = (12.0 - half_width.min(half_height) * 2.0).max(0.0) / 2.0;
            if local.x.abs() <= half_width + extra && local.y.abs() <= half_height + extra {
                return true;
            }
            continue;
        }
        if item.kind == draw::UiShapeKind::SelectionFrame
            && let Some(text_frame) = text_actual_frame
        {
            let text_local = local_view_point(text_frame, world, zoom);
            let inside_text = text_local.x.abs() <= text_frame.width * zoom / 2.0
                && text_local.y.abs() <= text_frame.height * zoom / 2.0;
            if local.x.abs() < half_width && local.y.abs() < half_height && !inside_text {
                return true;
            }
        }
        if matches!(
            item.kind,
            draw::UiShapeKind::SelectionFrame
                | draw::UiShapeKind::SelectionMultiFrame
                | draw::UiShapeKind::MagnifierSelectionFrame
        ) && half_width > 0.0
            && half_height > 0.0
        {
            let horizontal =
                (local.y.abs() - half_height).abs() <= 6.0 && local.x.abs() < half_width - 6.0;
            let vertical =
                (local.x.abs() - half_width).abs() <= 6.0 && local.y.abs() < half_height - 6.0;
            if horizontal || vertical {
                return true;
            }
        }
    }
    false
}

fn local_view_point(
    item: &draw::UiRectangleDisplayItem,
    world: draw::Point<f64>,
    zoom: f64,
) -> draw::Point<f64> {
    let (sin, cos) = item.rotation.sin_cos();
    let dx = (world.x - item.center_x) * zoom;
    let dy = (world.y - item.center_y) * zoom;
    draw::Point::new(dx * cos + dy * sin, -dx * sin + dy * cos)
}
