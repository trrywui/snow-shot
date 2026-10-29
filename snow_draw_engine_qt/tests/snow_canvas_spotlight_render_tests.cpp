#include "snow_canvas_display_item.h"
#include "snow_canvas_export.h"
#include "snow_canvas_ffi_handles.h"
#include "snow_canvas_render_diagnostics.h"
#include "snow_canvas_spotlight_renderer.h"
#include "snow_canvas_renderer.h"
#include "snow_canvas_filter_tile_cache.h"
#include "snow_canvas_viewport.h"
#include "snow_draw_engine_qt/snow_canvas_widget.h"
#include "snow_draw_engine_qt/snow_canvas_runtime.h"
#include "snow_draw_engine_qt/snow_canvas_custom_renderer.h"

#include <QApplication>
#include <QColor>
#include <QCoreApplication>
#include <QElapsedTimer>
#include <QEvent>
#include <QImage>
#include <QObject>
#include <QPainter>
#include <QPaintEvent>
#include <QRegion>
#include <QThread>

#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <limits>
#include <vector>

namespace {

void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(1);
    }
}

class PaintObserver final : public QObject {
  public:
    void begin() {
        m_sawPaint = false;
        m_observing = true;
    }

    bool sawPaint() const {
        return m_sawPaint;
    }

  protected:
    bool eventFilter(QObject* watched, QEvent* event) override {
        Q_UNUSED(watched);
        if (m_observing && event != nullptr && event->type() == QEvent::Paint) {
            m_sawPaint = true;
        }
        return false;
    }

  private:
    bool m_observing = false;
    bool m_sawPaint = false;
};

SceneDisplayInfo sceneInfo() {
    SceneDisplayInfo info;
    info.surface_width = 100.0;
    info.surface_height = 100.0;
    info.camera_center_x = 50.0;
    info.camera_center_y = 50.0;
    info.camera_zoom = 1.0;
    return info;
}

SnowSpotlightCutout cutout(double centerX, double centerY, double width, double height,
                           double rotation = 0.0) {
    SnowSpotlightCutout item{};
    item.center_x = centerX;
    item.center_y = centerY;
    item.width = width;
    item.height = height;
    item.rotation = rotation;
    return item;
}

QImage render(const SnowSpotlightCutout* items, std::uint32_t itemCount,
              const QRectF& renderArea = QRectF(0.0, 0.0, 100.0, 100.0),
              const QRegion& exposed = QRegion(QRect(0, 0, 100, 100)), bool active = true,
              const SceneDisplayInfo& projection = sceneInfo()) {
    QImage image(100, 100, QImage::Format_ARGB32_Premultiplied);
    image.fill(Qt::white);
    QPainter painter(&image);
    painter.setClipRegion(exposed);
    snow_canvas_spotlight_renderer::render(
        painter, projection, SpotlightDisplayInfo{SnowColorRgba8{0, 0, 0, 255}, 0.64, active},
        items, itemCount, renderArea, exposed);
    painter.end();
    return image;
}

bool isDefaultMaskPixel(QRgb pixel) {
    return qAlpha(pixel) == 255 && qRed(pixel) == qGreen(pixel) && qGreen(pixel) == qBlue(pixel) &&
           qRed(pixel) >= 91 && qRed(pixel) <= 93;
}

void defaultMaskHasExactOpacityAndTransparentHole() {
    const SnowSpotlightCutout item = cutout(50.0, 50.0, 40.0, 30.0);
    const QImage image = render(&item, 1);
    require(isDefaultMaskPixel(image.pixel(5, 5)),
            "default spotlight mask must composite 64% black");
    require(image.pixelColor(50, 50) == QColor(Qt::white),
            "spotlight rectangle must reveal the unmasked canvas");
}

SnowCanvasSceneItem orderFilter(std::uint32_t id, const QRectF& bounds, std::uint32_t type = 3) {
    SnowSceneDisplayItem item{};
    item.kind = SNOW_SCENE_DISPLAY_ITEM_FILTER;
    item.element_id = {id, 1};
    item.center_x = bounds.center().x();
    item.center_y = bounds.center().y();
    item.width = bounds.width();
    item.height = bounds.height();
    item.opacity = 1;
    item.filter = snow_filter_render_spec_resolve(type, 0.4);
    return SnowCanvasSceneItem(item);
}

QImage renderSceneImage(snow_canvas_renderer::SceneRenderRequest request, bool tiled,
                        double dpr = 1, QPainter::RenderHints hints = {},
                        const QImage& previous = {}) {
    const auto& info = *request.displayInfo;
    QImage image = previous.copy();
    if (image.isNull()) {
        image = QImage(QSize(qRound(info.surface_width * dpr), qRound(info.surface_height * dpr)),
                       QImage::Format_ARGB32_Premultiplied);
        image.setDevicePixelRatio(dpr);
        image.fill(request.clearBackgroundEnabled ? snow_canvas_renderer::toQColor(info.clear_color)
                                                  : Qt::transparent);
    }
    QPainter painter(&image);
    painter.setRenderHints(hints);
    painter.setClipRegion(request.exposedRegion);
    request.painter = &painter;
    if (tiled)
        snow_canvas_renderer::renderSceneItemsTiled(request);
    else
        snow_canvas_renderer::renderSceneItems(request);
    painter.end();
    return image;
}

QImage renderOrderedScene(const std::vector<SnowCanvasSceneItem>& items,
                          const SpotlightDisplayInfo& spotlight, const SnowSpotlightCutout& hole,
                          bool tiled, const void* cacheNamespace, double dpr = 1,
                          const QRegion& exposed = QRegion(QRect(0, 0, 600, 600)),
                          const QImage& previous = {}, bool useBackgroundImage = true) {
    auto info = sceneInfo();
    info.surface_width = info.surface_height = 600;
    info.camera_center_x = info.camera_center_y = 300;
    info.clear_color = {255, 255, 255, 255};
    QImage background(QSize(qRound(600 * dpr), qRound(600 * dpr)),
                      QImage::Format_ARGB32_Premultiplied);
    background.setDevicePixelRatio(dpr);
    background.fill(Qt::white);
    snow_canvas_renderer::SceneRenderRequest request;
    request.displayInfo = &info;
    request.sceneItems = items.data();
    request.sceneItemCount = static_cast<std::uint32_t>(items.size());
    request.backgroundImage = useBackgroundImage ? &background : nullptr;
    request.exposedRegion = exposed;
    request.cacheNamespace = cacheNamespace;
    request.filterTileContentKey = 17;
    request.spotlightInfo = &spotlight;
    request.spotlightCutouts = &hole;
    request.spotlightCutoutCount = 1;
    return renderSceneImage(request, tiled, dpr, QPainter::Antialiasing, previous);
}

void spotlightFormsBackdropForSceneItems() {
    SpotlightDisplayInfo spotlight{{255, 0, 0, 255}, 0.5, true};
    const auto hole = cutout(50, 50, 20, 20);
    const auto firstFilter = orderFilter(1, QRectF(0, 0, 600, 600));
    const auto secondFilter = orderFilter(2, QRectF(70, 10, 20, 20));
    SnowSceneDisplayItem rectangle{};
    rectangle.kind = SNOW_SCENE_DISPLAY_ITEM_DRAW_RECT;
    rectangle.element_id = {3, 1};
    rectangle.center_x = rectangle.center_y = 20;
    rectangle.width = rectangle.height = 10;
    rectangle.fill = {0, 0, 255, 255};
    rectangle.opacity = 1;
    rectangle.fill_style = SNOW_FILL_STYLE_SOLID;
    const std::vector<SnowCanvasSceneItem> items{firstFilter, secondFilter,
                                                 SnowCanvasSceneItem(rectangle)};
    for (double dpr : {1.0, 1.25, 2.0}) {
        const auto image = renderOrderedScene(items, spotlight, hole, false, nullptr, dpr);
        const auto pixel = [&](int x, int y) {
            return image.pixelColor(qRound(x * dpr), qRound(y * dpr));
        };
        require(pixel(30, 30).red() == 0 && std::abs(pixel(30, 30).green() - 128) <= 1 &&
                    pixel(30, 30).blue() == pixel(30, 30).green(),
                "scene filters must sample the Spotlight backdrop");
        require(pixel(20, 20) == QColor(Qt::blue),
                "ordinary annotations must paint above Spotlight");
        require(pixel(50, 50) == QColor(Qt::black),
                "filters must sample the unmasked background inside Spotlight cutouts");
        require(pixel(80, 20) == pixel(30, 30),
                "adjacent filters must retain their shared entering backdrop");
        require(renderOrderedScene(items, spotlight, hole, true, &items, dpr) == image,
                "tiled and direct Spotlight composition must match");
        snow_canvas_filter_tile_cache::invalidateNamespace(&items);
        for (bool tiled : {false, true}) {
            require(renderOrderedScene(items, spotlight, hole, tiled, &items, dpr,
                                       QRegion(QRect(0, 0, 600, 600)), {}, false) == image,
                    "the clear color must form the Spotlight backdrop without a background image");
            snow_canvas_filter_tile_cache::invalidateNamespace(&items);
        }
    }
    require(renderOrderedScene({}, spotlight, hole, false, nullptr).pixelColor(30, 30).green() <
                255,
            "Spotlight must render without scene items");
    require(renderOrderedScene({SnowCanvasSceneItem(rectangle)}, spotlight, hole, false, nullptr)
                    .pixelColor(20, 20) == QColor(Qt::blue),
            "Spotlight must sit below ordinary annotations without filters");
    spotlight.opacity = 0;
    const auto transparent = renderOrderedScene(items, spotlight, hole, false, nullptr);
    require(transparent.pixelColor(80, 20) == QColor(Qt::black),
            "a transparent Spotlight must leave the shared filter backdrop unchanged");
    spotlight.active = false;
    const auto inactive = renderOrderedScene(items, spotlight, hole, false, nullptr);
    require(inactive == transparent && inactive.pixelColor(20, 20) == QColor(Qt::blue),
            "inactive Spotlight must leave existing scene rendering unchanged");
}

void spotlightEditsRefreshRetainedFilterSources() {
    SpotlightDisplayInfo spotlight{{0, 0, 0, 255}, 0.64, true};
    auto hole = cutout(256, 300, 40, 40);
    const std::vector<SnowCanvasSceneItem> items{orderFilter(1, QRectF(0, 0, 600, 600), 2),
                                                 orderFilter(2, QRectF(220, 250, 90, 100), 1)};
    for (double dpr : {1.0, 1.25, 2.0}) {
        auto previous = renderOrderedScene(items, spotlight, hole, true, &items, dpr);
        require(previous == renderOrderedScene(items, spotlight, hole, false, nullptr, dpr),
                "filters sampling a Spotlight edge must agree across tile boundaries");
        spotlight.opacity = 0.35;
        spotlight.color = {255, 0, 0, 255};
        hole.center_x += 15;
        const auto expected = renderOrderedScene(items, spotlight, hole, false, nullptr, dpr);
        require(renderOrderedScene(items, spotlight, hole, true, &items, dpr) == expected,
                "warm filter sources must refresh after Spotlight style and cutout edits");
        const QRegion exposed(QRect(200, 200, 130, 200));
        const auto partial =
            renderOrderedScene(items, spotlight, hole, true, &items, dpr, exposed, previous);
        require(partial.copy(QRect(qRound(200 * dpr), qRound(200 * dpr), qRound(130 * dpr),
                                   qRound(200 * dpr))) ==
                    expected.copy(QRect(qRound(200 * dpr), qRound(200 * dpr), qRound(130 * dpr),
                                        qRound(200 * dpr))),
                "partial repaints must match a fresh Spotlight composition");
        snow_canvas_filter_tile_cache::invalidateNamespace(&items);
    }
}

void spotlightBackdropPreservesTransparency() {
    const auto info = sceneInfo();
    const auto filter = orderFilter(1, QRectF(0, 0, 100, 100));
    const SpotlightDisplayInfo spotlight{{255, 0, 0, 255}, 0.5, true};
    const auto hole = cutout(50, 50, 20, 20);
    QImage direct;
    for (bool tiled : {false, true}) {
        snow_canvas_renderer::SceneRenderRequest request;
        request.displayInfo = &info;
        request.sceneItems = &filter;
        request.sceneItemCount = 1;
        request.exposedRegion = QRegion(QRect(0, 0, 100, 100));
        request.cacheNamespace = &filter;
        request.clearBackgroundEnabled = false;
        request.spotlightInfo = &spotlight;
        request.spotlightCutouts = &hole;
        request.spotlightCutoutCount = 1;
        const auto image = renderSceneImage(request, tiled);
        const auto pixel = image.pixelColor(10, 10);
        require(pixel.red() == 0 && pixel.green() == 255 && pixel.blue() == 255 &&
                    std::abs(pixel.alpha() - 128) <= 1 && image.pixelColor(50, 50).alpha() == 0,
                "filters must preserve the alpha of a Spotlight-only backdrop and its cutouts");
        if (tiled)
            require(image == direct, "direct and tiled transparent Spotlight backdrops must match");
        else
            direct = image;
    }
    snow_canvas_filter_tile_cache::invalidateNamespace(&filter);
}

void invisibleSpotlightsPreserveFilterSources() {
    auto info = sceneInfo();
    info.clear_color = {255, 255, 255, 255};
    const auto hole = cutout(50, 50, 20, 20);
    SnowSceneDisplayItem rectangle{};
    rectangle.kind = SNOW_SCENE_DISPLAY_ITEM_DRAW_RECT;
    rectangle.element_id = {2, 1};
    rectangle.center_x = rectangle.center_y = 20;
    rectangle.width = rectangle.height = 10;
    rectangle.fill = {0, 0, 255, 255};
    rectangle.opacity = 1;
    rectangle.fill_style = SNOW_FILL_STYLE_SOLID;
    const SpotlightDisplayInfo visible{{255, 0, 0, 255}, 0.5, true};
    const std::vector<SpotlightDisplayInfo> invisible{
        {{255, 0, 0, 255}, 0.5, false},
        {{255, 0, 0, 255}, 0.0, true},
        {{255, 0, 0, 0}, 0.5, true},
        {{255, 0, 0, 255}, -0.5, true},
        {{255, 0, 0, 255}, std::numeric_limits<double>::quiet_NaN(), true},
    };
    for (bool withDrawable : {false, true}) {
        std::vector<SnowCanvasSceneItem> items;
        if (withDrawable)
            items.emplace_back(rectangle);
        items.push_back(orderFilter(1, QRectF(0, 0, 100, 100)));
        for (double dpr : {1.0, 1.25, 2.0}) {
            QImage background(QSize(qRound(100 * dpr), qRound(100 * dpr)),
                              QImage::Format_ARGB32_Premultiplied);
            background.setDevicePixelRatio(dpr);
            background.fill(QColor(60, 120, 180, 128));
            for (bool withBackground : {false, true}) {
                for (bool clear : {true, false}) {
                    // Exercise direct replay, retained source tiles, and tiled presentation.
                    for (int mode : {0, 1, 2}) {
                        snow_canvas_renderer::SceneRenderRequest request;
                        request.displayInfo = &info;
                        request.sceneItems = items.data();
                        request.sceneItemCount = static_cast<std::uint32_t>(items.size());
                        request.exposedRegion = QRegion(QRect(0, 0, 100, 100));
                        request.backgroundImage = withBackground ? &background : nullptr;
                        request.clearBackgroundEnabled = clear;
                        request.cacheNamespace = &items;
                        request.enableFilterTileCache = mode != 0;
                        request.filterTileContentKey = 23;
                        request.spotlightCutouts = &hole;
                        request.spotlightCutoutCount = 1;
                        const auto renderScene = [&](const SpotlightDisplayInfo* spotlight) {
                            request.spotlightInfo = spotlight;
                            return renderSceneImage(request, mode == 2, dpr);
                        };
                        const auto baseline = renderScene(nullptr);
                        for (const auto& spotlight : invisible) {
                            require(renderScene(&spotlight) == baseline,
                                    "invisible Spotlight must preserve existing filter sources");
                            require(renderScene(&visible) != baseline,
                                    "visible Spotlight must still contribute a filter backdrop");
                            require(renderScene(&spotlight) == baseline,
                                    "hiding Spotlight must retire its cached filter backdrop");
                        }
                        snow_canvas_filter_tile_cache::invalidateNamespace(&items);
                    }
                }
            }
        }
    }
}

class SpotlightTestBackground final : public SnowCanvasCustomRenderer {
  public:
    bool retained = false;
    std::optional<SnowCanvasFilterRenderReference> filterRenderReference() const override {
        if (retained)
            return SnowCanvasFilterRenderReference{QRectF(0, 0, 100, 100), 1.0};
        return std::nullopt;
    }
    void renderBeforeCanvas(QPainter& painter, const SnowCanvasRenderContext& context) override {
        painter.fillRect(context.viewportRect, Qt::white);
    }
};

void runtimeSpotlightOrderMatchesWidgetAndExports() {
    for (bool reverse : {false, true}) {
        SnowCanvasRuntime runtime;
        const QByteArray rectangle =
            R"({"type":"rectangle","bounds":[15,15,10,10],"style":{"fill":[0,0,255,255],"stroke_width":0}})";
        const QByteArray spotlight =
            R"({"type":"spotlight","bounds":[40,40,20,20],"style":{"color":[255,0,0,255],"opacity":0.64,"corner_radius":0}})";
        const auto fill = [&]() {
            SnowCanvasRuntimeEditor editor(runtime, SnowCanvasTool::AutoFilter);
            SnowCanvasFilterStyle style;
            style.type = SnowCanvasFilterType::Inversion;
            require(editor.setFilterStyleFromToolbar(style, SnowCanvasFilterStylePropertyType),
                    "configure inversion filter");
            const SnowCanvasAutoFilterRecord record{
                QRectF(0, 0, 100, 100), {{1, QRectF(0, 0, 100, 100), QStringLiteral("text")}}};
            require(editor.setAutoFilterRegions(record) &&
                        editor.fillAutoFilterCategory(QStringLiteral("text")),
                    "create automatic filter below all annotations");
        };
        if (!reverse)
            fill();
        const QByteArray operations =
            reverse ? spotlight + "," + rectangle : rectangle + "," + spotlight;
        require(
            !runtime
                 .applyAnnotationTransaction("{\"version\":1,\"operations\":[" + operations + "]}")
                 .isEmpty(),
            "create Spotlight and rectangle in either order");
        if (reverse)
            fill();
        SpotlightTestBackground backgroundRenderer;
        SnowCanvasWidget canvas(runtime);
        canvas.resize(100, 100);
        canvas.setCustomRenderer(&backgroundRenderer);
        require(canvas.setCanvasTool(SnowCanvasTool::Select), "hide Auto Filter editing overlays");
        canvas.show();
        require(canvas.setViewportCamera(50, 50, 1), "set test canvas camera");
        QCoreApplication::processEvents();
        QImage background(100, 100, QImage::Format_ARGB32_Premultiplied);
        background.fill(Qt::white);
        const QRectF bounds(0, 0, 100, 100);
        const QList<CanvasExportSource> sources{{background, bounds}};
        const auto exported = runtime.renderToImage(bounds, QSize(100, 100), sources);
        require(exported.pixelColor(20, 20) == QColor(Qt::blue) &&
                    exported.pixelColor(30, 30).red() == 0 &&
                    std::abs(exported.pixelColor(30, 30).green() - 163) <= 1 &&
                    exported.pixelColor(50, 50) == QColor(Qt::black),
                "scene filters must sample Spotlight regardless of creation order or origin");
        const auto tiled = runtime.renderTilesToImage(
            bounds, QSize(100, 100), QSize(100, 100),
            {{QRect(0, 0, 50, 100), QPoint(0, 0)}, {QRect(50, 0, 50, 100), QPoint(50, 0)}},
            sources);
        require(tiled == exported, "tiled export must match full Spotlight export");
        for (bool retained : {false, true}) {
            backgroundRenderer.retained = retained;
            auto config = canvas.canvasSpotlightConfig();
            config.opacity = 0.64;
            require(canvas.setCanvasSpotlightConfig(config), "reset Spotlight opacity");
            auto widgetImage = canvas.grab().toImage();
            require(widgetImage.pixelColor(20, 20) == QColor(Qt::blue) &&
                        widgetImage.pixelColor(30, 30).red() == 0 &&
                        std::abs(widgetImage.pixelColor(30, 30).green() - 163) <= 1,
                    "live and retained widgets must match export stacking");
            config.opacity = 0.25;
            require(canvas.setCanvasSpotlightConfig(config), "edit Spotlight opacity");
            widgetImage = canvas.grab().toImage();
            require(widgetImage.pixelColor(30, 30).red() == 0 &&
                        std::abs(widgetImage.pixelColor(30, 30).green() - 64) <= 1 &&
                        widgetImage.pixelColor(20, 20) == QColor(Qt::blue),
                    "Spotlight edits must refresh the retained widget without tinting annotations");
            require(
                !runtime
                     .applyAnnotationTransaction(
                         R"({"version":1,"operations":[{"type":"spotlight","bounds":[25,25,10,10],"style":{"color":[255,0,0,255],"opacity":0.25,"corner_radius":0}}]})")
                     .isEmpty(),
                "add a cutout after the widget has retained its scene");
            widgetImage = canvas.grab().toImage();
            require(widgetImage.pixelColor(30, 30) == QColor(Qt::black),
                    "cutout changes must invalidate retained Spotlight pixels");
            require(runtime.undo(), "restore the original cutout for the next renderer");
        }
    }
}

class SpotlightDamageObserver final : public QObject {
  public:
    QRegion painted;

  protected:
    bool eventFilter(QObject*, QEvent* event) override {
        if (event->type() == QEvent::Paint)
            painted += static_cast<QPaintEvent*>(event)->region();
        return false;
    }
};

class SpotlightReferenceBackground final : public SnowCanvasCustomRenderer {
  public:
    bool retained = false;
    QRectF bounds{0, 0, 300, 220};
    qreal sourceScale = 1.0;
    std::optional<SnowCanvasFilterRenderReference> filterRenderReference() const override {
        return retained ? std::optional(SnowCanvasFilterRenderReference{bounds, sourceScale})
                        : std::nullopt;
    }
    void renderBeforeCanvas(QPainter& painter, const SnowCanvasRenderContext& context) override {
        painter.fillRect(context.canvasToViewTransform.mapRect(bounds), Qt::white);
    }
};

QImage renderSpotlightWidget(SnowCanvasWidget& canvas) {
    QImage image(canvas.size(), QImage::Format_ARGB32_Premultiplied);
    image.fill(Qt::transparent);
    canvas.render(&image);
    return image;
}

void transparentSpotlightPreservesWidgetAndExports() {
    for (bool withFilter : {false, true}) {
        for (bool transparentColor : {false, true}) {
            for (int backgroundMode : {0, 1, 2}) {
                SnowCanvasRuntime runtime;
                if (withFilter) {
                    require(
                        !runtime
                             .applyAnnotationTransaction(
                                 R"({"version":1,"operations":[{"type":"rectangle_filter","bounds":[0,0,100,100],"style":{"filter":"inversion"}}]})")
                             .isEmpty(),
                        "create an inversion filter without Spotlight");
                }
                SpotlightTestBackground background;
                background.retained = backgroundMode == 2;
                SnowCanvasWidget canvas(runtime);
                canvas.resize(100, 100);
                if (backgroundMode != 0)
                    canvas.setCustomRenderer(&background);
                canvas.show();
                require(canvas.setViewportCamera(50, 50, 1), "set transparent Spotlight camera");
                QCoreApplication::processEvents();
                const auto baseline = renderSpotlightWidget(canvas);
                const QRectF bounds(0, 0, 100, 100);
                QImage image(100, 100, QImage::Format_ARGB32_Premultiplied);
                image.fill(Qt::white);
                const QList<CanvasExportSource> sources =
                    backgroundMode == 0 ? QList<CanvasExportSource>{}
                                        : QList<CanvasExportSource>{{image, bounds}};
                const auto baselineExport = runtime.renderToImage(bounds, image.size(), sources);
                require(
                    !runtime
                         .applyAnnotationTransaction(
                             R"({"version":1,"operations":[{"type":"spotlight","bounds":[40,40,20,20],"style":{"opacity":0,"corner_radius":0}}]})")
                         .isEmpty(),
                    "add a fully transparent Spotlight");
                const SnowCanvasSpotlightConfig hidden{
                    QColor(255, 0, 0, transparentColor ? 0 : 255), transparentColor ? 0.5 : 0.0};
                require(canvas.setCanvasSpotlightConfig(hidden), "set invisible Spotlight style");
                const auto requireHidden = [&] {
                    require(renderSpotlightWidget(canvas) == baseline,
                            "transparent Spotlight must preserve the widget before its creation");
                    require(runtime.renderToImage(bounds, image.size(), sources) == baselineExport,
                            "transparent Spotlight must preserve full exports");
                    require(runtime.renderTilesToImage(bounds, image.size(), image.size(),
                                                       {{QRect(0, 0, 50, 100), QPoint(0, 0)},
                                                        {QRect(50, 0, 50, 100), QPoint(50, 0)}},
                                                       sources) == baselineExport,
                            "transparent Spotlight must preserve tiled exports");
                };
                requireHidden();
                const SnowCanvasSpotlightConfig visible{QColor(255, 0, 0, 255), 0.5};
                require(canvas.setCanvasSpotlightConfig(visible), "show Spotlight");
                require(renderSpotlightWidget(canvas) != baseline,
                        "visible Spotlight must change the widget");
                canvas.previewCanvasSpotlightConfig(hidden);
                require(renderSpotlightWidget(canvas) == baseline,
                        "a transparent preview must remove the retained Spotlight backdrop");
                require(canvas.setCanvasSpotlightConfig(hidden), "commit transparent Spotlight");
                requireHidden();
                canvas.setCustomRenderer(nullptr);
            }
        }
    }
}

template <typename Mutation>
QRegion requireSpotlightDamageCoversChanges(SnowCanvasWidget& canvas, Mutation mutate,
                                            int presentationTolerance = 0) {
    QCoreApplication::processEvents();
    const auto before = renderSpotlightWidget(canvas);
    SpotlightDamageObserver observer;
    canvas.installEventFilter(&observer);
    mutate();
    QCoreApplication::processEvents();
    canvas.removeEventFilter(&observer);
    const auto after = renderSpotlightWidget(canvas);
    int changed = 0;
    for (int y = 0; y < before.height(); ++y) {
        for (int x = 0; x < before.width(); ++x) {
            if (before.pixel(x, y) != after.pixel(x, y)) {
                ++changed;
                require(observer.painted.contains(QPoint(x, y)),
                        "Spotlight damage must cover every changed filtered or resampled pixel");
            }
        }
    }
    require(changed > 0, "the Spotlight edit must change visible pixels");
    QImage partial = before;
    QPainter painter(&partial);
    canvas.render(&painter, observer.painted.boundingRect().topLeft(), observer.painted);
    painter.end();
    // Qt's clipped bilinear image spans can round a channel one level differently
    // from an unclipped span. Source-grid and unscaled comparisons remain exact.
    for (int y = 0; y < after.height(); ++y) {
        for (int x = 0; x < after.width(); ++x) {
            const QColor actual = partial.pixelColor(x, y);
            const QColor expected = after.pixelColor(x, y);
            require(std::abs(actual.red() - expected.red()) <= presentationTolerance &&
                        std::abs(actual.green() - expected.green()) <= presentationTolerance &&
                        std::abs(actual.blue() - expected.blue()) <= presentationTolerance &&
                        std::abs(actual.alpha() - expected.alpha()) <= presentationTolerance,
                    "scheduled Spotlight repaints must match a complete repaint");
        }
    }
    return observer.painted;
}

void spotlightDamagePropagatesThroughFilters() {
    for (bool retained : {false, true}) {
        for (double zoom : {1.0, 0.65, 1.4}) {
            SnowCanvasRuntime runtime;
            require(
                !runtime
                     .applyAnnotationTransaction(
                         R"({"version":1,"operations":[{"type":"spotlight","bounds":[250,180,10,10],"style":{"opacity":0.64,"corner_radius":0}},{"type":"rectangle_filter","bounds":[0,0,300,220],"style":{"filter":"gaussian_blur","strength":0.1}},{"type":"rectangle","bounds":[2,2,5,5],"style":{"fill":[0,0,255,255],"stroke_width":0}},{"type":"rectangle_filter","bounds":[0,0,300,220],"style":{"filter":"gaussian_blur","strength":0.1}}]})")
                     .isEmpty(),
                "create Spotlight and two dependent filter passes");
            SpotlightReferenceBackground background;
            background.retained = retained;
            SnowCanvasWidget canvas(runtime);
            canvas.resize(qRound(300 * zoom), qRound(220 * zoom));
            canvas.setCustomRenderer(&background);
            canvas.show();
            require(canvas.setViewportCamera(150, 110, zoom), "set damage test camera");
            const auto verifyDamage = [&](auto mutate) {
                return requireSpotlightDamageCoversChanges(canvas, mutate,
                                                           retained && zoom != 1.0 ? 1 : 0);
            };
            const auto damage = verifyDamage([&] {
                require(
                    !runtime
                         .applyAnnotationTransaction(
                             R"({"version":1,"operations":[{"type":"spotlight","bounds":[130,90,20,20],"style":{"opacity":0.64,"corner_radius":0}}]})")
                         .isEmpty(),
                    "add a cutout inside both filters");
            });
            require(damage != QRegion(canvas.rect()),
                    "local Spotlight edits must not repaint the whole viewport");
            verifyDamage([&] { require(runtime.undo(), "undo cutout creation"); });
            verifyDamage([&] { require(runtime.redo(), "redo cutout creation"); });
            canvas.setSpotlightRenderArea(QRectF(0, 0, 150, 220));
            verifyDamage([&] { canvas.setSpotlightRenderArea(QRectF(0, 0, 160, 220)); });
            canvas.setSpotlightRenderArea(QRectF(0, 0, 160.1, 220));
            verifyDamage([&] { canvas.setSpotlightRenderArea(QRectF(0, 0, 160.8, 220)); });
            verifyDamage([&] {
                auto config = canvas.canvasSpotlightConfig();
                config.opacity = 0.25;
                canvas.previewCanvasSpotlightConfig(config);
            });
            verifyDamage([&] {
                auto config = canvas.canvasSpotlightConfig();
                config.opacity = 0.4;
                require(canvas.setCanvasSpotlightConfig(config), "commit Spotlight style");
            });
            canvas.setCustomRenderer(nullptr);
        }
    }
}

void retainedSpotlightDamageIncludesCulledCutouts() {
    for (qreal sourceScale : {1.0, 2.0}) {
        SnowCanvasRuntime runtime;
        require(
            !runtime
                 .applyAnnotationTransaction(
                     R"({"version":1,"operations":[{"type":"spotlight","bounds":[250,180,10,10],"style":{"color":[0,0,0,255],"opacity":0.64,"corner_radius":0}},{"type":"rectangle_filter","bounds":[0,0,300,220],"style":{"filter":"gaussian_blur","strength":0.7}}]})")
                 .isEmpty(),
            "create a reference scene wider than the viewport");
        SpotlightReferenceBackground background;
        background.retained = true;
        background.sourceScale = sourceScale;
        SnowCanvasWidget canvas(runtime);
        canvas.resize(80, 80);
        canvas.setCustomRenderer(&background);
        canvas.show();
        require(canvas.setViewportCamera(150, 110, 1), "crop the live viewport");
        requireSpotlightDamageCoversChanges(
            canvas,
            [&] {
                require(
                    !runtime
                         .applyAnnotationTransaction(
                             R"({"version":1,"operations":[{"type":"spotlight","bounds":[85,100,10,10],"style":{"color":[0,0,0,255],"opacity":0.64,"corner_radius":0}}]})")
                         .isEmpty(),
                    "edit a cutout outside the live viewport but inside filter support");
            },
            sourceScale == 1.0 ? 0 : 1);
        canvas.setSpotlightRenderArea(QRectF(0, 0, 95, 220));
        // Cross the blur's downsample taps while keeping both edges left of the
        // visible canvas interval [110, 190). A narrower move can sample identical pixels.
        requireSpotlightDamageCoversChanges(
            canvas, [&] { canvas.setSpotlightRenderArea(QRectF(0, 0, 109, 220)); },
            sourceScale == 1.0 ? 0 : 1);
        canvas.setCustomRenderer(nullptr);
    }
}

void retainedSpotlightPreservesViewportCoverageAndReuse() {
    SnowCanvasRuntime runtime;
    require(
        !runtime
             .applyAnnotationTransaction(
                 R"({"version":1,"operations":[{"type":"spotlight","bounds":[130,90,20,20],"style":{"opacity":0.64,"corner_radius":0}},{"type":"rectangle_filter","bounds":[0,0,300,220],"style":{"filter":"inversion"}}]})")
             .isEmpty(),
        "create retained Spotlight coverage scene");
    SpotlightReferenceBackground background;
    SnowCanvasWidget canvas(runtime);
    canvas.resize(340, 260);
    canvas.setCustomRenderer(&background);
    canvas.show();
    require(canvas.setViewportCamera(150, 110, 1), "set coverage camera");
    for (bool clear : {true, false}) {
        canvas.setClearBackgroundEnabled(clear);
        background.retained = false;
        const auto direct = renderSpotlightWidget(canvas);
        background.retained = true;
        require(renderSpotlightWidget(canvas) == direct,
                "retained Spotlight must cover the viewport outside its reference surface");
    }
    for (bool configured : {false, true}) {
        if (configured)
            canvas.setSpotlightRenderArea(QRectF(-10.25, -5.5, 320.75, 240.25));
        renderSpotlightWidget(canvas);
        for (double zoom : {0.75, 1.25, 1.0}) {
            canvas.resize(qRound(340 * zoom), qRound(260 * zoom));
            require(canvas.setViewportCamera(140.25, 105.5, zoom), "pan and zoom Spotlight");
            renderSpotlightWidget(canvas);
            const auto diagnostics =
                snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread();
            require(diagnostics.referenceSceneBuildCount == 0 &&
                        diagnostics.referenceSceneHits == 1,
                    "camera and viewport changes must reuse the Spotlight reference pixels");
        }
    }
    canvas.setCustomRenderer(nullptr);
}

void spotlightTypesRenderDistinctHolesAndMixedOverlaps() {
    for (const auto shape : {SNOW_DISPLAY_RECT_SHAPE_RECTANGLE, SNOW_DISPLAY_RECT_SHAPE_ELLIPSE,
                             SNOW_DISPLAY_RECT_SHAPE_DIAMOND}) {
        SnowSpotlightCutout item = cutout(50.0, 50.0, 40.0, 40.0);
        item.shape = static_cast<std::uint8_t>(shape);
        const QImage image = render(&item, 1);
        require(image.pixelColor(50, 50) == QColor(Qt::white),
                "every spotlight type must reveal its interior");
        require((image.pixelColor(63, 63) == QColor(Qt::white)) ==
                    (shape != SNOW_DISPLAY_RECT_SHAPE_DIAMOND),
                "diamond corners must differ from rectangle and ellipse geometry");
        require((image.pixelColor(67, 67) == QColor(Qt::white)) ==
                    (shape == SNOW_DISPLAY_RECT_SHAPE_RECTANGLE),
                "ellipse and diamond must leave bounding-box corners masked");
        item.rotation = std::acos(-1.0) / 2.0;
        item.width = 60.0;
        item.height = 20.0;
        const QImage rotated = render(&item, 1);
        require(rotated.pixelColor(50, 70) == QColor(Qt::white) &&
                    isDefaultMaskPixel(rotated.pixel(70, 50)),
                "spotlight shape geometry must rotate with the element");
        SceneDisplayInfo zoomed = sceneInfo();
        zoomed.camera_zoom = 2.0;
        const QImage projected =
            render(&item, 1, QRectF(0, 0, 100, 100), QRegion(QRect(0, 0, 100, 100)), true, zoomed);
        require(projected.pixelColor(50, 90) == QColor(Qt::white),
                "spotlight paths must follow camera zoom");
        const QImage partial =
            render(&item, 1, QRectF(0, 0, 100, 100), QRegion(QRect(0, 0, 20, 20)));
        require(isDefaultMaskPixel(partial.pixel(5, 5)) &&
                    partial.pixelColor(90, 90) == QColor(Qt::white),
                "partial exposure must constrain work for every spotlight shape");
    }
    SnowSpotlightCutout items[] = {cutout(40, 50, 40, 40), cutout(60, 50, 40, 40)};
    items[0].shape = SNOW_DISPLAY_RECT_SHAPE_ELLIPSE;
    items[1].shape = SNOW_DISPLAY_RECT_SHAPE_DIAMOND;
    const QImage image = render(items, 2);
    require(image.pixelColor(50, 50) == QColor(Qt::white) &&
                image.pixelColor(60, 50) == QColor(Qt::white) &&
                isDefaultMaskPixel(image.pixel(75, 65)),
            "mixed spotlight holes must form a union without refilling overlaps");
}

void spotlightRoundedCornersMatchShapeGeometry() {
    SnowSpotlightCutout item = cutout(50, 50, 40, 40);
    item.corner_radii = SnowCornerRadii{16, 0, 0, 0};
    const QImage image = render(&item, 1);
    require(isDefaultMaskPixel(image.pixel(31, 31)) &&
                image.pixelColor(36, 36) == QColor(Qt::white) &&
                image.pixelColor(68, 31) == QColor(Qt::white),
            "spotlight rectangles must render independent rounded corners like shapes");
    item.rotation = std::acos(-1.0) / 2.0;
    const QImage rotated = render(&item, 1);
    require(isDefaultMaskPixel(rotated.pixel(68, 31)) &&
                rotated.pixelColor(31, 31) == QColor(Qt::white),
            "spotlight corner radii must rotate with the cutout");
    item.rotation = 0;
    SceneDisplayInfo zoomed = sceneInfo();
    zoomed.camera_zoom = 2;
    const QImage projected =
        render(&item, 1, QRectF(0, 0, 100, 100), QRegion(QRect(0, 0, 100, 100)), true, zoomed);
    require(isDefaultMaskPixel(projected.pixel(12, 12)) &&
                projected.pixelColor(24, 24) == QColor(Qt::white),
            "spotlight corner radii must scale with camera zoom");
    SnowSpotlightCutout overlap[] = {item, cutout(35, 35, 20, 20)};
    const QImage unionImage = render(overlap, 2);
    require(unionImage.pixelColor(31, 31) == QColor(Qt::white),
            "overlapping spotlight cutouts must reveal rounded corners covered by another hole");
    item.corner_radii = SnowCornerRadii{100, 100, 100, 100};
    const QImage clamped = render(&item, 1);
    require(isDefaultMaskPixel(clamped.pixel(31, 31)) &&
                clamped.pixelColor(50, 50) == QColor(Qt::white),
            "spotlight radii must clamp to fit small cutouts");
}

void spotlightTypeSurvivesDisplayPatchesAndExport() {
    for (const auto shape : {SNOW_RECTANGLE_SHAPE_RECTANGLE, SNOW_RECTANGLE_SHAPE_ELLIPSE,
                             SNOW_RECTANGLE_SHAPE_DIAMOND}) {
        SnowStyleDefaults defaults{};
        require(snow_runtime_style_defaults_default(&defaults) == SNOW_OK,
                "spotlight export fixture must load valid runtime defaults");
        defaults.spotlight_shape = shape;
        defaults.spotlight_corner_radii = SnowCornerRadii{16, 12, 8, 4};
        SnowRuntimeConfig runtimeConfig{&defaults};
        ScopedRuntimeHandle runtime;
        require(snow_runtime_create_with_config(&runtimeConfig, runtime.outParam()) == SNOW_OK,
                "spotlight export fixture must create a configured runtime");
        SnowCanvasViewport viewport;
        require(viewport.create(runtime.get(), snow_canvas_viewport::defaultEngineConfig()) &&
                    snow_viewport_set_surface_size(runtime.get(), viewport.get(), 100, 100) ==
                        SNOW_OK,
                "spotlight export fixture must create a viewport");
        ScopedChangedViewportList changed;
        require(snow_viewport_set_active_tool_ex(runtime.get(), viewport.get(),
                                                 SNOW_ACTIVE_TOOL_SPOTLIGHT,
                                                 changed.outParam()) == SNOW_OK,
                "spotlight export fixture must activate its tool");
        for (const auto type :
             {SNOW_POINTER_EVENT_DOWN, SNOW_POINTER_EVENT_MOVE, SNOW_POINTER_EVENT_UP}) {
            SnowInputEvent event{};
            event.kind = SNOW_INPUT_EVENT_POINTER;
            event.pointer.pointer_id = 1;
            event.pointer.event_type = type;
            event.pointer.device = SNOW_POINTER_DEVICE_MOUSE;
            event.pointer.position_x = type == SNOW_POINTER_EVENT_DOWN ? 30 : 70;
            event.pointer.position_y = type == SNOW_POINTER_EVENT_DOWN ? 30 : 70;
            event.pointer.button = type == SNOW_POINTER_EVENT_MOVE ? SNOW_POINTER_BUTTON_NONE
                                                                   : SNOW_POINTER_BUTTON_PRIMARY;
            event.pointer.buttons = type == SNOW_POINTER_EVENT_UP ? 0 : 1;
            SnowInteractionOutput output{};
            require(snow_viewport_process_input_ex(runtime.get(), viewport.get(), &event, &output,
                                                   changed.outParam()) == SNOW_OK,
                    "spotlight export fixture must create a cutout");
        }
        SnowCanvasDisplayCache cache;
        require(cache.sync(runtime.get(), viewport.get()) && cache.spotlightCutoutCount() == 1 &&
                    cache.spotlightCutouts()[0].shape == static_cast<std::uint8_t>(shape) &&
                    cache.spotlightCutouts()[0].corner_radii.top_left == 16,
                "dedicated spotlight display patches must retain the selected type and radii");
        const QImage expected = render(cache.spotlightCutouts(), 1, QRectF(0, 0, 100, 100),
                                       QRegion(QRect(0, 0, 100, 100)), true, cache.sceneInfo());
        QImage background(100, 100, QImage::Format_ARGB32_Premultiplied);
        background.fill(Qt::white);
        const QRectF canvasRect(-50, -50, 100, 100);
        const QImage exported =
            snow_canvas_export::renderToImage(runtime.get(), canvasRect, QSize(100, 100),
                                              {CanvasExportSource{background, canvasRect}});
        require(exported == expected,
                "spotlight exports must match the live cutout renderer for every type");
        for (const auto type : {SNOW_POINTER_EVENT_DOWN, SNOW_POINTER_EVENT_UP}) {
            SnowInputEvent event{};
            event.kind = SNOW_INPUT_EVENT_POINTER;
            event.pointer.pointer_id = 1;
            event.pointer.event_type = type;
            event.pointer.device = SNOW_POINTER_DEVICE_MOUSE;
            event.pointer.position_x = 50;
            event.pointer.position_y = 50;
            event.pointer.button = SNOW_POINTER_BUTTON_PRIMARY;
            event.pointer.buttons = type == SNOW_POINTER_EVENT_DOWN ? 1 : 0;
            SnowInteractionOutput output{};
            require(snow_viewport_process_input_ex(runtime.get(), viewport.get(), &event, &output,
                                                   changed.outParam()) == SNOW_OK,
                    "spotlight interior must support quick selection");
            require(cache.sync(runtime.get(), viewport.get()),
                    "spotlight selection must keep the display patch cursor current");
        }
        SnowStyleToolbarState toolbarState{};
        require(snow_viewport_get_style_toolbar_state(runtime.get(), viewport.get(),
                                                      &toolbarState) == SNOW_OK &&
                    toolbarState.source == SNOW_STYLE_TOOLBAR_SOURCE_SELECTED_SPOTLIGHT,
                "a spotlight shape edit must target the selected cutout");
        const auto sceneRevision = cache.patchCursor().scene_revision;
        SnowShapeStyle style{};
        style.shape = shape == SNOW_RECTANGLE_SHAPE_DIAMOND ? SNOW_RECTANGLE_SHAPE_ELLIPSE
                                                            : SNOW_RECTANGLE_SHAPE_DIAMOND;
        require(snow_viewport_set_shape_style_patch_ex(
                    runtime.get(), viewport.get(), &style, SNOW_SHAPE_STYLE_PROPERTY_SHAPE,
                    SNOW_SHAPE_KIND_SPOTLIGHT, changed.outParam()) == SNOW_OK &&
                    cache.sync(runtime.get(), viewport.get()),
                "spotlight type changes must propagate through display cache patches");
        require(cache.spotlightCutouts()[0].shape == static_cast<std::uint8_t>(style.shape),
                "spotlight type edits must change the selected cutout type");
        require(cache.patchCursor().scene_revision == sceneRevision &&
                    cache.sceneDirtyRectCount() == 0,
                "spotlight type edits must update decoration geometry without dirtying the "
                "retained scene");
    }
}

void overlappingAndRotatedCutoutsUseAPathUnion() {
    const SnowSpotlightCutout overlap[] = {
        cutout(40.0, 50.0, 30.0, 24.0),
        cutout(60.0, 50.0, 30.0, 24.0),
    };
    const QImage unionImage = render(overlap, 2);
    require(unionImage.pixelColor(50, 50) == QColor(Qt::white),
            "overlapping spotlight rectangles must form one transparent union");

    const SnowSpotlightCutout rotated = cutout(50.0, 50.0, 40.0, 20.0, std::acos(-1.0) / 4.0);
    const QImage rotatedImage = render(&rotated, 1);
    require(rotatedImage.pixelColor(50, 50) == QColor(Qt::white),
            "rotated spotlight must keep its center transparent");
    require(isDefaultMaskPixel(rotatedImage.pixel(68, 50)),
            "rotated spotlight must not use its axis-aligned bounding box as the hole");

    bool foundAntialiasedEdge = false;
    for (int y = 30; y <= 70 && !foundAntialiasedEdge; ++y) {
        for (int x = 30; x <= 70; ++x) {
            const int red = qRed(rotatedImage.pixel(x, y));
            if (red > 92 && red < 255) {
                foundAntialiasedEdge = true;
                break;
            }
        }
    }
    require(foundAntialiasedEdge, "rotated spotlight edge must be antialiased");
}

void renderAreaAndExposureLimitMaskWork() {
    const SnowSpotlightCutout item = cutout(50.0, 50.0, 16.0, 16.0);
    const QImage bounded = render(&item, 1, QRectF(20.0, 20.0, 60.0, 60.0));
    require(bounded.pixelColor(5, 5) == QColor(Qt::white),
            "pixels outside an explicit spotlight render area must remain untouched");
    require(isDefaultMaskPixel(bounded.pixel(25, 25)),
            "pixels inside the bounded render area must be masked");
    require(bounded.pixelColor(50, 50) == QColor(Qt::white),
            "cutouts must remain transparent inside a bounded render area");

    const QImage empty = render(&item, 1, QRectF());
    require(empty.pixelColor(25, 25) == QColor(Qt::white),
            "an explicitly empty render area must suppress the mask");

    QRegion exposed(QRect(0, 0, 20, 20));
    exposed += QRect(80, 80, 20, 20);
    const QImage clipped = render(nullptr, 0, QRectF(0, 0, 100, 100), exposed);
    require(isDefaultMaskPixel(clipped.pixel(5, 5)) && isDefaultMaskPixel(clipped.pixel(90, 90)),
            "every exposed region must receive the mask");
    require(clipped.pixelColor(50, 50) == QColor(Qt::white),
            "unexposed pixels must not be repainted by the mask pass");
}

void inactiveMaskLeavesCanvasUnchanged() {
    const SnowSpotlightCutout item = cutout(50.0, 50.0, 20.0, 20.0);
    const QImage image =
        render(&item, 1, QRectF(0.0, 0.0, 100.0, 100.0), QRegion(QRect(0, 0, 100, 100)), false);
    require(image.pixelColor(5, 5) == QColor(Qt::white),
            "removing the final spotlight cutout must remove the global mask");
}

void zeroAlphaAndNonintersectingExposureSkipGeometry() {
    const SnowSpotlightCutout item = cutout(50.0, 50.0, 30.0, 20.0);
    QImage image(100, 100, QImage::Format_ARGB32_Premultiplied);
    image.fill(Qt::white);
    QPainter painter(&image);
    snow_canvas_spotlight_renderer::resetDiagnosticsForCurrentThread();
    snow_canvas_spotlight_renderer::render(
        painter, sceneInfo(), SpotlightDisplayInfo{SnowColorRgba8{0, 0, 0, 0}, 1.0, true}, &item, 1,
        QRectF(0.0, 0.0, 100.0, 100.0), QRegion(QRect(0, 0, 100, 100)));
    snow_canvas_spotlight_renderer::render(
        painter, sceneInfo(), SpotlightDisplayInfo{SnowColorRgba8{0, 0, 0, 255}, 1.0, true}, &item,
        1, QRectF(0.0, 0.0, 20.0, 20.0), QRegion(QRect(80, 80, 20, 20)));
    painter.end();
    const auto diagnostics = snow_canvas_spotlight_renderer::diagnosticsForCurrentThread();
    require(diagnostics.earlyExitCount == 2,
            "transparent and nonintersecting spotlight paints must be diagnosed as early exits");
}

void displayCachePatchesSpotlightIndependentlyFromStyle() {
    ScopedRuntimeHandle runtime;
    require(snow_runtime_create(runtime.outParam()) == SNOW_OK,
            "spotlight runtime creation must succeed");
    SnowCanvasViewport viewport;
    SnowEngineConfig config = snow_canvas_viewport::defaultEngineConfig();
    require(viewport.create(runtime.get(), config), "spotlight viewport creation must succeed");
    require(snow_viewport_set_surface_size(runtime.get(), viewport.get(), 100, 100) == SNOW_OK,
            "spotlight surface setup must succeed");
    ScopedChangedViewportList toolChange;
    require(snow_viewport_set_active_tool_ex(runtime.get(), viewport.get(),
                                             SNOW_ACTIVE_TOOL_SPOTLIGHT,
                                             toolChange.outParam()) == SNOW_OK,
            "spotlight tool setup must succeed");

    SnowCanvasDisplayCache cache;
    require(cache.sync(runtime.get(), viewport.get()),
            "initial spotlight display cache sync must succeed");
    const std::uint64_t sceneRevision = cache.patchCursor().scene_revision;
    require(cache.executionPlan(1.0).buildCount == 1,
            "first paint must prepare the execution plan");
    require(cache.executionPlan(1.0).buildCount == 1,
            "unchanged paint must reuse the execution plan");

    const auto pointer = [&](SnowPointerEventType type, double x, double y, std::uint8_t buttons) {
        SnowInputEvent event{};
        event.kind = SNOW_INPUT_EVENT_POINTER;
        event.pointer.pointer_id = 1;
        event.pointer.event_type = type;
        event.pointer.device = SNOW_POINTER_DEVICE_MOUSE;
        event.pointer.position_x = x;
        event.pointer.position_y = y;
        event.pointer.button = type == SNOW_POINTER_EVENT_MOVE ? SNOW_POINTER_BUTTON_NONE
                                                               : SNOW_POINTER_BUTTON_PRIMARY;
        event.pointer.buttons = buttons;
        SnowInteractionOutput output{};
        ScopedChangedViewportList changedViewports;
        require(snow_viewport_process_input_ex(runtime.get(), viewport.get(), &event, &output,
                                               changedViewports.outParam()) == SNOW_OK,
                "spotlight pointer input must succeed");
    };
    pointer(SNOW_POINTER_EVENT_DOWN, 30.0, 30.0, 1);
    pointer(SNOW_POINTER_EVENT_MOVE, 70.0, 70.0, 1);
    require(cache.sync(runtime.get(), viewport.get()),
            "spotlight creation preview sync must succeed");
    require(cache.spotlightCutoutCount() == 1,
            "spotlight creation preview must populate dedicated cutout storage");
    require(cache.patchCursor().scene_revision == sceneRevision,
            "spotlight creation preview must not advance the scene revision");
    require(cache.sceneDirtyRectCount() == 0,
            "spotlight creation preview must not dirty retained scene content");
    require(!cache.spotlightDamage().isEmpty(),
            "cutout previews must expose backdrop damage to downstream filters");

    pointer(SNOW_POINTER_EVENT_UP, 70.0, 70.0, 0);
    require(cache.sync(runtime.get(), viewport.get()), "committed spotlight sync must succeed");
    SnowSpotlightConfig spotlight{};
    require(snow_viewport_get_spotlight_config(runtime.get(), viewport.get(), &spotlight) ==
                SNOW_OK,
            "spotlight config query must succeed");
    spotlight.opacity = 0.35;
    ScopedChangedViewportList spotlightChange;
    require(snow_viewport_set_spotlight_config_ex(runtime.get(), viewport.get(), &spotlight,
                                                  spotlightChange.outParam()) == SNOW_OK,
            "spotlight opacity update must succeed");
    require(cache.sync(runtime.get(), viewport.get()), "spotlight opacity sync must succeed");
    require(cache.patchCursor().scene_revision == sceneRevision,
            "spotlight color/opacity-only updates must preserve scene revision");
    require(!cache.spotlightDamage().isEmpty(),
            "Spotlight style changes must expose backdrop damage");
    SnowWatermarkConfig watermark{};
    require(snow_viewport_get_watermark_config(runtime.get(), viewport.get(), &watermark) ==
                SNOW_OK,
            "query watermark config");
    watermark.text_utf8[0] = 'W';
    watermark.text_utf8_len = 1;
    ScopedChangedViewportList watermarkChange;
    require(snow_viewport_set_watermark_config_ex(runtime.get(), viewport.get(), &watermark,
                                                  watermarkChange.outParam()) == SNOW_OK &&
                cache.sync(runtime.get(), viewport.get()),
            "sync a watermark-only edit");
    require(cache.decorationDirtyRectCount() != 0 && cache.spotlightDamage().isEmpty(),
            "watermark damage must not invalidate filter backdrops");
    require(cache.executionPlan(1.0).buildCount == 1,
            "decoration and editor changes must reuse the scene execution plan");
    require(cache.executionPlan(1.25).buildCount == 2,
            "DPR changes must refresh physical filter parameters");
    require(cache.executionPlan(1.25).buildCount == 2,
            "unchanged fractional DPR must reuse the plan");

    ScopedChangedViewportList filterToolChange;
    require(snow_viewport_set_active_tool_ex(runtime.get(), viewport.get(),
                                             SNOW_ACTIVE_TOOL_RECTANGLE_FILTER,
                                             filterToolChange.outParam()) == SNOW_OK,
            "filter tool setup must succeed");
    pointer(SNOW_POINTER_EVENT_DOWN, 20.0, 20.0, 1);
    pointer(SNOW_POINTER_EVENT_MOVE, 60.0, 60.0, 1);
    pointer(SNOW_POINTER_EVENT_UP, 60.0, 60.0, 0);
    require(cache.sync(runtime.get(), viewport.get()), "filter scene sync must succeed");
    const auto& retainedPlan = cache.executionPlan(1.25);
    require(retainedPlan.filters.size() == 1 && !retainedPlan.passes.empty(),
            "a filter scene must retain a nonempty execution plan");
    const auto originalPlan = retainedPlan;
    const SnowPatchCursor cursorBeforeClear = cache.patchCursor();
    const auto* sceneBeforeClear = cache.sceneItems();
    const auto sceneCountBeforeClear = cache.sceneItemCount();
    const auto* spotlightBeforeClear = cache.spotlightCutouts();
    const auto spotlightCountBeforeClear = cache.spotlightCutoutCount();
    const auto* renderPlanBeforeClear = cache.renderPlan().data();
    const auto renderPlanSizeBeforeClear = cache.renderPlan().size();

    cache.clearRenderState();
    require(retainedPlan.passes.capacity() == 0 && retainedPlan.filters.capacity() == 0 &&
                retainedPlan.filterForItem.capacity() == 0 &&
                retainedPlan.passForItem.capacity() == 0 &&
                retainedPlan.filterIndices.capacity() == 0,
            "clearing render state must release execution-plan allocations");
    require(cache.patchCursor().scene_revision == cursorBeforeClear.scene_revision &&
                cache.patchCursor().decoration_revision == cursorBeforeClear.decoration_revision &&
                cache.patchCursor().overlay_revision == cursorBeforeClear.overlay_revision,
            "clearing derived render state must preserve every patch cursor");
    require(cache.sceneItems() == sceneBeforeClear &&
                cache.sceneItemCount() == sceneCountBeforeClear &&
                cache.spotlightCutouts() == spotlightBeforeClear &&
                cache.spotlightCutoutCount() == spotlightCountBeforeClear &&
                cache.renderPlan().data() == renderPlanBeforeClear &&
                cache.renderPlan().size() == renderPlanSizeBeforeClear,
            "clearing derived render state must preserve current scene and decoration storage");

    const auto& rebuiltPlan = cache.executionPlan(1.25);
    require(rebuiltPlan.buildCount == 1 && rebuiltPlan.revision == originalPlan.revision &&
                rebuiltPlan.dpr == originalPlan.dpr &&
                rebuiltPlan.passForItem == originalPlan.passForItem &&
                rebuiltPlan.filterForItem == originalPlan.filterForItem &&
                rebuiltPlan.filterIndices == originalPlan.filterIndices &&
                rebuiltPlan.passes.size() == originalPlan.passes.size() &&
                rebuiltPlan.filters.size() == originalPlan.filters.size(),
            "the next paint must lazily rebuild equivalent scene execution mappings");
    const auto& originalFilter = originalPlan.filters.front();
    const auto& rebuiltFilter = rebuiltPlan.filters.front();
    require(rebuiltFilter.effective == originalFilter.effective &&
                rebuiltFilter.logicalSamplingRadius == originalFilter.logicalSamplingRadius &&
                rebuiltFilter.axisAlignedRect == originalFilter.axisAlignedRect &&
                rebuiltFilter.devicePixelAlignedRect == originalFilter.devicePixelAlignedRect &&
                rebuiltFilter.clipPath == originalFilter.clipPath &&
                rebuiltFilter.logicalBounds == originalFilter.logicalBounds,
            "rebuilding must preserve physical filter parameters and clipping geometry");
    require(cache.executionPlan(1.25).buildCount == 1,
            "the rebuilt plan must remain reusable for unchanged paints");
}

void unchangedRenderAreaDoesNotScheduleRepaint() {
    SnowCanvasWidget canvas;
    canvas.resize(1920, 1080);
    canvas.show();
    QApplication::processEvents();

    const QRectF renderArea(-800.0, -450.0, 1600.0, 900.0);
    canvas.setSpotlightRenderArea(renderArea);
    QApplication::processEvents();

    PaintObserver observer;
    canvas.installEventFilter(&observer);
    observer.begin();
    canvas.setSpotlightRenderArea(QRectF(renderArea.bottomRight(), renderArea.topLeft()));
    QApplication::processEvents();
    canvas.removeEventFilter(&observer);

    require(!observer.sawPaint(), "an unchanged spotlight render area must not schedule a repaint");
}

void spotlightPreviewCoalescesAndCommitCancelsQueuedValue() {
    SnowCanvasWidget canvas;
    int appliedCount = 0;
    QObject::connect(&canvas, &SnowCanvasWidget::spotlightPreviewApplied,
                     [&appliedCount]() { ++appliedCount; });

    SnowCanvasSpotlightConfig first;
    first.color = QColor(32, 96, 180, 255);
    first.opacity = 0.35;
    SnowCanvasSpotlightConfig latest = first;
    latest.color = QColor(210, 64, 48, 220);
    latest.opacity = 0.72;

    canvas.previewCanvasSpotlightConfig(first);
    require(appliedCount == 1, "the first spotlight preview should apply synchronously");
    canvas.previewCanvasSpotlightConfig(first);
    require(appliedCount == 1,
            "an unchanged spotlight preview should not emit another application");
    canvas.previewCanvasSpotlightConfig(latest);
    require(appliedCount == 1,
            "later spotlight preview writes should coalesce until the refresh callback");

    QElapsedTimer previewWait;
    previewWait.start();
    while (appliedCount < 2 && previewWait.elapsed() < 100) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 5);
        QThread::msleep(1);
    }
    require(appliedCount == 2,
            "the refresh-paced spotlight callback should deliver the latest value once");

    SnowCanvasWidget cancellationCanvas;
    int cancellationCount = 0;
    QObject::connect(&cancellationCanvas, &SnowCanvasWidget::spotlightPreviewApplied,
                     [&cancellationCount]() { ++cancellationCount; });
    cancellationCanvas.previewCanvasSpotlightConfig(first);
    SnowCanvasSpotlightConfig stale = first;
    stale.opacity = 0.91;
    SnowCanvasSpotlightConfig committed = latest;
    committed.color = QColor(24, 144, 88, 255);
    committed.opacity = 0.48;
    cancellationCanvas.previewCanvasSpotlightConfig(stale);
    require(cancellationCanvas.setCanvasSpotlightConfig(committed),
            "a spotlight commit should succeed while a preview is queued");
    QCoreApplication::processEvents();
    require(cancellationCount == 1,
            "a persistent spotlight commit should cancel its queued transient preview");
    require(cancellationCanvas.canvasSpotlightConfig() == committed,
            "the persistent spotlight commit should remain authoritative");
}

} // namespace

int main(int argc, char** argv) {
    QApplication application(argc, argv);
    snow_canvas_render_diagnostics::setEnabled(true);
    defaultMaskHasExactOpacityAndTransparentHole();
    spotlightFormsBackdropForSceneItems();
    spotlightEditsRefreshRetainedFilterSources();
    spotlightBackdropPreservesTransparency();
    invisibleSpotlightsPreserveFilterSources();
    runtimeSpotlightOrderMatchesWidgetAndExports();
    transparentSpotlightPreservesWidgetAndExports();
    spotlightDamagePropagatesThroughFilters();
    retainedSpotlightDamageIncludesCulledCutouts();
    retainedSpotlightPreservesViewportCoverageAndReuse();
    spotlightTypesRenderDistinctHolesAndMixedOverlaps();
    spotlightRoundedCornersMatchShapeGeometry();
    spotlightTypeSurvivesDisplayPatchesAndExport();
    overlappingAndRotatedCutoutsUseAPathUnion();
    renderAreaAndExposureLimitMaskWork();
    inactiveMaskLeavesCanvasUnchanged();
    zeroAlphaAndNonintersectingExposureSkipGeometry();
    displayCachePatchesSpotlightIndependentlyFromStyle();
    unchangedRenderAreaDoesNotScheduleRepaint();
    spotlightPreviewCoalescesAndCommitCancelsQueuedValue();
    return 0;
}
