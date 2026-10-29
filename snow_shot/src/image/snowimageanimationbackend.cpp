#include "snowimageanimationbridge.h"
#include "snowimageimportoptions.h"

#include <snow/image/animation.h>
#include <snow/image/service.h>

#include <algorithm>
#include <bit>
#include <cstring>
#include <limits>
#include <memory>
#include <span>
#include <string_view>

struct SnowShotAnimationDecoder final {
    snow::image::AnimationDecodeSession session;
};

namespace {
constexpr uint64_t kMiB = 1024U * 1024U;
void setError(char* error, uint64_t capacity, std::string_view message = {}) noexcept {
    if (!error || !capacity)
        return;
    const auto count = static_cast<std::size_t>(std::min(capacity - 1U, uint64_t{message.size()}));
    if (count > 0)
        std::memcpy(error, message.data(), count);
    error[count] = '\0';
}

class BorrowedSource final : public snow::image::ByteSource {
  public:
    explicit BorrowedSource(std::span<const std::byte> bytes) : bytes_(bytes) {}
    snow::image::Result<uint64_t> size() const override {
        return bytes_.size();
    }
    snow::image::Result<std::size_t> read_at(uint64_t offset,
                                             std::span<std::byte> destination) const override {
        if (offset > bytes_.size())
            return std::size_t{0};
        const auto count = std::min(destination.size(), bytes_.size() - std::size_t{offset});
        if (count > 0)
            std::memcpy(destination.data(), bytes_.data() + offset, count);
        return count;
    }
    std::optional<std::span<const std::byte>> contiguous_bytes() const override {
        return bytes_;
    }

  private:
    std::span<const std::byte> bytes_;
};

snow::image::DecodeOptions options(bool firstFrameOnly) {
    if (firstFrameOnly)
        return snow_shot::image_codec::imageImportOptions();
    snow::image::DecodeOptions result;
    result.limits.maximum_input_bytes = 256U * kMiB;
    result.limits.maximum_pixels = 64'000'000;
    result.limits.maximum_owned_output_bytes = 256U * kMiB;
    result.limits.maximum_working_bytes = 512U * kMiB;
    result.limits.maximum_metadata_bytes = 8U * kMiB;
    result.limits.maximum_frames = 10'000;
    return result;
}

uint32_t bridgeFormat(snow::image::Format format) {
    switch (format) {
    case snow::image::Format::gif:
        return SNOW_SHOT_IMAGE_CODEC_FORMAT_GIF;
    case snow::image::Format::png:
        return SNOW_SHOT_IMAGE_CODEC_FORMAT_PNG;
    case snow::image::Format::webp:
        return SNOW_SHOT_IMAGE_CODEC_FORMAT_WEBP;
    default:
        return SNOW_SHOT_IMAGE_CODEC_FORMAT_UNKNOWN;
    }
}

snow::image::AnimationCancellationCheck cancellation(SnowShotAnimationCancel check, void* context) {
    return check ? snow::image::AnimationCancellationCheck([=] { return check(context) != 0; })
                 : snow::image::AnimationCancellationCheck{};
}
} // namespace

uint32_t SNOW_SHOT_IMAGE_CODEC_CALL snow_shot_animation_abi_version(void) {
    return SNOW_SHOT_ANIMATION_CODEC_ABI_VERSION;
}

SnowShotAnimationDecoder* SNOW_SHOT_IMAGE_CODEC_CALL snow_shot_animation_open(
    const uint8_t* encoded, uint64_t size, int first_frame_only, SnowShotAnimationCancel cancelled,
    void* cancel_context, SnowShotAnimationAdmit admit, void* admit_context,
    SnowShotAnimationInfo* info, char* error, uint64_t error_capacity) {
    setError(error, error_capacity);
    if (info)
        *info = {};
    if (!encoded || !size || size > std::numeric_limits<std::size_t>::max())
        return nullptr;
    try {
        const snow::image::Service service;
        snow::image::Input input{
            std::make_shared<BorrowedSource>(std::span(reinterpret_cast<const std::byte*>(encoded),
                                                       static_cast<std::size_t>(size))),
            {}};
        bool staticGif = false;
        auto opened = service.open_animation_decoder(
            input, options(first_frame_only != 0), {}, cancellation(cancelled, cancel_context),
            [&](const snow::image::AnimationInfo& animation) {
                const auto& document = animation.document;
                staticGif =
                    document.format == snow::image::Format::gif && document.frames.size() == 1;
                if (staticGif)
                    return false;
                if (info) {
                    info->width = document.canvas_width;
                    info->height = document.canvas_height;
                    info->frame_count = static_cast<uint32_t>(document.frames.size());
                    info->format = bridgeFormat(document.format);
                    info->orientation = static_cast<uint32_t>(document.metadata.orientation);
                    info->total_play_count = animation.total_play_count;
                    info->color.icc_profile = const_cast<uint8_t*>(
                        reinterpret_cast<const uint8_t*>(document.color.icc_profile.data()));
                    info->color.icc_profile_size = document.color.icc_profile.size();
                    info->color.primaries = static_cast<uint32_t>(document.color.primaries);
                    info->color.transfer = static_cast<uint32_t>(document.color.transfer);
                }
                const auto required =
                    size + uint64_t{document.canvas_width} * document.canvas_height * 16U +
                    8U * kMiB;
                return !admit || admit(admit_context, required) != 0;
            });
        if (!opened) {
            const bool staticImage =
                opened.error().code == snow::image::ErrorCode::unsupported_feature &&
                (opened.error().message == "PNG has no animation." ||
                 opened.error().message == "WebP has no animation.");
            if (!staticGif && !staticImage &&
                opened.error().code != snow::image::ErrorCode::unsupported_format)
                setError(error, error_capacity, opened.error().message);
            return nullptr;
        }
        return new SnowShotAnimationDecoder{std::move(opened).value()};
    } catch (...) {
        setError(error, error_capacity, "Could not open animation decoder.");
        return nullptr;
    }
}

void SNOW_SHOT_IMAGE_CODEC_CALL snow_shot_animation_destroy(SnowShotAnimationDecoder* decoder) {
    delete decoder;
}
int64_t SNOW_SHOT_IMAGE_CODEC_CALL
snow_shot_animation_duration(const SnowShotAnimationDecoder* decoder, uint32_t frame) {
    if (!decoder || frame >= decoder->session.info().document.frames.size())
        return 0;
    return decoder->session.info().document.frames[frame].duration.count();
}
int64_t SNOW_SHOT_IMAGE_CODEC_CALL
snow_shot_animation_index(const SnowShotAnimationDecoder* decoder) {
    return decoder ? decoder->session.frame_index() : -1;
}

int SNOW_SHOT_IMAGE_CODEC_CALL snow_shot_animation_advance(SnowShotAnimationDecoder* decoder,
                                                           SnowShotAnimationCancel cancelled,
                                                           void* cancel_context, char* error,
                                                           uint64_t error_capacity) {
    setError(error, error_capacity);
    if (!decoder)
        return -1;
    try {
        auto result = decoder->session.advance({}, cancellation(cancelled, cancel_context));
        if (result)
            return result.value() ? 1 : 0;
        setError(error, error_capacity, result.error().message);
    } catch (...) {
        setError(error, error_capacity, "Animation decoding failed.");
    }
    return -1;
}

int SNOW_SHOT_IMAGE_CODEC_CALL snow_shot_animation_reset(SnowShotAnimationDecoder* decoder,
                                                         char* error, uint64_t error_capacity) {
    setError(error, error_capacity);
    if (!decoder)
        return 0;
    try {
        const auto result = decoder->session.reset();
        if (result)
            return 1;
        setError(error, error_capacity, result.error().message);
    } catch (...) {
        setError(error, error_capacity, "Animation reset failed.");
    }
    return 0;
}

int SNOW_SHOT_IMAGE_CODEC_CALL snow_shot_animation_snapshot(const SnowShotAnimationDecoder* decoder,
                                                            SnowShotAnimationFrame* output,
                                                            char* error, uint64_t error_capacity) {
    setError(error, error_capacity);
    if (!decoder || !output || output->owner)
        return 0;
    try {
        constexpr bool bgra = std::endian::native == std::endian::little;
        constexpr snow::image::PixelFormat format{snow::image::SampleType::unsigned_integer,
                                                  bgra ? snow::image::ChannelLayout::bgra
                                                       : snow::image::ChannelLayout::rgba,
                                                  snow::image::AlphaMode::premultiplied, 8, true};
        auto result = decoder->session.snapshot_pixels(format);
        if (!result) {
            setError(error, error_capacity, result.error().message);
            return 0;
        }
        auto owner = std::make_unique<snow::image::Image>(std::move(result).value());
        const auto& image = *owner;
        output->data = reinterpret_cast<const uint8_t*>(image.pixels().data());
        output->size = image.pixels().size();
        output->row_stride = image.row_stride();
        output->width = image.width();
        output->height = image.height();
        output->pixel_format = bgra ? SNOW_SHOT_ANIMATION_PIXEL_FORMAT_BGRA8_PREMULTIPLIED
                                    : SNOW_SHOT_ANIMATION_PIXEL_FORMAT_RGBA8_PREMULTIPLIED;
        output->owner = owner.release();
        return 1;
    } catch (...) {
        setError(error, error_capacity, "Could not publish animation frame.");
        return 0;
    }
}

void SNOW_SHOT_IMAGE_CODEC_CALL snow_shot_animation_frame_release(SnowShotAnimationFrame* frame) {
    if (frame) {
        delete static_cast<snow::image::Image*>(frame->owner);
        *frame = {};
    }
}
