#pragma once

#include <QtGlobal>
#include <optional>
#include <algorithm>

namespace adqt::widgets::detail {
// Constraints belong to the application; requested values belong to Qt/AppKit.
// Resolve each setter against the current constraints, retaining the requested
// value so removing a constraint restores the latest request.
struct NativeWindowConstraints {
  quint64 excludedStyleMask = 0;
  quint64 requiredCollectionBehavior = 0;
  quint64 excludedCollectionBehavior = 0;
  std::optional<qintptr> level;
  std::optional<bool> hidesOnDeactivate;
  std::optional<bool> movable;
  std::optional<bool> movableByWindowBackground;
  std::optional<bool> hasShadow;
  std::optional<bool> ignoresMouseEvents;

  quint64 styleMask(quint64 requested) const { return requested & ~excludedStyleMask; }
  quint64 collectionBehavior(quint64 requested) const {
    return (requested & ~excludedCollectionBehavior) | requiredCollectionBehavior;
  }
  static bool resolve(bool requested, const std::optional<bool>& required) {
    return required.value_or(requested);
  }
  // Independent owners can share a surface (for example a pinned toolbar).
  // Combine restrictions; releasing one owner cannot undo another owner's rule.
  void include(const NativeWindowConstraints& other) {
    excludedStyleMask |= other.excludedStyleMask;
    requiredCollectionBehavior |= other.requiredCollectionBehavior;
    excludedCollectionBehavior |= other.excludedCollectionBehavior;
    if (other.level) level = level ? std::max(*level, *other.level) : other.level;
    const auto restrict = [](std::optional<bool>& value, const std::optional<bool>& next,
                             bool restrictive) {
      if (next && (!value || *next == restrictive)) value = next;
    };
    restrict(movable, other.movable, false);
    restrict(hidesOnDeactivate, other.hidesOnDeactivate, false);
    restrict(movableByWindowBackground, other.movableByWindowBackground, false);
    restrict(hasShadow, other.hasShadow, false);
    restrict(ignoresMouseEvents, other.ignoresMouseEvents, true);
  }
};
}  // namespace adqt::widgets::detail
