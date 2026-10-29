#include "recordingeffectssource.h"

namespace {
struct SnowRecordingEffectsDeleter {
    void operator()(SnowRecordingEffects* handle) const {
        snow_recording_effects_destroy(handle);
    }
};

} // namespace

std::shared_ptr<RecordingEffectsFrame>
takeRecordingEffectsFrame(SnowRecordingEffectsFrame* native) {
    if (native == nullptr) {
        return {};
    }
    auto frame = std::make_shared<RecordingEffectsFrame>();
    frame->lease = std::shared_ptr<void>(native, [](void* value) {
        snow_recording_effects_release_frame(static_cast<SnowRecordingEffectsFrame*>(value));
    });
    SnowRecordingEffectsFrameInfo info{};
    if (snow_recording_effects_frame_info(native, &info) == 0) {
        return {};
    }
    frame->generation = info.generation;
    frame->revision = info.revision;
    frame->output = QSize(static_cast<int>(info.width), static_cast<int>(info.height));
    if (info.error_utf8 != nullptr) {
        frame->error = QString::fromUtf8(info.error_utf8);
    }
    const auto appendLayer = [&frame](const SnowRecordingEffectsFrameInfo& layer) {
        frame->tiles.reserve(frame->tiles.size() + layer.tile_count);
        for (uint32_t i = 0; i < layer.tile_count; ++i) {
            const auto& tile = layer.tiles[i];
            const QRect rect(static_cast<int>(tile.x), static_cast<int>(tile.y),
                             static_cast<int>(tile.width), static_cast<int>(tile.height));
            frame->tiles.push_back(
                {rect,
                 QImage(tile.rgba_premultiplied, rect.width(), rect.height(),
                        static_cast<qsizetype>(tile.stride), QImage::Format_RGBA8888_Premultiplied),
                 QSize(static_cast<int>(layer.width), static_cast<int>(layer.height))});
        }
    };
    appendLayer(info);
    if (snow_recording_effects_frame_keyboard_info(native, &info) == 0) {
        return {};
    }
    appendLayer(info);
    return frame;
}

namespace {
class NativeRecordingEffectsSource final : public RecordingEffectsSource {
  public:
    // The handle owns the native observers (including the low-level input hooks
    // while active), so every path releases it through the same deleter.
    using Handle = std::unique_ptr<SnowRecordingEffects, SnowRecordingEffectsDeleter>;

    ~NativeRecordingEffectsSource() override {
        // Joins the native worker before m_notify is destroyed.
        stop();
    }
    bool start(const SnowRecordingEffectsConfig& config, std::function<void()> notify,
               QString& error) override {
        stop();
        m_notify = std::move(notify);
        m_handle.reset(snow_recording_effects_create(
            &config,
            [](void* context) {
                auto* self = static_cast<NativeRecordingEffectsSource*>(context);
                self->m_notify();
            },
            this));
        if (m_handle != nullptr && snow_recording_effects_set_active(m_handle.get(), 1) != 0) {
            return true;
        }
        error = QString::fromUtf8(snow_recording_effects_last_error());
        stop();
        return false;
    }
    bool configure(const SnowRecordingEffectsConfig& config, QString& error) override {
        if (snow_recording_effects_configure(m_handle.get(), &config) != 0) {
            return true;
        }
        error = QString::fromUtf8(snow_recording_effects_last_error());
        return false;
    }
    void stop() override {
        m_handle.reset();
        m_notify = {};
    }
    std::shared_ptr<RecordingEffectsFrame> acquire() override {
        auto* native = snow_recording_effects_acquire_frame(m_handle.get());
        return takeRecordingEffectsFrame(native);
    }

  private:
    Handle m_handle;
    std::function<void()> m_notify;
};
} // namespace

std::unique_ptr<RecordingEffectsSource> createNativeRecordingEffectsSource() {
    return std::make_unique<NativeRecordingEffectsSource>();
}
