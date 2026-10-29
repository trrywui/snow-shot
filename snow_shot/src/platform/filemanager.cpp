#include "snow_shot/platform/filemanager.h"

#include <QDesktopServices>
#include <QDir>
#include <QFileInfo>
#include <QProcess>

namespace snow_shot::platform {

bool revealInFileManager(const QString& path, const FileManagerLauncher& launcher) {
    const QFileInfo file(path);
    if (path.isEmpty() || (!file.isFile() && !file.isDir()))
        return false;
    const auto openDirectory = [&launcher](const QString& directory) {
        const QUrl url = QUrl::fromLocalFile(directory);
        return launcher.openDirectory ? launcher.openDirectory(url)
                                      : QDesktopServices::openUrl(url);
    };
    if (file.isDir())
        return openDirectory(file.absoluteFilePath());
#if defined(Q_OS_WIN) || defined(Q_OS_MACOS)
    QString program;
    QStringList arguments;
    QString nativeArguments;
#ifdef Q_OS_WIN
    program = QStringLiteral("explorer.exe");
    nativeArguments =
        QStringLiteral("/select,\"%1\"").arg(QDir::toNativeSeparators(file.absoluteFilePath()));
#else
    program = QStringLiteral("/usr/bin/open");
    arguments = {QStringLiteral("-R"), file.absoluteFilePath()};
#endif
    if (launcher.startDetached)
        return launcher.startDetached(program, arguments, nativeArguments);
    QProcess process;
    process.setProgram(program);
    process.setArguments(arguments);
#ifdef Q_OS_WIN
    process.setNativeArguments(nativeArguments);
#endif
    return process.startDetached();
#else
    return openDirectory(file.absolutePath());
#endif
}

} // namespace snow_shot::platform
