#include "screenshotpinnedrestoregeometry.h"

#include <array>
#include <iostream>
#include <stdexcept>

namespace {
namespace restore_geometry = screenshot_pinned_restore_geometry;
using namespace snow_shot::presentation;
using snow_shot::storage::PinnedGeometryUnits;
using snow_shot::storage::PinnedWindowPlacement;

void require(bool condition, const char* message) {
    if (!condition) {
        throw std::runtime_error(message);
    }
}

void visiblePlacementsStayExact() {
    for (const auto units :
         {PinnedGeometryUnits::PhysicalPixels, PinnedGeometryUnits::LogicalPixels}) {
        for (const qreal scale : {1.0, 1.25, 2.0}) {
            const PinnedDisplayGeometry target{QStringLiteral("left"), QStringLiteral("serial"),
                                               QRectF(-1600, -300, 1600, 1000),
                                               QRectF(-1600, -262, 1600, 930), scale};
            const QRectF available =
                target.usableBounds.translated(-target.desktopBounds.topLeft());
            const std::array positions{
                QPointF(100.5, 200.5),
                QPointF(available.left() - 10.5, 200.5),
                QPointF(available.right() - 10.5, 200.5),
                QPointF(100.5, available.top() - 10.5),
                QPointF(100.5, available.bottom() - 10.5),
                QPointF(available.left() - 10.5, available.top() - 10.5),
                QPointF(available.right() - 10.5, available.bottom() - 10.5)};
            for (const auto& position : positions) {
                restore_geometry::PlacementState saved;
                saved.placement = {target.name, target.serial, position, QSize(321, 181), units};
                saved.preThumbnailPlacement = saved.placement;
                saved.preThumbnailPlacement.windowSize = QSize(642, 362);
                saved.hideToTopPlacement = saved.placement;
                saved.hideToTopPlacement.position =
                    QPointF(available.left() - 1.5, available.top());
                saved.hideToTopPlacement.windowSize = QSize(30, 6);
                const auto restored =
                    restore_geometry::reconcileSavedState(saved, target, {target});
                require(restored.placement == saved.placement &&
                            restored.preThumbnailPlacement == saved.preThumbnailPlacement &&
                            restored.hideToTopPlacement == saved.hideToTopPlacement,
                        "visible normal, thumbnail and handle placements must retain exact "
                        "positions and units");
            }

            const PinnedWindowPlacement oversized{target.name, target.serial, QPointF(-100, -100),
                                                  QSize(10000, 10000), units};
            const auto restored =
                restore_geometry::reconcileSavedState({oversized, {}, {}}, target, {target});
            require(
                restored.placement == oversized,
                "an oversized window that still intersects the display must not move or resize");
            require(!restored.preThumbnailPlacement.isValid() &&
                        !restored.hideToTopPlacement.isValid(),
                    "absent optional placements must remain absent");
        }
    }
}

void visibilityOnAnotherDisplayPreservesPlacement() {
    const PinnedDisplayGeometry target{
        QStringLiteral("primary"), {}, QRectF(0, 0, 1920, 1080), QRectF(0, 0, 1920, 1040), 1.25};
    const PinnedDisplayGeometry other{QStringLiteral("left"),
                                      {},
                                      QRectF(-1600, -300, 1600, 1000),
                                      QRectF(-1600, -262, 1600, 930),
                                      2.0};
    for (const auto units :
         {PinnedGeometryUnits::PhysicalPixels, PinnedGeometryUnits::LogicalPixels}) {
        const PinnedWindowPlacement placement{target.name, target.serial, QPointF(-400.5, 100.5),
                                              QSize(321, 181), units};
        require(!target.usableBounds.intersects(pinnedDesktopRect(placement, target)) &&
                    other.usableBounds.intersects(pinnedDesktopRect(placement, target)),
                "fixture must be visible only on the other display");
        const auto restored =
            restore_geometry::reconcileSavedState({placement, {}, {}}, target, {target, other});
        require(restored.placement == placement,
                "restore must check every connected display before recovering a placement");
    }
}

void connectedVariantDisplaysKeepTheirOwnDestinations() {
    const PinnedDisplayGeometry primary{QStringLiteral("primary"), QStringLiteral("primary-id"),
                                        QRectF(0, 0, 2560, 1440), QRectF(0, 0, 2560, 1400), 1.5};
    const PinnedDisplayGeometry left{QStringLiteral("left"), QStringLiteral("left-id"),
                                     QRectF(-3840, 0, 3840, 2160), QRectF(-3840, 0, 3840, 2120),
                                     1.0};
    for (const auto units :
         {PinnedGeometryUnits::PhysicalPixels, PinnedGeometryUnits::LogicalPixels}) {
        restore_geometry::PlacementState saved;
        saved.placement = {left.name, left.serial, QPointF(3780, 1044), QSize(80, 50), units};
        saved.preThumbnailPlacement = {primary.name, primary.serial, QPointF(-40, 696),
                                       QSize(160, 100), units};
        saved.hideToTopPlacement = {primary.name, primary.serial, QPointF(20, 30), QSize(40, 20),
                                    units};
        const auto restored = restore_geometry::reconcileSavedState(saved, left, {primary, left});
        require(restored.placement == saved.placement &&
                    restored.preThumbnailPlacement == saved.preThumbnailPlacement &&
                    restored.hideToTopPlacement == saved.hideToTopPlacement,
                "connected display anchors must preserve every placement variant independently");
        require(pinnedDesktopRect(restored.preThumbnailPlacement, primary) ==
                        pinnedDesktopRect(saved.preThumbnailPlacement, primary) &&
                    pinnedDesktopRect(restored.hideToTopPlacement, primary) ==
                        pinnedDesktopRect(saved.hideToTopPlacement, primary),
                "restoring a thumbnail on another display must not relocate its expanded image");

        saved.preThumbnailPlacement.displayName = QStringLiteral("disconnected");
        saved.preThumbnailPlacement.displaySerial = QStringLiteral("missing-id");
        const auto fallback = restore_geometry::reconcileSavedState(saved, left, {primary, left});
        require(fallback.placement == saved.placement &&
                    fallback.hideToTopPlacement == saved.hideToTopPlacement &&
                    fallback.preThumbnailPlacement.displayName == left.name &&
                    fallback.preThumbnailPlacement.displaySerial == left.serial &&
                    fallback.preThumbnailPlacement.position ==
                        saved.preThumbnailPlacement.position &&
                    fallback.preThumbnailPlacement.windowSize ==
                        saved.preThumbnailPlacement.windowSize &&
                    fallback.preThumbnailPlacement.units == units,
                "only a disconnected variant must fall back to the selected restore display");
    }
}

void changedDisplayPreservesLocalPositionAndWindowUnits() {
    for (const auto units :
         {PinnedGeometryUnits::PhysicalPixels, PinnedGeometryUnits::LogicalPixels}) {
        const PinnedWindowPlacement placement{
            QStringLiteral("disconnected"), {}, QPointF(-10.5, 80.5), QSize(321, 181), units};
        for (const qreal scale : {1.0, 1.5, 2.0}) {
            const PinnedDisplayGeometry target{QStringLiteral("replacement"), QStringLiteral("new"),
                                               QRectF(5000, 200, 1920, 1080),
                                               QRectF(5000, 240, 1920, 1040), scale};
            const auto restored =
                restore_geometry::reconcileSavedState({placement, {}, {}}, target, {target});
            require(restored.placement.position == placement.position &&
                        restored.placement.windowSize == placement.windowSize &&
                        restored.placement.units == placement.units &&
                        restored.placement.displayName == target.name &&
                        restored.placement.displaySerial == target.serial,
                    "monitor relocation or DPI changes must retain display-local offsets and "
                    "window extents");
            require(pinnedDesktopRect(restored.placement, target).topLeft() ==
                        target.desktopBounds.topLeft() + placement.position,
                    "the saved offset must be rebased onto the replacement display");
        }
    }
}

void unreachablePlacementsRecoverIndependently() {
    const PinnedDisplayGeometry target{QStringLiteral("target"), QStringLiteral("serial"),
                                       QRectF(-1600, -300, 1600, 1000),
                                       QRectF(-1600, -262, 1600, 930), 1.5};
    for (const auto units :
         {PinnedGeometryUnits::PhysicalPixels, PinnedGeometryUnits::LogicalPixels}) {
        for (const QPointF position : {QPointF(-10000, -10000), QPointF(10000, 10000),
                                       QPointF(1600, 100), QPointF(100, 968)}) {
            restore_geometry::PlacementState saved;
            saved.placement = {target.name, target.serial, QPointF(-10.5, 100), QSize(160, 100),
                               units};
            saved.preThumbnailPlacement = {target.name, target.serial, position, QSize(640, 400),
                                           units};
            saved.hideToTopPlacement = {target.name, target.serial, position, QSize(30, 6), units};
            const auto restored = restore_geometry::reconcileSavedState(saved, target, {target});
            require(
                restored.placement == saved.placement,
                "recovering an unreachable optional placement must not move the visible window");
            for (const auto& recovered :
                 {restored.preThumbnailPlacement, restored.hideToTopPlacement}) {
                require(target.usableBounds.contains(pinnedDesktopRect(recovered, target)) &&
                            recovered.units == units,
                        "fully unreachable placements must recover into the target usable area");
            }
            require(restored.preThumbnailPlacement.windowSize ==
                            saved.preThumbnailPlacement.windowSize &&
                        restored.hideToTopPlacement.windowSize ==
                            saved.hideToTopPlacement.windowSize,
                    "recovery must preserve optional window extents");
            saved.placement = saved.preThumbnailPlacement;
            saved.placement.windowSize = QSize(10000, 10000);
            saved.placement.position = QPointF(10000, 10000);
            const auto oversized = restore_geometry::reconcileSavedState(saved, target, {target});
            require(pinnedDesktopRect(oversized.placement, target).topLeft() ==
                            target.usableBounds.topLeft() &&
                        oversized.placement.windowSize == saved.placement.windowSize,
                    "unreachable oversized pins must expose their top-left without resizing");
        }
    }
}
} // namespace

int main() {
    try {
        visiblePlacementsStayExact();
        visibilityOnAnotherDisplayPreservesPlacement();
        connectedVariantDisplaysKeepTheirOwnDestinations();
        changedDisplayPreservesLocalPositionAndWindowUnits();
        unreachablePlacementsRecoverIndependently();
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
    return 0;
}
