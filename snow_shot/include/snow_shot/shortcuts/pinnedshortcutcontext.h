#pragma once

#include <QString>

namespace snow_shot::shortcuts {
// Animated pins and static-content editing never handle these commands at the same time.
inline bool pinnedShortcutActionsMayConflict(const QString& first, const QString& second) {
    const auto animation = [](const QString& id) {
        return id == QStringLiteral("toggle_playback") || id == QStringLiteral("next_frame") ||
               id == QStringLiteral("previous_frame") ||
               id.endsWith(QStringLiteral("playback_speed"));
    };
    const auto staticContent = [](const QString& id) {
        return id == QStringLiteral("drawing_mode") || id == QStringLiteral("resize_window") ||
               id == QStringLiteral("show_text_recognition_results");
    };
    return !((animation(first) && staticContent(second)) ||
             (animation(second) && staticContent(first)));
}
} // namespace snow_shot::shortcuts
