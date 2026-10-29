// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include "snow_shot/presentation/screenshotclipboardcontent.h"
#include <QGuiApplication>
#include <QPalette>
#include <QThread>

// Capture on the GUI thread before rendering persisted text on an import worker.
struct ScreenshotPinnedTextPreparation final {
    QColor baseColor;
    PinnedColorSwatchStyle colorSwatchStyle;
};

inline ScreenshotPinnedTextPreparation captureScreenshotPinnedTextPreparation() {
    Q_ASSERT(QCoreApplication::instance() &&
             QThread::currentThread() == QCoreApplication::instance()->thread());
    return {QGuiApplication::palette().color(QPalette::Base), capturePinnedColorSwatchStyle()};
}

inline std::optional<ScreenshotClipboardContent>
prepareScreenshotPinnedText(const ScreenshotClipboardOriginalContent& original, qreal dpi,
                            const ScreenshotPinnedTextPreparation& preparation) {
    return ScreenshotClipboardContentReader::renderOriginalText(
        original, dpi, preparation.baseColor, {}, preparation.colorSwatchStyle);
}
