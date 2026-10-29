// SPDX-License-Identifier: GPL-3.0-or-later
#include "native.h"
#include <array>
#include <cstddef>
#include <cstdint>
#include <vector>
namespace {
struct Buffers {
    std::array<std::vector<uint8_t>, 4> values;
};
uint8_t* allocate(void* p, uint32_t slot, size_t size) {
    if (slot > 3 || size > (slot == 0 ? 1024u * 1024u : 16384u))
        return nullptr;
    try {
        auto& bytes = static_cast<Buffers*>(p)->values[slot];
        bytes.resize(size);
        return bytes.data();
    } catch (...) {
        return nullptr;
    }
}
} // namespace
extern "C" int LLVMFuzzerTestOneInput(const uint8_t* data, size_t size) {
    if (size < 2 || size > 16 * 1024 * 1024)
        return 0;
    const uint32_t format = 1 + data[0] % 4;
    ++data;
    --size;
    SnowDecodeOptions options{};
    options.max_output_bytes = 1024 * 1024;
    options.max_memory_bytes = 8 * 1024 * 1024;
    options.max_metadata_bytes = 16384;
    options.threads = 1;
    SnowError error{};
    SnowDesc desc{};
    Buffers buffers;
    if (snow_native_decode(format, data, size, &options, 1, &desc, {&buffers, allocate}, &error)) {
        Buffers pixels;
        snow_native_decode(format, data, size, &options, 0, &desc, {&pixels, allocate}, &error);
        if (format == 2 && desc.animated) {
            auto* session = snow_webp_animation_open(data, size, &options, &desc, &error);
            if (session) {
                for (unsigned frame = 0; frame < 4; ++frame) {
                    Buffers snapshot;
                    uint32_t duration = 0;
                    if (snow_webp_animation_next(session, {&snapshot, allocate}, &desc, &duration,
                                                 &error) != 1)
                        break;
                }
                snow_webp_animation_reset(session);
                snow_webp_animation_close(session);
            }
        }
    }
    return 0;
}
