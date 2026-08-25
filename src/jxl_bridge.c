#include <jxl/color_encoding.h>
#include <jxl/encode.h>
#include <stdint.h>
#include <stdlib.h>

typedef struct PressJxlData {
    uint8_t *data;
    size_t size;
} PressJxlData;

PressJxlData *press_jxl_encode(const uint8_t *pixels,
                               size_t size,
                               uint32_t width,
                               uint32_t height,
                               int has_alpha,
                               int lossless,
                               float distance) {
    const size_t channels = has_alpha ? 4 : 3;
    if (!pixels || !width || !height || width > SIZE_MAX / height ||
        (size_t)width * height > SIZE_MAX / channels ||
        size != (size_t)width * height * channels) {
        return NULL;
    }

    JxlEncoder *encoder = JxlEncoderCreate(NULL);
    if (!encoder) {
        return NULL;
    }

    JxlBasicInfo info;
    JxlEncoderInitBasicInfo(&info);
    info.xsize = width;
    info.ysize = height;
    info.bits_per_sample = 8;
    info.num_color_channels = 3;
    info.uses_original_profile = lossless ? JXL_TRUE : JXL_FALSE;
    if (has_alpha) {
        info.alpha_bits = 8;
        info.num_extra_channels = 1;
    }
    if (JxlEncoderSetBasicInfo(encoder, &info) != JXL_ENC_SUCCESS) {
        JxlEncoderDestroy(encoder);
        return NULL;
    }

    JxlColorEncoding color;
    JxlColorEncodingSetToSRGB(&color, JXL_FALSE);
    if (JxlEncoderSetColorEncoding(encoder, &color) != JXL_ENC_SUCCESS) {
        JxlEncoderDestroy(encoder);
        return NULL;
    }

    JxlEncoderFrameSettings *settings =
        JxlEncoderFrameSettingsCreate(encoder, NULL);
    if (!settings ||
        JxlEncoderFrameSettingsSetOption(
            settings, JXL_ENC_FRAME_SETTING_EFFORT, 5) != JXL_ENC_SUCCESS ||
        (lossless
             ? JxlEncoderSetFrameLossless(settings, JXL_TRUE)
             : JxlEncoderSetFrameDistance(settings, distance)) !=
            JXL_ENC_SUCCESS) {
        JxlEncoderDestroy(encoder);
        return NULL;
    }

    JxlPixelFormat format;
    format.num_channels = (uint32_t)channels;
    format.data_type = JXL_TYPE_UINT8;
    format.endianness = JXL_NATIVE_ENDIAN;
    format.align = 0;
    if (JxlEncoderAddImageFrame(settings, &format, pixels, size) !=
        JXL_ENC_SUCCESS) {
        JxlEncoderDestroy(encoder);
        return NULL;
    }
    JxlEncoderCloseInput(encoder);

    size_t capacity = 4096;
    uint8_t *output = malloc(capacity);
    if (!output) {
        JxlEncoderDestroy(encoder);
        return NULL;
    }
    uint8_t *next = output;
    size_t available = capacity;
    JxlEncoderStatus status;
    do {
        status = JxlEncoderProcessOutput(encoder, &next, &available);
        if (status == JXL_ENC_NEED_MORE_OUTPUT) {
            const size_t used = (size_t)(next - output);
            if (capacity > SIZE_MAX / 2) {
                free(output);
                JxlEncoderDestroy(encoder);
                return NULL;
            }
            capacity *= 2;
            uint8_t *grown = realloc(output, capacity);
            if (!grown) {
                free(output);
                JxlEncoderDestroy(encoder);
                return NULL;
            }
            output = grown;
            next = output + used;
            available = capacity - used;
        }
    } while (status == JXL_ENC_NEED_MORE_OUTPUT);
    JxlEncoderDestroy(encoder);
    if (status != JXL_ENC_SUCCESS) {
        free(output);
        return NULL;
    }

    PressJxlData *encoded = malloc(sizeof(*encoded));
    if (!encoded) {
        free(output);
        return NULL;
    }
    encoded->data = output;
    encoded->size = (size_t)(next - output);
    return encoded;
}

const uint8_t *press_jxl_data(const PressJxlData *encoded) {
    return encoded ? encoded->data : NULL;
}

size_t press_jxl_size(const PressJxlData *encoded) {
    return encoded ? encoded->size : 0;
}

void press_jxl_free(PressJxlData *encoded) {
    if (encoded) {
        free(encoded->data);
        free(encoded);
    }
}
