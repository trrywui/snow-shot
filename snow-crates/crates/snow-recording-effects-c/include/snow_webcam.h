#pragma once
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct SnowWebcam SnowWebcam;
typedef struct SnowWebcamPlacement {
    uint32_t x, y, size;
} SnowWebcamPlacement;
/* Pure placement in physical preview pixels or output video pixels. */
SnowWebcamPlacement snow_webcam_placement(uint32_t width, uint32_t height, uint32_t size,
                                          uint32_t offset_left, uint32_t offset_bottom);
/* Created inactive. Device work and destruction run on a worker; no callbacks enter the host.
 * A configuration consumer retains its own reference. Size: 50..500; fps: 1..30. */
SnowWebcam* snow_webcam_create(uint32_t size, uint32_t fps);
uint8_t snow_webcam_configure(const SnowWebcam* source, uint32_t size, uint32_t fps);
void snow_webcam_set_active(const SnowWebcam* source, uint8_t active);
void snow_webcam_retry(const SnowWebcam* source);
void snow_webcam_destroy(SnowWebcam* source);
/* Zero means no error. Otherwise returns bytes including NUL; short buffers are terminated. */
size_t snow_webcam_error(const SnowWebcam* source, char* buffer, size_t capacity);
#ifdef __cplusplus
}
#endif
