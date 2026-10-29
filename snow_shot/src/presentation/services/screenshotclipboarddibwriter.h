// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <cstdint>
#include <cstring>

#if defined(__SSE2__) || defined(_M_X64)
#include <emmintrin.h>
#elif defined(__aarch64__) || defined(_M_ARM64)
#if defined(_MSC_VER)
#include <arm64_neon.h>
#else
#include <arm_neon.h>
#endif
#endif

namespace snow_shot::presentation::clipboard_detail {
inline void writeBgrScalar(const std::uint8_t* rgba, std::uint8_t* bgr, int pixels, bool opaque) {
    for (int x = 0; x < pixels; ++x, rgba += 4, bgr += 3) {
        const unsigned alpha = rgba[3];
        for (int channel = 0; channel < 3; ++channel)
            bgr[channel] =
                opaque ? rgba[2 - channel]
                       : static_cast<std::uint8_t>(
                             (rgba[2 - channel] * alpha + 255U * (255U - alpha) + 127U) / 255U);
    }
}

#if defined(__SSE2__) || defined(_M_X64)
inline __m128i flattenWhite(__m128i channels) {
    const __m128i maximum = _mm_set1_epi16(255);
    __m128i alpha = _mm_shufflelo_epi16(channels, _MM_SHUFFLE(3, 3, 3, 3));
    alpha = _mm_shufflehi_epi16(alpha, _MM_SHUFFLE(3, 3, 3, 3));
    // Complementary form of round((color * alpha + white * (255-alpha)) / 255).
    __m128i product = _mm_mullo_epi16(_mm_sub_epi16(maximum, channels), alpha);
    product = _mm_add_epi16(product, _mm_set1_epi16(127));
    const __m128i divided = _mm_srli_epi16(
        _mm_add_epi16(_mm_add_epi16(product, _mm_set1_epi16(1)), _mm_srli_epi16(product, 8)), 8);
    return _mm_sub_epi16(maximum, divided);
}
#endif

// Exact 24-bit Windows DIB pixels. No reads or stores extend past the pixel span, including tails.
inline void writeBgr(const std::uint8_t* rgba, std::uint8_t* bgr, int pixels, bool opaque) {
    int x = 0;
#if defined(__SSE2__) || defined(_M_X64)
    for (; x <= pixels - 4; x += 4, rgba += 16, bgr += 12) {
        __m128i values = _mm_loadu_si128(reinterpret_cast<const __m128i*>(rgba));
        if (!opaque) {
            const __m128i zero = _mm_setzero_si128();
            values = _mm_packus_epi16(flattenWhite(_mm_unpacklo_epi8(values, zero)),
                                      flattenWhite(_mm_unpackhi_epi8(values, zero)));
        }
        const __m128i colors = _mm_or_si128(
            _mm_and_si128(values, _mm_set1_epi32(0x0000ff00)),
            _mm_or_si128(_mm_and_si128(_mm_srli_epi32(values, 16), _mm_set1_epi32(0x000000ff)),
                         _mm_slli_epi32(_mm_and_si128(values, _mm_set1_epi32(0x000000ff)), 16)));
        const __m128i packed = _mm_or_si128(
            _mm_or_si128(
                _mm_and_si128(colors, _mm_set_epi32(0, 0, 0, 0x00ffffff)),
                _mm_and_si128(_mm_srli_si128(colors, 1),
                              _mm_set_epi32(0, 0, 0x0000ffff, static_cast<int>(0xff000000U)))),
            _mm_or_si128(
                _mm_and_si128(_mm_srli_si128(colors, 2),
                              _mm_set_epi32(0, 0x000000ff, static_cast<int>(0xffff0000U), 0)),
                _mm_and_si128(_mm_srli_si128(colors, 3),
                              _mm_set_epi32(0, static_cast<int>(0xffffff00U), 0, 0))));
        _mm_storel_epi64(reinterpret_cast<__m128i*>(bgr), packed);
        const std::uint32_t tail =
            static_cast<std::uint32_t>(_mm_cvtsi128_si32(_mm_srli_si128(packed, 8)));
        std::memcpy(bgr + 8, &tail, sizeof(tail));
    }
#elif defined(__aarch64__) || defined(_M_ARM64)
    for (; x <= pixels - 16; x += 16, rgba += 64, bgr += 48) {
        const uint8x16x4_t values = vld4q_u8(rgba);
        uint8x16x3_t colors;
        for (int channel = 0; channel < 3; ++channel) {
            uint8x16_t color = values.val[2 - channel];
            if (!opaque) {
                const auto flatten = [](uint8x8_t c, uint8x8_t a) {
                    uint16x8_t product = vmull_u8(vsub_u8(vdup_n_u8(255), c), a);
                    product = vaddq_u16(product, vdupq_n_u16(127));
                    const auto divided = vshrn_n_u16(
                        vaddq_u16(vaddq_u16(product, vdupq_n_u16(1)), vshrq_n_u16(product, 8)), 8);
                    return vsub_u8(vdup_n_u8(255), divided);
                };
                color = vcombine_u8(flatten(vget_low_u8(color), vget_low_u8(values.val[3])),
                                    flatten(vget_high_u8(color), vget_high_u8(values.val[3])));
            }
            colors.val[channel] = color;
        }
        vst3q_u8(bgr, colors);
    }
#endif
    writeBgrScalar(rgba, bgr, pixels - x, opaque);
}
} // namespace snow_shot::presentation::clipboard_detail
