#include "window_surface_observer_mac_p.h"

#include <QGuiApplication>
#include <QSet>
#import <AppKit/AppKit.h>
#import <objc/runtime.h>

@interface AdQtWindowAttachmentObservation : NSObject {
  @public
    std::function<void()> changed;
}
@end
@implementation AdQtWindowAttachmentObservation
@end

namespace adqt::widgets::detail {
namespace {
char observationsKey;
void installAttachmentObservation(NSView* view) {
    static QSet<Class> installed;
    Class viewClass = [view class];
    if (installed.contains(viewClass))
        return;
    const SEL selector = @selector(viewDidMoveToWindow);
    const Method method = class_getInstanceMethod(viewClass, selector);
    const IMP original = method_getImplementation(method);
    const IMP replacement = imp_implementationWithBlock(^(NSView* receiver) {
      reinterpret_cast<void (*)(id, SEL)>(original)(receiver, selector);
      if (!receiver.window)
          return;
      NSArray* observations = [objc_getAssociatedObject(receiver, &observationsKey) copy];
      for (AdQtWindowAttachmentObservation* observation in observations) {
          const auto changed = observation->changed;
          if (changed)
              changed();
      }
      [observations release];
    });
    // Keep the concrete Qt view class and AppKit's KVO identity intact.
    class_replaceMethod(viewClass, selector, replacement, method_getTypeEncoding(method));
    installed.insert(viewClass);
}
} // namespace

void* observeMacWindowAttachment(quintptr identity, std::function<void()> changed) {
    if (!identity || QGuiApplication::platformName() != QStringLiteral("cocoa"))
        return nullptr;
    NSView* view = reinterpret_cast<NSView*>(identity);
    installAttachmentObservation(view);
    auto* observations =
        static_cast<NSMutableArray*>(objc_getAssociatedObject(view, &observationsKey));
    if (!observations) {
        observations = [NSMutableArray array];
        objc_setAssociatedObject(view, &observationsKey, observations,
                                 OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
    // Remove cancelled subscriptions without dereferencing a possibly retired view
    // during cancellation. The subscriber and view each retain the token.
    NSArray* previous = [observations copy];
    for (AdQtWindowAttachmentObservation* observation in previous) {
        if (!observation->changed)
            [observations removeObjectIdenticalTo:observation];
    }
    [previous release];
    auto* observation = [AdQtWindowAttachmentObservation new];
    observation->changed = std::move(changed);
    [observations addObject:observation];
    return observation;
}
void releaseMacWindowObservation(void* token) {
    auto* observation = static_cast<AdQtWindowAttachmentObservation*>(token);
    if (!observation)
        return;
    observation->changed = {};
    [observation release];
}
} // namespace adqt::widgets::detail
