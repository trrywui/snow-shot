#include "snow_shot/presentation/screenshotselectionmodel.h"

#include "snow_shot/presentation/screenshotselectionlimits.h"

#include <algorithm>

namespace {
using snow_shot::presentation::kScreenshotSelectionCornerRadiusMax;
using snow_shot::presentation::kScreenshotSelectionShadowWidthMax;
constexpr qreal kNewMarqueeAspectRatio = 1.0;
} // namespace

// Composite operations may temporarily replace a region before restoring an equivalent operand.
// Publish one revision for the resulting semantic change, without counting those intermediates.
class ScreenshotSelectionModel::ChangeRevisionGuard final {
  public:
    explicit ChangeRevisionGuard(ScreenshotSelectionModel& model)
        : m_model(model), m_previous(model) {}

    ~ChangeRevisionGuard() {
        m_model.m_changeRevision =
            m_previous.m_changeRevision + !m_model.hasSameSelectionState(m_previous);
    }

  private:
    ScreenshotSelectionModel& m_model;
    ScreenshotSelectionModel m_previous;
};

bool ScreenshotSelectionModel::hasSameSelectionState(const ScreenshotSelectionModel& other) const {
    return m_regionType == other.m_regionType && m_draftRegion == other.m_draftRegion &&
           m_draftVertices == other.m_draftVertices && m_region == other.m_region &&
           m_confirmedRegion == other.m_confirmedRegion &&
           m_regionOperation == other.m_regionOperation && m_start == other.m_start &&
           m_end == other.m_end && m_cornerRadius == other.m_cornerRadius &&
           m_shadowWidth == other.m_shadowWidth && m_shadowColor == other.m_shadowColor &&
           m_aspectRatioLockEnabled == other.m_aspectRatioLockEnabled &&
           m_aspectRatioPreset == other.m_aspectRatioPreset &&
           m_selectionFromDrag == other.m_selectionFromDrag &&
           qFuzzyCompare(1.0 + m_lockedAspectRatio, 1.0 + other.m_lockedAspectRatio);
}

ScreenshotSelectionModel&
ScreenshotSelectionModel::operator=(const ScreenshotSelectionModel& other) {
    if (this == &other)
        return *this;
    const bool changed = !hasSameSelectionState(other);
    m_draftVertices = other.m_draftVertices;
    m_cachedSelectionRegion = other.m_cachedSelectionRegion;
    m_cachedMarqueeRegion = other.m_cachedMarqueeRegion;
    m_regionType = other.m_regionType;
    m_draftRegion = other.m_draftRegion;
    m_region = other.m_region;
    m_confirmedRegion = other.m_confirmedRegion;
    m_moveOriginalRegion = other.m_moveOriginalRegion;
    m_regionOperation = other.m_regionOperation;
    m_start = other.m_start;
    m_end = other.m_end;
    m_moveStart = other.m_moveStart;
    m_moveOriginalSelection = other.m_moveOriginalSelection;
    m_cornerRadius = other.m_cornerRadius;
    m_shadowWidth = other.m_shadowWidth;
    m_shadowColor = other.m_shadowColor;
    m_aspectRatioLockEnabled = other.m_aspectRatioLockEnabled;
    m_aspectRatioPreset = other.m_aspectRatioPreset;
    m_selectionFromDrag = other.m_selectionFromDrag;
    m_lockedAspectRatio = other.m_lockedAspectRatio;
    if (changed)
        ++m_changeRevision;
    return *this;
}

ScreenshotSelectionModel& ScreenshotSelectionModel::operator=(ScreenshotSelectionModel&& other) {
    // The implicitly shared geometry and Qt values stay cheap to copy while assignment keeps
    // the same destination-owned revision semantics for both lvalues and rvalues.
    return operator=(static_cast<const ScreenshotSelectionModel&>(other));
}

void ScreenshotSelectionModel::setRegionType(ScreenshotRegionType type) {
    if (m_regionType == type)
        return;
    m_regionType = type;
    ++m_changeRevision;
}

void ScreenshotSelectionModel::reset() {
    const ChangeRevisionGuard revision(*this);
    m_cachedSelectionRegion.reset();
    m_cachedMarqueeRegion.reset();
    m_draftVertices.clear();
    m_draftRegion.reset();
    m_region.reset();
    m_confirmedRegion = {};
    m_moveOriginalRegion = {};
    m_regionOperation = RegionOperation::Replace;
    m_start = QPointF();
    m_end = QPointF();
    m_moveStart = QPointF();
    m_moveOriginalSelection = QRectF();
    m_cornerRadius = 0;
    m_shadowWidth = 0;
    m_shadowColor = QColor(0x33, 0x33, 0x33);
    m_aspectRatioLockEnabled = false;
    m_aspectRatioPreset = ScreenshotSelectionAspectRatioPreset::Free;
    m_selectionFromDrag = false;
    m_lockedAspectRatio = 0.0;
}

QRectF ScreenshotSelectionModel::normalizedSelection() const {
    return normalizedScreenshotSelection(m_start, m_end);
}

QRect ScreenshotSelectionModel::pixelSelection() const {
    return screenshotPixelRectForSelection(normalizedSelection());
}

bool ScreenshotSelectionModel::hasPixelSelection() const {
    const QRect selection = pixelSelection();
    return selection.width() >= 1 && selection.height() >= 1;
}

void ScreenshotSelectionModel::clearSelection() {
    const ChangeRevisionGuard revision(*this);
    m_cachedSelectionRegion.reset();
    m_cachedMarqueeRegion.reset();
    m_draftVertices.clear();
    m_draftRegion.reset();
    m_region.reset();
    m_confirmedRegion = {};
    m_moveOriginalRegion = {};
    m_regionOperation = RegionOperation::Replace;
    m_start = QPointF();
    m_end = QPointF();
    m_selectionFromDrag = false;
    if (m_aspectRatioPreset == ScreenshotSelectionAspectRatioPreset::Free) {
        m_lockedAspectRatio = 0.0;
    }
}

bool ScreenshotSelectionModel::setSelectionRect(const QRectF& selection) {
    const QRectF normalized = selection.normalized();
    const bool previouslyDragged = m_selectionFromDrag;
    m_selectionFromDrag = false;
    if (!m_draftRegion && !m_region && m_draftVertices.isEmpty() &&
        m_start == normalized.topLeft() && m_end == normalized.bottomRight()) {
        if (previouslyDragged)
            ++m_changeRevision;
        return false;
    }
    // Subpixel detection/animation targets can change without changing their
    // capture pixels. Only rectangle operands in the same transaction can reuse
    // the authoritative composition; construction must retain its exact path.
    if (m_cachedSelectionRegion &&
        (!regionOperationActive() || constructionActive() ||
         pixelSelection() != screenshotPixelRectForSelection(normalized))) {
        m_cachedSelectionRegion.reset();
    }
    m_draftVertices.clear();
    m_draftRegion.reset();
    m_region.reset();
    m_start = normalized.topLeft();
    m_end = normalized.bottomRight();
    ++m_changeRevision;
    return true;
}

void ScreenshotSelectionModel::setSelectionStartEnd(const QPointF& start, const QPointF& end) {
    setSelectionRect(marqueeScreenshotSelectionRect(start, end));
}

void ScreenshotSelectionModel::beginMoveDrag(const QPointF& startPosition) {
    m_moveOriginalRegion = selectionRegion();
    m_moveStart = startPosition;
    m_moveOriginalSelection = normalizedSelection();
}

void ScreenshotSelectionModel::rebaseMoveDrag(const QPointF& startPosition) {
    beginMoveDrag(startPosition);
}

QRectF ScreenshotSelectionModel::moveOriginalSelection() const {
    return m_moveOriginalSelection;
}

QRectF ScreenshotSelectionModel::selectionRectForDrag(ScreenshotSelectionDragMode dragMode,
                                                      const QPointF& position, const QRectF& bounds,
                                                      qreal minimumSelectionSize,
                                                      qreal lockedAspectRatioOverride) const {
    qreal lockedAspectRatio =
        lockedAspectRatioOverride >= 0.0 ? lockedAspectRatioOverride : m_lockedAspectRatio;
    if (lockedAspectRatioOverride < 0.0 && lockedAspectRatio <= 0.0 && m_aspectRatioLockEnabled &&
        dragMode == ScreenshotSelectionDragMode::Marquee) {
        lockedAspectRatio = kNewMarqueeAspectRatio;
    }
    return draggedScreenshotSelectionRect(dragMode, m_moveOriginalSelection, m_moveStart, position,
                                          bounds, minimumSelectionSize, lockedAspectRatio);
}

QRectF ScreenshotSelectionModel::boundedSelectionRect(const QRectF& selection, const QRectF& bounds,
                                                      bool preserveSize,
                                                      qreal minimumSelectionSize) const {
    return boundedScreenshotSelectionRect(selection, bounds, preserveSize, minimumSelectionSize);
}

bool ScreenshotSelectionModel::adjustFromToolbar(int minDx, int minDy, int maxDx, int maxDy,
                                                 const QRectF& bounds, qreal minimumSelectionSize) {
    const ChangeRevisionGuard revision(*this);
    if (!rectangular() && (minDx != maxDx || minDy != maxDy)) {
        return false;
    }
    const ScreenshotRegionGeometry originalRegion = selectionRegion();
    QRectF selection = normalizedSelection();
    if (!selection.isValid() || selection.width() < minimumSelectionSize ||
        selection.height() < minimumSelectionSize) {
        return false;
    }

    selection.adjust(minDx, minDy, maxDx, maxDy);
    if (m_lockedAspectRatio > 0.0 && minDx == 0 && minDy == 0 && (maxDx != 0 || maxDy != 0)) {
        double width = std::max<qreal>(minimumSelectionSize, selection.width());
        double height = std::max<qreal>(minimumSelectionSize, selection.height());
        const int deltaValue = maxDx + maxDy;
        if (deltaValue > 0) {
            if (width * (width * m_lockedAspectRatio) > (height / m_lockedAspectRatio) * height) {
                height = width * m_lockedAspectRatio;
            } else {
                width = height / m_lockedAspectRatio;
            }
        } else {
            if (width * (width * m_lockedAspectRatio) < (height / m_lockedAspectRatio) * height) {
                height = width * m_lockedAspectRatio;
            } else {
                width = height / m_lockedAspectRatio;
            }
        }
        selection.setWidth(width);
        selection.setHeight(height);
    }

    if (m_lockedAspectRatio > 0.0 && minDx == 0 && minDy == 0 && (maxDx != 0 || maxDy != 0)) {
        // Wheel edits keep the leading corner fixed. Limit both dimensions
        // together instead of independently clipping the paired dimension.
        const QRectF anchoredBounds =
            bounds.isNull() ? bounds
                            : QRectF(selection.topLeft(), bounds.normalized().bottomRight());
        selection = aspectRatioScreenshotSelectionRect(selection, anchoredBounds,
                                                       m_lockedAspectRatio, minimumSelectionSize);
        if (selection.isEmpty()) {
            return false;
        }
        setSelectionRect(selection);
    } else {
        setSelectionRect(boundedSelectionRect(selection.normalized(), bounds,
                                              minDx == maxDx && minDy == maxDy,
                                              minimumSelectionSize));
    }
    if (originalRegion.rectCount() > 1) {
        setSelectionRegion(originalRegion.translated(pixelSelection().topLeft() -
                                                     originalRegion.boundingRect().topLeft()));
    }
    return true;
}

int ScreenshotSelectionModel::cornerRadius() const {
    return m_cornerRadius;
}

int ScreenshotSelectionModel::shadowWidth() const {
    return m_shadowWidth;
}

QColor ScreenshotSelectionModel::shadowColor() const {
    return m_shadowColor;
}

bool ScreenshotSelectionModel::aspectRatioLocked() const {
    return m_aspectRatioLockEnabled;
}

ScreenshotSelectionAspectRatioPreset ScreenshotSelectionModel::aspectRatioPreset() const {
    return m_aspectRatioPreset;
}

bool ScreenshotSelectionModel::setCornerRadius(int radius) {
    const int clampedRadius = std::clamp(radius, 0, kScreenshotSelectionCornerRadiusMax);
    if (m_cornerRadius == clampedRadius) {
        return false;
    }

    m_cornerRadius = clampedRadius;
    ++m_changeRevision;
    return true;
}

bool ScreenshotSelectionModel::setShadowWidth(int shadowWidth) {
    const int clampedShadowWidth = std::clamp(shadowWidth, 0, kScreenshotSelectionShadowWidthMax);
    if (m_shadowWidth == clampedShadowWidth) {
        return false;
    }

    m_shadowWidth = clampedShadowWidth;
    ++m_changeRevision;
    return true;
}

void ScreenshotSelectionModel::setShadowColor(const QColor& color) {
    const QColor sanitized = color.isValid() ? color : QColor(0x33, 0x33, 0x33);
    if (m_shadowColor == sanitized)
        return;
    m_shadowColor = sanitized;
    ++m_changeRevision;
}

bool ScreenshotSelectionModel::setAspectRatioLockEnabled(bool enabled, qreal minimumSelectionSize) {
    const ChangeRevisionGuard revision(*this);
    const bool enabledChanged = m_aspectRatioLockEnabled != enabled;
    m_aspectRatioLockEnabled = enabled;
    if (!enabled) {
        const bool ratioChanged = m_lockedAspectRatio > 0.0 ||
                                  m_aspectRatioPreset != ScreenshotSelectionAspectRatioPreset::Free;
        m_aspectRatioPreset = ScreenshotSelectionAspectRatioPreset::Free;
        m_lockedAspectRatio = 0.0;
        return enabledChanged || ratioChanged;
    }

    if (m_aspectRatioPreset != ScreenshotSelectionAspectRatioPreset::Free) {
        const qreal nextAspectRatio =
            screenshotSelectionAspectRatioHeightOverWidth(m_aspectRatioPreset);
        const bool ratioChanged = !qFuzzyCompare(1.0 + m_lockedAspectRatio, 1.0 + nextAspectRatio);
        m_lockedAspectRatio = nextAspectRatio;
        return enabledChanged || ratioChanged;
    }

    const QRectF selection = normalizedSelection();
    if (selection.width() >= minimumSelectionSize && selection.height() >= minimumSelectionSize) {
        const qreal nextAspectRatio = selection.height() / selection.width();
        const bool ratioChanged = !qFuzzyCompare(1.0 + m_lockedAspectRatio, 1.0 + nextAspectRatio);
        m_lockedAspectRatio = nextAspectRatio;
        return enabledChanged || ratioChanged;
    }
    const bool ratioChanged = m_lockedAspectRatio > 0.0;
    m_lockedAspectRatio = 0.0;
    return enabledChanged || ratioChanged;
}

void ScreenshotSelectionModel::toggleAspectRatioLock(qreal minimumSelectionSize) {
    static_cast<void>(setAspectRatioLockEnabled(!aspectRatioLocked(), minimumSelectionSize));
}

bool ScreenshotSelectionModel::setAspectRatioPreset(ScreenshotSelectionAspectRatioPreset preset,
                                                    const QRectF& bounds,
                                                    qreal minimumSelectionSize) {
    const ChangeRevisionGuard revision(*this);
    if (preset == ScreenshotSelectionAspectRatioPreset::Free) {
        return setAspectRatioLockEnabled(false, minimumSelectionSize);
    }
    const qreal ratio = screenshotSelectionAspectRatioHeightOverWidth(preset);
    if (ratio <= 0.0) {
        return false;
    }
    const QRectF current = normalizedSelection();
    QRectF resized = current;
    if (hasPixelSelection()) {
        if (!rectangular()) {
            return false;
        }
        resized = aspectRatioScreenshotSelectionRect(current, bounds, ratio, minimumSelectionSize);
        if (resized.isEmpty()) {
            return false;
        }
    }
    const bool geometryChanged = current != resized;
    const bool changed = geometryChanged || m_aspectRatioPreset != preset ||
                         !m_aspectRatioLockEnabled ||
                         !qFuzzyCompare(1.0 + m_lockedAspectRatio, 1.0 + ratio);
    if (geometryChanged) {
        setSelectionRect(resized);
    }
    m_aspectRatioPreset = preset;
    m_aspectRatioLockEnabled = true;
    m_lockedAspectRatio = ratio;
    return changed;
}

bool ScreenshotSelectionModel::setDraggedAspectRatioPreset(
    ScreenshotSelectionAspectRatioPreset preset) {
    const ChangeRevisionGuard revision(*this);
    const qreal ratio = screenshotSelectionAspectRatioHeightOverWidth(preset);
    if (ratio <= 0.0) {
        return false;
    }
    const bool changed = m_aspectRatioPreset != preset || !m_aspectRatioLockEnabled ||
                         !qFuzzyCompare(1.0 + m_lockedAspectRatio, 1.0 + ratio);
    m_aspectRatioPreset = preset;
    m_aspectRatioLockEnabled = true;
    m_lockedAspectRatio = ratio;
    return changed;
}

bool ScreenshotSelectionModel::finalizeAspectRatio(const QRectF& bounds,
                                                   qreal minimumSelectionSize) {
    const ChangeRevisionGuard revision(*this);
    bool presetCleared = false;
    if (m_selectionFromDrag && m_aspectRatioPreset != ScreenshotSelectionAspectRatioPreset::Free &&
        rectangular()) {
        const QRectF selection = normalizedSelection();
        const qreal presetRatio =
            screenshotSelectionAspectRatioHeightOverWidth(m_aspectRatioPreset);
        if (!qFuzzyCompare(1.0 + selection.height() / selection.width(), 1.0 + presetRatio)) {
            // A gesture can override the preset. Keep its result as a custom lock without
            // changing the separately persisted preference for the next capture.
            clearAspectRatioPresetForReplacement();
            presetCleared = true;
        }
    }
    if (m_aspectRatioPreset != ScreenshotSelectionAspectRatioPreset::Free) {
        return setAspectRatioPreset(m_aspectRatioPreset, bounds, minimumSelectionSize);
    }
    return setAspectRatioLockEnabled(m_aspectRatioLockEnabled, minimumSelectionSize) ||
           presetCleared;
}

void ScreenshotSelectionModel::clearAspectRatioPresetForReplacement() {
    const ChangeRevisionGuard revision(*this);
    m_aspectRatioPreset = ScreenshotSelectionAspectRatioPreset::Free;
    static_cast<void>(setAspectRatioLockEnabled(
        m_aspectRatioLockEnabled, snow_shot::presentation::kScreenshotSelectionMinimumSize));
}

ScreenshotSelectionParams ScreenshotSelectionModel::params(const QRect& bounds) const {
    ScreenshotSelectionParams result;
    result.selection = pixelSelection();
    result.region = m_region;
    result.radius = m_cornerRadius;
    result.shadowWidth = m_shadowWidth;
    result.shadowColor = m_shadowColor;
    result.lockAspectRatio = aspectRatioLocked();
    result.lockDragAspectRatio = aspectRatioLocked();
    return clampScreenshotSelectionParams(result, bounds);
}

bool ScreenshotSelectionModel::applyParams(const ScreenshotSelectionParams& params,
                                           const QRect& bounds) {
    const ChangeRevisionGuard revision(*this);
    if (bounds.isEmpty()) {
        return false;
    }

    const ScreenshotSelectionParams clamped = clampScreenshotSelectionParams(params, bounds);
    if (clamped.selection.width() < 1 || clamped.selection.height() < 1) {
        clearSelection();
        return false;
    }

    cancelRegionOperation();
    setSelectionRect(QRectF(clamped.selection));
    if (clamped.region) {
        setSelectionRegion(*clamped.region);
    }
    m_cornerRadius = clamped.radius;
    m_shadowWidth = clamped.shadowWidth;
    setShadowColor(clamped.shadowColor);
    m_aspectRatioLockEnabled = clamped.lockDragAspectRatio;
    m_aspectRatioPreset = ScreenshotSelectionAspectRatioPreset::Free;
    m_lockedAspectRatio = clamped.lockDragAspectRatio
                              ? static_cast<double>(std::max(1, clamped.selection.height())) /
                                    static_cast<double>(std::max(1, clamped.selection.width()))
                              : 0.0;
    return true;
}

ScreenshotRegionGeometry ScreenshotSelectionModel::confirmedRegion() const {
    return regionOperationActive() ? m_confirmedRegion : selectionRegion();
}

ScreenshotRegionGeometry ScreenshotSelectionModel::selectionRegion() const {
    if (m_cachedSelectionRegion)
        return *m_cachedSelectionRegion;
    if (regionOperationActive() && !constructionActive() && m_cachedMarqueeRegion &&
        m_cachedMarqueeRegion->pixels == pixelSelection()) {
        // A new capture target may be the frame already on screen. Preserve its
        // immutable operands and derived contours instead of recomposing it.
        return *(m_cachedSelectionRegion = m_cachedMarqueeRegion->region);
    }
    const ScreenshotRegionGeometry marquee =
        m_draftRegion.value_or(ScreenshotRegionGeometry(pixelSelection()));
    if (m_regionOperation == RegionOperation::Add)
        return *(m_cachedSelectionRegion = m_confirmedRegion.united(marquee));
    if (m_regionOperation == RegionOperation::Subtract)
        return *(m_cachedSelectionRegion = m_confirmedRegion.subtracted(marquee));
    return m_draftRegion.value_or(m_region.value_or(marquee));
}

ScreenshotRegionGeometry
ScreenshotSelectionModel::selectionRegionForMarquee(const QRectF& marquee) const {
    if (!regionOperationActive() || constructionActive())
        return selectionRegion();
    const QRect pixels = screenshotPixelRectForSelection(marquee);
    if (pixels.isEmpty())
        return m_confirmedRegion;
    if (pixels == pixelSelection()) {
        const auto region = selectionRegion();
        // A settled animation is still the latest displayed frame. Retain it
        // when the next target invalidates the authoritative composition.
        m_cachedMarqueeRegion = MarqueeRegion{pixels, region};
        return region;
    }
    if (m_cachedMarqueeRegion && m_cachedMarqueeRegion->pixels == pixels)
        return m_cachedMarqueeRegion->region;
    const ScreenshotRegionGeometry operand(pixels);
    m_cachedMarqueeRegion = MarqueeRegion{pixels, m_regionOperation == RegionOperation::Add
                                                      ? m_confirmedRegion.united(operand)
                                                      : m_confirmedRegion.subtracted(operand)};
    return m_cachedMarqueeRegion->region;
}

bool ScreenshotSelectionModel::rectangular() const {
    return !constructionActive() && !regionOperationActive() && selectionRegion().rectCount() == 1;
}

bool ScreenshotSelectionModel::regionOperationActive() const {
    return m_regionOperation != RegionOperation::Replace;
}

ScreenshotSelectionModel::RegionOperation ScreenshotSelectionModel::regionOperation() const {
    return m_regionOperation;
}

QRectF ScreenshotSelectionModel::pendingMarquee() const {
    return regionOperationActive() && !constructionActive() ? normalizedSelection() : QRectF();
}

void ScreenshotSelectionModel::beginRegionOperation(RegionOperation operation) {
    const ChangeRevisionGuard revision(*this);
    const ScreenshotRegionGeometry confirmed = confirmedRegion();
    clearSelection();
    m_confirmedRegion = confirmed;
    m_regionOperation = operation;
}

void ScreenshotSelectionModel::setSelectionRegion(const ScreenshotRegionGeometry& region) {
    const ChangeRevisionGuard revision(*this);
    const ScreenshotRegionGeometry snapshot = region;
    // Replacing the operation/confirmed region changes the composed result even
    // when its bounding rectangle is the same as the previous marquee.
    m_cachedSelectionRegion.reset();
    m_cachedMarqueeRegion.reset();
    setSelectionRect(QRectF(snapshot.boundingRect()));
    m_regionOperation = RegionOperation::Replace;
    m_confirmedRegion = {};
    if (snapshot.rectCount() > 1)
        m_region = snapshot;
}

void ScreenshotSelectionModel::commitRegionOperation() {
    if (regionOperationActive())
        setSelectionRegion(selectionRegion());
}

void ScreenshotSelectionModel::cancelRegionOperation() {
    if (regionOperationActive())
        setSelectionRegion(m_confirmedRegion);
}

void ScreenshotSelectionModel::setDraggedSelectionRect(const QRectF& rect,
                                                       ScreenshotSelectionDragMode mode) {
    const ChangeRevisionGuard revision(*this);
    if (!regionOperationActive() && mode == ScreenshotSelectionDragMode::All &&
        m_moveOriginalRegion.rectCount() > 1) {
        setSelectionRegion(m_moveOriginalRegion.translated(
            screenshotPixelRectForSelection(rect).topLeft() -
            screenshotPixelRectForSelection(m_moveOriginalSelection).topLeft()));
    } else {
        setSelectionRect(rect);
    }
    m_selectionFromDrag = true;
}

ScreenshotResultStyle ScreenshotSelectionModel::resultStyle() const {
    ScreenshotResultStyle style{m_cornerRadius, m_shadowWidth, m_shadowColor, {}, 1.0};
    if (m_region) {
        style.region = m_region->translated(-pixelSelection().topLeft());
    }
    return style;
}

void ScreenshotSelectionModel::setDraftRegion(const ScreenshotRegionGeometry& region,
                                              const QVector<QPointF>& vertices) {
    const ChangeRevisionGuard revision(*this);
    m_cachedSelectionRegion.reset();
    m_cachedMarqueeRegion.reset();
    m_draftVertices = vertices;
    m_draftRegion = region;
    const QRectF bounds(region.boundingRect());
    m_start = bounds.topLeft();
    m_end = bounds.bottomRight();
    m_selectionFromDrag = false;
}

void ScreenshotSelectionModel::clearDraftRegion() {
    const ChangeRevisionGuard revision(*this);
    m_cachedSelectionRegion.reset();
    m_cachedMarqueeRegion.reset();
    m_draftVertices.clear();
    m_draftRegion.reset();
    m_start = {};
    m_end = {};
    m_selectionFromDrag = false;
}

void ScreenshotSelectionModel::commitDraftRegion(const QRect& canvasBounds) {
    if (m_draftRegion) {
        const auto region = selectionRegion();
        setSelectionRegion(canvasBounds.isEmpty() ? region : region.intersected(canvasBounds));
    }
}
