// SPDX-License-Identifier: GPL-3.0-or-later
// setjmp/longjmp never crosses an FFI boundary or a C++ destructor.
#include "native.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <setjmp.h>
#include <limits.h>
#include <jpeglib.h>
#include <jerror.h>

typedef struct JpegError {
    struct jpeg_error_mgr pub;
    jmp_buf jump;
    SnowError* result;
} JpegError;
static void fail_jpeg(j_common_ptr common) {
    JpegError* error = (JpegError*)common->err;
    error->result->code = 4;
    (*common->err->format_message)(common, error->result->message);
    longjmp(error->jump, 1);
}
static void quiet_jpeg(j_common_ptr common) {
    (void)common;
}
static int fail(SnowError* e, unsigned code, const char* message) {
    e->code = code;
    snprintf(e->message, sizeof(e->message), "%s", message);
    return 0;
}
static int check_metadata(const uint8_t* data, size_t size, uint64_t max, SnowError* e) {
    size_t offset = 2;
    uint64_t metadata = 0;
    while (offset + 1 < size) {
        if (data[offset++] != 0xff)
            return fail(e, 1, "Invalid JPEG marker");
        while (offset < size && data[offset] == 0xff)
            ++offset;
        if (offset == size)
            break;
        const unsigned marker = data[offset++];
        if (marker == 0xda || marker == 0xd9)
            break;
        if (marker == 0x01 || (marker >= 0xd0 && marker <= 0xd7))
            continue;
        if (offset + 2 > size)
            return fail(e, 1, "Truncated JPEG marker");
        const size_t length = (size_t)data[offset] * 256 + data[offset + 1];
        if (length < 2 || length > size - offset)
            return fail(e, 1, "Invalid JPEG marker length");
        if ((marker >= 0xe0 && marker <= 0xef) || marker == 0xfe) {
            metadata += length;
            if (metadata > max)
                return fail(e, 3, "JPEG metadata exceeds budget");
        }
        offset += length;
    }
    return 1;
}
typedef struct DecodeState {
    struct jpeg_decompress_struct c;
    JpegError error;
    unsigned char* icc;
    unsigned int icc_size;
} DecodeState;
int snow_jpeg_decode(const uint8_t* data, size_t size, const SnowDecodeOptions* options,
                     int inspect, SnowDesc* desc, SnowBuffers buffers, SnowError* e) {
    if (size < 2)
        return fail(e, 1, "Truncated JPEG input");
    if (size > ULONG_MAX)
        return fail(e, 3, "JPEG input exceeds native source size");
    if (!check_metadata(data, size, options->max_metadata_bytes, e))
        return 0;
    if (!snow_jpeg_metadata(data, size, buffers, e))
        return 0;
    DecodeState* state = (DecodeState*)calloc(1, sizeof(*state));
    if (!state)
        return fail(e, 3, "JPEG state allocation failed");
    state->c.err = jpeg_std_error(&state->error.pub);
    state->error.pub.error_exit = fail_jpeg;
    state->error.pub.output_message = quiet_jpeg;
    state->error.result = e;
    if (setjmp(state->error.jump)) {
        free(state->icc);
        jpeg_destroy_decompress(&state->c);
        free(state);
        return 0;
    }
    jpeg_create_decompress(&state->c);
    state->c.mem->max_memory_to_use =
        (long)(options->max_memory_bytes > 0x7fffffff ? 0x7fffffff : options->max_memory_bytes);
    jpeg_mem_src(&state->c, data, (unsigned long)size);
    jpeg_save_markers(&state->c, JPEG_APP0 + 2, 0xffff);
    jpeg_read_header(&state->c, TRUE);
    if (state->c.data_precision != 8) {
        jpeg_destroy_decompress(&state->c);
        free(state);
        return fail(e, 2, "Only 8-bit JPEG sample decoding is currently supported");
    }
    if (jpeg_read_icc_profile(&state->c, &state->icc, &state->icc_size)) {
        uint8_t* out = buffers.allocate(buffers.context, 1, state->icc_size);
        if (!out && state->icc_size) {
            free(state->icc);
            jpeg_destroy_decompress(&state->c);
            free(state);
            return fail(e, 3, "ICC allocation refused");
        }
        memcpy(out, state->icc, state->icc_size);
        free(state->icc);
        state->icc = NULL;
    }
    desc->width = state->c.image_width;
    desc->height = state->c.image_height;
    desc->sample = 0;
    desc->significant_bits = 8;
    desc->transfer = 1;
    desc->primaries = 1;
    desc->orientation = snow_jpeg_orientation(data, size);
    desc->has_alpha = 0;
    desc->frame_count = 1;
    if (!inspect) {
        state->c.out_color_space = JCS_EXT_RGBA;
        if (options->target_width && options->target_height) {
            unsigned numerator = 1;
            while (numerator < 8 &&
                   (((uint64_t)state->c.image_width * numerator + 7) / 8 < options->target_width ||
                    ((uint64_t)state->c.image_height * numerator + 7) / 8 < options->target_height))
                ++numerator;
            state->c.scale_num = numerator;
            state->c.scale_denom = 8;
        }
        jpeg_calc_output_dimensions(&state->c);
        const uint64_t bytes = (uint64_t)state->c.output_width * state->c.output_height * 4;
        if (bytes > options->max_output_bytes || bytes > SIZE_MAX) {
            jpeg_destroy_decompress(&state->c);
            free(state);
            return fail(e, 3, "JPEG pixels exceed budget");
        }
        uint8_t* output = buffers.allocate(buffers.context, 0, (size_t)bytes);
        if (!output) {
            jpeg_destroy_decompress(&state->c);
            free(state);
            return fail(e, 3, "JPEG allocation refused");
        }
        jpeg_start_decompress(&state->c);
        desc->width = state->c.output_width;
        desc->height = state->c.output_height;
        while (state->c.output_scanline < state->c.output_height) {
            JSAMPROW row = output + (size_t)state->c.output_scanline * desc->width * 4;
            if (jpeg_read_scanlines(&state->c, &row, 1) != 1) {
                jpeg_destroy_decompress(&state->c);
                free(state);
                return fail(e, 4, "JPEG decoding made no progress");
            }
        }
        jpeg_finish_decompress(&state->c);
        if (state->error.pub.num_warnings) {
            jpeg_destroy_decompress(&state->c);
            free(state);
            return fail(e, 1, "JPEG stream contained malformed or truncated data");
        }
    }
    jpeg_destroy_decompress(&state->c);
    free(state);
    return 1;
}
typedef struct EncodeState {
    struct jpeg_compress_struct c;
    JpegError error;
    struct jpeg_destination_mgr destination;
    uint8_t buffer[16384];
    uint8_t* row;
    SnowWrite write;
    void* context;
    uint64_t written, maximum;
} EncodeState;
static void destination_init(j_compress_ptr c) {
    EncodeState* s = (EncodeState*)c;
    s->destination.next_output_byte = s->buffer;
    s->destination.free_in_buffer = sizeof(s->buffer);
}
static int destination_write(EncodeState* s, size_t amount) {
    if (amount > s->maximum - s->written || !s->write(s->context, s->buffer, amount)) {
        s->error.result->code = 3;
        snprintf(s->error.result->message, sizeof(s->error.result->message),
                 "JPEG output exceeds budget or sink failed");
        longjmp(s->error.jump, 1);
    }
    s->written += amount;
    return 1;
}
static boolean destination_full(j_compress_ptr c) {
    EncodeState* s = (EncodeState*)c;
    destination_write(s, sizeof(s->buffer));
    destination_init(c);
    return TRUE;
}
static void destination_finish(j_compress_ptr c) {
    EncodeState* s = (EncodeState*)c;
    destination_write(s, sizeof(s->buffer) - s->destination.free_in_buffer);
}
static int encode_rows(const SnowDesc* desc, const uint8_t* icc, size_t icc_size,
                       const SnowEncodeOptions* options, SnowReadRow read, void* read_context,
                       SnowWrite write, void* context, SnowError* e) {
    if (desc->sample != 0 || options->lossless || options->exact)
        return fail(e, 2, "JPEG requires lossy 8-bit input");
    EncodeState* s = (EncodeState*)calloc(1, sizeof(*s));
    if (!s)
        return fail(e, 3, "JPEG encoder allocation failed");
    s->write = write;
    s->context = context;
    s->maximum = options->max_output_bytes;
    s->c.err = jpeg_std_error(&s->error.pub);
    s->error.pub.error_exit = fail_jpeg;
    s->error.pub.output_message = quiet_jpeg;
    s->error.result = e;
    if (setjmp(s->error.jump)) {
        free(s->row);
        jpeg_destroy_compress(&s->c);
        free(s);
        return 0;
    }
    jpeg_create_compress(&s->c);
    s->destination.init_destination = destination_init;
    s->destination.empty_output_buffer = destination_full;
    s->destination.term_destination = destination_finish;
    s->c.dest = &s->destination;
    s->c.image_width = desc->width;
    s->c.image_height = desc->height;
    s->c.input_components = 4;
    s->c.in_color_space = JCS_EXT_RGBA;
    jpeg_set_defaults(&s->c);
    jpeg_set_quality(&s->c, (int)options->quality, TRUE);
    s->c.comp_info[0].h_samp_factor = options->jpeg_subsampling == 444 ? 1 : 2;
    s->c.comp_info[0].v_samp_factor = options->jpeg_subsampling == 420 ? 2 : 1;
    if (options->progressive)
        jpeg_simple_progression(&s->c);
    s->row = (uint8_t*)malloc((size_t)desc->width * 4);
    if (!s->row) {
        jpeg_destroy_compress(&s->c);
        free(s);
        return fail(e, 3, "JPEG row allocation failed");
    }
    jpeg_start_compress(&s->c, TRUE);
    if (icc_size)
        jpeg_write_icc_profile(&s->c, icc, (unsigned int)icc_size);
    while (s->c.next_scanline < s->c.image_height) {
        if (!read(read_context, s->c.next_scanline, s->row, (size_t)desc->width * 4)) {
            free(s->row);
            jpeg_destroy_compress(&s->c);
            free(s);
            return fail(e, 1, "JPEG row source failed");
        }
        for (size_t x = 3; x < (size_t)desc->width * 4; x += 4) {
            if (s->row[x] != 255) {
                free(s->row);
                jpeg_destroy_compress(&s->c);
                free(s);
                return fail(e, 2, "JPEG cannot preserve alpha; flatten explicitly");
            }
        }
        JSAMPROW row = s->row;
        jpeg_write_scanlines(&s->c, &row, 1);
    }
    jpeg_finish_compress(&s->c);
    free(s->row);
    jpeg_destroy_compress(&s->c);
    free(s);
    return 1;
}
typedef struct MemoryRows {
    const uint8_t* pixels;
    size_t stride;
} MemoryRows;
static int memory_row(void* context, uint32_t row, uint8_t* dest, size_t size) {
    MemoryRows* m = (MemoryRows*)context;
    memcpy(dest, m->pixels + row * m->stride, size);
    return 1;
}
int snow_jpeg_encode(const SnowDesc* desc, const uint8_t* pixels, size_t size, const uint8_t* icc,
                     size_t icc_size, const SnowEncodeOptions* options, SnowWrite write,
                     void* context, SnowError* e) {
    (void)size;
    MemoryRows rows = {pixels, (size_t)desc->width * 4};
    return encode_rows(desc, icc, icc_size, options, memory_row, &rows, write, context, e);
}
int snow_jpeg_encode_rows(const SnowDesc* desc, const SnowEncodeOptions* options, SnowReadRow read,
                          void* read_context, SnowWrite write, void* context, SnowError* e) {
    return encode_rows(desc, NULL, 0, options, read, read_context, write, context, e);
}

static uint16_t exif_u16(const uint8_t* p, int little) {
    return little ? (uint16_t)(p[0] | p[1] << 8) : (uint16_t)(p[0] << 8 | p[1]);
}
static uint32_t exif_u32(const uint8_t* p, int little) {
    return little
               ? (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24
               : (uint32_t)p[0] << 24 | (uint32_t)p[1] << 16 | (uint32_t)p[2] << 8 | (uint32_t)p[3];
}
uint32_t snow_exif_orientation(const uint8_t* data, size_t size) {
    if (size >= 6 && !memcmp(data, "Exif\0\0", 6)) {
        data += 6;
        size -= 6;
    }
    if (size < 8 || !((data[0] == 'I' && data[1] == 'I') || (data[0] == 'M' && data[1] == 'M')))
        return 1;
    const int little = data[0] == 'I';
    if (exif_u16(data + 2, little) != 42)
        return 1;
    const size_t offset = exif_u32(data + 4, little);
    if (offset > size - 2)
        return 1;
    const size_t count = exif_u16(data + offset, little);
    if (count > (size - offset - 2) / 12)
        return 1;
    for (size_t i = 0; i < count; ++i) {
        const uint8_t* entry = data + offset + 2 + i * 12;
        if (exif_u16(entry, little) == 0x112 && exif_u16(entry + 2, little) == 3 &&
            exif_u32(entry + 4, little) == 1) {
            const uint32_t orientation = exif_u16(entry + 8, little);
            return orientation >= 1 && orientation <= 8 ? orientation : 1;
        }
    }
    return 1;
}
uint32_t snow_jpeg_orientation(const uint8_t* data, size_t size) {
    size_t offset = 2;
    while (offset + 1 < size) {
        if (data[offset++] != 0xff)
            return 1;
        while (offset < size && data[offset] == 0xff)
            ++offset;
        if (offset == size)
            return 1;
        const unsigned marker = data[offset++];
        if (marker == 0xda || marker == 0xd9)
            return 1;
        if (marker == 0x01 || (marker >= 0xd0 && marker <= 0xd7))
            continue;
        if (offset + 2 > size)
            return 1;
        const size_t length = (size_t)data[offset] * 256 + data[offset + 1];
        if (length < 2 || length > size - offset)
            return 1;
        if (marker == 0xe1 && length > 8 && !memcmp(data + offset + 2, "Exif\0\0", 6))
            return snow_exif_orientation(data + offset + 2, length - 2);
        offset += length;
    }
    return 1;
}

int snow_jpeg_metadata(const uint8_t* data, size_t size, SnowBuffers b, SnowError* e) {
    size_t offset = 2;
    int have_exif = 0, have_xmp = 0;
    while (offset + 1 < size) {
        if (data[offset++] != 0xff)
            return 1;
        while (offset < size && data[offset] == 0xff)
            ++offset;
        if (offset == size)
            return 1;
        const unsigned marker = data[offset++];
        if (marker == 0xda || marker == 0xd9)
            return 1;
        if (marker == 0x01 || (marker >= 0xd0 && marker <= 0xd7))
            continue;
        if (offset + 2 > size)
            return 1;
        const size_t length = (size_t)data[offset] * 256 + data[offset + 1];
        if (length < 2 || length > size - offset)
            return 1;
        const uint8_t* payload = data + offset + 2;
        size_t payload_size = length - 2;
        unsigned slot = 0;
        size_t skip = 0;
        if (marker == 0xe1 && !have_exif && payload_size >= 6 && !memcmp(payload, "Exif\0\0", 6)) {
            slot = 2;
            skip = 6;
            have_exif = 1;
        }
        static const char xmp_header[] = "http://ns.adobe.com/xap/1.0/";
        if (marker == 0xe1 && !have_xmp && payload_size >= sizeof(xmp_header) &&
            !memcmp(payload, xmp_header, sizeof(xmp_header))) {
            slot = 3;
            skip = sizeof(xmp_header);
            have_xmp = 1;
        }
        if (slot && payload_size > skip) {
            uint8_t* output = b.allocate(b.context, slot, payload_size - skip);
            if (!output)
                return fail(e, 3, "JPEG metadata allocation refused");
            memcpy(output, payload + skip, payload_size - skip);
        }
        offset += length;
    }
    return 1;
}
