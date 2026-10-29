#ifndef SNOW_SHOT_STORAGE_RECORDINGAREAHISTORY_H
#define SNOW_SHOT_STORAGE_RECORDINGAREAHISTORY_H

#include "snow_shot/storage/configurationschema.h"

#include <QJsonArray>
#include <QRect>

#include <cmath>
#include <limits>

namespace snow_shot::storage {
inline constexpr int recordingAreaHistoryLimit = 100;

inline QString recordingAreaCoordinateSpace() {
#ifdef Q_OS_MACOS
    return QStringLiteral("desktop_points");
#else
    return QStringLiteral("physical_pixels");
#endif
}

struct RecordingAreaHistoryEntry {
    QRect rectangle;
    QString coordinateSpace;
    friend bool operator==(const RecordingAreaHistoryEntry&,
                           const RecordingAreaHistoryEntry&) = default;
};

inline QJsonObject recordingAreaToJson(const RecordingAreaHistoryEntry& entry) {
    return {{QStringLiteral("x"), entry.rectangle.x()},
            {QStringLiteral("y"), entry.rectangle.y()},
            {QStringLiteral("width"), entry.rectangle.width()},
            {QStringLiteral("height"), entry.rectangle.height()},
            {QStringLiteral("coordinate_space"), entry.coordinateSpace}};
}

inline ConfigurationNormalization normalizeRecordingAreaHistory(const QJsonValue& value) {
    if (!value.isArray())
        return {};
    QJsonArray result;
    for (const auto& item : value.toArray()) {
        if (!item.isObject())
            continue;
        const auto object = item.toObject();
        const auto space = object.value(QStringLiteral("coordinate_space")).toString();
        if (space != QStringLiteral("physical_pixels") && space != QStringLiteral("desktop_points"))
            continue;
        const auto integer = [&object](const QString& key, int minimum) {
            const auto number = object.value(key);
            return number.isDouble() && std::isfinite(number.toDouble()) &&
                   std::floor(number.toDouble()) == number.toDouble() &&
                   number.toDouble() >= minimum &&
                   number.toDouble() <= std::numeric_limits<int>::max();
        };
        if (!integer(QStringLiteral("x"), std::numeric_limits<int>::min()) ||
            !integer(QStringLiteral("y"), std::numeric_limits<int>::min()) ||
            !integer(QStringLiteral("width"), 1) || !integer(QStringLiteral("height"), 1))
            continue;
        const int x = object.value(QStringLiteral("x")).toInt();
        const int y = object.value(QStringLiteral("y")).toInt();
        const int width = object.value(QStringLiteral("width")).toInt();
        const int height = object.value(QStringLiteral("height")).toInt();
        if (qint64(x) + width > std::numeric_limits<int>::max() ||
            qint64(y) + height > std::numeric_limits<int>::max())
            continue;
        result.append(recordingAreaToJson({QRect(x, y, width, height), space}));
        if (result.size() > recordingAreaHistoryLimit)
            result.removeFirst();
    }
    return {result, true, result != value.toArray()};
}
} // namespace snow_shot::storage

#endif // SNOW_SHOT_STORAGE_RECORDINGAREAHISTORY_H
