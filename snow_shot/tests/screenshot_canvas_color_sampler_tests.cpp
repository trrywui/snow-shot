#include "snow_shot/presentation/screenshotcanvascolorsampler.h"
#include "snow_draw_engine_qt/snow_canvas_custom_renderer.h"
#include "snow_draw_engine_qt/snow_canvas_runtime.h"
#include "snow_draw_engine_qt/snow_canvas_widget.h"

#include <QApplication>
#include <QColor>
#include <QImage>
#include <QPainter>

#include <iostream>
#include <stdexcept>

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        throw std::runtime_error(message);
    }
}

QColor colorForPixel(int x, int y) {
    return QColor((x * 19 + y * 3) % 256, (x * 7 + y * 23) % 256, (x * 29 + y * 11) % 256);
}

void exactPhysicalPixelsRemainDistinct() {
    QImage raster(13, 11, QImage::Format_ARGB32_Premultiplied);
    for (int y = 0; y < raster.height(); ++y) {
        for (int x = 0; x < raster.width(); ++x) {
            raster.setPixelColor(x, y, colorForPixel(x, y));
        }
    }

    const QRect physicalBounds(-40, 75, raster.width(), raster.height());
    const QPoint firstPhysical = physicalBounds.topLeft() + QPoint(5, 4);
    const QPoint secondPhysical = firstPhysical + QPoint(1, 0);
    const QImage first = ScreenshotCanvasColorSampler::previewFromPhysicalRaster(
        raster, physicalBounds, firstPhysical);
    const QImage second = ScreenshotCanvasColorSampler::previewFromPhysicalRaster(
        raster, physicalBounds, secondPhysical);

    require(first.size() == QSize(ScreenshotCanvasColorSampler::PreviewSize,
                                  ScreenshotCanvasColorSampler::PreviewSize),
            "the canvas sampler preview must remain 7 by 7 physical pixels");
    require(first.pixelColor(3, 3) == colorForPixel(5, 4),
            "the preview center did not preserve the requested physical pixel");
    require(second.pixelColor(3, 3) == colorForPixel(6, 4) &&
                second.pixelColor(3, 3) != first.pixelColor(3, 3),
            "adjacent physical pixels collapsed to the same sampled color");
}

void fractionalLogicalCoordinatesMapBeforeRounding() {
    const QRect physicalBounds(500, 200, 150, 120);
    const QSize logicalSize(100, 80);
    const QPoint first = ScreenshotCanvasColorSampler::physicalPointForLocalPosition(
        QPointF(100.0 / 1.5, 45.0 / 1.5), logicalSize, physicalBounds);
    const QPoint second = ScreenshotCanvasColorSampler::physicalPointForLocalPosition(
        QPointF(101.0 / 1.5, 45.0 / 1.5), logicalSize, physicalBounds);

    require(first == QPoint(600, 245),
            "the first fractional logical position mapped to the wrong physical pixel");
    require(second == QPoint(601, 245),
            "the adjacent fractional logical position was rounded before physical mapping");
}

void previewClampsAtPhysicalEdges() {
    QImage raster(4, 3, QImage::Format_ARGB32_Premultiplied);
    for (int y = 0; y < raster.height(); ++y) {
        for (int x = 0; x < raster.width(); ++x) {
            raster.setPixelColor(x, y, colorForPixel(x, y));
        }
    }

    const QRect physicalBounds(10, 20, raster.width(), raster.height());
    const QImage preview = ScreenshotCanvasColorSampler::previewFromPhysicalRaster(
        raster, physicalBounds, physicalBounds.topLeft());
    require(preview.pixelColor(0, 0) == colorForPixel(0, 0) &&
                preview.pixelColor(3, 3) == colorForPixel(0, 0) &&
                preview.pixelColor(6, 6) == colorForPixel(3, 2),
            "the physical preview did not clamp consistently at the raster edge");
}

void contentSnapshotsPreservePhysicalPixelsAndReuseTheRaster() {
    class SourceRenderer final : public SnowCanvasCustomRenderer {
      public:
        QList<SnowCanvasBaseImageSource> sources;
        mutable int sourceRequests = 0;
        QList<SnowCanvasBaseImageSource> baseImageSources() const override {
            ++sourceRequests;
            return sources;
        }
        void renderBeforeCanvas(QPainter& painter,
                                const SnowCanvasRenderContext& context) override {
            for (const auto& source : sources) {
                painter.save();
                if (!source.coverage.isEmpty())
                    painter.setClipRect(context.canvasToViewTransform.mapRect(source.coverage),
                                        Qt::IntersectClip);
                painter.drawImage(context.canvasToViewTransform.mapRect(source.canvasRect),
                                  source.image);
                painter.restore();
            }
        }
    } renderer;
    QImage image(150, 120, QImage::Format_ARGB32_Premultiplied);
    for (int y = 0; y < image.height(); ++y)
        for (int x = 0; x < image.width(); ++x)
            image.setPixelColor(x, y, colorForPixel(x, y));
    const QRectF sourceBounds(-300, -200, 150, 120);
    renderer.sources.append({image, sourceBounds});
    QImage patch(30, 20, QImage::Format_ARGB32_Premultiplied);
    patch.fill(Qt::yellow);
    renderer.sources.append({patch, QRectF(-200, -120, 30, 20), QRectF(-195, -115, 5, 5)});
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    canvas.resize(100, 80);
    canvas.show();
    require(
        canvas.setViewportCamera(sourceBounds.center().x(), sourceBounds.center().y(), 1.0 / 1.5),
        "configure the fractional-DPI content viewport");
    canvas.setCustomRenderer(&renderer);
    const QRect physicalBounds(-450, 300, 150, 120);
    ScreenshotCanvasColorSampler sampler;
    require(sampler.ensureContentSnapshot(canvas, physicalBounds),
            "create a snapshot at the display's physical resolution");
    const int snapshotRequests = renderer.sourceRequests;
    for (int sample = 0; sample < 100; ++sample) {
        require(sampler.ensureContentSnapshot(canvas, physicalBounds),
                "cursor movement reuses the current content snapshot");
        const QPoint pixel(4 + sample, 40);
        const QImage preview = sampler.previewAtPhysicalPoint(physicalBounds.topLeft() + pixel);
        for (int y = 0; y < preview.height(); ++y)
            for (int x = 0; x < preview.width(); ++x)
                require(preview.pixelColor(x, y) == colorForPixel(pixel.x() + x - 3, 40 + y - 3),
                        "fractional-DPI content snapshots preserve every physical preview pixel");
    }
    require(renderer.sourceRequests == snapshotRequests,
            "cursor sampling must not rebuild the document raster or reread image sources");
    require(sampler.previewAtPhysicalPoint(physicalBounds.topLeft() + QPoint(106, 86))
                        .pixelColor(3, 3) == QColor(Qt::yellow) &&
                sampler.previewAtPhysicalPoint(physicalBounds.topLeft() + QPoint(101, 81))
                        .pixelColor(3, 3) == colorForPixel(101, 81),
            "content snapshots must preserve image-layer ordering and coverage");
    sampler.reset();
    require(sampler.ensureContentSnapshot(canvas, physicalBounds) &&
                renderer.sourceRequests == snapshotRequests + 1,
            "a new sampling session must rebuild the snapshot once");
    const QRect movedBounds = physicalBounds.translated(200, 0);
    require(sampler.ensureContentSnapshot(canvas, movedBounds) &&
                renderer.sourceRequests == snapshotRequests + 2,
            "changing the display bounds must replace the cached snapshot");
    require(!sampler.ensureContentSnapshot(canvas, {}) &&
                sampler.previewAtPhysicalPoint(movedBounds.center()).isNull(),
            "invalid display bounds must discard the old sampling pixels");
    canvas.setCustomRenderer(nullptr);
}

void contentSnapshotsKeepFilterCachesSeparateFromWidgetPixels(bool useReference) {
    class PurposeRenderer final : public SnowCanvasCustomRenderer {
      public:
        bool reference = false;
        std::optional<SnowCanvasFilterRenderReference> filterRenderReference() const override {
            if (reference)
                return SnowCanvasFilterRenderReference{QRectF(0, 0, 100, 80), 1.0};
            return std::nullopt;
        }
        void renderBeforeCanvas(QPainter& painter,
                                const SnowCanvasRenderContext& context) override {
            painter.fillRect(context.viewportRect,
                             context.purpose == SnowCanvasRenderContext::Purpose::Widget ? Qt::green
                                                                                         : Qt::red);
        }
    } renderer;
    renderer.reference = useReference;
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    canvas.resize(100, 80);
    canvas.show();
    require(canvas.setViewportCamera(50, 40, 1), "configure the filter snapshot viewport");
    canvas.setCustomRenderer(&renderer);
    require(
        !runtime
             .applyAnnotationTransaction(QByteArrayLiteral(
                 R"({"version":1,"operations":[{"type":"rectangle_filter","bounds":[10,10,80,60],"style":{"filter":"inversion"}}]})"))
             .isEmpty(),
        "create a filter that samples the host's rendered background");
    const auto widgetPixels = canvas.grab().toImage();
    require(widgetPixels.pixelColor(50, 40) == QColor(Qt::magenta),
            "the widget must invert its green background");
    const auto contentPixels = canvas.renderContentToImage(canvas.size());
    require(contentPixels.pixelColor(50, 40) == QColor(Qt::cyan),
            "content snapshots must pass their render purpose through filters and cache lookups");
    require(canvas.grab().toImage() == widgetPixels,
            "content snapshot filter tiles must not replace the widget's cached pixels");
    canvas.setCustomRenderer(nullptr);
}
} // namespace

int main(int argc, char** argv) {
    QApplication application(argc, argv);
    try {
        exactPhysicalPixelsRemainDistinct();
        fractionalLogicalCoordinatesMapBeforeRounding();
        previewClampsAtPhysicalEdges();
        contentSnapshotsPreservePhysicalPixelsAndReuseTheRaster();
        contentSnapshotsKeepFilterCachesSeparateFromWidgetPixels(false);
        contentSnapshotsKeepFilterCachesSeparateFromWidgetPixels(true);
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
    return 0;
}
