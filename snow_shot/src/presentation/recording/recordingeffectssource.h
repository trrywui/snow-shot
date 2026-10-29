#pragma once

#include "snow_recording_effects.h"
#include <QImage>
#include <QRect>
#include <QString>
#include <functional>
#include <memory>
#include <vector>

struct RecordingEffectsFrame {
    quint64 generation = 0;
    quint64 revision = 0;
    QSize output;
    struct Tile {
        QRect rect;
        QImage image;
        QSize coordinateSize;
    };
    // Images borrow the native frame. Destroy images before releasing the owner.
    std::shared_ptr<void> lease;
    std::vector<Tile> tiles;
    QString error;
};

// Injectable observation boundary. A stopped source must join before returning.
class RecordingEffectsSource {
  public:
    virtual ~RecordingEffectsSource() = default;
    virtual bool start(const SnowRecordingEffectsConfig& config, std::function<void()> notify,
                       QString& error) = 0;
    virtual bool configure(const SnowRecordingEffectsConfig& config, QString& error) = 0;
    virtual void stop() = 0;
    virtual std::shared_ptr<RecordingEffectsFrame> acquire() = 0;
};

[[nodiscard]] std::unique_ptr<RecordingEffectsSource> createNativeRecordingEffectsSource();
[[nodiscard]] std::shared_ptr<RecordingEffectsFrame>
takeRecordingEffectsFrame(SnowRecordingEffectsFrame* native);
