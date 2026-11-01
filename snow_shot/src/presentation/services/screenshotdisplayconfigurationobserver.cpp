#include "snow_shot/presentation/screenshotdisplayconfigurationobserver.h"

#include "icon_renderer.h"
#include "snow_shot/diagnostics/diagnostics.h"

#include <QGuiApplication>
#include <QRect>
#include <QScreen>

#include <utility>

namespace {
void traceDisplayChange(const char* reason) {
    const auto screens = QGuiApplication::screens();
    snow_shot::diagnostics::logEvent(QStringLiteral("snow_shot.capture_trace"),
                                     QStringLiteral("display.notification"),
                                     {{QStringLiteral("reason"), QString::fromLatin1(reason)},
                                      {QStringLiteral("count"), screens.size()}});
    for (qsizetype index = 0; index < screens.size(); ++index) {
        const auto* screen = screens[index];
        const auto geometry = screen->geometry();
        snow_shot::diagnostics::logEvent(QStringLiteral("snow_shot.capture_trace"),
                                         QStringLiteral("display.snapshot"),
                                         {{QStringLiteral("display_index"), index},
                                          {QStringLiteral("x"), geometry.x()},
                                          {QStringLiteral("y"), geometry.y()},
                                          {QStringLiteral("width"), geometry.width()},
                                          {QStringLiteral("height"), geometry.height()},
                                          {QStringLiteral("scale"), screen->devicePixelRatio()},
                                          {QStringLiteral("refresh_rate"), screen->refreshRate()}});
    }
}
} // namespace

ScreenshotDisplayConfigurationObserver::ScreenshotDisplayConfigurationObserver(
    ChangeHandler onChanged, QObject* parent)
    : QObject(parent), m_onChanged(std::move(onChanged)) {}

void ScreenshotDisplayConfigurationObserver::connectApplicationSignals(
    QGuiApplication* application) {
    if (application == nullptr) {
        return;
    }

    QObject::connect(application, &QGuiApplication::screenAdded, this,
                     [this](QScreen* screen) { handleScreenAdded(screen); });
    QObject::connect(application, &QGuiApplication::screenRemoved, this,
                     [this](QScreen* screen) { handleScreenRemoved(screen); });
    QObject::connect(application, &QGuiApplication::applicationStateChanged, this,
                     [this](Qt::ApplicationState state) { handleApplicationStateChanged(state); });
}

void ScreenshotDisplayConfigurationObserver::observeCurrentScreens() {
    observeScreens(QGuiApplication::screens());
    traceDisplayChange("initial_snapshot");
}

void ScreenshotDisplayConfigurationObserver::observeScreens(const QList<QScreen*>& screens) {
    for (QScreen* screen : screens) {
        observeScreen(screen);
    }
}

void ScreenshotDisplayConfigurationObserver::observeScreen(QScreen* screen) {
    if (screen == nullptr || m_observedScreens.contains(screen)) {
        return;
    }

    m_observedScreens.insert(screen);
    QObject::connect(screen, &QObject::destroyed, this,
                     [this, screen]() { m_observedScreens.remove(screen); });
    QObject::connect(screen, &QScreen::geometryChanged, this,
                     [this](const QRect&) { handleObservedScreenChanged(); });
    QObject::connect(screen, &QScreen::virtualGeometryChanged, this,
                     [this](const QRect&) { handleObservedScreenChanged(); });
    QObject::connect(screen, &QScreen::logicalDotsPerInchChanged, this,
                     [this](qreal) { handleObservedScreenChanged(); });
    QObject::connect(screen, &QScreen::physicalDotsPerInchChanged, this,
                     [this](qreal) { handleObservedScreenChanged(); });
}

void ScreenshotDisplayConfigurationObserver::handleScreenAdded(QScreen* screen) {
    traceDisplayChange("screen_added");
    notifyChanged();
    observeScreen(screen);
}

void ScreenshotDisplayConfigurationObserver::handleScreenRemoved(QScreen* screen) {
    traceDisplayChange("screen_removed");
    m_observedScreens.remove(screen);
    notifyChanged();
}

void ScreenshotDisplayConfigurationObserver::handleApplicationStateChanged(
    Qt::ApplicationState state) {
    snow_shot::diagnostics::logEvent(QStringLiteral("snow_shot.capture_trace"),
                                     QStringLiteral("application.state_changed"),
                                     {{QStringLiteral("status"), static_cast<int>(state)}});
    if (state == Qt::ApplicationHidden || state == Qt::ApplicationSuspended) {
        traceDisplayChange(state == Qt::ApplicationHidden ? "application_hidden"
                                                          : "application_suspended");
        // Hidden/suspended windows do not need their toolbar rasters retained at the active budget.
        adqt::icons::trimIconCache(512 * 1024);
        notifyChanged();
    }
}

void ScreenshotDisplayConfigurationObserver::handleObservedScreenChanged() {
    traceDisplayChange("screen_metrics_changed");
    notifyChanged();
}

void ScreenshotDisplayConfigurationObserver::notifyChanged() {
    if (m_onChanged) {
        m_onChanged();
    }
}
