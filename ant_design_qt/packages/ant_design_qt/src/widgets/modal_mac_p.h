#pragma once

#include <memory>
#include <functional>

class QWidget;

namespace adqt::widgets::detail {

class MacModalSession {
 public:
  virtual ~MacModalSession() = default;
  virtual void synchronize() = 0;
  virtual void beginHide() = 0;
};

std::unique_ptr<MacModalSession> createMacModalSession(
    QWidget* surface, QWidget* blocker, std::function<void(bool)> interactionChanged);

}  // namespace adqt::widgets::detail
