#pragma once

#include <QApplication>
#include <QPointer>
#include <QWidget>

namespace snow_shot::presentation {

// Popups redirect mouse input even when no modal dialog is active. Dismiss them before
// their owning dialogs, and recheck both stacks because close handlers can open another blocker.
[[nodiscard]] inline bool closeCaptureInputBlockers() {
    while (true) {
        QWidget* blocker = QApplication::activePopupWidget();
        if (blocker == nullptr) {
            blocker = QApplication::activeModalWidget();
        }
        if (blocker == nullptr) {
            return true;
        }
        const QPointer<QWidget> guard(blocker);
        blocker->close();
        // Some dialogs ignore Close while hiding their own surface. Check the outcome,
        // not the event result, and do not start capture if input is still blocked.
        if (guard != nullptr && guard->isVisible()) {
            return false;
        }
    }
}

} // namespace snow_shot::presentation
