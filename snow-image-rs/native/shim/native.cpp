// SPDX-License-Identifier: GPL-3.0-or-later
#include "native.h"
#include <algorithm>
#include <array>
#include <cmath>
#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <memory>
#include <new>
#include <mutex>
#include <webp/decode.h>
#include <webp/encode.h>
#include <webp/demux.h>
#include <webp/mux.h>
#include <jxl/decode.h>
#include <jxl/encode.h>
#include <jxl/thread_parallel_runner.h>
#include <jxl/color_encoding.h>
#include <avif/avif.h>
#include <lcms2.h>
#include <zimg.h>
#if defined(__aarch64__) || defined(_M_ARM64)
#include <arm_neon.h>
#elif defined(__x86_64__) || defined(_M_X64)
#include <xmmintrin.h>
#endif

namespace {
int fail(SnowError* e, uint32_t code, const char* message) {
    e->code = code;
    std::snprintf(e->message, sizeof(e->message), "%s", message);
    return 0;
}
size_t pixel_size(const SnowDesc* d) {
    return d->sample == 0 ? 4 : d->sample == 3 ? 16 : 8;
}
bool byte_size(const SnowDesc* d, uint64_t maximum, size_t* out) {
    const uint64_t row = uint64_t(d->width) * pixel_size(d);
    if (!d->width || !d->height || row > maximum / d->height || row > SIZE_MAX / d->height)
        return false;
    *out = size_t(row * d->height);
    return true;
}
int profile_copy(const uint8_t* data, size_t size, uint64_t max, SnowBuffers b, SnowError* e) {
    if (size > max)
        return fail(e, 3, "Color profile exceeds metadata budget");
    if (!size)
        return 1;
    uint8_t* out = b.allocate(b.context, 1, size);
    if (!out)
        return fail(e, 3, "Color profile allocation refused");
    std::memcpy(out, data, size);
    return 1;
}
uint32_t transfer_from_cicp(unsigned t) {
    return t == 13 ? 1 : t == 8 ? 2 : t == 16 ? 3 : t == 18 ? 4 : 0;
}
uint32_t primaries_from_cicp(unsigned p) {
    return p == 1 ? 1 : p == 12 ? 2 : p == 9 ? 3 : 0;
}
unsigned transfer_to_cicp(unsigned t) {
    return t == 1 ? 13 : t == 2 ? 8 : t == 3 ? 16 : t == 4 ? 18 : 2;
}
unsigned primaries_to_cicp(unsigned p) {
    return p == 1 ? 1 : p == 2 ? 12 : p == 3 ? 9 : 2;
}

int decode_webp(const uint8_t* data, size_t size, const SnowDecodeOptions* o, bool inspect,
                SnowDesc* d, SnowBuffers b, SnowError* e) {
    WebPBitstreamFeatures features{};
    if (WebPGetFeatures(data, size, &features) != VP8_STATUS_OK)
        return fail(e, 1, "Invalid WebP header");
    d->width = features.width;
    d->height = features.height;
    d->has_alpha = features.has_alpha;
    d->sample = 0;
    d->significant_bits = 8;
    d->transfer = 1;
    d->primaries = 1;
    d->orientation = 1;
    d->animated = features.has_animation;
    d->frame_count = 1;
    WebPData bytes{data, size};
    std::unique_ptr<WebPDemuxer, decltype(&WebPDemuxDelete)> demux(WebPDemux(&bytes),
                                                                   WebPDemuxDelete);
    if (!demux)
        return fail(e, 1, "Invalid or truncated WebP container");
    d->frame_count = WebPDemuxGetI(demux.get(), WEBP_FF_FRAME_COUNT);
    d->loop_count = WebPDemuxGetI(demux.get(), WEBP_FF_LOOP_COUNT);
    WebPChunkIterator it{};
    if (WebPDemuxGetChunk(demux.get(), "ICCP", 1, &it)) {
        int result = profile_copy(it.chunk.bytes, it.chunk.size, o->max_metadata_bytes, b, e);
        WebPDemuxReleaseChunkIterator(&it);
        if (!result)
            return 0;
    }
    uint64_t metadata_size = 0;
    for (const char* tag : {"ICCP", "EXIF", "XMP "}) {
        if (WebPDemuxGetChunk(demux.get(), tag, 1, &it)) {
            metadata_size += it.chunk.size;
            if (tag[0] == 'E')
                d->orientation = snow_exif_orientation(it.chunk.bytes, it.chunk.size);
            if (tag[0] != 'I' && it.chunk.size) {
                auto* output = b.allocate(b.context, tag[0] == 'E' ? 2 : 3, it.chunk.size);
                if (!output) {
                    WebPDemuxReleaseChunkIterator(&it);
                    return fail(e, 3, "WebP metadata allocation refused");
                }
                std::memcpy(output, it.chunk.bytes, it.chunk.size);
            }
            WebPDemuxReleaseChunkIterator(&it);
        }
    }
    if (metadata_size > o->max_metadata_bytes)
        return fail(e, 3, "WebP metadata exceeds budget");
    if (inspect)
        return 1;
    size_t amount;
    if (!byte_size(d, o->max_output_bytes, &amount))
        return fail(e, 3, "WebP pixels exceed budget");
    uint8_t* output = b.allocate(b.context, 0, amount);
    if (!output)
        return fail(e, 3, "WebP pixel allocation refused");
    if (d->animated) {
        WebPAnimDecoderOptions options{};
        WebPAnimDecoderOptionsInit(&options);
        options.color_mode = MODE_RGBA;
        options.use_threads = o->threads > 1;
        std::unique_ptr<WebPAnimDecoder, decltype(&WebPAnimDecoderDelete)> decoder(
            WebPAnimDecoderNew(&bytes, &options), WebPAnimDecoderDelete);
        uint8_t* frame = nullptr;
        int time = 0;
        if (!decoder || !WebPAnimDecoderGetNext(decoder.get(), &frame, &time))
            return fail(e, 1, "Invalid first WebP animation frame");
        std::memcpy(output, frame, amount);
    } else if (!WebPDecodeRGBAInto(data, size, output, amount, int(d->width * 4))) {
        return fail(e, 1, "Invalid WebP image data");
    }
    return 1;
}
struct WriteContext {
    SnowWrite write;
    void* context;
    uint64_t written;
    uint64_t maximum;
};
int webp_write(const uint8_t* data, size_t size, const WebPPicture* picture) {
    auto* ctx = static_cast<WriteContext*>(picture->custom_ptr);
    if (size > ctx->maximum - ctx->written || !ctx->write(ctx->context, data, size))
        return 0;
    ctx->written += size;
    return 1;
}
struct BoundedOutput {
    uint8_t* data = nullptr;
    size_t size = 0;
    size_t capacity = 0;
    uint64_t maximum;
    explicit BoundedOutput(uint64_t limit) : maximum(limit) {}
    ~BoundedOutput() {
        std::free(data);
    }
};
int bounded_write(void* opaque, const uint8_t* data, size_t size) {
    auto* output = static_cast<BoundedOutput*>(opaque);
    if (size > output->maximum - output->size || size > SIZE_MAX - output->size)
        return 0;
    const size_t required = output->size + size;
    if (required > output->capacity) {
        const size_t doubled = output->capacity > SIZE_MAX / 2 ? SIZE_MAX : output->capacity * 2;
        const size_t next =
            size_t(std::min<uint64_t>(std::max(required, doubled), output->maximum));
        auto* pointer = static_cast<uint8_t*>(std::realloc(output->data, next));
        if (!pointer)
            return 0;
        output->data = pointer;
        output->capacity = next;
    }
    std::memcpy(output->data + output->size, data, size);
    output->size += size;
    return 1;
}
int encode_webp(const SnowDesc* d, const uint8_t* pixels, const uint8_t* icc, size_t icc_size,
                const SnowEncodeOptions* o, SnowWrite write, void* context, SnowError* e) {
    if (d->sample != 0)
        return fail(e, 2, "WebP accepts only 8-bit samples");
    if (!d->width || !d->height || d->width > WEBP_MAX_DIMENSION || d->height > WEBP_MAX_DIMENSION)
        return fail(e, 2, "Image exceeds WebP dimensions");
    if (icc_size > o->max_output_bytes)
        return fail(e, 3, "WebP ICC exceeds output budget");
    BoundedOutput encoded(o->max_output_bytes);
    WebPConfig config{};
    WebPPicture picture{};
    if (!WebPConfigInit(&config) || !WebPPictureInit(&picture))
        return fail(e, 4, "WebP ABI initialization failed");
    config.lossless = o->lossless;
    config.quality = o->quality;
    config.method = int(std::min(o->effort, 6u));
    config.exact = o->exact;
    config.alpha_quality = int(o->alpha_quality);
    config.thread_level = o->threads > 1;
    if (!WebPValidateConfig(&config))
        return fail(e, 1, "Invalid WebP settings");
    picture.width = int(d->width);
    picture.height = int(d->height);
    picture.use_argb = 1;
    WriteContext writer{icc_size ? bounded_write : write, icc_size ? &encoded : context, 0,
                        o->max_output_bytes};
    picture.writer = webp_write;
    picture.custom_ptr = &writer;
    if (!WebPPictureImportRGBA(&picture, pixels, int(d->width * 4))) {
        WebPPictureFree(&picture);
        return fail(e, 3, "WebP materialization allocation failed");
    }
    int success = WebPEncode(&config, &picture);
    auto code = picture.error_code;
    WebPPictureFree(&picture);
    if (!success)
        return fail(e,
                    code == VP8_ENC_ERROR_BAD_WRITE || code == VP8_ENC_ERROR_OUT_OF_MEMORY ? 3 : 4,
                    "WebP encoding failed or output budget exceeded");
    if (icc_size) {
        // Muxing may add VP8X (18 bytes) and ICCP (8-byte header plus padded payload).
        const uint64_t overhead = 26 + uint64_t(icc_size) + (icc_size & 1);
        if (overhead > o->max_output_bytes || encoded.size > o->max_output_bytes - overhead)
            return fail(e, 3, "WebP container exceeds output budget");
        WebPData bytes{encoded.data, encoded.size};
        WebPData profile{icc, icc_size};
        std::unique_ptr<WebPMux, decltype(&WebPMuxDelete)> mux(WebPMuxCreate(&bytes, 0),
                                                               WebPMuxDelete);
        WebPData output{};
        if (!mux || WebPMuxSetChunk(mux.get(), "ICCP", &profile, 1) != WEBP_MUX_OK ||
            WebPMuxAssemble(mux.get(), &output) != WEBP_MUX_OK)
            return fail(e, 4, "WebP ICC container assembly failed");
        const bool written =
            output.size <= o->max_output_bytes && write(context, output.bytes, output.size);
        WebPDataClear(&output);
        if (!written)
            return fail(e, 3, "WebP container output refused");
    }
    return 1;
}

// libjxl routes all allocations through this bounded manager, including its thread runner.
struct AllocationBudget {
    std::mutex mutex;
    size_t used = 0;
    size_t maximum;
    explicit AllocationBudget(size_t max) : maximum(max) {}
};
struct alignas(std::max_align_t) AllocationHeader {
    size_t size;
};
void* jxl_allocate(void* opaque, size_t size) {
    auto* b = static_cast<AllocationBudget*>(opaque);
    std::lock_guard<std::mutex> lock(b->mutex);
    if (size > SIZE_MAX - sizeof(AllocationHeader) ||
        size + sizeof(AllocationHeader) > b->maximum - b->used)
        return nullptr;
    auto* p = static_cast<AllocationHeader*>(std::malloc(size + sizeof(AllocationHeader)));
    if (!p)
        return nullptr;
    p->size = size + sizeof(AllocationHeader);
    b->used += p->size;
    return p + 1;
}
void jxl_free(void* opaque, void* pointer) {
    if (!pointer)
        return;
    auto* b = static_cast<AllocationBudget*>(opaque);
    auto* p = static_cast<AllocationHeader*>(pointer) - 1;
    std::lock_guard<std::mutex> lock(b->mutex);
    b->used -= p->size;
    std::free(p);
}
JxlDataType sample_to_jxl(unsigned sample) {
    return sample == 0   ? JXL_TYPE_UINT8
           : sample == 1 ? JXL_TYPE_UINT16
           : sample == 2 ? JXL_TYPE_FLOAT16
                         : JXL_TYPE_FLOAT;
}
uint32_t load_be32(const uint8_t* p) {
    return uint32_t(p[0]) << 24 | uint32_t(p[1]) << 16 | uint32_t(p[2]) << 8 | p[3];
}
void store_be32(uint8_t* p, uint32_t value) {
    p[0] = uint8_t(value >> 24);
    p[1] = uint8_t(value >> 16);
    p[2] = uint8_t(value >> 8);
    p[3] = uint8_t(value);
}
int read_scrgb_box(const uint8_t* data, size_t size, float* white, SnowError* e) {
    static const uint8_t signature[12] = {0, 0, 0, 12, 'J', 'X', 'L', ' ', 13, 10, 0x87, 10};
    if (size < 12 || std::memcmp(data, signature, 12))
        return 1;
    size_t position = 12;
    bool found = false;
    while (position < size) {
        if (size - position < 8)
            return fail(e, 1, "Truncated JXL container box");
        uint64_t length = load_be32(data + position);
        size_t header = 8;
        if (length == 1) {
            if (size - position < 16)
                return fail(e, 1, "Truncated extended JXL box");
            length =
                uint64_t(load_be32(data + position + 8)) << 32 | load_be32(data + position + 12);
            header = 16;
        } else if (length == 0)
            length = size - position;
        if (length < header || length > size - position)
            return fail(e, 1, "Invalid JXL container box length");
        if (!std::memcmp(data + position + 4, "sHDR", 4)) {
            if (found || length - header != 8 || load_be32(data + position + header) != 1)
                return fail(e, 1, "Invalid scRGB metadata box");
            const uint32_t bits = load_be32(data + position + header + 4);
            std::memcpy(white, &bits, sizeof(bits));
            if (!std::isfinite(*white) || *white <= 0)
                return fail(e, 1, "Invalid scRGB reference white");
            found = true;
        }
        position += size_t(length);
    }
    return 1;
}
int decode_jxl(const uint8_t* data, size_t size, const SnowDecodeOptions* o, bool inspect,
               SnowDesc* d, SnowBuffers b, SnowError* e) {
    AllocationBudget budget{size_t(std::min<uint64_t>(o->max_memory_bytes, SIZE_MAX))};
    JxlMemoryManager mm{&budget, jxl_allocate, jxl_free};
    if (!read_scrgb_box(data, size, &d->hdr_reference_white_nits, e))
        return 0;
    std::unique_ptr<JxlDecoder, decltype(&JxlDecoderDestroy)> decoder(JxlDecoderCreate(&mm),
                                                                      JxlDecoderDestroy);
    if (!decoder)
        return fail(e, 3, "JXL decoder allocation failed");
    std::unique_ptr<void, decltype(&JxlThreadParallelRunnerDestroy)> runner(
        JxlThreadParallelRunnerCreate(&mm, o->threads > 1 ? o->threads - 1 : 0),
        JxlThreadParallelRunnerDestroy);
    if (!runner || JxlDecoderSetParallelRunner(decoder.get(), JxlThreadParallelRunner,
                                               runner.get()) != JXL_DEC_SUCCESS)
        return fail(e, 3, "JXL thread runner allocation failed");
    JxlDecoderSetKeepOrientation(decoder.get(), JXL_TRUE);
    JxlDecoderSetUnpremultiplyAlpha(decoder.get(), JXL_TRUE);
    JxlDecoderSubscribeEvents(decoder.get(),
                              JXL_DEC_BASIC_INFO | JXL_DEC_COLOR_ENCODING | JXL_DEC_FULL_IMAGE);
    JxlDecoderSetInput(decoder.get(), data, size);
    JxlDecoderCloseInput(decoder.get());
    JxlPixelFormat format{4, JXL_TYPE_UINT8, JXL_NATIVE_ENDIAN, 0};
    bool have_info = false;
    for (;;) {
        auto status = JxlDecoderProcessInput(decoder.get());
        if (status == JXL_DEC_BASIC_INFO) {
            JxlBasicInfo info{};
            if (JxlDecoderGetBasicInfo(decoder.get(), &info) != JXL_DEC_SUCCESS)
                return fail(e, 1, "Invalid JXL basic info");
            d->width = info.xsize;
            d->height = info.ysize;
            d->has_alpha = info.alpha_bits != 0;
            d->sample = info.exponent_bits_per_sample ? (info.bits_per_sample == 16 ? 2 : 3)
                        : info.bits_per_sample > 8    ? 1
                                                      : 0;
            d->significant_bits = info.bits_per_sample;
            d->orientation = info.orientation;
            d->animated = info.have_animation;
            d->frame_count = info.have_animation ? 0 : 1;
            d->loop_count = info.have_animation ? info.animation.num_loops : 0;
            format.data_type = sample_to_jxl(d->sample);
            have_info = true;
        } else if (status == JXL_DEC_COLOR_ENCODING) {
            JxlColorEncoding color{};
            if (JxlDecoderGetColorAsEncodedProfile(decoder.get(), JXL_COLOR_PROFILE_TARGET_DATA,
                                                   &color) == JXL_DEC_SUCCESS) {
                d->transfer = transfer_from_cicp(color.transfer_function);
                d->primaries = color.primaries == JXL_PRIMARIES_SRGB   ? 1
                               : color.primaries == JXL_PRIMARIES_P3   ? 2
                               : color.primaries == JXL_PRIMARIES_2100 ? 3
                                                                       : 0;
            }
            size_t profile_size = 0;
            if (JxlDecoderGetICCProfileSize(decoder.get(), JXL_COLOR_PROFILE_TARGET_DATA,
                                            &profile_size) == JXL_DEC_SUCCESS &&
                profile_size) {
                if (profile_size > o->max_metadata_bytes)
                    return fail(e, 3, "JXL ICC exceeds metadata budget");
                auto* profile = b.allocate(b.context, 1, profile_size);
                if (!profile)
                    return fail(e, 3, "JXL ICC allocation refused");
                if (JxlDecoderGetColorAsICCProfile(decoder.get(), JXL_COLOR_PROFILE_TARGET_DATA,
                                                   profile, profile_size) != JXL_DEC_SUCCESS)
                    return fail(e, 1, "Invalid JXL ICC profile");
            }
            if (d->hdr_reference_white_nits > 0 &&
                (d->transfer != 2 || d->primaries != 1 || d->sample < 2))
                return fail(e, 1, "scRGB metadata does not match JXL color samples");
            if (inspect)
                return 1;
            if (d->animated)
                return fail(e, 2, "Animated JXL decoding is outside the still-image contract");
        } else if (status == JXL_DEC_NEED_IMAGE_OUT_BUFFER) {
            size_t bytes = 0, expected = 0;
            if (!have_info || !byte_size(d, o->max_output_bytes, &expected) ||
                JxlDecoderImageOutBufferSize(decoder.get(), &format, &bytes) != JXL_DEC_SUCCESS ||
                bytes != expected)
                return fail(e, 3, "JXL pixels exceed budget or have invalid layout");
            auto* output = b.allocate(b.context, 0, bytes);
            if (!output)
                return fail(e, 3, "JXL allocation refused");
            if (JxlDecoderSetImageOutBuffer(decoder.get(), &format, output, bytes) !=
                JXL_DEC_SUCCESS)
                return fail(e, 4, "JXL output setup failed");
        } else if (status == JXL_DEC_FULL_IMAGE) {
            return 1;
        } else if (status == JXL_DEC_ERROR || status == JXL_DEC_NEED_MORE_INPUT ||
                   status == JXL_DEC_SUCCESS) {
            return fail(e, 1, "Invalid or incomplete JXL image");
        }
    }
}
struct ChunkHeader {
    ChunkHeader* next;
    ChunkHeader* previous;
};
struct ChunkInput {
    AllocationBudget* budget;
    const SnowDesc* description;
    SnowReadRow read;
    void* context;
    std::mutex mutex;
    ChunkHeader* active = nullptr;
    bool failed = false;
    ChunkInput(AllocationBudget* b, const SnowDesc* d, SnowReadRow r, void* c)
        : budget(b), description(d), read(r), context(c) {}
    ~ChunkInput() {
        while (active) {
            auto* next = active->next;
            jxl_free(budget, active);
            active = next;
        }
    }
};
void chunk_format(void* opaque, JxlPixelFormat* format) {
    auto* state = static_cast<ChunkInput*>(opaque);
    *format = {4, sample_to_jxl(state->description->sample), JXL_NATIVE_ENDIAN, 0};
}
void chunk_extra_format(void* opaque, size_t, JxlPixelFormat* format) {
    chunk_format(opaque, format);
    format->num_channels = 1;
}
const void* chunk_data(ChunkInput* state, size_t x, size_t y, size_t w, size_t h, size_t* stride,
                       bool alpha) {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->failed || x > state->description->width || w > state->description->width - x ||
        y > state->description->height || h > state->description->height - y)
        return nullptr;
    const size_t sample = pixel_size(state->description) / 4;
    *stride = w * (alpha ? sample : sample * 4);
    if (h && *stride > (SIZE_MAX - sizeof(ChunkHeader)) / h) {
        state->failed = true;
        return nullptr;
    }
    auto* header =
        static_cast<ChunkHeader*>(jxl_allocate(state->budget, sizeof(ChunkHeader) + h * *stride));
    auto* row = static_cast<uint8_t*>(
        jxl_allocate(state->budget, size_t(state->description->width) * sample * 4));
    if (!header || !row) {
        jxl_free(state->budget, header);
        jxl_free(state->budget, row);
        state->failed = true;
        return nullptr;
    }
    auto* pixels = reinterpret_cast<uint8_t*>(header + 1);
    for (size_t i = 0; i < h; ++i) {
        if (!state->read(state->context, uint32_t(y + i), row,
                         size_t(state->description->width) * sample * 4)) {
            jxl_free(state->budget, row);
            jxl_free(state->budget, header);
            state->failed = true;
            return nullptr;
        }
        // libjxl requires finite floating-point samples. Check streamed rows at
        // the same boundary as resident input, before handing them to the codec.
        if (state->description->sample >= 2) {
            for (size_t c = 0; c < size_t(state->description->width) * 4; ++c) {
                bool valid;
                if (sample == 4) {
                    float value;
                    std::memcpy(&value, row + c * sample, sizeof(value));
                    valid = std::isfinite(value) && (c % 4 != 3 || (value >= 0 && value <= 1));
                } else {
                    uint16_t value;
                    std::memcpy(&value, row + c * sample, sizeof(value));
                    valid = (value & 0x7c00) != 0x7c00 &&
                            (c % 4 != 3 || value <= 0x3c00 || value == 0x8000);
                }
                if (!valid) {
                    jxl_free(state->budget, row);
                    jxl_free(state->budget, header);
                    state->failed = true;
                    return nullptr;
                }
            }
        }
        if (alpha) {
            for (size_t c = 0; c < w; ++c)
                std::memcpy(pixels + i * *stride + c * sample, row + ((x + c) * 4 + 3) * sample,
                            sample);
        } else {
            std::memcpy(pixels + i * *stride, row + x * sample * 4, *stride);
        }
    }
    jxl_free(state->budget, row);
    header->previous = nullptr;
    header->next = state->active;
    if (state->active)
        state->active->previous = header;
    state->active = header;
    return pixels;
}
const void* chunk_color(void* opaque, size_t x, size_t y, size_t w, size_t h, size_t* stride) {
    return chunk_data(static_cast<ChunkInput*>(opaque), x, y, w, h, stride, false);
}
const void* chunk_extra(void* opaque, size_t, size_t x, size_t y, size_t w, size_t h,
                        size_t* stride) {
    return chunk_data(static_cast<ChunkInput*>(opaque), x, y, w, h, stride, true);
}
void chunk_release(void* opaque, const void* data) {
    if (!data)
        return;
    auto* state = static_cast<ChunkInput*>(opaque);
    std::lock_guard<std::mutex> lock(state->mutex);
    auto* header = const_cast<ChunkHeader*>(static_cast<const ChunkHeader*>(data)) - 1;
    if (header->previous)
        header->previous->next = header->next;
    else
        state->active = header->next;
    if (header->next)
        header->next->previous = header->previous;
    jxl_free(state->budget, header);
}
struct JxlOutput {
    SnowWrite write;
    void* context;
    uint64_t maximum;
    uint64_t written = 0;
    bool failed = false;
    uint8_t buffer[16384];
};
void* jxl_output_buffer(void* opaque, size_t* size) {
    auto* state = static_cast<JxlOutput*>(opaque);
    if (state->failed || state->written == state->maximum) {
        *size = 0;
        return nullptr;
    }
    *size = size_t(std::min<uint64_t>(sizeof(state->buffer), state->maximum - state->written));
    return state->buffer;
}
void jxl_output_release(void* opaque, size_t size) {
    auto* state = static_cast<JxlOutput*>(opaque);
    if (size > sizeof(state->buffer) || size > state->maximum - state->written ||
        !state->write(state->context, state->buffer, size)) {
        state->failed = true;
        return;
    }
    state->written += size;
}
void jxl_output_finalize(void*, uint64_t) {}
struct JxlMemoryRows {
    const uint8_t* input;
    size_t stride;
};
int jxl_memory_read(void* p, uint32_t y, uint8_t* data, size_t size) {
    auto* state = static_cast<JxlMemoryRows*>(p);
    std::memcpy(data, state->input + y * state->stride, size);
    return 1;
}

int encode_jxl(const SnowDesc* d, const uint8_t* pixels, size_t bytes, const uint8_t* icc,
               size_t icc_size, const SnowEncodeOptions* o, SnowWrite write, void* context,
               SnowError* e, SnowReadRow row_reader = nullptr, void* row_context = nullptr) {
    (void)bytes;
    AllocationBudget budget{size_t(std::min<uint64_t>(o->max_memory_bytes, SIZE_MAX))};
    JxlMemoryManager mm{&budget, jxl_allocate, jxl_free};
    std::unique_ptr<JxlEncoder, decltype(&JxlEncoderDestroy)> encoder(JxlEncoderCreate(&mm),
                                                                      JxlEncoderDestroy);
    if (!encoder)
        return fail(e, 3, "JXL encoder allocation failed");
    std::unique_ptr<void, decltype(&JxlThreadParallelRunnerDestroy)> runner(
        JxlThreadParallelRunnerCreate(&mm, o->threads > 1 ? o->threads - 1 : 0),
        JxlThreadParallelRunnerDestroy);
    if (!runner || JxlEncoderSetParallelRunner(encoder.get(), JxlThreadParallelRunner,
                                               runner.get()) != JXL_ENC_SUCCESS)
        return fail(e, 3, "JXL thread runner allocation failed");
    JxlBasicInfo info{};
    JxlEncoderInitBasicInfo(&info);
    info.xsize = d->width;
    info.ysize = d->height;
    info.num_color_channels = 3;
    info.bits_per_sample = d->significant_bits;
    info.exponent_bits_per_sample = d->sample == 2 ? 5 : d->sample == 3 ? 8 : 0;
    info.num_extra_channels = 1;
    info.alpha_bits = info.bits_per_sample;
    info.alpha_exponent_bits = info.exponent_bits_per_sample;
    info.orientation = static_cast<JxlOrientation>(d->orientation);
    info.uses_original_profile = o->lossless || d->sample >= 2 ? JXL_TRUE : JXL_FALSE;
    if (JxlEncoderSetBasicInfo(encoder.get(), &info) != JXL_ENC_SUCCESS)
        return fail(e, 1, "Invalid JXL input description");
    if (icc_size) {
        if (JxlEncoderSetICCProfile(encoder.get(), icc, icc_size) != JXL_ENC_SUCCESS)
            return fail(e, 1, "Invalid JXL ICC profile");
    } else {
        JxlColorEncoding color{};
        JxlColorEncodingSetToSRGB(&color, JXL_FALSE);
        color.transfer_function = static_cast<JxlTransferFunction>(transfer_to_cicp(d->transfer));
        color.primaries = d->primaries == 3   ? JXL_PRIMARIES_2100
                          : d->primaries == 2 ? JXL_PRIMARIES_P3
                                              : JXL_PRIMARIES_SRGB;
        if (JxlEncoderSetColorEncoding(encoder.get(), &color) != JXL_ENC_SUCCESS)
            return fail(e, 1, "Unsupported JXL color encoding");
    }
    if (d->hdr_reference_white_nits > 0) {
        uint8_t metadata[8] = {0, 0, 0, 1, 0, 0, 0, 0};
        uint32_t bits;
        std::memcpy(&bits, &d->hdr_reference_white_nits, sizeof(bits));
        store_be32(metadata + 4, bits);
        if (JxlEncoderUseBoxes(encoder.get()) != JXL_ENC_SUCCESS ||
            JxlEncoderAddBox(encoder.get(), "sHDR", metadata, sizeof(metadata), JXL_FALSE) !=
                JXL_ENC_SUCCESS)
            return fail(e, 4, "Could not write explicit scRGB metadata");
    }
    auto* settings = JxlEncoderFrameSettingsCreate(encoder.get(), nullptr);
    if (!settings || JxlEncoderSetFrameLossless(settings, o->lossless) != JXL_ENC_SUCCESS ||
        JxlEncoderSetFrameDistance(settings, o->lossless ? 0 : o->distance) != JXL_ENC_SUCCESS ||
        JxlEncoderFrameSettingsSetOption(settings, JXL_ENC_FRAME_SETTING_EFFORT, o->effort) !=
            JXL_ENC_SUCCESS ||
        JxlEncoderFrameSettingsSetOption(settings, JXL_ENC_FRAME_SETTING_KEEP_INVISIBLE,
                                         o->exact) != JXL_ENC_SUCCESS)
        return fail(e, 1, "Invalid JXL encoder settings");
    // Original-profile VarDCT is unsuitable for over-range float samples in
    // libjxl 0.12. Modular encodes floating-point representations directly and
    // preserves the source's highlight scale, including negative scRGB values.
    if (d->sample >= 2 && JxlEncoderFrameSettingsSetOption(settings, JXL_ENC_FRAME_SETTING_MODULAR,
                                                           1) != JXL_ENC_SUCCESS)
        return fail(e, 1, "JXL floating-point modular setup failed");
    JxlOutput output{write, context, o->max_output_bytes, 0, false, {}};
    JxlEncoderOutputProcessor processor{&output, jxl_output_buffer, jxl_output_release, nullptr,
                                        jxl_output_finalize};
    if (JxlEncoderSetOutputProcessor(encoder.get(), processor) != JXL_ENC_SUCCESS ||
        JxlEncoderFrameSettingsSetOption(settings, JXL_ENC_FRAME_SETTING_BUFFERING, 2) !=
            JXL_ENC_SUCCESS ||
        JxlEncoderFrameSettingsSetOption(settings, JXL_ENC_FRAME_SETTING_OUTPUT_MODE, 0) !=
            JXL_ENC_SUCCESS)
        return fail(e, 4, "JXL streaming initialization failed");
    JxlMemoryRows resident{pixels, size_t(d->width) * pixel_size(d)};
    ChunkInput input(&budget, d, row_reader ? row_reader : jxl_memory_read,
                     row_reader ? row_context : &resident);
    JxlChunkedFrameInputSource chunks{&input,      chunk_format, chunk_color, chunk_extra_format,
                                      chunk_extra, chunk_release};
    const auto result = JxlEncoderAddChunkedFrame(settings, JXL_TRUE, chunks);
    JxlEncoderCloseInput(encoder.get());
    const bool succeeded = result == JXL_ENC_SUCCESS &&
                           JxlEncoderFlushInput(encoder.get()) == JXL_ENC_SUCCESS &&
                           !output.failed && !input.failed;
    encoder
        .reset(); // release any outstanding source buffers before the callback context is destroyed
    if (!succeeded)
        return fail(e, output.failed ? 3 : 4,
                    "JXL streaming encode failed, source refused, or budget exceeded");
    return 1;
}

int decode_avif(const uint8_t* data, size_t size, const SnowDecodeOptions* o, bool inspect,
                SnowDesc* d, SnowBuffers b, SnowError* e) {
    std::unique_ptr<avifDecoder, decltype(&avifDecoderDestroy)> decoder(avifDecoderCreate(),
                                                                        avifDecoderDestroy);
    if (!decoder)
        return fail(e, 3, "AVIF decoder allocation failed");
    decoder->codecChoice = AVIF_CODEC_CHOICE_DAV1D;
    decoder->maxThreads = int(o->threads);
    decoder->ignoreExif = AVIF_FALSE;
    decoder->ignoreXMP = AVIF_FALSE;
    decoder->imageSizeLimit = uint32_t(std::min<uint64_t>(o->max_output_bytes / 4, UINT32_MAX));
    decoder->imageCountLimit = 10000;
    if (avifDecoderSetIOMemory(decoder.get(), data, size) != AVIF_RESULT_OK ||
        avifDecoderParse(decoder.get()) != AVIF_RESULT_OK)
        return fail(e, 1, "Invalid AVIF header");
    auto* image = decoder->image;
    d->width = image->width;
    d->height = image->height;
    d->sample = image->depth > 8 ? 1 : 0;
    d->significant_bits = image->depth;
    d->has_alpha = decoder->alphaPresent;
    d->transfer = transfer_from_cicp(image->transferCharacteristics);
    d->primaries = primaries_from_cicp(image->colorPrimaries);
    d->orientation = 1;
    d->animated = decoder->imageCount > 1;
    d->frame_count = decoder->imageCount;
    if (image->transformFlags & AVIF_TRANSFORM_CLAP)
        return fail(e, 2, "AVIF clean aperture must be explicitly applied");
    if (image->transformFlags & AVIF_TRANSFORM_IROT) {
        static constexpr uint32_t orientations[] = {1, 8, 3, 6};
        d->orientation = orientations[image->irot.angle & 3];
    }
    if (image->transformFlags & AVIF_TRANSFORM_IMIR) {
        // HEIF imir axis=0 mirrors top/bottom; axis=1 mirrors left/right.
        static constexpr uint32_t horizontal[] = {0, 2, 1, 4, 3, 6, 5, 8, 7};
        static constexpr uint32_t vertical[] = {0, 4, 3, 2, 1, 8, 7, 6, 5};
        d->orientation =
            image->imir.axis == 1 ? horizontal[d->orientation] : vertical[d->orientation];
    }
    if (!profile_copy(image->icc.data, image->icc.size, o->max_metadata_bytes, b, e))
        return 0;
    for (const auto pair : {std::pair<unsigned, const avifRWData*>(2, &image->exif),
                            std::pair<unsigned, const avifRWData*>(3, &image->xmp)}) {
        if (pair.second->size) {
            if (pair.second->size > o->max_metadata_bytes)
                return fail(e, 3, "AVIF metadata exceeds budget");
            auto* output = b.allocate(b.context, pair.first, pair.second->size);
            if (!output)
                return fail(e, 3, "AVIF metadata allocation refused");
            std::memcpy(output, pair.second->data, pair.second->size);
        }
    }
    if (inspect)
        return 1;
    size_t amount;
    if (!byte_size(d, o->max_output_bytes, &amount) ||
        uint64_t(d->width) * pixel_size(d) > UINT32_MAX)
        return fail(e, 3, "AVIF pixels exceed budget or row stride limit");
    auto* output = b.allocate(b.context, 0, amount);
    if (!output)
        return fail(e, 3, "AVIF allocation refused");
    if (avifDecoderNextImage(decoder.get()) != AVIF_RESULT_OK)
        return fail(e, 1, "Invalid AVIF image data");
    avifRGBImage rgb{};
    avifRGBImageSetDefaults(&rgb, image);
    rgb.format = AVIF_RGB_FORMAT_RGBA;
    rgb.depth = d->sample == 0 ? 8 : 16;
    rgb.alphaPremultiplied = AVIF_FALSE;
    rgb.pixels = output;
    rgb.rowBytes = d->width * uint32_t(pixel_size(d));
    rgb.maxThreads = int(o->threads);
    if (avifImageYUVToRGB(image, &rgb) != AVIF_RESULT_OK)
        return fail(e, 4, "AVIF YUV conversion failed");
    return 1;
}
int encode_avif(const SnowDesc* d, const uint8_t* pixels, const uint8_t* icc, size_t icc_size,
                const SnowEncodeOptions* o, SnowWrite write, void* context, SnowError* e) {
    if (d->sample > 1 || (o->bit_depth != 8 && o->bit_depth != 10 && o->bit_depth != 12) ||
        o->bit_depth < d->significant_bits || (o->exact && o->bit_depth != d->significant_bits))
        return fail(e, 2,
                    "AVIF requires explicit 8, 10, or 12 bit integer samples without implicit "
                    "quantization");
    if (uint64_t(d->width) * pixel_size(d) > UINT32_MAX)
        return fail(e, 3, "AVIF row stride exceeds native limit");
    std::unique_ptr<avifImage, decltype(&avifImageDestroy)> image(
        avifImageCreate(d->width, d->height, o->bit_depth, AVIF_PIXEL_FORMAT_YUV444),
        avifImageDestroy);
    std::unique_ptr<avifEncoder, decltype(&avifEncoderDestroy)> encoder(avifEncoderCreate(),
                                                                        avifEncoderDestroy);
    if (!image || !encoder)
        return fail(e, 3, "AVIF encoder allocation failed");
    image->colorPrimaries = avifColorPrimaries(primaries_to_cicp(d->primaries));
    image->transferCharacteristics = avifTransferCharacteristics(transfer_to_cicp(d->transfer));
    image->matrixCoefficients =
        o->lossless ? AVIF_MATRIX_COEFFICIENTS_IDENTITY : AVIF_MATRIX_COEFFICIENTS_BT709;
    image->yuvRange = AVIF_RANGE_FULL;
    if (icc_size && avifImageSetProfileICC(image.get(), icc, icc_size) != AVIF_RESULT_OK)
        return fail(e, 3, "AVIF ICC allocation failed");
    avifRGBImage rgb{};
    avifRGBImageSetDefaults(&rgb, image.get());
    rgb.format = AVIF_RGB_FORMAT_RGBA;
    rgb.depth = d->sample == 0 ? 8 : 16;
    rgb.alphaPremultiplied = AVIF_FALSE;
    rgb.pixels = const_cast<uint8_t*>(pixels);
    rgb.rowBytes = d->width * uint32_t(pixel_size(d));
    rgb.maxThreads = int(o->threads);
    if (avifImageRGBToYUV(image.get(), &rgb) != AVIF_RESULT_OK)
        return fail(e, 4, "AVIF RGB conversion failed");
    encoder->codecChoice = AVIF_CODEC_CHOICE_AOM;
    encoder->maxThreads = int(o->threads);
    encoder->speed = int(o->speed);
    encoder->quality = o->lossless ? AVIF_QUALITY_LOSSLESS : int(o->quality);
    encoder->qualityAlpha = o->lossless ? AVIF_QUALITY_LOSSLESS : int(o->alpha_quality);
    avifRWData output = AVIF_DATA_EMPTY;
    auto result = avifEncoderWrite(encoder.get(), image.get(), &output);
    int okay = result == AVIF_RESULT_OK && output.size <= o->max_output_bytes &&
               write(context, output.data, output.size);
    avifRWDataFree(&output);
    if (!okay)
        return fail(e, result == AVIF_RESULT_OK ? 3 : 4,
                    "AVIF encoding failed or output exceeds budget");
    return 1;
}
} // namespace

extern "C" int snow_native_decode(uint32_t format, const uint8_t* data, size_t size,
                                  const SnowDecodeOptions* o, int inspect, SnowDesc* d,
                                  SnowBuffers b, SnowError* e) {
    try {
        if (format == 1)
            return snow_jpeg_decode(data, size, o, inspect, d, b, e);
        if (format == 2)
            return decode_webp(data, size, o, inspect != 0, d, b, e);
        if (format == 3)
            return decode_jxl(data, size, o, inspect != 0, d, b, e);
        if (format == 4)
            return decode_avif(data, size, o, inspect != 0, d, b, e);
        return fail(e, 2, "Unsupported native format");
    } catch (const std::bad_alloc&) {
        return fail(e, 3, "Native allocation failed");
    } catch (...) {
        return fail(e, 4, "Native decoder exception");
    }
}
extern "C" int snow_native_encode(uint32_t format, const SnowDesc* d, const uint8_t* pixels,
                                  size_t size, const uint8_t* icc, size_t icc_size,
                                  const SnowEncodeOptions* o, SnowWrite write, void* context,
                                  SnowError* e) {
    try {
        if (format == 1)
            return snow_jpeg_encode(d, pixels, size, icc, icc_size, o, write, context, e);
        if (format == 2)
            return encode_webp(d, pixels, icc, icc_size, o, write, context, e);
        if (format == 3)
            return encode_jxl(d, pixels, size, icc, icc_size, o, write, context, e);
        if (format == 4)
            return encode_avif(d, pixels, icc, icc_size, o, write, context, e);
        return fail(e, 2, "Unsupported native format");
    } catch (const std::bad_alloc&) {
        return fail(e, 3, "Native allocation failed");
    } catch (...) {
        return fail(e, 4, "Native encoder exception");
    }
}

namespace {
struct CmsContextDeleter {
    void operator()(void* value) const {
        cmsDeleteContext(static_cast<cmsContext>(value));
    }
};
struct CmsProfileDeleter {
    void operator()(void* value) const {
        cmsCloseProfile(value);
    }
};
struct CmsTransformDeleter {
    void operator()(void* value) const {
        cmsDeleteTransform(value);
    }
};
struct CmsCurveDeleter {
    void operator()(cmsToneCurve* value) const {
        cmsFreeToneCurve(value);
    }
};
void cms_error(cmsContext, cmsUInt32Number, const char*) {}
int create_rgb_profile(const double* white_xy, const double* primaries_xy, const double* gamma,
                       uint64_t maximum, SnowBuffers buffers, SnowError* error) {
    const auto valid_xy = [](const double* xy) {
        return std::isfinite(xy[0]) && std::isfinite(xy[1]) && xy[0] >= 0 && xy[1] > 0 &&
               xy[0] + xy[1] <= 1;
    };
    if (!valid_xy(white_xy))
        return fail(error, 1, "Invalid RGB profile white point");
    for (unsigned c = 0; c < 3; ++c) {
        if (!valid_xy(primaries_xy + c * 2) || !std::isfinite(gamma[c]) || gamma[c] <= 0 ||
            gamma[c] > 32767)
            return fail(error, 1, "Invalid RGB profile primaries or gamma");
    }
    std::unique_ptr<void, CmsContextDeleter> context(cmsCreateContext(nullptr, nullptr));
    if (!context)
        return fail(error, 3, "ICC context allocation failed");
    auto ctx = static_cast<cmsContext>(context.get());
    cmsSetLogErrorHandlerTHR(ctx, cms_error);
    std::array<std::unique_ptr<cmsToneCurve, CmsCurveDeleter>, 3> owned_curves;
    cmsToneCurve* curves[3];
    for (unsigned c = 0; c < 3; ++c) {
        owned_curves[c].reset(cmsBuildGamma(ctx, gamma[c]));
        if (!owned_curves[c])
            return fail(error, 3, "ICC curve allocation failed");
        curves[c] = owned_curves[c].get();
    }
    cmsCIExyY white{white_xy[0], white_xy[1], 1};
    cmsCIExyYTRIPLE primaries{{primaries_xy[0], primaries_xy[1], 1},
                              {primaries_xy[2], primaries_xy[3], 1},
                              {primaries_xy[4], primaries_xy[5], 1}};
    std::unique_ptr<void, CmsProfileDeleter> profile(
        cmsCreateRGBProfileTHR(ctx, &white, &primaries, curves));
    if (!profile)
        return fail(error, 1, "Calibrated RGB profile could not be constructed");
    cmsUInt32Number bytes = 0;
    if (!cmsSaveProfileToMem(profile.get(), nullptr, &bytes) || !bytes)
        return fail(error, 4, "ICC profile sizing failed");
    if (bytes > maximum)
        return fail(error, 3, "ICC profile exceeds output budget");
    auto* output = buffers.allocate(buffers.context, 1, bytes);
    if (!output)
        return fail(error, 3, "ICC profile allocation refused");
    const cmsUInt32Number expected = bytes;
    if (!cmsSaveProfileToMem(profile.get(), output, &bytes) || bytes != expected)
        return fail(error, 4, "ICC profile serialization failed");
    return 1;
}
int transform(const uint8_t* icc, size_t icc_size, void* pixels, size_t count, uint32_t sample,
              bool linear, SnowError* e) {
    if (icc_size > UINT32_MAX || count > UINT32_MAX)
        return fail(e, 3, "ICC transform exceeds native limits");
    std::unique_ptr<void, CmsContextDeleter> ctx(cmsCreateContext(nullptr, nullptr));
    if (!ctx)
        return fail(e, 3, "ICC context allocation failed");
    cmsSetLogErrorHandlerTHR(static_cast<cmsContext>(ctx.get()), cms_error);
    std::unique_ptr<void, CmsProfileDeleter> input(
        icc_size ? cmsOpenProfileFromMemTHR(static_cast<cmsContext>(ctx.get()), icc,
                                            cmsUInt32Number(icc_size))
                 : cmsCreate_sRGBProfileTHR(static_cast<cmsContext>(ctx.get())));
    std::unique_ptr<void, CmsProfileDeleter> output;
    if (linear) {
        cmsCIExyY white{0.3127, 0.3290, 1.0};
        cmsCIExyYTRIPLE primaries{{0.64, 0.33, 1.0}, {0.30, 0.60, 1.0}, {0.15, 0.06, 1.0}};
        cmsToneCurve* gamma = cmsBuildGamma(static_cast<cmsContext>(ctx.get()), 1.0);
        if (!gamma)
            return fail(e, 3, "ICC curve allocation failed");
        cmsToneCurve* curves[] = {gamma, gamma, gamma};
        output.reset(
            cmsCreateRGBProfileTHR(static_cast<cmsContext>(ctx.get()), &white, &primaries, curves));
        cmsFreeToneCurve(gamma);
    } else {
        output.reset(cmsCreate_sRGBProfileTHR(static_cast<cmsContext>(ctx.get())));
    }
    if (!input || !output)
        return fail(e, 1, "Invalid or unsupported ICC profile");
    if (cmsGetColorSpace(input.get()) != cmsSigRgbData)
        return fail(e, 2, "ICC profile must describe RGB input samples");
    cmsUInt32Number type = sample == 0 ? TYPE_RGBA_8 : sample == 1 ? TYPE_RGBA_16 : TYPE_RGBA_FLT;
    auto flags = cmsFLAGS_BLACKPOINTCOMPENSATION | cmsFLAGS_COPY_ALPHA | cmsFLAGS_NOCACHE;
    std::unique_ptr<void, CmsTransformDeleter> conversion(
        cmsCreateTransformTHR(static_cast<cmsContext>(ctx.get()), input.get(), type, output.get(),
                              type, INTENT_RELATIVE_COLORIMETRIC, flags));
    if (!conversion)
        return fail(e, 1, "ICC transform could not be constructed");
    cmsDoTransform(conversion.get(), pixels, pixels, cmsUInt32Number(count));
    return 1;
}
struct GraphDeleter {
    void operator()(zimg_filter_graph* g) const {
        zimg_filter_graph_free(g);
    }
};
using Graph = std::unique_ptr<zimg_filter_graph, GraphDeleter>;
Graph resize_graph(uint32_t w, uint32_t h, uint32_t ow, uint32_t oh) {
    zimg_image_format input{}, output{};
    zimg_image_format_default(&input, ZIMG_API_VERSION);
    zimg_image_format_default(&output, ZIMG_API_VERSION);
    input.width = w;
    input.height = h;
    input.pixel_type = ZIMG_PIXEL_FLOAT;
    input.color_family = ZIMG_COLOR_RGB;
    input.matrix_coefficients = ZIMG_MATRIX_RGB;
    input.transfer_characteristics = ZIMG_TRANSFER_LINEAR;
    input.color_primaries = ZIMG_PRIMARIES_709;
    input.pixel_range = ZIMG_RANGE_FULL;
    input.alpha = ZIMG_ALPHA_PREMULTIPLIED;
    input.depth = 32;
    output = input;
    output.width = ow;
    output.height = oh;
    zimg_graph_builder_params params{};
    zimg_graph_builder_params_default(&params, ZIMG_API_VERSION);
    params.resample_filter = ZIMG_RESIZE_LANCZOS;
    params.filter_param_a = 3;
    return Graph(zimg_filter_graph_build(&input, &output, &params));
}
size_t stride_for(unsigned width) {
    return (size_t(width) * sizeof(float) + 63) & ~size_t(63);
}
int resize_query(zimg_filter_graph* g, uint32_t w, uint32_t h, uint32_t ow, uint32_t oh,
                 SnowResizeRequirements* r, SnowError* e) {
    size_t scratch = 0;
    unsigned input = 0, output = 0;
    if (zimg_filter_graph_get_tmp_size(g, &scratch) ||
        zimg_filter_graph_get_input_buffering(g, &input) ||
        zimg_filter_graph_get_output_buffering(g, &output))
        return fail(e, 4, "zimg buffer query failed");
    auto input_mask = zimg_select_buffer_mask(std::min(input, h));
    auto output_mask = zimg_select_buffer_mask(std::min(output, oh));
    if (input_mask == ZIMG_BUFFER_MAX || output_mask == ZIMG_BUFFER_MAX)
        return fail(e, 3, "zimg requires an unbounded row buffer");
    const uint64_t rows = 4 * ((uint64_t(input_mask) + 1) * stride_for(w) +
                               (uint64_t(output_mask) + 1) * stride_for(ow));
    if (rows > SIZE_MAX - 128 || scratch > SIZE_MAX - rows - 128)
        return fail(e, 3, "zimg scratch size overflow");
    r->scratch_bytes = scratch;
    r->input_buffer_rows = input_mask + 1;
    r->output_buffer_rows = output_mask + 1;
    // Include one interleaved transfer row per side and alignment padding in the query.
    r->row_buffer_bytes = rows + uint64_t(w + uint64_t(ow)) * 4 * sizeof(float) + 128;
    return 1;
}
struct AlignedFree {
    void operator()(void* p) const {
#ifdef _WIN32
        _aligned_free(p);
#else
        std::free(p);
#endif
    }
};
std::unique_ptr<void, AlignedFree> aligned(size_t size) {
    void* p = nullptr;
#ifdef _WIN32
    p = _aligned_malloc(std::max<size_t>(size, 64), 64);
#else
    if (posix_memalign(&p, 64, std::max<size_t>(size, 64)))
        p = nullptr;
#endif
    return std::unique_ptr<void, AlignedFree>(p);
}
struct ResizeRows {
    SnowReadRow read;
    void* read_context;
    SnowWriteRegion write;
    void* write_context;
    zimg_image_buffer_const input{};
    zimg_image_buffer output{};
    unsigned width, out_width;
    float* in_row;
    float* out_row;
    const float* resident_input;
    float* resident_output;
};
void deinterleave(const float* source, float* r, float* g, float* b, float* a, unsigned count) {
    unsigned x = 0;
    for (; x + 4 <= count; x += 4) {
#if defined(__aarch64__) || defined(_M_ARM64)
        const float32x4x4_t rgba = vld4q_f32(source + x * 4);
        vst1q_f32(r + x, rgba.val[0]);
        vst1q_f32(g + x, rgba.val[1]);
        vst1q_f32(b + x, rgba.val[2]);
        vst1q_f32(a + x, rgba.val[3]);
#elif defined(__x86_64__) || defined(_M_X64)
        __m128 p0 = _mm_loadu_ps(source + x * 4), p1 = _mm_loadu_ps(source + x * 4 + 4);
        __m128 p2 = _mm_loadu_ps(source + x * 4 + 8), p3 = _mm_loadu_ps(source + x * 4 + 12);
        _MM_TRANSPOSE4_PS(p0, p1, p2, p3);
        _mm_storeu_ps(r + x, p0);
        _mm_storeu_ps(g + x, p1);
        _mm_storeu_ps(b + x, p2);
        _mm_storeu_ps(a + x, p3);
#else
        break;
#endif
    }
    for (; x < count; ++x) {
        r[x] = source[x * 4];
        g[x] = source[x * 4 + 1];
        b[x] = source[x * 4 + 2];
        a[x] = source[x * 4 + 3];
    }
}
void interleave(const float* r, const float* g, const float* b, const float* a, float* destination,
                unsigned count) {
    unsigned x = 0;
    for (; x + 4 <= count; x += 4) {
#if defined(__aarch64__) || defined(_M_ARM64)
        const float32x4x4_t rgba{
            {vld1q_f32(r + x), vld1q_f32(g + x), vld1q_f32(b + x), vld1q_f32(a + x)}};
        vst4q_f32(destination + x * 4, rgba);
#elif defined(__x86_64__) || defined(_M_X64)
        __m128 p0 = _mm_loadu_ps(r + x), p1 = _mm_loadu_ps(g + x);
        __m128 p2 = _mm_loadu_ps(b + x), p3 = _mm_loadu_ps(a + x);
        _MM_TRANSPOSE4_PS(p0, p1, p2, p3);
        _mm_storeu_ps(destination + x * 4, p0);
        _mm_storeu_ps(destination + x * 4 + 4, p1);
        _mm_storeu_ps(destination + x * 4 + 8, p2);
        _mm_storeu_ps(destination + x * 4 + 12, p3);
#else
        break;
#endif
    }
    for (; x < count; ++x) {
        destination[x * 4] = r[x];
        destination[x * 4 + 1] = g[x];
        destination[x * 4 + 2] = b[x];
        destination[x * 4 + 3] = a[x];
    }
}
int unpack(void* opaque, unsigned row, unsigned left, unsigned right) {
    auto* state = static_cast<ResizeRows*>(opaque);
    if (!state->resident_input &&
        !state->read(state->read_context, row, reinterpret_cast<uint8_t*>(state->in_row),
                     size_t(state->width) * 16))
        return 1;
    float* planes[4];
    for (unsigned c = 0; c < 4; ++c) {
        planes[c] = reinterpret_cast<float*>(
                        static_cast<uint8_t*>(const_cast<void*>(state->input.plane[c].data)) +
                        (row & state->input.plane[c].mask) * state->input.plane[c].stride) +
                    left;
    }
    const float* source = state->resident_input
                              ? state->resident_input + size_t(row) * state->width * 4
                              : state->in_row;
    deinterleave(source + size_t(left) * 4, planes[0], planes[1], planes[2], planes[3],
                 right - left);
    return 0;
}
int pack(void* opaque, unsigned row, unsigned left, unsigned right) {
    auto* state = static_cast<ResizeRows*>(opaque);
    const float* planes[4];
    for (unsigned c = 0; c < 4; ++c) {
        planes[c] = reinterpret_cast<float*>(static_cast<uint8_t*>(state->output.plane[c].data) +
                                             (row & state->output.plane[c].mask) *
                                                 state->output.plane[c].stride) +
                    left;
    }
    float* destination =
        (state->resident_output ? state->resident_output + size_t(row) * state->out_width * 4
                                : state->out_row) +
        size_t(left) * 4;
    interleave(planes[0], planes[1], planes[2], planes[3], destination, right - left);
    if (state->resident_output)
        return 0;
    // zimg traverses each horizontal tile over all rows. Forward its region coordinates;
    // accumulating whole rows would require retaining the full destination canvas.
    return state->write(state->write_context, row, left,
                        reinterpret_cast<const uint8_t*>(state->out_row + size_t(left) * 4),
                        size_t(right - left) * 16)
               ? 0
               : 1;
}
int resize_rows(unsigned w, unsigned h, unsigned ow, unsigned oh, uint64_t max, SnowReadRow read,
                void* read_context, SnowWriteRegion write, void* write_context, SnowError* e,
                const float* resident_input = nullptr, float* resident_output = nullptr) {
    Graph graph = resize_graph(w, h, ow, oh);
    if (!graph)
        return fail(e, 1, "zimg could not construct resize graph");
    SnowResizeRequirements r{};
    if (!resize_query(graph.get(), w, h, ow, oh, &r, e))
        return 0;
    if (r.scratch_bytes > max || r.row_buffer_bytes > max - r.scratch_bytes)
        return fail(e, 3, "zimg scratch exceeds budget");
    auto storage = aligned(size_t(r.scratch_bytes + r.row_buffer_bytes));
    if (!storage)
        return fail(e, 3, "zimg scratch allocation failed");
    auto* base = static_cast<uint8_t*>(storage.get());
    auto* scratch = base;
    base += (r.scratch_bytes + 63) & ~size_t(63);
    ResizeRows state{read,    read_context, write,          write_context,  {}, {}, w, ow,
                     nullptr, nullptr,      resident_input, resident_output};
    state.input.version = ZIMG_API_VERSION;
    state.output.version = ZIMG_API_VERSION;
    for (unsigned c = 0; c < 4; ++c) {
        state.input.plane[c].data = base;
        state.input.plane[c].stride = ptrdiff_t(stride_for(w));
        state.input.plane[c].mask = r.input_buffer_rows - 1;
        base += stride_for(w) * r.input_buffer_rows;
        state.output.plane[c].data = base;
        state.output.plane[c].stride = ptrdiff_t(stride_for(ow));
        state.output.plane[c].mask = r.output_buffer_rows - 1;
        base += stride_for(ow) * r.output_buffer_rows;
    }
    state.in_row = reinterpret_cast<float*>(base);
    base += size_t(w) * 16;
    state.out_row = reinterpret_cast<float*>(base);
    if (zimg_filter_graph_process(graph.get(), &state.input, &state.output, scratch, unpack, &state,
                                  pack, &state))
        return fail(e, 4, "zimg resize failed or row callback refused");
    return 1;
}
struct WebPAnimationState {
    WebPAnimDecoder* decoder;
    SnowDesc desc;
    uint64_t max_pixels;
    int previous_timestamp = 0;
    ~WebPAnimationState() {
        WebPAnimDecoderDelete(decoder);
    }
};
} // namespace
extern "C" int snow_native_transform(const uint8_t* icc, size_t icc_size, void* pixels,
                                     size_t count, uint32_t sample, uint32_t linear, SnowError* e) {
    try {
        return transform(icc, icc_size, pixels, count, sample, linear != 0, e);
    } catch (...) {
        return fail(e, 4, "ICC adapter exception");
    }
}
extern "C" int snow_native_rgb_profile(const double white[2], const double primaries[6],
                                       const double gamma[3], uint64_t maximum, SnowBuffers buffers,
                                       SnowError* error) {
    try {
        return create_rgb_profile(white, primaries, gamma, maximum, buffers, error);
    } catch (...) {
        return fail(error, 3, "ICC profile construction allocation failed");
    }
}
extern "C" int snow_native_resize_query(uint32_t w, uint32_t h, uint32_t ow, uint32_t oh,
                                        SnowResizeRequirements* r, SnowError* e) {
    try {
        auto g = resize_graph(w, h, ow, oh);
        return g ? resize_query(g.get(), w, h, ow, oh, r, e)
                 : fail(e, 1, "Invalid resize dimensions");
    } catch (...) {
        return fail(e, 3, "zimg query allocation failed");
    }
}
extern "C" int snow_native_resize(const float* input, uint32_t w, uint32_t h, float* output,
                                  uint32_t ow, uint32_t oh, uint64_t max, SnowError* e) {
    try {
        return resize_rows(w, h, ow, oh, max, nullptr, nullptr, nullptr, nullptr, e, input, output);
    } catch (...) {
        return fail(e, 3, "zimg processing allocation failed");
    }
}
extern "C" int snow_native_resize_regions(uint32_t w, uint32_t h, uint32_t ow, uint32_t oh,
                                          uint64_t max, SnowReadRow read, void* read_context,
                                          SnowWriteRegion write, void* write_context,
                                          SnowError* e) {
    try {
        return resize_rows(w, h, ow, oh, max, read, read_context, write, write_context, e);
    } catch (...) {
        return fail(e, 3, "zimg row processing allocation failed");
    }
}
extern "C" void* snow_webp_animation_open(const uint8_t* data, size_t size,
                                          const SnowDecodeOptions* o, SnowDesc* d, SnowError* e) {
    WebPData bytes{data, size};
    WebPBitstreamFeatures features{};
    if (WebPGetFeatures(data, size, &features) != VP8_STATUS_OK) {
        fail(e, 1, "Invalid WebP animation");
        return nullptr;
    }
    d->width = features.width;
    d->height = features.height;
    d->sample = 0;
    d->significant_bits = 8;
    d->transfer = 1;
    d->primaries = 1;
    d->orientation = 1;
    d->has_alpha = features.has_alpha;
    size_t amount = 0;
    if (!byte_size(d, o->max_output_bytes, &amount)) {
        fail(e, 3, "WebP animation canvas exceeds budget");
        return nullptr;
    }
    WebPAnimDecoderOptions options{};
    WebPAnimDecoderOptionsInit(&options);
    options.color_mode = MODE_RGBA;
    options.use_threads = o->threads > 1;
    auto* decoder = WebPAnimDecoderNew(&bytes, &options);
    if (!decoder) {
        fail(e, 1, "Invalid WebP animation container");
        return nullptr;
    }
    WebPAnimInfo info{};
    if (!WebPAnimDecoderGetInfo(decoder, &info)) {
        WebPAnimDecoderDelete(decoder);
        fail(e, 1, "Invalid WebP animation info");
        return nullptr;
    }
    d->animated = info.frame_count > 1;
    d->frame_count = info.frame_count;
    d->loop_count = info.loop_count;
    auto* state = new (std::nothrow) WebPAnimationState{decoder, *d, o->max_output_bytes};
    if (!state) {
        WebPAnimDecoderDelete(decoder);
        fail(e, 3, "Animation session allocation failed");
    }
    return state;
}
extern "C" int snow_webp_animation_next(void* pointer, SnowBuffers b, SnowDesc* d,
                                        uint32_t* duration, SnowError* e) {
    auto* state = static_cast<WebPAnimationState*>(pointer);
    if (!WebPAnimDecoderHasMoreFrames(state->decoder))
        return 2;
    uint8_t* frame = nullptr;
    int time = 0;
    if (!WebPAnimDecoderGetNext(state->decoder, &frame, &time))
        return fail(e, 1, "Invalid WebP animation frame");
    *d = state->desc;
    size_t amount = 0;
    if (!byte_size(d, state->max_pixels, &amount))
        return fail(e, 3, "WebP frame exceeds budget");
    auto* output = b.allocate(b.context, 0, amount);
    if (!output)
        return fail(e, 3, "WebP frame allocation refused");
    if (time < state->previous_timestamp)
        return fail(e, 1, "WebP animation timestamp overflow");
    std::memcpy(output, frame, amount);
    *duration = uint32_t(time - state->previous_timestamp);
    state->previous_timestamp = time;
    return 1;
}
extern "C" void snow_webp_animation_reset(void* pointer) {
    auto* state = static_cast<WebPAnimationState*>(pointer);
    WebPAnimDecoderReset(state->decoder);
    state->previous_timestamp = 0;
}
extern "C" void snow_webp_animation_close(void* pointer) {
    delete static_cast<WebPAnimationState*>(pointer);
}

extern "C" int snow_jxl_encode_rows(const SnowDesc* d, const uint8_t* icc, size_t icc_size,
                                    const SnowEncodeOptions* o, SnowReadRow read,
                                    void* read_context, SnowWrite write, void* write_context,
                                    SnowError* e) {
    try {
        return encode_jxl(d, nullptr, 0, icc, icc_size, o, write, write_context, e, read,
                          read_context);
    } catch (...) {
        return fail(e, 3, "JXL streaming allocation failed");
    }
}
