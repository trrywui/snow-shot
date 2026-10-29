// SPDX-License-Identifier: GPL-3.0-only
#pragma once

#include "../../include/snow_shot/image/imageimportlimits.h"

#include <snow/image/codec.h>
#include <limits>

namespace snow_shot::image_codec {
inline snow::image::DecodeOptions imageImportOptions() {
    snow::image::DecodeOptions options;
    options.limits.maximum_width = std::numeric_limits<int>::max() / 4;
    options.limits.maximum_height = std::numeric_limits<int>::max();
    options.limits.maximum_pixels = image::maximumImageImportPixels;
    options.limits.maximum_input_bytes = image::maximumImageImportBytes;
    options.limits.maximum_owned_output_bytes = image::maximumImageImportBytes;
    return options;
}
} // namespace snow_shot::image_codec
