// SPDX-License-Identifier: GPL-3.0-only
#pragma once

#include <cstdint>

namespace snow_shot::image {
// Editing imports and collage rasters share this ceiling. Consumers must still
// reserve their encoded input, decoded pixels and preparation scratch before use.
inline constexpr std::int64_t maximumImageImportBytes = std::int64_t{8} << 30;
inline constexpr std::int64_t maximumImageImportPixels = maximumImageImportBytes / 4;
} // namespace snow_shot::image
