#include "snow_shot/app/edition.h"
#include "snow_shot/update/updateservice.h"

#include <QCoreApplication>
#include <QEventLoop>
#include <QTcpServer>
#include <QTimer>
#include <cstdio>
#include <cstdlib>

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        std::fprintf(stderr, "%s\n", message);
        std::exit(1);
    }
}
void events() {
    QEventLoop loop;
    QTimer::singleShot(30, &loop, &QEventLoop::quit);
    loop.exec();
}
} // namespace

int main(int argc, char** argv) {
    QCoreApplication application(argc, argv);
    namespace edition = snow_shot::app::edition;
    require(edition::isMacOS14Personal && !edition::isMini, "personal build retains Full features");
    require(!edition::automaticPinnedTextRecognition, "pinning does not start OCR by default");
    require(edition::qrRecognition && !edition::automaticQrRecognition,
            "manual QR remains available without automatic screenshot recognition");
    require(edition::applicationName() == u"snowshot", "isolated settings identity");
    require(edition::registryName() == u"trrywui.snowshot", "isolated organization identity");
    require(edition::bundleId() == u"io.github.trrywui.snowshot", "isolated native identity");
    require(edition::executableName() == u"snow_shot", "bundled executable name remains valid");

    QTcpServer releases;
    require(releases.listen(QHostAddress::LocalHost), "local update trap starts");
    snow_shot::update::UpdateService::Options options;
    options.githubApiUrl =
        QUrl(QStringLiteral("http://127.0.0.1:%1/releases").arg(releases.serverPort()));
    options.giteeApiUrl = options.githubApiUrl;
    options.allowLocalHttp = true;
    options.startupCheckDelay = std::chrono::milliseconds(1);
    options.automaticCheckInterval = std::chrono::milliseconds(1);
    snow_shot::update::UpdateService updates(options);
    updates.start();
    updates.check(false);
    events();
    require(!updates.busy() && !releases.hasPendingConnections(), "no automatic upstream requests");
    updates.check(true);
    events();
    require(updates.status().state == snow_shot::update::UpdateState::Failed,
            "manual checks explain personal updates");
    require(updates.status().error.contains(u"manual updates"),
            "manual update guidance is visible");
    require(updates.status().error.contains(u"https://github.com/trrywui/snow-shot/releases"),
            "manual updates point to the maintained releases");
    require(updates.status().downloadUrl.isEmpty(), "no official installer is offered");
    require(!updates.busy() && !releases.hasPendingConnections(),
            "manual checks cannot fetch official packages");
    std::puts("macOS 14 personal identity and update tests passed");
}
