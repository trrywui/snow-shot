#pragma once

#include "native_window_policy.h"
#include <memory>
#import <AppKit/AppKit.h>

namespace adqt::widgets::detail {
// Scoped interaction ownership. Multiple owners share one native snapshot and
// combine constraints; stacking/presentation keep their independent ownership.
class MacNativeWindowPolicy final {
 public:
  MacNativeWindowPolicy();
  ~MacNativeWindowPolicy();
  MacNativeWindowPolicy(const MacNativeWindowPolicy&) = delete;
  MacNativeWindowPolicy& operator=(const MacNativeWindowPolicy&) = delete;
  void apply(NSWindow* window, const NativeWindowConstraints& constraints);
  void reset();

 private:
  NSWindow* window_ = nil;
};
}  // namespace adqt::widgets::detail
