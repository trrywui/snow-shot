#include "snow_shot/app/shellimagelaunch.h"
#include "snow_shot/image/supportedimagefiles.h"

#include <QCoreApplication>
#include <QDir>

#include <cstdlib>
#include <iostream>

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(EXIT_FAILURE);
    }
}

void launchArguments() {
    using namespace snow_shot::app;
    const QStringList paths{QDir::temp().filePath(QStringLiteral("image one.png")),
                            QDir::temp().filePath(QStringLiteral("图片 two.JPEG")),
                            QDir::temp().filePath(QStringLiteral("bitmap three.BMP"))};
    auto arguments = shellImageLaunchArguments(paths);
    arguments.prepend(QStringLiteral("snow-shot.exe"));
    arguments.append(QStringLiteral("--show-main-window"));
    arguments.append(QStringLiteral("--skip-startup-update"));
    const auto parsed = parseShellImageLaunchRequest(arguments);
    require(parsed.requested && parsed.valid && parsed.paths == paths,
            "repeated shell operands preserve spaced Unicode paths and order");
    const auto normal = parseShellImageLaunchRequest(
        {QStringLiteral("snow-shot.exe"), QStringLiteral("--autostart")});
    require(!normal.requested && normal.valid && normal.paths.isEmpty(),
            "normal launch flags do not become image requests");

    const QString flag = QStringLiteral("--edit-with-snow-shot");
    for (const QStringList& invalid :
         {QStringList{flag}, QStringList{flag, QString{}},
          QStringList{flag, QStringLiteral("relative.png")},
          QStringList{flag, QStringLiteral("--autostart")}, QStringList{flag, paths.first(), flag},
          QStringList{flag, paths.first() + QChar(0)},
          QStringList{flag, paths.first() + QChar(127)}, QStringList{flag, paths.first() + u'\n'},
          QStringList{flag, paths.first() + u'"'}}) {
        const auto request = parseShellImageLaunchRequest(invalid);
        require(request.requested && !request.valid && request.paths.isEmpty(),
                "malformed shell operands reject the whole request without activation");
    }
#ifdef Q_OS_WIN
    require(!parseShellImageLaunchRequest({flag, QStringLiteral("/rooted.png")}).valid &&
                !parseShellImageLaunchRequest({flag, QStringLiteral("C:relative.png")}).valid,
            "Windows shell operands must include a drive or UNC root");
    const QStringList windowsPaths{QStringLiteral("C:\\images\\one.png"),
                                   QStringLiteral("\\\\server\\share\\图片 two.jpeg")};
    require(parseShellImageLaunchRequest(shellImageLaunchArguments(windowsPaths)).paths ==
                windowsPaths,
            "drive and UNC operands survive forwarding without command-string parsing");
#endif
}

void imageSuffixes() {
    const auto extensions = snow_shot::image::supportedImageFileExtensions();
    require(extensions ==
                QStringList{QStringLiteral("gif"), QStringLiteral("apng"), QStringLiteral("png"),
                            QStringLiteral("jpg"), QStringLiteral("jpeg"), QStringLiteral("webp"),
                            QStringLiteral("jxl"), QStringLiteral("avif"), QStringLiteral("bmp")},
            "shell registration covers the formats accepted by Snow Shot's file image reader");
    require(!extensions.contains(QStringLiteral("txt")) &&
                !extensions.contains(QStringLiteral("html")) &&
                !extensions.contains(QStringLiteral("htm")),
            "formatted-text pinning extensions are excluded from the image shell verb");
}
} // namespace

int main(int argc, char** argv) {
    QCoreApplication application(argc, argv);
    launchArguments();
    imageSuffixes();
    return EXIT_SUCCESS;
}
