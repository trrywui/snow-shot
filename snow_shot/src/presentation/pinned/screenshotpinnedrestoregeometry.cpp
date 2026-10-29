#include "screenshotpinnedrestoregeometry.h"
#include "pinneddisplayselection.h"

#include <QVarLengthArray>

#include <algorithm>

namespace screenshot_pinned_restore_geometry {
namespace {

snow_shot::storage::PinnedWindowPlacement
restorePlacement(snow_shot::storage::PinnedWindowPlacement placement,
                 const snow_shot::presentation::PinnedDisplayGeometry& fallbackScreen,
                 const QList<snow_shot::presentation::PinnedDisplayGeometry>& screens) {
    if (!placement.isValid()) {
        return placement;
    }
    QVarLengthArray<snow_shot::presentation::PinnedDisplayIdentity, 4> identities;
    for (const auto& screen : screens)
        identities.append({screen.name, screen.serial});
    const auto index = snow_shot::presentation::pinnedDisplayIndex(
        placement, {identities.data(), static_cast<std::size_t>(identities.size())});
    const auto& display = index ? screens.at(static_cast<qsizetype>(*index)) : fallbackScreen;
    placement.displayName = display.name;
    placement.displaySerial = display.serial;
    const QRectF geometry = snow_shot::presentation::pinnedDesktopRect(placement, display);
    const bool visible =
        std::any_of(screens.begin(), screens.end(), [&geometry](const auto& screen) {
            return screen.usableBounds.intersects(geometry);
        });
    return visible ? placement
                   : snow_shot::presentation::recoverPinnedPlacement(placement, display);
}

} // namespace

PlacementState
reconcileSavedState(const PlacementState& saved,
                    const snow_shot::presentation::PinnedDisplayGeometry& fallbackScreen,
                    const QList<snow_shot::presentation::PinnedDisplayGeometry>& screens) {
    return {restorePlacement(saved.placement, fallbackScreen, screens),
            restorePlacement(saved.preThumbnailPlacement, fallbackScreen, screens),
            restorePlacement(saved.hideToTopPlacement, fallbackScreen, screens)};
}

} // namespace screenshot_pinned_restore_geometry
