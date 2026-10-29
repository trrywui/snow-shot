// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef SNOW_SHOT_TESTS_SPREADSHEET_OPEN_TEST_SUPPORT_H
#define SNOW_SHOT_TESTS_SPREADSHEET_OPEN_TEST_SUPPORT_H

#include <QDesktopServices>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QObject>
#include <QUrl>

// Intercept the platform launcher so host tests never start a real spreadsheet application.
class SpreadsheetOpenTestReceiver final : public QObject {
    Q_OBJECT
  public:
    SpreadsheetOpenTestReceiver() {
        QDesktopServices::setUrlHandler(QStringLiteral("file"), this, "open");
    }
    ~SpreadsheetOpenTestReceiver() override {
        QDesktopServices::unsetUrlHandler(QStringLiteral("file"));
        if (!path.isEmpty()) {
            QFile::remove(path);
            QDir().rmdir(QFileInfo(path).absolutePath());
        }
    }
    QString path;
    int calls = 0;
  public slots:
    void open(const QUrl& url) {
        path = url.toLocalFile();
        ++calls;
    }
};

#endif
