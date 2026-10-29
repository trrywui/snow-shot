#ifndef SNOW_SHOT_PRESENTATION_SCREENSHOTOVERLAYCURSOR_H
#define SNOW_SHOT_PRESENTATION_SCREENSHOTOVERLAYCURSOR_H

#include <QPoint>
#include <QRect>
#include <utility>

namespace snow_shot::presentation::detail {

template <typename CommitCursor>
void commitOverlayCursor(const QRect& captureGeometry, const QPoint& cursorPosition,
                         CommitCursor&& commitCursor) {
    // The native cursor belongs to the pointer, not the last overlay to reveal or focus.
    if (captureGeometry.contains(cursorPosition)) {
        std::forward<CommitCursor>(commitCursor)();
    }
}

} // namespace snow_shot::presentation::detail

#endif // SNOW_SHOT_PRESENTATION_SCREENSHOTOVERLAYCURSOR_H
