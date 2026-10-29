// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef SNOW_IMAGE_NATIVE_H
#define SNOW_IMAGE_NATIVE_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct SnowDesc {
    uint32_t width, height, sample, significant_bits, transfer, primaries, orientation;
    uint32_t has_alpha, animated, frame_count, loop_count;
    float hdr_reference_white_nits;
} SnowDesc;
typedef struct SnowDecodeOptions {
    uint64_t max_output_bytes, max_memory_bytes, max_metadata_bytes;
    uint32_t target_width, target_height, threads;
} SnowDecodeOptions;
typedef struct SnowEncodeOptions {
    float quality;
    uint32_t lossless, effort, exact, threads;
    uint64_t max_output_bytes, max_memory_bytes;
    uint32_t jpeg_subsampling, progressive;
    float distance, alpha_quality;
    uint32_t speed, bit_depth;
} SnowEncodeOptions;
typedef struct SnowError {
    uint32_t code;
    char message[256];
} SnowError;
typedef uint8_t* (*SnowAllocate)(void*, uint32_t slot, size_t size);
typedef int (*SnowWrite)(void*, const uint8_t*, size_t);
typedef int (*SnowReadRow)(void*, uint32_t row, uint8_t*, size_t);
typedef int (*SnowWriteRegion)(void*, uint32_t row, uint32_t column, const uint8_t*, size_t);
typedef struct SnowBuffers {
    void* context;
    SnowAllocate allocate;
} SnowBuffers;
int snow_native_decode(uint32_t format, const uint8_t*, size_t, const SnowDecodeOptions*,
                       int inspect, SnowDesc*, SnowBuffers, SnowError*);
int snow_native_encode(uint32_t format, const SnowDesc*, const uint8_t*, size_t, const uint8_t*,
                       size_t, const SnowEncodeOptions*, SnowWrite, void*, SnowError*);
uint32_t snow_exif_orientation(const uint8_t*, size_t);
int snow_jpeg_metadata(const uint8_t*, size_t, SnowBuffers, SnowError*);
uint32_t snow_jpeg_orientation(const uint8_t*, size_t);
int snow_jpeg_decode(const uint8_t*, size_t, const SnowDecodeOptions*, int inspect, SnowDesc*,
                     SnowBuffers, SnowError*);
int snow_jpeg_encode(const SnowDesc*, const uint8_t*, size_t, const uint8_t*, size_t,
                     const SnowEncodeOptions*, SnowWrite, void*, SnowError*);
int snow_jxl_encode_rows(const SnowDesc*, const uint8_t*, size_t, const SnowEncodeOptions*,
                         SnowReadRow, void*, SnowWrite, void*, SnowError*);
int snow_jpeg_encode_rows(const SnowDesc*, const SnowEncodeOptions*, SnowReadRow, void*, SnowWrite,
                          void*, SnowError*);
int snow_native_transform(const uint8_t*, size_t, void*, size_t, uint32_t sample, uint32_t linear,
                          SnowError*);
int snow_native_rgb_profile(const double white[2], const double primaries[6], const double gamma[3],
                            uint64_t maximum, SnowBuffers, SnowError*);
typedef struct SnowResizeRequirements {
    uint64_t scratch_bytes;
    uint32_t input_buffer_rows, output_buffer_rows;
    uint64_t row_buffer_bytes;
} SnowResizeRequirements;
int snow_native_resize_query(uint32_t, uint32_t, uint32_t, uint32_t, SnowResizeRequirements*,
                             SnowError*);
int snow_native_resize(const float*, uint32_t, uint32_t, float*, uint32_t, uint32_t, uint64_t,
                       SnowError*);
int snow_native_resize_regions(uint32_t, uint32_t, uint32_t, uint32_t, uint64_t, SnowReadRow, void*,
                               SnowWriteRegion, void*, SnowError*);
void* snow_webp_animation_open(const uint8_t*, size_t, const SnowDecodeOptions*, SnowDesc*,
                               SnowError*);
int snow_webp_animation_next(void*, SnowBuffers, SnowDesc*, uint32_t*, SnowError*);
void snow_webp_animation_reset(void*);
void snow_webp_animation_close(void*);
#ifdef __cplusplus
}
#endif
#endif
