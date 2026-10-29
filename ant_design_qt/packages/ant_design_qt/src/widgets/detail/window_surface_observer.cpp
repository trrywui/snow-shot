#include "window_surface_observer.h"

#include <QEvent>
#include <QPlatformSurfaceEvent>
#include <QScopedValueRollback>
#include <QWidget>
#include <QWindow>

#ifdef Q_OS_MACOS
#include "window_surface_observer_mac_p.h"
#endif

namespace adqt::widgets::detail {
WindowSurfaceObserver::WindowSurfaceObserver(QWidget* widget, std::function<void()> changed,
                                             QObject* parent, std::function<void()> retiring)
    : QObject(parent),
      widget_(widget),
      changed_(std::move(changed)),
      retiring_(std::move(retiring)) {
  widget->installEventFilter(this);
  observeSurface();
}

WindowSurfaceObserver::WindowSurfaceObserver(QWindow* surface, std::function<void()> changed,
                                             QObject* parent, std::function<void()> retiring)
    : QObject(parent),
      surface_(surface),
      changed_(std::move(changed)),
      retiring_(std::move(retiring)),
      widgetSource_(false) {
  surface->installEventFilter(this);
  observeSurface();
}

WindowSurfaceObserver::~WindowSurfaceObserver() {
#ifdef Q_OS_MACOS
  releaseMacWindowObservation(nativeObservation_);
#endif
}

void WindowSurfaceObserver::observeSurface() {
  QWindow* surface =
      widgetSource_ ? (widget_ ? widget_->windowHandle() : nullptr) : surface_.data();
  if (surface_ != surface) {
    if (surface_) surface_->removeEventFilter(this);
    surface_ = surface;
    if (surface_) surface_->installEventFilter(this);
  }
#ifdef Q_OS_MACOS
  const auto view = widgetSource_ ? (widget_ ? widget_->internalWinId() : 0)
                                  : (surface_ && surface_->handle() ? surface_->winId() : 0);
  if (nativeView_ != view) {
    releaseMacWindowObservation(nativeObservation_);
    nativeView_ = view;
    nativeObservation_ = observeMacWindowAttachment(view, [this] { synchronize(); });
  }
#endif
}

void WindowSurfaceObserver::synchronize() {
  const auto synchronizing = synchronizing_;
  if (*synchronizing || (widgetSource_ ? !widget_ : !surface_)) return;
  const QScopedValueRollback guard(*synchronizing, true);
  observeSurface();
  // Copy the callback: a surface transition may delete its observer/owner.
  const auto changed = changed_;
  changed();
}

bool WindowSurfaceObserver::eventFilter(QObject* object, QEvent* event) {
  const QPointer<QObject> lifetime(object);
  // QWidget forwards its QWindow's surface event. Handle the actual surface
  // once, while its native resources are still alive.
  if (event->type() == QEvent::PlatformSurface && object != surface_) return false;
  if (event->type() == QEvent::PlatformSurface &&
      static_cast<QPlatformSurfaceEvent*>(event)->surfaceEventType() ==
          QPlatformSurfaceEvent::SurfaceAboutToBeDestroyed) {
#ifdef Q_OS_MACOS
    releaseMacWindowObservation(nativeObservation_);
    nativeObservation_ = nullptr;
    nativeView_ = 0;
#endif
    const auto retiring = retiring_;
    if (retiring) retiring();
    return !lifetime;
  }
  if (event->type() == QEvent::WinIdChange || event->type() == QEvent::ShowToParent ||
      event->type() == QEvent::HideToParent || event->type() == QEvent::Expose ||
      event->type() == QEvent::PlatformSurface ||
      (object == surface_ && event->type() == QEvent::Show)) {
    synchronize();
  }
  return !lifetime;
}
}  // namespace adqt::widgets::detail
