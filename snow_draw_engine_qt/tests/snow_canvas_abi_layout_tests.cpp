#include "snow_draw_engine.h"

#include <cstddef>

// Mirror crates/snow-draw-engine-c/src/abi/layout_tests.rs so both sides of
// the FFI boundary enforce the same published layout on supported 64-bit targets.
static_assert(sizeof(SnowShapeStyle) == 120);
static_assert(alignof(SnowShapeStyle) == 8);
static_assert(offsetof(SnowShapeStyle, arrow_outline_color) == 108);
static_assert(offsetof(SnowShapeStyle, arrow_outline_width) == 112);
static_assert(sizeof(SnowArrowStyle) == 64);
static_assert(alignof(SnowArrowStyle) == 8);
static_assert(offsetof(SnowArrowStyle, arrow_outline_color) == 52);
static_assert(offsetof(SnowArrowStyle, arrow_outline_width) == 56);

static_assert(sizeof(SnowSceneDisplayItem) == 440);
static_assert(alignof(SnowSceneDisplayItem) == 8);
static_assert(offsetof(SnowSceneDisplayItem, text_bold) == 145);
static_assert(offsetof(SnowSceneDisplayItem, text_italic) == 146);
static_assert(offsetof(SnowSceneDisplayItem, arrow_start_head) == 148);
static_assert(offsetof(SnowSceneDisplayItem, font_size) == 200);
static_assert(offsetof(SnowSceneDisplayItem, text_utf8) == 344);
static_assert(offsetof(SnowSceneDisplayItem, text_ink_reserve) == 416);
static_assert(offsetof(SnowSceneDisplayItem, arrow_outline_color) == 424);
static_assert(offsetof(SnowSceneDisplayItem, arrow_outline_width) == 432);
static_assert(offsetof(SnowSceneDisplayItem, serial_number_type) ==
              offsetof(SnowSceneDisplayItem, rect_shape) + 1);

static_assert(sizeof(SnowOverlayDisplayItem) == 328);
static_assert(alignof(SnowOverlayDisplayItem) == 8);
static_assert(offsetof(SnowOverlayDisplayItem, arrow_outline_color) == 312);
static_assert(offsetof(SnowOverlayDisplayItem, arrow_outline_width) == 320);

static_assert(sizeof(SnowTextStyle) == 232);
static_assert(alignof(SnowTextStyle) == 8);
static_assert(offsetof(SnowTextStyle, font_family_utf8) == 100);
static_assert(offsetof(SnowTextStyle, bold) == 97);
static_assert(offsetof(SnowTextStyle, italic) == 98);
static_assert(sizeof(SnowTextElementInfo) == 1352);
static_assert(alignof(SnowTextElementInfo) == 8);
static_assert(offsetof(SnowTextElementInfo, text_utf8) == 104);
static_assert(offsetof(SnowTextElementInfo, font_family_utf8) == 1136);

int main() {
    return 0;
}
