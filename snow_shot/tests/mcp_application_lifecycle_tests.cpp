#include "snow_shot/app/applicationcontroller.h"
#include "snow_shot/app/mcp/screenshotmcpserver.h"
#include "snow_shot/app/mcp/screenshotmcpsession.h"
#include "snow_shot/presentation/languagemanager.h"
#include "snow_shot/presentation/styles/thememanager.h"
#include "snow_shot/storage/applicationstorage.h"
#include "widgets/platform_compatibility.h"

#include <QApplication>
#include <QElapsedTimer>
#include <QEvent>
#include <QPointer>
#include <QStandardPaths>
#include <QTemporaryDir>
#include <QThread>

#include <cstdlib>
#include <iostream>

int runMcpApplicationLifecycleTests(int argc, char** argv);

namespace {
class TestApplication final : public QApplication {
  public:
    using QApplication::QApplication;

    int quitObserverCount() const {
        return receivers(SIGNAL(aboutToQuit()));
    }
};

void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(1);
    }
}
template <typename Predicate> void await(Predicate predicate) {
    QElapsedTimer timer;
    timer.start();
    while (!predicate() && timer.elapsed() < 5000) {
        QCoreApplication::processEvents();
        QThread::msleep(1);
    }
    require(predicate(), "MCP application lifecycle transition completes");
}
} // namespace

// Linked only into test builds, before startup can access the user's configuration.
int runMcpApplicationLifecycleTests(int argc, char** argv) {
    using namespace snow_shot;
    using namespace snow_shot::app::mcp;
    QStandardPaths::setTestModeEnabled(true);
    QTemporaryDir temporary;
    require(temporary.isValid(), "isolated MCP lifecycle storage");
    qputenv("SNOW_SHOT_MCP_DESCRIPTOR",
            temporary.filePath(QStringLiteral("descriptor.json")).toUtf8());
    TestApplication application(argc, argv);
    application.setQuitOnLastWindowClosed(false);
    adqt::widgets::initializePlatformCompatibility(application);
    auto& storage = storage::ApplicationStorage::instance();
    require(storage.initialize({temporary.path(), temporary.path(), 60000}).success,
            "initialize MCP lifecycle storage");
    auto& configuration = storage.configuration();
    require(configuration.setValues(
                {{QStringLiteral("mcp/enabled"), true}, {QStringLiteral("tray/enabled"), false}}),
            "enable MCP without a tray");
    presentation::LanguageManager::instance().initialize();
    presentation::styles::ThemeManager::instance().initialize(application);
    {
        app::ApplicationController controller(application);
        const int quitObservers = application.quitObserverCount();
        for (int iteration = 0; iteration < 3; ++iteration) {
            require(application.quitObserverCount() == quitObservers,
                    "MCP restarts do not accumulate application quit observers");
            auto* server = controller.findChild<ScreenshotMcpServer*>();
            require(server && server->isRunning(), "MCP server starts or restarts");
            QPointer<ScreenshotMcpSession> session = controller.findChild<ScreenshotMcpSession*>();
            require(session, "a running MCP server owns a screenshot session");
            ScreenshotMcpRequest request;
            request.connectionId = 1;
            request.requestId = QStringLiteral("status");
            request.method = QStringLiteral("snow_shot_mcp_status");
            bool completed = false;
            session->request(request, [&](auto response) {
                require(response.ok, "screenshot tools remain available after MCP restarts");
                completed = true;
            });
            require(completed, "session status completes synchronously");
            require(configuration.setValue(QStringLiteral("mcp/enabled"), false),
                    "disable MCP through application configuration");
            await([&] { return !server->isRunning(); });
            require(!session, "stopping MCP retires the terminal screenshot session");
            require(configuration.setValue(QStringLiteral("mcp/enabled"), true),
                    "re-enable MCP through application configuration");
            await([&] { return server->isRunning(); });
        }
    }
    QCoreApplication::sendPostedEvents(nullptr, QEvent::DeferredDelete);
    storage.shutdown();
    std::cout << "MCP application lifecycle tests passed\n";
    return 0;
}
