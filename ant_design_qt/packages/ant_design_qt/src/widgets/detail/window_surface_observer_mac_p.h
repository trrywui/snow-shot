#pragma once

#include <QtGlobal>
#include <functional>

namespace adqt::widgets::detail {
void* observeMacWindowAttachment(quintptr view, std::function<void()> changed);
void releaseMacWindowObservation(void* observation);
}  // namespace adqt::widgets::detail
