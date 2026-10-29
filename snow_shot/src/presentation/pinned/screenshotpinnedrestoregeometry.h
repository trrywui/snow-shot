#ifndef SNOW_SHOT_PRESENTATION_SCREENSHOTPINNEDRESTOREGEOMETRY_H
#define SNOW_SHOT_PRESENTATION_SCREENSHOTPINNEDRESTOREGEOMETRY_H

#include "pinnedplacementgeometry.h"

#include <QList>

namespace screenshot_pinned_restore_geometry {

struct PlacementState {
    snow_shot::storage::PinnedWindowPlacement placement;
    // Invalid when the window was not in thumbnail mode.
    snow_shot::storage::PinnedWindowPlacement preThumbnailPlacement;
    // Invalid when no hide-to-top handle was saved.
    snow_shot::storage::PinnedWindowPlacement hideToTopPlacement;
};

// Preserve each connected display anchor and its local position, window extent
// and geometry units. Missing displays use the selected fallback. Only wholly
// unreachable variants need recovery. Keep each resolved placement consistent
// with the rectangle used for presentation and subsequent persistence.
[[nodiscard]] PlacementState
reconcileSavedState(const PlacementState& saved,
                    const snow_shot::presentation::PinnedDisplayGeometry& fallbackScreen,
                    const QList<snow_shot::presentation::PinnedDisplayGeometry>& screens);

} // namespace screenshot_pinned_restore_geometry

#endif // SNOW_SHOT_PRESENTATION_SCREENSHOTPINNEDRESTOREGEOMETRY_H
