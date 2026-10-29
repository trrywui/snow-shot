#pragma once

#include "snowimagecodecbridge.h"

#define SNOW_SHOT_ANIMATION_CODEC_ABI_VERSION 3U

#ifdef __cplusplus
extern "C" {
#endif

typedef struct SnowShotAnimationDecoder SnowShotAnimationDecoder;
typedef int(SNOW_SHOT_IMAGE_CODEC_CALL* SnowShotAnimationCancel)(void* context);
typedef int(SNOW_SHOT_IMAGE_CODEC_CALL* SnowShotAnimationAdmit)(void* context, uint64_t bytes);

enum SnowShotAnimationPixelFormat {
    SNOW_SHOT_ANIMATION_PIXEL_FORMAT_RGBA8_PREMULTIPLIED = 1,
    SNOW_SHOT_ANIMATION_PIXEL_FORMAT_BGRA8_PREMULTIPLIED = 2,
};

typedef struct SnowShotAnimationInfo {
    uint32_t width;
    uint32_t height;
    uint32_t frame_count;
    uint32_t format;
    uint32_t orientation;
    uint64_t total_play_count;
    // Read-only color data borrowed from the decoder until destroy.
    SnowShotImageCodecColorEncoding color;
} SnowShotAnimationInfo;

typedef struct SnowShotAnimationFrame {
    const uint8_t* data;
    uint64_t size;
    uint64_t row_stride;
    uint32_t width;
    uint32_t height;
    uint32_t pixel_format;
    void* owner;
} SnowShotAnimationFrame;

SNOW_SHOT_IMAGE_CODEC_API uint32_t SNOW_SHOT_IMAGE_CODEC_CALL snow_shot_animation_abi_version(void);

// The caller keeps encoded bytes immutable and alive until destroy. All session
// operations, including destroy, are serialized. Published frames are independent.
SNOW_SHOT_IMAGE_CODEC_API SnowShotAnimationDecoder* SNOW_SHOT_IMAGE_CODEC_CALL
snow_shot_animation_open(const uint8_t* encoded, uint64_t size, int first_frame_only,
                         SnowShotAnimationCancel cancelled, void* cancel_context,
                         SnowShotAnimationAdmit admit, void* admit_context,
                         SnowShotAnimationInfo* info, char* error, uint64_t error_capacity);
SNOW_SHOT_IMAGE_CODEC_API void SNOW_SHOT_IMAGE_CODEC_CALL
snow_shot_animation_destroy(SnowShotAnimationDecoder* decoder);
SNOW_SHOT_IMAGE_CODEC_API int64_t SNOW_SHOT_IMAGE_CODEC_CALL
snow_shot_animation_duration(const SnowShotAnimationDecoder* decoder, uint32_t frame);
SNOW_SHOT_IMAGE_CODEC_API int64_t SNOW_SHOT_IMAGE_CODEC_CALL
snow_shot_animation_index(const SnowShotAnimationDecoder* decoder);
// 1: advanced; 0: sequence ended; -1: failed/cancelled.
SNOW_SHOT_IMAGE_CODEC_API int SNOW_SHOT_IMAGE_CODEC_CALL
snow_shot_animation_advance(SnowShotAnimationDecoder* decoder, SnowShotAnimationCancel cancelled,
                            void* cancel_context, char* error, uint64_t error_capacity);
SNOW_SHOT_IMAGE_CODEC_API int SNOW_SHOT_IMAGE_CODEC_CALL
snow_shot_animation_reset(SnowShotAnimationDecoder* decoder, char* error, uint64_t error_capacity);
SNOW_SHOT_IMAGE_CODEC_API int SNOW_SHOT_IMAGE_CODEC_CALL
snow_shot_animation_snapshot(const SnowShotAnimationDecoder* decoder, SnowShotAnimationFrame* frame,
                             char* error, uint64_t error_capacity);
SNOW_SHOT_IMAGE_CODEC_API void SNOW_SHOT_IMAGE_CODEC_CALL
snow_shot_animation_frame_release(SnowShotAnimationFrame* frame);

#ifdef __cplusplus
}
#endif
