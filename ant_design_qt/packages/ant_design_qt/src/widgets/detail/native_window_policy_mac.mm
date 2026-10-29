#include "native_window_policy_mac_p.h"

#include <QScopedValueRollback>
#include <QSet>
#include <QHash>
#import <objc/runtime.h>

namespace {
struct NativeWindowSettings {
    NSInteger level;
    NSWindowCollectionBehavior collectionBehavior;
    BOOL hidesOnDeactivate;
    NSWindowStyleMask styleMask;
    BOOL movable;
    BOOL movableByWindowBackground;
    BOOL hasShadow;
    BOOL ignoresMouseEvents;
};
} // namespace

@interface AdQtNativeWindowPolicy : NSObject {
  @public
    adqt::widgets::detail::NativeWindowConstraints constraints;
    adqt::widgets::detail::NativeWindowConstraints owned;
    QHash<const void*, adqt::widgets::detail::NativeWindowConstraints> owners;
    NativeWindowSettings requested;
    bool applying;
}
@end
@implementation AdQtNativeWindowPolicy
@end

namespace adqt::widgets::detail {
namespace {
char policyKey;
AdQtNativeWindowPolicy* policy(NSWindow* window) {
    return static_cast<AdQtNativeWindowPolicy*>(objc_getAssociatedObject(window, &policyKey));
}

template <typename Value>
void installSetter(Class windowClass, SEL selector, Value NativeWindowSettings::* requested,
                   Value (^resolve)(AdQtNativeWindowPolicy*, Value)) {
    const Method method = class_getInstanceMethod(windowClass, selector);
    const IMP original = method_getImplementation(method);
    const IMP replacement = imp_implementationWithBlock(^(NSWindow* receiver, Value value) {
      if (auto* state = policy(receiver)) {
          if (!state->applying)
              state->requested.*requested = value;
          value = resolve(state, value);
      }
      reinterpret_cast<void (*)(id, SEL, Value)>(original)(receiver, selector, value);
    });
    class_replaceMethod(windowClass, selector, replacement, method_getTypeEncoding(method));
}

void installPolicy(NSWindow* window) {
    static QSet<Class> installed;
    Class windowClass = [window class];
    if (installed.contains(windowClass))
        return;
    installSetter<NSInteger>(windowClass, @selector(setLevel:), &NativeWindowSettings::level,
                             ^NSInteger(AdQtNativeWindowPolicy* state, NSInteger value) {
                               return state->constraints.level.value_or(value);
                             });
    installSetter<NSWindowCollectionBehavior>(
        windowClass, @selector(setCollectionBehavior:), &NativeWindowSettings::collectionBehavior,
        ^NSWindowCollectionBehavior(AdQtNativeWindowPolicy* state,
                                    NSWindowCollectionBehavior value) {
          return static_cast<NSWindowCollectionBehavior>(
              state->constraints.collectionBehavior(value));
        });
    installSetter<BOOL>(
        windowClass, @selector(setHidesOnDeactivate:), &NativeWindowSettings::hidesOnDeactivate,
        ^BOOL(AdQtNativeWindowPolicy* state, BOOL value) {
          return NativeWindowConstraints::resolve(value, state->constraints.hidesOnDeactivate);
        });
    installSetter<NSWindowStyleMask>(
        windowClass, @selector(setStyleMask:), &NativeWindowSettings::styleMask,
        ^NSWindowStyleMask(AdQtNativeWindowPolicy* state, NSWindowStyleMask value) {
          return static_cast<NSWindowStyleMask>(state->constraints.styleMask(value));
        });
    installSetter<BOOL>(windowClass, @selector(setMovable:), &NativeWindowSettings::movable,
                        ^BOOL(AdQtNativeWindowPolicy* state, BOOL value) {
                          return NativeWindowConstraints::resolve(value,
                                                                  state->constraints.movable);
                        });
    installSetter<BOOL>(windowClass, @selector(setMovableByWindowBackground:),
                        &NativeWindowSettings::movableByWindowBackground,
                        ^BOOL(AdQtNativeWindowPolicy* state, BOOL value) {
                          return NativeWindowConstraints::resolve(
                              value, state->constraints.movableByWindowBackground);
                        });
    installSetter<BOOL>(windowClass, @selector(setHasShadow:), &NativeWindowSettings::hasShadow,
                        ^BOOL(AdQtNativeWindowPolicy* state, BOOL value) {
                          return NativeWindowConstraints::resolve(value,
                                                                  state->constraints.hasShadow);
                        });
    installSetter<BOOL>(
        windowClass, @selector(setIgnoresMouseEvents:), &NativeWindowSettings::ignoresMouseEvents,
        ^BOOL(AdQtNativeWindowPolicy* state, BOOL value) {
          return NativeWindowConstraints::resolve(value, state->constraints.ignoresMouseEvents);
        });
    installed.insert(windowClass);
}

void applySettings(NSWindow* window, AdQtNativeWindowPolicy* state) {
    if (state->owned.level &&
        window.level != state->constraints.level.value_or(state->requested.level))
        window.level = state->requested.level;
    if ((state->owned.requiredCollectionBehavior || state->owned.excludedCollectionBehavior) &&
        window.collectionBehavior !=
            state->constraints.collectionBehavior(state->requested.collectionBehavior))
        window.collectionBehavior = state->requested.collectionBehavior;
    if (state->owned.excludedStyleMask &&
        window.styleMask != state->constraints.styleMask(state->requested.styleMask))
        window.styleMask = state->requested.styleMask;
    const auto set = [window](BOOL current, BOOL requested, const std::optional<bool>& required,
                              const std::optional<bool>& owned, SEL selector) {
        if (owned && current != NativeWindowConstraints::resolve(requested, required))
            reinterpret_cast<void (*)(id, SEL, BOOL)>([window methodForSelector:selector])(
                window, selector, requested);
    };
    set(window.hidesOnDeactivate, state->requested.hidesOnDeactivate,
        state->constraints.hidesOnDeactivate, state->owned.hidesOnDeactivate,
        @selector(setHidesOnDeactivate:));
    set(window.movable, state->requested.movable, state->constraints.movable, state->owned.movable,
        @selector(setMovable:));
    set(window.movableByWindowBackground, state->requested.movableByWindowBackground,
        state->constraints.movableByWindowBackground, state->owned.movableByWindowBackground,
        @selector(setMovableByWindowBackground:));
    set(window.hasShadow, state->requested.hasShadow, state->constraints.hasShadow,
        state->owned.hasShadow, @selector(setHasShadow:));
    set(window.ignoresMouseEvents, state->requested.ignoresMouseEvents,
        state->constraints.ignoresMouseEvents, state->owned.ignoresMouseEvents,
        @selector(setIgnoresMouseEvents:));
}

void updateConstraints(AdQtNativeWindowPolicy* state) {
    state->constraints = {};
    for (const auto& constraints : state->owners)
        state->constraints.include(constraints);
}
} // namespace

MacNativeWindowPolicy::MacNativeWindowPolicy() = default;
MacNativeWindowPolicy::~MacNativeWindowPolicy() {
    reset();
}

void MacNativeWindowPolicy::apply(NSWindow* window, const NativeWindowConstraints& constraints) {
    // A temporary detached view does not relinquish the old window's constraints.
    // It can be AppKit moving the view while applying that very window's style.
    if (!window)
        return;
    if (window_ != window) {
        reset();
        window_ = [window retain];
        if (!policy(window)) {
            auto* state = [AdQtNativeWindowPolicy new];
            state->requested.level = window.level;
            state->requested.collectionBehavior = window.collectionBehavior;
            state->requested.hidesOnDeactivate = window.hidesOnDeactivate;
            state->requested.styleMask = window.styleMask;
            state->requested.movable = window.movable;
            state->requested.movableByWindowBackground = window.movableByWindowBackground;
            state->requested.hasShadow = window.hasShadow;
            state->requested.ignoresMouseEvents = window.ignoresMouseEvents;
            objc_setAssociatedObject(window, &policyKey, state, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
            [state release];
            installPolicy(window);
        }
    }
    auto* state = [policy(window_) retain];
    state->owners.insert(this, constraints);
    state->owned.include(constraints);
    updateConstraints(state);
    {
        const QScopedValueRollback guard(state->applying, true);
        applySettings(window_, state);
    }
    [state release];
}

void MacNativeWindowPolicy::reset() {
    if (!window_)
        return;
    NSWindow* window = window_;
    window_ = nil;
    auto* state = [policy(window) retain];
    state->owners.remove(this);
    updateConstraints(state);
    {
        const QScopedValueRollback guard(state->applying, true);
        applySettings(window, state);
    }
    if (state->owners.isEmpty())
        objc_setAssociatedObject(window, &policyKey, nil, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    [state release];
    [window release];
}
} // namespace adqt::widgets::detail
