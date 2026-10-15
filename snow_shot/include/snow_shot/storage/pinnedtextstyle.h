#ifndef SNOW_SHOT_STORAGE_PINNEDTEXTSTYLE_H
#define SNOW_SHOT_STORAGE_PINNEDTEXTSTYLE_H

#include <QColor>
#include <QString>

namespace snow_shot::storage {

// Resolved at import time, independently of later preference or palette changes.
struct PinnedTextStyle final {
    int maximumWidth = 1024;
    QString fontFamily;
    int fontSize = 16;
    QColor textColor = Qt::black;
    QColor backgroundColor = Qt::white;

    [[nodiscard]] bool isValid() const {
        return maximumWidth >= 100 && maximumWidth <= 2000 && fontSize >= 8 && fontSize <= 96 &&
               !fontFamily.isEmpty() && fontFamily.size() <= 1024 && textColor.isValid() &&
               backgroundColor.isValid();
    }

    friend bool operator==(const PinnedTextStyle&, const PinnedTextStyle&) = default;
};

} // namespace snow_shot::storage

#endif // SNOW_SHOT_STORAGE_PINNEDTEXTSTYLE_H
