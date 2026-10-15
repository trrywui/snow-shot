#include "snow_shot/presentation/screenshotclipboardcontent.h"
#include "snow_shot/storage/settingsadapters.h"
#include "snow_shot/storage/applicationstorage.h"

#include <QCoreApplication>
#include <QFont>
#include <QFontInfo>
#include <QGuiApplication>
#include <QPalette>
#include <QThread>

snow_shot::storage::PinnedTextStyle ScreenshotClipboardContentReader::capturePinnedTextStyle() {
    Q_ASSERT(QCoreApplication::instance() &&
             QThread::currentThread() == QCoreApplication::instance()->thread());
    const snow_shot::storage::PinToScreenSettings settings;
    snow_shot::storage::PinnedTextStyle style;
    const bool initialized = snow_shot::storage::ApplicationStorage::instance().isInitialized();
    if (initialized)
        style.maximumWidth = settings.textMaximumWidth();
    QFont font = QGuiApplication::font();
    if (initialized && !settings.textFontFamily().isEmpty())
        font.setFamily(settings.textFontFamily());
    style.fontFamily = QFontInfo(font).family();
    if (style.fontFamily.isEmpty())
        style.fontFamily = font.family();
    if (initialized)
        style.fontSize = settings.textFontSize();
    style.textColor = initialized ? settings.textColor() : QColor();
    if (!style.textColor.isValid())
        style.textColor = QGuiApplication::palette().color(QPalette::Text);
    style.backgroundColor = initialized ? settings.textBackgroundColor() : QColor();
    if (!style.backgroundColor.isValid())
        style.backgroundColor = QGuiApplication::palette().color(QPalette::Base);
    return style;
}
