#include <avif/avif.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef struct ImageGuideAvifData {
    avifRWData raw;
} ImageGuideAvifData;

typedef struct ImageGuideAvifHeader {
    uint32_t width;
    uint32_t height;
    uint32_t depth;
    int alpha_present;
    int icc_present;
    int unsupported_transform;
} ImageGuideAvifHeader;

typedef struct ImageGuideAvifDecoded {
    uint32_t width;
    uint32_t height;
    uint32_t depth;
    uint8_t *icc;
    size_t icc_size;
} ImageGuideAvifDecoded;

static int imageguide_avif_header(avifDecoder *decoder,
                                  ImageGuideAvifHeader *header) {
    if (!decoder || !decoder->image || !header || !decoder->image->width ||
        !decoder->image->height || !decoder->image->depth) {
        return 0;
    }
    header->width = decoder->image->width;
    header->height = decoder->image->height;
    header->depth = decoder->image->depth;
    header->alpha_present = decoder->alphaPresent;
    header->icc_present = decoder->image->icc.size != 0;
    header->unsupported_transform =
        (decoder->image->transformFlags & (AVIF_TRANSFORM_CLAP | AVIF_TRANSFORM_IMIR)) != 0 ||
        ((decoder->image->transformFlags & AVIF_TRANSFORM_IROT) &&
         decoder->image->irot.angle != 0);
    return 1;
}

int imageguide_avif_probe_memory(const uint8_t *data,
                                 size_t size,
                                 ImageGuideAvifHeader *header) {
    if (!data || !size || !header) {
        return 0;
    }
    avifDecoder *decoder = avifDecoderCreate();
    if (!decoder) {
        return 0;
    }
    decoder->ignoreExif = AVIF_TRUE;
    decoder->ignoreXMP = AVIF_TRUE;
    int parsed = avifDecoderSetIOMemory(decoder, data, size) == AVIF_RESULT_OK &&
                 avifDecoderParse(decoder) == AVIF_RESULT_OK &&
                 imageguide_avif_header(decoder, header);
    avifDecoderDestroy(decoder);
    return parsed;
}

int imageguide_avif_probe_file(const char *path, ImageGuideAvifHeader *header) {
    if (!path || !header) {
        return 0;
    }
    avifDecoder *decoder = avifDecoderCreate();
    if (!decoder) {
        return 0;
    }
    decoder->ignoreExif = AVIF_TRUE;
    decoder->ignoreXMP = AVIF_TRUE;
    int parsed = avifDecoderSetIOFile(decoder, path) == AVIF_RESULT_OK &&
                 avifDecoderParse(decoder) == AVIF_RESULT_OK &&
                 imageguide_avif_header(decoder, header);
    avifDecoderDestroy(decoder);
    return parsed;
}

int imageguide_avif_decode_memory(const uint8_t *data,
                                  size_t size,
                                  uint32_t image_size_limit,
                                  uint32_t image_dimension_limit,
                                  uint8_t *pixels,
                                  size_t pixels_size,
                                  ImageGuideAvifDecoded *decoded) {
    if (!data || !size || !image_size_limit || !image_dimension_limit || !pixels ||
        !pixels_size || !decoded) {
        return 0;
    }
    memset(decoded, 0, sizeof(*decoded));
    avifDecoder *decoder = avifDecoderCreate();
    avifImage *image = avifImageCreateEmpty();
    if (!decoder || !image) {
        if (decoder) {
            avifDecoderDestroy(decoder);
        }
        if (image) {
            avifImageDestroy(image);
        }
        return 0;
    }
    /* The decoder budget is forwarded to dav1d's frame_size_limit. Keep the
       backend explicit so another auto-selected codec cannot bypass it. */
    decoder->codecChoice = AVIF_CODEC_CHOICE_DAV1D;
    decoder->imageSizeLimit = image_size_limit;
    decoder->imageDimensionLimit = image_dimension_limit;
    decoder->ignoreExif = AVIF_TRUE;
    decoder->ignoreXMP = AVIF_TRUE;
    if (avifDecoderReadMemory(decoder, image, data, size) != AVIF_RESULT_OK ||
        !image->width || !image->height || image->depth > 16 ||
        image->width > image_dimension_limit || image->height > image_dimension_limit ||
        (image->transformFlags & (AVIF_TRANSFORM_CLAP | AVIF_TRANSFORM_IMIR)) != 0 ||
        ((image->transformFlags & AVIF_TRANSFORM_IROT) && image->irot.angle != 0)) {
        avifImageDestroy(image);
        avifDecoderDestroy(decoder);
        return 0;
    }

    avifRGBImage rgb;
    avifRGBImageSetDefaults(&rgb, image);
    rgb.format = AVIF_RGB_FORMAT_RGBA;
    rgb.depth = image->depth > 8 ? 16 : 8;
    rgb.isFloat = AVIF_FALSE;
    uint64_t row_bytes = (uint64_t)image->width * 4 * (rgb.depth > 8 ? 2 : 1);
    uint64_t total_bytes = row_bytes * image->height;
    if (row_bytes > UINT32_MAX || total_bytes > pixels_size) {
        avifImageDestroy(image);
        avifDecoderDestroy(decoder);
        return 0;
    }
    rgb.rowBytes = (uint32_t)row_bytes;
    rgb.pixels = pixels;
    if (avifImageYUVToRGB(image, &rgb) != AVIF_RESULT_OK) {
        avifImageDestroy(image);
        avifDecoderDestroy(decoder);
        return 0;
    }
    decoded->width = image->width;
    decoded->height = image->height;
    decoded->depth = image->depth;
    if (image->icc.data && image->icc.size) {
        decoded->icc = malloc(image->icc.size);
        if (!decoded->icc) {
            memset(decoded, 0, sizeof(*decoded));
            avifImageDestroy(image);
            avifDecoderDestroy(decoder);
            return 0;
        }
        memcpy(decoded->icc, image->icc.data, image->icc.size);
        decoded->icc_size = image->icc.size;
    }
    avifImageDestroy(image);
    avifDecoderDestroy(decoder);
    return 1;
}

void imageguide_avif_decoded_free(ImageGuideAvifDecoded *decoded) {
    if (decoded) {
        free(decoded->icc);
        memset(decoded, 0, sizeof(*decoded));
    }
}

ImageGuideAvifData *imageguide_avif_encode(const uint8_t *pixels,
                                           uint32_t width,
                                           uint32_t height,
                                           int has_alpha,
                                           int quality,
                                           int speed,
                                           int threads,
                                           const uint8_t *profile,
                                           size_t profile_size) {
    const uint32_t channels = has_alpha ? 4 : 3;
    if (!pixels || !width || !height || width > UINT32_MAX / channels) {
        return NULL;
    }

    avifImage *image = avifImageCreate(width, height, 8, AVIF_PIXEL_FORMAT_YUV444);
    if (!image) {
        return NULL;
    }

    /* Without the source profile the file claims nothing, and a browser reads
       untagged AVIF as sRGB — the wrong answer for every wide gamut photo. */
    if (profile && profile_size &&
        avifImageSetProfileICC(image, profile, profile_size) != AVIF_RESULT_OK) {
        avifImageDestroy(image);
        return NULL;
    }

    avifRGBImage rgb;
    avifRGBImageSetDefaults(&rgb, image);
    rgb.format = has_alpha ? AVIF_RGB_FORMAT_RGBA : AVIF_RGB_FORMAT_RGB;
    rgb.pixels = (uint8_t *)pixels;
    rgb.rowBytes = width * channels;
    if (avifImageRGBToYUV(image, &rgb) != AVIF_RESULT_OK) {
        avifImageDestroy(image);
        return NULL;
    }

    avifEncoder *encoder = avifEncoderCreate();
    if (!encoder) {
        avifImageDestroy(image);
        return NULL;
    }
    encoder->codecChoice = AVIF_CODEC_CHOICE_AOM;
    encoder->quality = quality;
    encoder->qualityAlpha = quality;
    encoder->speed = speed;
    encoder->maxThreads = threads;

    ImageGuideAvifData *encoded = malloc(sizeof(*encoded));
    if (!encoded) {
        avifEncoderDestroy(encoder);
        avifImageDestroy(image);
        return NULL;
    }
    encoded->raw.data = NULL;
    encoded->raw.size = 0;
    if (avifEncoderWrite(encoder, image, &encoded->raw) != AVIF_RESULT_OK) {
        avifRWDataFree(&encoded->raw);
        free(encoded);
        encoded = NULL;
    }

    avifEncoderDestroy(encoder);
    avifImageDestroy(image);
    return encoded;
}

const uint8_t *imageguide_avif_data(const ImageGuideAvifData *encoded) {
    return encoded ? encoded->raw.data : NULL;
}

size_t imageguide_avif_size(const ImageGuideAvifData *encoded) {
    return encoded ? encoded->raw.size : 0;
}

void imageguide_avif_free(ImageGuideAvifData *encoded) {
    if (encoded) {
        avifRWDataFree(&encoded->raw);
        free(encoded);
    }
}
