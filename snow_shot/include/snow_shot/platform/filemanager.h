#ifndef SNOW_SHOT_PLATFORM_FILEMANAGER_H
#define SNOW_SHOT_PLATFORM_FILEMANAGER_H

#include <QString>
#include <QStringList>
#include <QUrl>
#include <functional>

namespace snow_shot::platform {

struct FileManagerLauncher {
    std::function<bool(const QString&, const QStringList&, const QString&)> startDetached;
    std::function<bool(const QUrl&)> openDirectory;
};

// Select a file in the platform file manager, or open a directory's contents.
// Empty callbacks use the native launcher. No shell command interpreter is used.
[[nodiscard]] bool revealInFileManager(const QString& path,
                                       const FileManagerLauncher& launcher = {});

} // namespace snow_shot::platform

#endif
