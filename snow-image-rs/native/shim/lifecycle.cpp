// SPDX-License-Identifier: GPL-3.0-or-later
#include "native.h"
#include <array>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>
#include <string>
#include <cmath>
namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        std::fprintf(stderr, "%s\n", message);
        std::abort();
    }
}
struct Buffers {
    std::array<std::vector<uint8_t>, 4> values;
    size_t maximum = 16 * 1024 * 1024;
};
uint8_t* allocate(void* context, uint32_t slot, size_t size) {
    auto& state = *static_cast<Buffers*>(context);
    if (slot >= state.values.size() || size > state.maximum)
        return nullptr;
    try {
        state.values[slot].resize(size);
        return state.values[slot].data();
    } catch (...) {
        return nullptr;
    }
}
int append(void* context, const uint8_t* data, size_t size) {
    auto& output = *static_cast<std::vector<uint8_t>*>(context);
    if (size > 16 * 1024 * 1024 - output.size())
        return 0;
    try {
        output.insert(output.end(), data, data + size);
        return 1;
    } catch (...) {
        return 0;
    }
}
int refuse(void*, const uint8_t*, size_t) {
    return 0;
}
} // namespace
int main(int argc, char** argv) {
    SnowDesc input{};
    input.width = 13;
    input.height = 17;
    input.sample = 0;
    input.significant_bits = 8;
    input.transfer = 1;
    input.primaries = 1;
    input.orientation = 1;
    input.has_alpha = 1;
    std::vector<uint8_t> pixels(13 * 17 * 4);
    for (size_t i = 0; i < pixels.size(); ++i)
        pixels[i] = (i % 4 == 3) ? 255 : uint8_t(i * 17);
    SnowDecodeOptions decode{};
    decode.max_output_bytes = 16 << 20;
    decode.max_memory_bytes = 64 << 20;
    decode.max_metadata_bytes = 8 << 20;
    decode.threads = 2;
    for (unsigned iteration = 0; iteration < 32; ++iteration) {
        for (unsigned format = 1; format <= 4; ++format) {
            SnowEncodeOptions options{};
            options.quality = 90;
            options.effort = 5;
            options.threads = 2;
            options.max_output_bytes = 16 << 20;
            options.max_memory_bytes = 64 << 20;
            options.jpeg_subsampling = 444;
            options.distance = 1;
            options.alpha_quality = 100;
            options.speed = 8;
            options.bit_depth = 8;
            options.lossless = format != 1;
            options.exact = format != 1;
            SnowError error{};
            std::vector<uint8_t> encoded;
            require(snow_native_encode(format, &input, pixels.data(), pixels.size(), nullptr, 0,
                                       &options, append, &encoded, &error),
                    error.message);
            if (iteration == 0 && argc == 2) {
                const std::string path = std::string(argv[1]) + "/codec-" + std::to_string(format);
                std::FILE* file = std::fopen(path.c_str(), "wb");
                require(file != nullptr, "create fuzz seed");
                const uint8_t selector = uint8_t(format - 1);
                std::fwrite(&selector, 1, 1, file);
                std::fwrite(encoded.data(), 1, encoded.size(), file);
                std::fclose(file);
            }
            Buffers output;
            SnowDesc result{};
            require(snow_native_decode(format, encoded.data(), encoded.size(), &decode, 0, &result,
                                       {&output, allocate}, &error),
                    error.message);
            require(result.width == input.width && result.height == input.height,
                    "roundtrip dimensions");
            if (format != 1)
                require(output.values[0] == pixels, "lossless samples");
            Buffers refused;
            refused.maximum = 1;
            require(!snow_native_decode(format, encoded.data(), encoded.size(), &decode, 0, &result,
                                        {&refused, allocate}, &error),
                    "refused allocation must fail");
            require(!snow_native_encode(format, &input, pixels.data(), pixels.size(), nullptr, 0,
                                        &options, refuse, nullptr, &error),
                    "refused output must fail");
            if (format == 2) {
                auto* session = snow_webp_animation_open(encoded.data(), encoded.size(), &decode,
                                                         &result, &error);
                require(session != nullptr, "WebP session creation");
                uint32_t duration = 0;
                Buffers frame;
                require(snow_webp_animation_next(session, {&frame, allocate}, &result, &duration,
                                                 &error) == 1,
                        "WebP frame");
                snow_webp_animation_reset(session);
                snow_webp_animation_close(session);
                require(frame.values[0] == pixels, "snapshot survives session closure");
            }
        }
        {
            SnowDesc hdr = input;
            hdr.width = 2;
            hdr.height = 1;
            hdr.sample = 3;
            hdr.significant_bits = 32;
            hdr.transfer = 2;
            hdr.hdr_reference_white_nits = 160;
            const std::array<float, 8> samples{2, 1, 0.5f, 1, 4, -0.25f, 1, 1};
            SnowEncodeOptions options{};
            options.effort = 7;
            options.threads = 2;
            options.max_output_bytes = 16 << 20;
            options.max_memory_bytes = 64 << 20;
            options.distance = 0.01f;
            SnowError error{};
            std::vector<uint8_t> encoded;
            require(snow_native_encode(3, &hdr, reinterpret_cast<const uint8_t*>(samples.data()),
                                       sizeof(samples), nullptr, 0, &options, append, &encoded,
                                       &error),
                    error.message);
            Buffers result;
            SnowDesc description{};
            require(snow_native_decode(3, encoded.data(), encoded.size(), &decode, 0, &description,
                                       {&result, allocate}, &error),
                    error.message);
            require(description.hdr_reference_white_nits == 160, "scRGB reference white");
            for (size_t i = 0; i < samples.size(); ++i) {
                float value;
                std::memcpy(&value, result.values[0].data() + i * sizeof(float), sizeof(value));
                require(std::abs(value - samples[i]) < 0.05f, "scRGB extended-range sample");
            }
        }
        std::vector<float> source(32 * 32 * 4, 0.5f), output(7 * 9 * 4);
        SnowResizeRequirements requirements{};
        SnowError error{};
        require(snow_native_resize_query(32, 32, 7, 9, &requirements, &error), error.message);
        const uint64_t scratch = requirements.scratch_bytes + requirements.row_buffer_bytes;
        require(snow_native_resize(source.data(), 32, 32, output.data(), 7, 9, scratch, &error),
                error.message);
        require(
            !snow_native_resize(source.data(), 32, 32, output.data(), 7, 9, scratch - 1, &error),
            "resize scratch refusal");
        require(snow_native_transform(nullptr, 0, source.data(), 32 * 32, 3, 1, &error),
                error.message);
        const double white[2] = {0.3127, 0.3290};
        const double primaries[6] = {0.64, 0.33, 0.30, 0.60, 0.15, 0.06};
        const double gamma[3] = {1, 2, 3};
        Buffers profile;
        require(!snow_native_rgb_profile(white, primaries, gamma, 8, {&profile, allocate}, &error),
                "ICC profile budget refusal");
        require(
            snow_native_rgb_profile(white, primaries, gamma, 4096, {&profile, allocate}, &error),
            error.message);
        float calibrated[4] = {0.5f, 0.5f, 0.5f, 1};
        require(snow_native_transform(profile.values[1].data(), profile.values[1].size(),
                                      calibrated, 1, 3, 1, &error),
                error.message);
        require(std::abs(calibrated[0] - 0.5f) < 0.0002f &&
                    std::abs(calibrated[1] - 0.25f) < 0.0002f &&
                    std::abs(calibrated[2] - 0.125f) < 0.0002f,
                "calibrated ICC TRCs");
    }
    std::puts("Native lifecycle: 32 repetitions of all codecs, refused allocations/writes, WebP "
              "snapshots, scRGB, resize and ICC passed.");
}
