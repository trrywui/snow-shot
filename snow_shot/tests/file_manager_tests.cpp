#include "snow_shot/platform/filemanager.h"

#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QGuiApplication>
#include <QTemporaryDir>
#include <cstdlib>
#include <iostream>

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(1);
    }
}
} // namespace

int main(int argc, char** argv) {
    // Opt-in native smoke check; the default test never launches a file manager.
    if (argc == 3 && QString::fromLocal8Bit(argv[1]) == QStringLiteral("--native-reveal")) {
        QGuiApplication application(argc, argv);
        return snow_shot::platform::revealInFileManager(application.arguments()[2]) ? 0 : 1;
    }
    QCoreApplication application(argc, argv);
    QTemporaryDir directory;
    require(directory.isValid(), "create file-manager fixture directory");
    const QString path = directory.filePath(QString::fromUtf8("capture 测试, & $(test) #1.png"));
    QFile file(path);
    require(file.open(QIODevice::WriteOnly), "create file-manager fixture file");
    file.close();
    int processCalls = 0;
    int directoryCalls = 0;
    bool launchSucceeds = true;
    snow_shot::platform::FileManagerLauncher launcher;
    launcher.startDetached = [&](const QString& program, const QStringList& arguments,
                                 const QString& nativeArguments) {
        ++processCalls;
#ifdef Q_OS_WIN
        require(program == QStringLiteral("explorer.exe") && arguments.isEmpty() &&
                    nativeArguments ==
                        QStringLiteral("/select,\"%1\"").arg(QDir::toNativeSeparators(path)),
                "Explorer receives one quoted Unicode selection operand without a shell");
#elif defined(Q_OS_MACOS)
        require(program == QStringLiteral("/usr/bin/open") &&
                    arguments == QStringList{QStringLiteral("-R"), path} &&
                    nativeArguments.isEmpty(),
                "Finder receives the entire Unicode path as one argument");
#else
        Q_UNUSED(program);
        Q_UNUSED(arguments);
        Q_UNUSED(nativeArguments);
        require(false, "other platforms open the containing directory");
#endif
        return launchSucceeds;
    };
    launcher.openDirectory = [&](const QUrl& url) {
        ++directoryCalls;
        require(url.isLocalFile() && url.toLocalFile() == directory.path(),
                "directories are opened as correctly encoded local-file URLs");
        return launchSucceeds;
    };
    using snow_shot::platform::revealInFileManager;
    require(revealInFileManager(path, launcher), "reveal an existing file");
#if defined(Q_OS_WIN) || defined(Q_OS_MACOS)
    require(processCalls == 1 && directoryCalls == 0, "select the file, not merely its parent");
#else
    require(directoryCalls == 1, "open the containing directory on other platforms");
#endif
    require(revealInFileManager(directory.path(), launcher), "open a directory's contents");
    launchSucceeds = false;
    require(!revealInFileManager(path, launcher) &&
                !revealInFileManager(directory.path(), launcher),
            "propagate native launcher failures");
    const int previousCalls = processCalls + directoryCalls;
    require(!revealInFileManager({}, launcher) &&
                !revealInFileManager(directory.filePath(QStringLiteral("missing")), launcher),
            "empty and missing targets fail");
    require(processCalls + directoryCalls == previousCalls,
            "invalid targets never launch anything");
    std::cout << "File-manager tests passed\n";
}
