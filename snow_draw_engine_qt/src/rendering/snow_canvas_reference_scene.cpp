#include "snow_draw_engine_qt/snow_canvas_image.h"
#include "snow_canvas_reference_scene.h"

#include "snow_canvas_pen_mask_atlas.h"
#include "snow_canvas_filter_tile_cache.h"

#include <QtMath>

#include <cmath>
#include <limits>

SnowCanvasReferenceScene::~SnowCanvasReferenceScene() {
    snow_canvas_filter_tile_cache::invalidateNamespace(this);
}

std::uint64_t SnowCanvasReferenceScene::viewportId() const {
    std::uint64_t id = 0;
    m_viewport.id(&id);
    return id;
}

void SnowCanvasReferenceScene::clearRenderState() {
    m_image = {};
    m_pendingDamage = {};
    m_backgroundRenderer = nullptr;
    m_displayCache.clearRenderState();
    m_penMasks.clear();
    snow_canvas_filter_tile_cache::invalidateNamespace(this);
}

void SnowCanvasReferenceScene::reset() {
    if (!m_viewport.isValid() && m_image.isNull()) {
        return;
    }
    clearRenderState();
    m_viewport.reset();
    m_displayCache.reset({255, 255, 255, 255});
    m_reference = {};
    m_pixelSize = {};
}

bool SnowCanvasReferenceScene::sync(SnowRuntime runtime,
                                    const SnowCanvasFilterRenderReference& reference) {
    const QRectF bounds = reference.canvasRect;
    const qreal scale = reference.pixelsPerCanvasUnit;
    const double width = std::ceil(bounds.width() * scale);
    const double height = std::ceil(bounds.height() * scale);
    if (runtime == nullptr || !bounds.isValid() || !std::isfinite(bounds.x()) ||
        !std::isfinite(bounds.y()) || !std::isfinite(scale) || scale <= 0 ||
        !std::isfinite(width) || !std::isfinite(height) || width < 1 || height < 1 ||
        width >= std::numeric_limits<int>::max() || height >= std::numeric_limits<int>::max()) {
        reset();
        return false;
    }
    const QSize pixelSize(static_cast<int>(width), static_cast<int>(height));
    if (!m_viewport.isValid() || m_viewport.runtime() != runtime ||
        m_reference.canvasRect != bounds || m_reference.pixelsPerCanvasUnit != scale) {
        reset();
        auto config = snow_canvas_viewport::defaultEngineConfig();
        config.min_zoom = qMin(config.min_zoom, scale);
        config.max_zoom = qMax(config.max_zoom, scale);
        if (!m_viewport.create(runtime, config) ||
            snow_viewport_set_surface_size(
                runtime, m_viewport.get(), static_cast<std::uint32_t>(pixelSize.width()),
                static_cast<std::uint32_t>(pixelSize.height())) != SNOW_OK ||
            snow_viewport_set_camera(runtime, m_viewport.get(), bounds.x() + width / (2 * scale),
                                     bounds.y() + height / (2 * scale), scale) != SNOW_OK) {
            reset();
            return false;
        }
        m_reference = reference;
        m_pixelSize = pixelSize;
    }
    const auto previousRevision = m_displayCache.patchCursor().scene_revision;
    if (!m_displayCache.sync(runtime, m_viewport.get())) {
        return false;
    }
    if (m_displayCache.filterIndices().empty()) {
        clearRenderState();
        return false;
    }
    // Damage scheduling can sync again before painting, replacing the geometry deltas.
    // Keep the source-grid masks in step with every patch while those deltas are available.
    m_penMasks.beginFrame(this, m_displayCache.sceneInfo(), 1.0, &m_displayCache);
    if (!m_image.isNull() && previousRevision != m_displayCache.patchCursor().scene_revision) {
        const QRect sourceRect(QPoint(), m_pixelSize);
        // Consume every patch before another sync replaces its dirty rectangles. Effects can
        // read neighboring pixels, so propagate damage through the ordered filter passes.
        QRegion damage;
        if (m_displayCache.sceneDirtyRectCount() == 0)
            damage = sourceRect;
        else
            for (std::uint32_t index = 0; index < m_displayCache.sceneDirtyRectCount(); ++index)
                damage += snow_canvas_display::dirtyRectToUpdateRect(
                    m_displayCache.sceneDirtyRects()[index]);
        // Clip after expansion: a filter at the image edge may sample changed pixels outside it.
        m_pendingDamage += snow_canvas_renderer::sceneDamageThroughFilters(
                               m_displayCache.executionPlan(1.0), damage)
                               .intersected(sourceRect);
    }
    return true;
}

QTransform SnowCanvasReferenceScene::sourceTransform() const {
    const auto scale = m_reference.pixelsPerCanvasUnit;
    return QTransform(scale, 0, 0, scale, -m_reference.canvasRect.x() * scale,
                      -m_reference.canvasRect.y() * scale);
}

QRegion SnowCanvasReferenceScene::mapSourceDamage(const QRegion& damage,
                                                  const QTransform& canvasToView) const {
    QRegion result;
    if (damage.isEmpty())
        return result;
    const auto sourceToCanvas = sourceTransform().inverted();
    const auto affected =
        snow_canvas_renderer::sceneDamageThroughFilters(m_displayCache.executionPlan(1.0), damage);
    for (const QRect& rect : affected.intersected(QRect(QPoint(), m_pixelSize))) {
        // Presentation can sample a neighboring source pixel during linear resampling.
        result += canvasToView.mapRect(sourceToCanvas.mapRect(QRectF(rect).adjusted(-1, -1, 1, 1)))
                      .toAlignedRect();
    }
    return result;
}

QRegion SnowCanvasReferenceScene::spotlightDamage(const QTransform& canvasToView) const {
    return mapSourceDamage(m_displayCache.spotlightDamage(), canvasToView);
}

QRegion SnowCanvasReferenceScene::backdropDamage(const QRegion& viewDamage,
                                                 const QTransform& canvasToView) const {
    QRegion sourceDamage;
    const auto viewToCanvas = canvasToView.inverted();
    const auto transform = sourceTransform();
    for (const QRect& rect : viewDamage)
        sourceDamage += transform.mapRect(viewToCanvas.mapRect(QRectF(rect))).toAlignedRect();
    return viewDamage +
           mapSourceDamage(sourceDamage.intersected(QRect(QPoint(), m_pixelSize)), canvasToView);
}

bool SnowCanvasReferenceScene::render(SnowRuntime runtime,
                                      const SnowCanvasFilterRenderReference& reference,
                                      const snow_canvas_renderer::SceneRenderRequest& request,
                                      const QTransform& canvasToView) {
    if (request.painter == nullptr || !sync(runtime, reference))
        return false;

    const auto backgroundRevision =
        request.backgroundRenderer != nullptr ? request.backgroundRenderer->contentRevision() : 0;
    const auto purpose = request.backgroundContext != nullptr
                             ? request.backgroundContext->purpose
                             : SnowCanvasRenderContext::Purpose::Widget;
    auto sourceRequest = request;
    sourceRequest.spotlightCutouts = m_displayCache.spotlightCutouts();
    sourceRequest.spotlightCutoutCount = m_displayCache.spotlightCutoutCount();
    const auto spotlightKey = snow_canvas_renderer::spotlightFingerprint(sourceRequest);
    const bool fullRebuild =
        m_image.isNull() || m_spotlightFingerprint != spotlightKey ||
        m_backgroundRenderer != request.backgroundRenderer ||
        m_backgroundRevision != backgroundRevision || m_renderPurpose != purpose ||
        m_clearBackgroundEnabled != request.clearBackgroundEnabled ||
        m_font != request.painter->font() || m_renderHints != request.painter->renderHints();
    const QRegion damage = fullRebuild ? QRegion(QRect(QPoint(), m_pixelSize)) : m_pendingDamage;
    const bool rebuild = !damage.isEmpty();
    if (rebuild) {
        if (m_image.isNull()) {
            m_image = snowCanvasAllocateImage(m_pixelSize, QImage::Format_ARGB32_Premultiplied);
            if (m_image.isNull())
                return false;
        }
        QPainter painter(&m_image);
        painter.setClipRegion(damage);
        painter.setCompositionMode(QPainter::CompositionMode_Source);
        painter.fillRect(m_image.rect(), request.clearBackgroundEnabled
                                             ? snow_canvas_renderer::toQColor(
                                                   m_displayCache.sceneInfo().clear_color)
                                             : Qt::transparent);
        painter.setCompositionMode(QPainter::CompositionMode_SourceOver);
        snow_canvas_renderer::copyPainterFontAndHints(painter, *request.painter);
        const SnowCanvasRenderContext context{m_image.rect(), damage, sourceTransform(), 1.0,
                                              purpose};
        snow_canvas_filter_render::RenderWorkspace workspace;
        sourceRequest.painter = &painter;
        sourceRequest.displayInfo = &m_displayCache.sceneInfo();
        sourceRequest.sceneItems = m_displayCache.sceneItems();
        sourceRequest.sceneItemCount = m_displayCache.sceneItemCount();
        sourceRequest.exposedRegion = context.exposedRegion;
        sourceRequest.candidateIndices = nullptr;
        sourceRequest.candidateCount = 0;
        sourceRequest.backgroundContext = &context;
        sourceRequest.displayCache = &m_displayCache;
        sourceRequest.workspace = &workspace;
        sourceRequest.penMaskAtlas = &m_penMasks;
        sourceRequest.enableFilterTileCache = false;
        sourceRequest.enableOriginalBackgroundCache = true;
        sourceRequest.cacheNamespace = this;
        sourceRequest.executionPlan = nullptr;
        sourceRequest.renderPlan = nullptr;
        sourceRequest.diagnostics = nullptr;
        snow_canvas_renderer::renderSceneItems(sourceRequest);
        painter.end();
        m_displayCache.clearRenderState();
        m_pendingDamage = {};
        m_spotlightFingerprint = spotlightKey;
        m_backgroundRevision = backgroundRevision;
        m_renderPurpose = purpose;
        m_backgroundRenderer = request.backgroundRenderer;
        m_clearBackgroundEnabled = request.clearBackgroundEnabled;
        m_font = request.painter->font();
        m_renderHints = request.painter->renderHints();
    }

    const QRectF rasterBounds(m_reference.canvasRect.topLeft(),
                              QSizeF(m_pixelSize) / m_reference.pixelsPerCanvasUnit);
    const QRectF target = canvasToView.mapRect(rasterBounds);
    QPainter& painter = *request.painter;
    painter.save();
    painter.setClipRegion(request.exposedRegion, Qt::IntersectClip);
    // Match pinned image sampling: pixel-exact at 1:1, linear at every other scale.
    const QRectF deviceTarget = painter.deviceTransform().mapRect(target);
    const QSize deviceSize(qRound(deviceTarget.width()), qRound(deviceTarget.height()));
    painter.setRenderHint(QPainter::SmoothPixmapTransform, deviceSize != m_pixelSize);
    if (reference.presentation == SnowCanvasFilterRenderReference::Presentation::Composite) {
        // The host has already painted its background. Clearing it would turn transparent
        // and antialiased edge pixels black on an opaque backing store. The reference
        // includes its entire backdrop, so composite it exactly once, including Spotlight.
        painter.setCompositionMode(QPainter::CompositionMode_SourceOver);
    } else {
        painter.setCompositionMode(QPainter::CompositionMode_Source);
        if (request.backgroundContext != nullptr) {
            painter.fillRect(
                request.backgroundContext->viewportRect,
                request.clearBackgroundEnabled
                    ? snow_canvas_renderer::toQColor(m_displayCache.sceneInfo().clear_color)
                    : Qt::transparent);
        }
        // The reference owns pixels only inside its canvas rectangle. Spotlight also
        // covers the surrounding viewport; Source presentation replaces the interior
        // (including transparent pixels) so the backdrop is never composited twice.
        const QRect coveredInterior(qCeil(target.left()), qCeil(target.top()),
                                    qFloor(target.right()) - qCeil(target.left()),
                                    qFloor(target.bottom()) - qCeil(target.top()));
        const QRegion outside = request.exposedRegion.subtracted(QRegion(coveredInterior));
        if (!outside.isEmpty()) {
            painter.setCompositionMode(QPainter::CompositionMode_SourceOver);
            snow_canvas_renderer::renderSpotlightBackdrop(painter, request, outside);
        }
        painter.setCompositionMode(QPainter::CompositionMode_Source);
    }
    painter.drawImage(target, m_image);
    painter.restore();
    std::size_t updatedPixels = 0;
    for (const auto& rect : damage)
        updatedPixels +=
            static_cast<std::size_t>(rect.width()) * static_cast<std::size_t>(rect.height());
    snow_canvas_renderer::recordReferenceScenePresentation(
        rebuild, static_cast<std::size_t>(m_image.sizeInBytes()), updatedPixels);
    if (request.diagnostics != nullptr) {
        *request.diagnostics = snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread();
    }
    return true;
}
