#pragma once

#include <QCoreApplication>
#include <QScopedValueRollback>
#include <QWindow>
#include <private/qhighdpiscaling_p.h>
#include <private/qwindow_p.h>
#include <qpa/qplatformwindow.h>

namespace safe_area_tests {
// The callback may inspect geometry and send synchronous events, but must not
// dispatch queued events or paint while the native handle is substituted.
template <typename Verify> void exercise(QWindow& window, Verify verify) {
    class SafeAreaWindow final : public QPlatformWindow {
      public:
        explicit SafeAreaWindow(QWindow* window) : QPlatformWindow(window) {}
        QMargins safeAreaMargins() const override {
            return margins;
        }
        QMargins margins;
    } platform(&window);
    platform.setGeometry(QHighDpi::toNativePixels(window.geometry(), &window));
    const QScopedValueRollback handle(QWindowPrivate::get(&window)->platformWindow,
                                      static_cast<QPlatformWindow*>(&platform));
    for (const QMargins margins : {QMargins(), QMargins(0, 32, 0, 0), QMargins(0, 10, 0, 0),
                                   QMargins(), QMargins(7, 32, 9, 5), QMargins()}) {
        platform.margins = QHighDpi::toNativePixels(margins, &window);
        QEvent change(QEvent::SafeAreaMarginsChange);
        QCoreApplication::sendEvent(&window, &change);
        verify(margins);
    }
}
} // namespace safe_area_tests
