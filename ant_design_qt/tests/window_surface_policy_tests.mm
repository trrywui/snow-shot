#include "widgets/detail/window_surface_observer.h"
#include "widgets/detail/native_window_policy_mac_p.h"

#include <QApplication>
#include <QEvent>
#include <QWidget>
#include <QWindow>
#import <objc/runtime.h>
#include <cstdlib>
#include <iostream>

namespace {
using namespace adqt::widgets::detail;
void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(EXIT_FAILURE);
    }
}
class Surface final : public QWidget {
  public:
    Surface() : QWidget(nullptr, Qt::Tool | Qt::FramelessWindowHint) {}
    void retire() {
        hide();
        destroy();
    }
};

void constraintsPreserveUnownedSettings() {
    NativeWindowConstraints constraints;
    constexpr quint64 resizable = 8;
    constexpr quint64 unrelated = (quint64{1} << 60) | 3;
    require(constraints.styleMask(unrelated | resizable) == (unrelated | resizable),
            "ordinary windows must keep every requested style bit");
    constraints.excludedStyleMask = resizable;
    require(constraints.styleMask(unrelated | resizable) == unrelated,
            "controlled windows must remove only their owned style bits");
    for (const bool requested : {false, true}) {
        require(NativeWindowConstraints::resolve(requested, {}) == requested,
                "unowned input and geometry settings must retain the caller's request");
        require(NativeWindowConstraints::resolve(requested, false) == false &&
                    NativeWindowConstraints::resolve(requested, true) == true,
                "active constraints must survive opposite Qt requests");
    }
    NativeWindowConstraints toolbar;
    toolbar.movable = false;
    toolbar.ignoresMouseEvents = false;
    constraints.ignoresMouseEvents = true;
    constraints.include(toolbar);
    require(constraints.movable == false && constraints.ignoresMouseEvents == true &&
                constraints.styleMask(unrelated | resizable) == unrelated,
            "sharing a native surface must preserve every owner's interaction restriction");
    constraints.requiredCollectionBehavior = 4;
    constraints.excludedCollectionBehavior = 2;
    require(constraints.collectionBehavior(unrelated | 2) == ((unrelated & ~quint64{2}) | 4),
            "Space constraints must preserve unrelated flags while removing exclusive policies");
}

void observersFollowQtSurfacesWithoutCreatingThem() {
    Surface surface;
    int changes = 0;
    int retirements = 0;
    WindowSurfaceObserver observer(&surface, [&] { ++changes; }, nullptr, [&] { ++retirements; });
    require(!surface.internalWinId() && !surface.windowHandle() && changes == 0,
            "observing a pooled widget must not create its native resources");
    surface.show();
    const auto id = surface.internalWinId();
    QWindow* handle = surface.windowHandle();
    const int before = changes;
    handle->hide();
    handle->show();
    require(changes > before && surface.internalWinId() == id && surface.windowHandle() == handle,
            "a QWindow reveal must notify even when QWidget and its native ID stay unchanged");
    surface.retire();
    require(retirements == 1, "native resources must be released at the surface teardown boundary");
    observer.synchronize();
    require(!surface.internalWinId() && !surface.windowHandle(),
            "observing retirement must never resurrect a native surface");
    surface.show();
    const int recreated = changes;
    QEvent exposed(QEvent::Expose);
    QCoreApplication::sendEvent(surface.windowHandle(), &exposed);
    require(changes > recreated, "replacement QWindows must acquire surface observation");
    QWindow owner;
    int ownerChanges = 0;
    WindowSurfaceObserver ownerObserver(&owner, [&] { ++ownerChanges; }, nullptr);
    require(!owner.handle(), "observing a transient owner must not create it");
    owner.show();
    require(ownerChanges > 0, "transient-owner QWindow reveals must notify their dependants");
    std::unique_ptr<WindowSurfaceObserver> temporary;
    temporary =
        std::make_unique<WindowSurfaceObserver>(&owner, [&] { temporary.reset(); }, nullptr);
    QEvent expose(QEvent::Expose);
    QCoreApplication::sendEvent(&owner, &expose);
    require(!temporary,
            "native callbacks must permit their observer to be destroyed synchronously");
}

void nativeAttachmentDoesNotDependOnQtIdentityChanges() {
    Surface surface;
    static_cast<void>(surface.winId());
    int changes = 0;
    auto observer = std::make_unique<WindowSurfaceObserver>(&surface, [&] { ++changes; }, nullptr);
    NSView* view = [reinterpret_cast<NSView*>(surface.internalWinId()) retain];
    NSWindow* window = [view.window retain];
    window.contentView = nil;
    const int before = changes;
    window.contentView = view;
    require(changes > before, "native view attachment must notify before any Qt reveal event");
    observer.reset();
    const int cancelled = changes;
    window.contentView = nil;
    window.contentView = view;
    require(changes == cancelled, "cancelled native observations must not call expired owners");
    [view release];
    [window release];
}

void nativeConstraintsFollowRequestsAndRestoreOwnership() {
    Surface surface;
    surface.show();
    NSWindow* window = reinterpret_cast<NSView*>(surface.internalWinId()).window;
    Class originalClass = object_getClass(window);
    NativeWindowConstraints constraints;
    constraints.excludedStyleMask = NSWindowStyleMaskResizable;
    constraints.movable = false;
    constraints.hasShadow = false;
    constraints.ignoresMouseEvents = true;
    MacNativeWindowPolicy policy;
    policy.apply(window, constraints);
    MacNativeWindowPolicy toolbarPolicy;
    NativeWindowConstraints toolbar;
    toolbar.movableByWindowBackground = false;
    toolbarPolicy.apply(window, toolbar);
    surface.windowHandle()->setFlag(Qt::WindowDoesNotAcceptFocus, true);
    window.movable = YES;
    window.hasShadow = YES;
    window.ignoresMouseEvents = NO;
    require(!(window.styleMask & NSWindowStyleMaskResizable) && !window.movable &&
                !window.hasShadow && window.ignoresMouseEvents,
            "Qt and native setter updates must preserve every owned interaction constraint");
    require(object_getClass(window) == originalClass,
            "native policy must preserve the window's concrete class and KVO identity");
    constraints.ignoresMouseEvents = false;
    policy.apply(window, constraints);
    require(!window.ignoresMouseEvents, "changing a constraint must take effect immediately");
    policy.reset();
    require((window.styleMask & NSWindowStyleMaskResizable) && window.movable && window.hasShadow &&
                !window.ignoresMouseEvents,
            "releasing ownership must restore Qt's latest requested settings");
    require(!window.movableByWindowBackground,
            "releasing one policy must preserve the other owner's constraints");
    toolbarPolicy.reset();
    window.ignoresMouseEvents = YES;
    require(window.ignoresMouseEvents, "released windows must accept native requests unchanged");
    window.ignoresMouseEvents = NO;
}
} // namespace

int main(int argc, char** argv) {
    QApplication app(argc, argv);
    constraintsPreserveUnownedSettings();
    observersFollowQtSurfacesWithoutCreatingThem();
    if (QGuiApplication::platformName() == QStringLiteral("cocoa")) {
        @autoreleasepool {
            nativeConstraintsFollowRequestsAndRestoreOwnership();
            nativeAttachmentDoesNotDependOnQtIdentityChanges();
        }
    }
}
