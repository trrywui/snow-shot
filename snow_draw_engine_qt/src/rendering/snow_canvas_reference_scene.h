#pragma once

#include "snow_canvas_display_cache.h"
#include "snow_canvas_renderer.h"
#include "snow_canvas_pen_mask_atlas.h"
#include "snow_canvas_viewport.h"
#include "snow_draw_engine_qt/snow_canvas_custom_renderer.h"

#include <QFont>
#include <QPainter>

// Widget-owned, single-resolution surface. The fixed viewport preserves the full
// source scene and its explicit filter passes even when the live viewport crops it.
class SnowCanvasReferenceScene final {
  public:
    ~SnowCanvasReferenceScene();
    std::uint64_t viewportId() const;
    // Synchronize the fixed source grid before scheduling damage or rendering.
    bool sync(SnowRuntime runtime, const SnowCanvasFilterRenderReference& reference);
    QRegion spotlightDamage(const QTransform& canvasToView) const;
    QRegion backdropDamage(const QRegion& viewDamage, const QTransform& canvasToView) const;
    bool render(SnowRuntime runtime, const SnowCanvasFilterRenderReference& reference,
                const snow_canvas_renderer::SceneRenderRequest& request,
                const QTransform& canvasToView);
    void clearRenderState();
    void reset();

  private:
    QTransform sourceTransform() const;
    QRegion mapSourceDamage(const QRegion& damage, const QTransform& canvasToView) const;
    SnowCanvasViewport m_viewport;
    SnowCanvasDisplayCache m_displayCache;
    snow_canvas_pen_mask::PenMaskAtlas m_penMasks;
    SnowCanvasFilterRenderReference m_reference;
    QSize m_pixelSize;
    QImage m_image;
    QRegion m_pendingDamage;
    std::uint64_t m_spotlightFingerprint = 0;
    std::uint64_t m_backgroundRevision = 0;
    SnowCanvasRenderContext::Purpose m_renderPurpose = SnowCanvasRenderContext::Purpose::Widget;
    SnowCanvasCustomRenderer* m_backgroundRenderer = nullptr;
    QFont m_font;
    QPainter::RenderHints m_renderHints;
    bool m_clearBackgroundEnabled = true;
};
