#include "top_level_popup_window.h"
#include "window_surface_observer.h"

#include <QEvent>
#include <QGuiApplication>
#include <QPointer>
#include <QWidget>
#include <QWindow>
#include <QScopedValueRollback>
#include <memory>
#import <AppKit/AppKit.h>

namespace adqt::widgets::detail {
namespace {
// Qt's transient parent sets a minimum window level on Cocoa, but does not
// establish NSWindow child ordering for tools. Equal-level popups can therefore
// disappear behind an owner that is raised during layout or activation.
class MacPopupOwnership final : public QObject {
  public:
    explicit MacPopupOwnership(QWidget* popup)
        : QObject(popup), popup_(popup),
          popupObserver_(popup, [this] { sync(); }, this, [this] { detach(); }) {}
    ~MacPopupOwnership() override {
        ownerObserver_.reset();
        detach();
    }

    void sync() {
        if (synchronizing_)
            return;
        const QScopedValueRollback guard(synchronizing_, true);
        QWindow* handle = popup_->windowHandle();
        if (handle != observedHandle_) {
            QObject::disconnect(ownerChanged_);
            observedHandle_ = handle;
            if (handle) {
                ownerChanged_ =
                    connect(handle, &QWindow::transientParentChanged, this, [this] { sync(); });
            }
        }
        NSWindow* child = popup_->internalWinId()
                              ? reinterpret_cast<NSView*>(popup_->internalWinId()).window
                              : nil;
        QWindow* ownerHandle = handle ? handle->transientParent() : nullptr;
        if (ownerHandle != observedOwner_) {
            ownerObserver_.reset();
            observedOwner_ = ownerHandle;
            if (ownerHandle)
                ownerObserver_ = std::make_unique<WindowSurfaceObserver>(
                    ownerHandle, [this] { sync(); }, this, [this] { detach(); });
        }
        NSWindow* owner = ownerHandle && ownerHandle->handle()
                              ? reinterpret_cast<NSView*>(ownerHandle->winId()).window
                              : nil;
        if (!popup_->isVisible() || !child || !owner || child == owner) {
            detach();
            return;
        }
        if (child_ != child) {
            detach();
            child_ = [child retain];
        }
        if (child_.parentWindow != owner) {
            [child_.parentWindow removeChildWindow:child_];
            [owner addChildWindow:child_ ordered:NSWindowAbove];
        }
    }

  private:
    void detach() {
        if (child_) {
            [child_.parentWindow removeChildWindow:child_];
            [child_ release];
            child_ = nil;
        }
    }
    QWidget* popup_;
    QPointer<QWindow> observedHandle_;
    QPointer<QWindow> observedOwner_;
    QMetaObject::Connection ownerChanged_;
    NSWindow* child_ = nil;
    bool synchronizing_ = false;
    WindowSurfaceObserver popupObserver_;
    std::unique_ptr<WindowSurfaceObserver> ownerObserver_;
};
} // namespace

void syncMacTopLevelPopupOwnership(QWidget* popup) {
    if (QGuiApplication::platformName() != QStringLiteral("cocoa")) {
        return;
    }
    MacPopupOwnership* ownership = nullptr;
    for (QObject* child : popup->children()) {
        if ((ownership = dynamic_cast<MacPopupOwnership*>(child))) {
            break;
        }
    }
    if (!ownership) {
        ownership = new MacPopupOwnership(popup);
    }
    ownership->sync();
}
} // namespace adqt::widgets::detail
