//! Layout guards for the published ABI on the supported 64-bit targets.
//! Keep the C header checks in tests/snow_canvas_abi_layout_tests.cpp in sync.

use super::types::*;
use std::mem::{align_of, offset_of, size_of};

#[test]
fn arrow_outline_styles_match_published_layout() {
    assert_eq!(size_of::<SnowShapeStyle>(), 120);
    assert_eq!(align_of::<SnowShapeStyle>(), 8);
    assert_eq!(offset_of!(SnowShapeStyle, arrow_outline_color), 108);
    assert_eq!(offset_of!(SnowShapeStyle, arrow_outline_width), 112);

    assert_eq!(size_of::<SnowArrowStyle>(), 64);
    assert_eq!(align_of::<SnowArrowStyle>(), 8);
    assert_eq!(offset_of!(SnowArrowStyle, arrow_outline_color), 52);
    assert_eq!(offset_of!(SnowArrowStyle, arrow_outline_width), 56);
}

#[test]
fn scene_display_view_matches_published_layout() {
    assert_eq!(size_of::<SnowSceneDisplayItem>(), 440);
    assert_eq!(align_of::<SnowSceneDisplayItem>(), 8);
    assert_eq!(offset_of!(SnowSceneDisplayItem, text_bold), 145);
    assert_eq!(offset_of!(SnowSceneDisplayItem, text_italic), 146);
    assert_eq!(offset_of!(SnowSceneDisplayItem, arrow_start_head), 148);
    assert_eq!(offset_of!(SnowSceneDisplayItem, font_size), 200);
    assert_eq!(offset_of!(SnowSceneDisplayItem, text_utf8), 344);
    assert_eq!(offset_of!(SnowSceneDisplayItem, text_ink_reserve), 416);
    assert_eq!(offset_of!(SnowSceneDisplayItem, arrow_outline_color), 424);
    assert_eq!(offset_of!(SnowSceneDisplayItem, arrow_outline_width), 432);
    assert_eq!(
        offset_of!(SnowSceneDisplayItem, serial_number_type),
        offset_of!(SnowSceneDisplayItem, rect_shape) + 1
    );
}

#[test]
fn overlay_display_view_matches_published_layout() {
    assert_eq!(size_of::<SnowOverlayDisplayItem>(), 328);
    assert_eq!(align_of::<SnowOverlayDisplayItem>(), 8);
    assert_eq!(offset_of!(SnowOverlayDisplayItem, arrow_outline_color), 312);
    assert_eq!(offset_of!(SnowOverlayDisplayItem, arrow_outline_width), 320);
}

#[test]
fn text_styles_and_elements_match_published_layout() {
    assert_eq!(size_of::<SnowTextStyle>(), 232);
    assert_eq!(align_of::<SnowTextStyle>(), 8);
    assert_eq!(offset_of!(SnowTextStyle, font_family_utf8), 100);
    assert_eq!(offset_of!(SnowTextStyle, bold), 97);
    assert_eq!(offset_of!(SnowTextStyle, italic), 98);
    assert_eq!(size_of::<SnowTextElementInfo>(), 1352);
    assert_eq!(align_of::<SnowTextElementInfo>(), 8);
    assert_eq!(offset_of!(SnowTextElementInfo, text_utf8), 104);
    assert_eq!(offset_of!(SnowTextElementInfo, font_family_utf8), 1136);
}
