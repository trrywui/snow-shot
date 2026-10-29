#ifndef SNOW_SHOT_PRESENTATION_SCREENSHOTCLIPBOARDFILEPAYLOAD_H
#define SNOW_SHOT_PRESENTATION_SCREENSHOTCLIPBOARDFILEPAYLOAD_H

#include <QByteArray>
#include <QList>
#include <QUrl>

#ifdef Q_OS_WIN
namespace snow_shot::presentation::clipboard_detail {
struct WindowsClipboardFileData {
    QByteArray paths;
    QByteArray wideUrl;
    QByteArray url;
    QByteArray preferredDropEffect;

    [[nodiscard]] bool isValid() const {
        return !paths.isEmpty();
    }
};

// CF_HDROP is a DROPFILES header followed by double-null-terminated UTF-16 paths.
// General and mixed URLs stay with Qt's MIME publisher instead of losing remote entries.
[[nodiscard]] WindowsClipboardFileData encodeWindowsClipboardFiles(const QList<QUrl>& urls);
} // namespace snow_shot::presentation::clipboard_detail
#endif

#endif
