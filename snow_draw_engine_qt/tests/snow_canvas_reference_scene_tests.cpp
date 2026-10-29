#include "snow_canvas_reference_scene_fixture.h"
#include "snow_canvas_ffi_handles.h"
#include "snow_canvas_runtime_access.h"
#include "snow_canvas_viewport.h"
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>

namespace {
QImage scaledReference(const Fixture& fixture, const QImage& reference, double dpr = 1.0,
                       const QRegion& exposed = {}) {
    QImage output(QSize(qCeil(fixture.canvas.width() * dpr), qCeil(fixture.canvas.height() * dpr)),
                  QImage::Format_ARGB32_Premultiplied);
    output.setDevicePixelRatio(dpr);
    output.fill(Qt::transparent);
    QPainter painter(&output);
    painter.setRenderHint(QPainter::Antialiasing, true);
    if (!exposed.isEmpty())
        painter.setClipRegion(exposed);
    const QRectF target =
        fixture.canvas.canvasToViewTransform().mapRect(fixture.renderer.reference.canvasRect);
    const QRectF deviceTarget = painter.deviceTransform().mapRect(target);
    const QSize deviceSize(qRound(deviceTarget.width()), qRound(deviceTarget.height()));
    painter.setRenderHint(QPainter::SmoothPixmapTransform, deviceSize != reference.size());
    painter.setCompositionMode(QPainter::CompositionMode_Source);
    painter.drawImage(target, reference);
    return output;
}

void requireReuse(const snow_canvas_renderer::FilterRenderDiagnostics& diagnostics) {
    require(diagnostics.referenceSceneHits == 1 && diagnostics.referenceSceneBuildCount == 0,
            "zoom must reuse one reference surface");
    require(diagnostics.effectDispatchCount == 0 && diagnostics.executionPlanBuildCount == 0 &&
                diagnostics.penRasterizedTileCount == 0 && diagnostics.sourceTileMisses == 0 &&
                diagnostics.sourceTileEvictions == 0 && diagnostics.totalWorkingPixelCount == 0,
            "zoom must do no filter, mask, tile-cache or execution-plan work");
}

void filtersScaleTheirReferencePixels() {
    for (const auto type : {SnowCanvasFilterType::Mosaic, SnowCanvasFilterType::GaussianBlur,
                            SnowCanvasFilterType::Grayscale, SnowCanvasFilterType::Inversion,
                            SnowCanvasFilterType::Emboss, SnowCanvasFilterType::Brightness}) {
        for (bool pen : {false, true}) {
            Fixture fixture;
            fixture.filter(type, pen);
            fixture.renderer.enabled = false;
            const QImage reference = fixture.render();
            fixture.renderer.enabled = true;
            // The first fast-path frame can arrive while the window is already reduced.
            fixture.zoom(0.5);
            require(fixture.render() == scaledReference(fixture, reference),
                    "first reduced frame must scale the 100% filter pixels");
            auto diagnostics = snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread();
            require(diagnostics.referenceSceneBuildCount == 1,
                    "first filtered frame builds exactly one reference surface");
            const int backgroundCalls = fixture.renderer.beforeCalls;
            for (const auto [zoom, dpr] :
                 {std::pair{1.0, 1.0}, {2.0, 1.0}, {0.75, 1.25}, {0.5, 2.0}, {1.4, 1.0}}) {
                fixture.zoom(zoom);
                const auto actual = fixture.render(dpr);
                const auto expected = scaledReference(fixture, reference, dpr);
                require(actual == expected,
                        "zoom and DPR changes must only resample the reference pixels");
                diagnostics = snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread();
                requireReuse(diagnostics);
                require(diagnostics.retainedReferenceSceneBytes == 160u * 120u * 4u,
                        "every scale retains only the same reference image");
            }
            require(fixture.renderer.beforeCalls == backgroundCalls,
                    "zoom must not replay the backdrop renderer");
            fixture.zoom(1.0, {95, 0});
            fixture.render();
            requireReuse(snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread());
            fixture.zoom(1.0);
            require(fixture.render() == reference, "panning must preserve culled filter sources");
            requireReuse(snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread());
        }
    }
}

void sourceOrderingAndRetinaReferenceStayStable() {
    Fixture fixture({320, 240}, {20.25, -10.5, 160, 120});
    fixture.filter(SnowCanvasFilterType::Inversion, false, 1.0);
    fixture.draw(SnowCanvasTool::Shape, {250, 30}, {290, 75});
    fixture.filter(SnowCanvasFilterType::GaussianBlur, true);
    fixture.filter(SnowCanvasFilterType::Mosaic);
    fixture.renderer.enabled = false;
    const auto reference = fixture.render();
    fixture.renderer.enabled = true;
    require(fixture.render() == reference,
            "reference rendering preserves explicit passes, annotations, pen masks and density");
    fixture.zoom(0.5);
    require(fixture.render(2.0) == scaledReference(fixture, reference, 2.0),
            "Retina density must stay fixed when the live DPR changes");
    requireReuse(snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread());
}

void contentChangesReplaceTheSurface() {
    Fixture fixture;
    fixture.filter(SnowCanvasFilterType::Inversion);
    const auto original = fixture.render();
    fixture.renderer.image.fill(QColor(90, 150, 210));
    ++fixture.renderer.revision;
    const auto replaced = fixture.render();
    require(replaced != original, "same-size background replacement updates filter pixels");
    require(
        snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread().referenceSceneBuildCount ==
            1,
        "background revision replaces the retained image");
    fixture.filter(SnowCanvasFilterType::Grayscale);
    const auto edited = fixture.render();
    require(edited != replaced, "filter edits replace the retained image");
    require(fixture.canvas.undo(), "undo filter edit");
    require(fixture.render() == replaced, "undo restores the reference pixels");
    require(fixture.canvas.redo(), "redo filter edit");
    require(fixture.render() == edited, "redo restores the edited reference pixels");
    fixture.canvas.clearRenderState();
    require(fixture.render() == edited, "cache clearing preserves reference pixels");
    require(
        snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread().referenceSceneBuildCount ==
            1,
        "explicit cache clearing rebuilds exactly once");
    const auto session = fixture.runtime.serializeDocumentSession();
    require(fixture.runtime.restoreDocumentSession(session), "replace runtime with saved session");
    fixture.zoom(1.0);
    require(fixture.render() == edited,
            "runtime replacement releases and rebuilds reference viewport");
    require(fixture.canvas.deleteAllElements(), "delete filters");
    require(fixture.render() == fixture.renderer.image,
            "filter-free scene returns to direct background rendering");
    require(snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread()
                    .retainedReferenceSceneBytes == 0,
            "removing the last filter releases the reference surface");
}

void partialPaintsPreserveAlphaAndUnexposedPixels() {
    Fixture fixture;
    {
        QPainter painter(&fixture.renderer.image);
        painter.setCompositionMode(QPainter::CompositionMode_Source);
        painter.fillRect(QRect(30, 20, 50, 40), Qt::transparent);
    }
    ++fixture.renderer.revision;
    fixture.filter(SnowCanvasFilterType::GaussianBlur, false, 0.5);
    fixture.renderer.enabled = false;
    const auto reference = fixture.render();
    fixture.renderer.enabled = true;
    require(fixture.render() == reference, "transparent filter backdrop matches 100% output");
    // QWidget::render places source-region bounds at targetOffset; use a region
    // starting at the origin to retain the same coordinates as normal paint events.
    const QRegion exposed = QRegion(QRect(0, 0, 40, 30)) | QRegion(QRect(70, 40, 12, 10));
    for (const double zoom : {0.75, 1.5}) {
        fixture.zoom(zoom);
        require(fixture.render(1.25, exposed) == scaledReference(fixture, reference, 1.25, exposed),
                "partial paints must scale reference alpha and leave unexposed pixels untouched");
        requireReuse(snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread());
    }
}

void changingTheReferenceGridRebuildsOnce() {
    Fixture fixture;
    fixture.filter(SnowCanvasFilterType::GaussianBlur);
    fixture.render();
    fixture.renderer.reference.pixelsPerCanvasUnit = 2;
    fixture.zoom(1.0);
    fixture.renderer.enabled = false;
    const auto expected = fixture.render();
    fixture.renderer.enabled = true;
    require(fixture.render() == expected,
            "a changed source density must rebuild filters on the new reference grid");
    require(
        snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread().referenceSceneBuildCount ==
            1,
        "new reference grid must build exactly once");
    fixture.zoom(0.5);
    require(fixture.render() == scaledReference(fixture, expected),
            "zoom must use the replacement reference grid");
    requireReuse(snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread());
}

void erasersRetainOriginalReferencePixelsAndExportThem() {
    for (const auto tool : {SnowCanvasTool::RectangleEraser, SnowCanvasTool::BrushEraser}) {
        Fixture fixture;
        {
            QPainter painter(&fixture.renderer.image);
            painter.setCompositionMode(QPainter::CompositionMode_Source);
            painter.fillRect(QRect(45, 30, 50, 45), Qt::transparent);
        }
        ++fixture.renderer.revision;
        fixture.filter(SnowCanvasFilterType::Inversion, false, 1.0);
        fixture.draw(tool, {30, 25}, {120, 95});
        fixture.renderer.enabled = false;
        const auto reference = fixture.render();
        fixture.renderer.enabled = true;
        require(fixture.render() == reference, "eraser reference must match source-grid rendering");
        for (const auto [zoom, dpr] : {std::pair{0.5, 2.0}, {0.75, 1.25}, {1.5, 1.0}}) {
            fixture.zoom(zoom);
            require(fixture.render(dpr) == scaledReference(fixture, reference, dpr),
                    "eraser zoom and DPR changes must resample the fixed original source pixels");
            requireReuse(snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread());
        }
        const QList<CanvasExportSource> sources{
            {fixture.renderer.image, fixture.renderer.reference.canvasRect}};
        const auto exported = fixture.runtime.renderToImage(fixture.renderer.reference.canvasRect,
                                                            fixture.renderer.image.size(), sources);
        require(exported == reference,
                "runtime export must preserve original eraser pixels and fractional canvas origin");
        const QRect crop(30, 20, 70, 60);
        const QRectF canvasCrop(fixture.renderer.reference.canvasRect.topLeft() + crop.topLeft(),
                                crop.size());
        require(fixture.runtime.renderToImage(canvasCrop, crop.size(), sources) ==
                    reference.copy(crop),
                "cropped export must preserve chronological restoration");
    }
}

void referenceSceneEditsReuseImmutableEraserMasks() {
    Fixture fixture;
    fixture.filter(SnowCanvasFilterType::Inversion, false, 1.0);
    fixture.draw(SnowCanvasTool::BrushEraser, {30, 25}, {120, 95});
    fixture.render();
    const int originalCalls = fixture.renderer.originalCalls;
    require(
        snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread().penRasterizedTileCount != 0,
        "first reference eraser builds its masks");
    // A normal edit preserves caches; the fixture's explicit editing reset clears them.
    fixture.draw(SnowCanvasTool::Shape, {3, 3}, {15, 15}, false);
    const auto edited = fixture.render();
    const auto diagnostics = snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread();
    require(diagnostics.referenceSceneBuildCount == 1 && diagnostics.penRasterizedTileCount == 0 &&
                diagnostics.referenceSceneUpdatedPixels < 160u * 120u / 2u &&
                fixture.renderer.originalCalls == originalCalls,
            "a small edit updates only damaged pixels and preserves immutable eraser backing");
    fixture.canvas.clearRenderState();
    require(fixture.render() == edited, "reference mask clearing must preserve restored pixels");
    require(
        snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread().penRasterizedTileCount !=
                0 &&
            snow_canvas_renderer::filterRenderDiagnosticsForCurrentThread().pristineTileMisses ==
                1 &&
            fixture.renderer.originalCalls == originalCalls + 1,
        "explicit clearing rebuilds the reference masks and pristine source");
}

void accumulatedEditsMatchFreshReferencePixels() {
    for (const auto type : {SnowCanvasFilterType::Mosaic, SnowCanvasFilterType::GaussianBlur,
                            SnowCanvasFilterType::Grayscale, SnowCanvasFilterType::Inversion,
                            SnowCanvasFilterType::Emboss, SnowCanvasFilterType::Brightness}) {
        Fixture fixture({320, 240}, {20.25, -10.5, 160, 120});
        fixture.filter(type);
        fixture.draw(SnowCanvasTool::BrushEraser, {180, 150}, {270, 190});
        fixture.render();
        // Several document patches can arrive before a paint event consumes the damage.
        fixture.draw(SnowCanvasTool::Shape, {10, 12}, {24, 28}, false);
        fixture.draw(SnowCanvasTool::Shape, {110, 100}, {130, 120}, false);
        fixture.draw(SnowCanvasTool::Shape, {290, 210}, {310, 230}, false);
        const auto incremental = fixture.render();
        fixture.canvas.clearRenderState();
        require(fixture.render() == incremental,
                "accumulated reference damage matches a fresh full-resolution render");
        require(fixture.canvas.undo(), "undo the last accumulated edit");
        const auto undone = fixture.render();
        fixture.canvas.clearRenderState();
        require(fixture.render() == undone, "undo damage restores exact reference pixels");
        require(fixture.canvas.redo(), "redo the last accumulated edit");
        require(fixture.render() == incremental, "redo damage restores exact reference pixels");
        fixture.zoom(0.75);
        require(fixture.render(1.25) == scaledReference(fixture, incremental, 1.25),
                "partial reference updates preserve fractional zoom and DPR sampling");
    }
}

void editsBeneathFiltersPreserveSamplingAtImageEdges() {
    Fixture fixture;
    const auto created = fixture.runtime.applyAnnotationTransaction(QByteArrayLiteral(
        R"({"version":1,"operations":[{"type":"rectangle","bounds":[14,10,5,20],"style":{"fill":[255,0,0,255]}},{"type":"rectangle","bounds":[30,25,10,10],"style":{"fill":[0,0,255,255]}}]})"));
    require(!created.isEmpty(), "create underlying annotations inside and beyond the source grid");
    const auto ids = QJsonDocument::fromJson(created)
                         .object()
                         .value(QStringLiteral("created_element_ids"))
                         .toArray();
    require(
        !fixture.runtime
             .applyAnnotationTransaction(QByteArrayLiteral(
                 R"({"version":1,"operations":[{"type":"rectangle_filter","bounds":[20.25,-10.5,160,120],"style":{"filter":"gaussian_blur","strength":0.7}}]})"))
             .isEmpty(),
        "create an edge-to-edge filter above the annotations");
    const auto before = fixture.render();
    const auto deletion =
        QJsonDocument(
            QJsonObject{{QStringLiteral("version"), 1},
                        {QStringLiteral("operations"),
                         QJsonArray{QJsonObject{{QStringLiteral("type"), QStringLiteral("delete")},
                                                {QStringLiteral("ids"), ids}}}}})
            .toJson(QJsonDocument::Compact);
    require(!fixture.runtime.applyAnnotationTransaction(deletion).isEmpty(),
            "remove underlying annotations without resetting the reference cache");
    const auto edited = fixture.render();
    require(edited != before, "removing underlying annotations changes the filtered pixels");
    fixture.canvas.clearRenderState();
    require(fixture.render() == edited,
            "partial updates include sampling dependencies from underneath edge filters");
}

void growingPenStrokesMatchFreshReferencePixels(
    SnowCanvasFilterRenderReference::Presentation presentation, double zoom, double dpr) {
    for (const auto type : {SnowCanvasFilterType::Mosaic, SnowCanvasFilterType::GaussianBlur,
                            SnowCanvasFilterType::Grayscale, SnowCanvasFilterType::Inversion,
                            SnowCanvasFilterType::Emboss, SnowCanvasFilterType::Brightness}) {
        Fixture fixture({320, 240}, {20.25, -10.5, 160, 120});
        fixture.renderer.reference.presentation = presentation;
        fixture.zoom(zoom);
        require(fixture.canvas.setCanvasFilterStyle({type, 0.65, 0.7, 22},
                                                    SnowCanvasFilterStylePropertyType |
                                                        SnowCanvasFilterStylePropertyStrength |
                                                        SnowCanvasFilterStylePropertyOpacity |
                                                        SnowCanvasFilterStylePropertyStrokeWidth),
                "configure growing pen filter");
        require(fixture.canvas.setCanvasTool(SnowCanvasTool::PenFilter), "activate pen filter");
        // Dispatch input directly so the test does not depend on the
        // widget's timer for coalescing pointer moves.
        const auto runtime = snow_canvas_runtime::Access::handle(fixture.runtime);
        SnowCanvasViewport inputViewport;
        const auto center = fixture.renderer.reference.canvasRect.center();
        ScopedChangedViewportList toolChanged;
        require(inputViewport.create(runtime, snow_canvas_viewport::defaultEngineConfig()) &&
                    snow_viewport_set_surface_size(runtime, inputViewport.get(), 320, 240) ==
                        SNOW_OK &&
                    snow_viewport_set_camera(runtime, inputViewport.get(), center.x(), center.y(),
                                             2.0) == SNOW_OK &&
                    snow_viewport_set_active_tool_ex(runtime, inputViewport.get(),
                                                     SNOW_ACTIVE_TOOL_PEN_FILTER,
                                                     toolChanged.outParam()) == SNOW_OK,
                "create pen input viewport");
        const auto pointer = [&](SnowPointerEventType eventType, QPointF position) {
            SnowInputEvent event{};
            event.kind = SNOW_INPUT_EVENT_POINTER;
            event.pointer.pointer_id = 1;
            event.pointer.event_type = eventType;
            event.pointer.device = SNOW_POINTER_DEVICE_MOUSE;
            event.pointer.position_x = position.x();
            event.pointer.position_y = position.y();
            event.pointer.button = eventType == SNOW_POINTER_EVENT_MOVE
                                       ? SNOW_POINTER_BUTTON_NONE
                                       : SNOW_POINTER_BUTTON_PRIMARY;
            event.pointer.buttons = eventType == SNOW_POINTER_EVENT_UP ? 0 : 1;
            SnowInteractionOutput output{};
            ScopedChangedViewportList changed;
            require(snow_viewport_process_input_ex(runtime, inputViewport.get(), &event, &output,
                                                   changed.outParam()) == SNOW_OK,
                    "process pen input");
            snow_canvas_runtime::Access::syncChangedViewports(fixture.runtime, changed.get());
        };
        pointer(SNOW_POINTER_EVENT_DOWN, {30, 35});
        fixture.render(dpr);
        QPointF previous(30, 35);
        int step = 0;
        for (const QPointF point : {QPointF(45, 40), QPointF(60, 50), QPointF(90, 70),
                                    QPointF(140, 110), QPointF(210, 170), QPointF(280, 200),
                                    QPointF(250, 150), QPointF(160, 140), QPointF(70, 80)}) {
            // Alternate one patch per paint with multiple patches coalesced before painting.
            if (++step % 2 == 0)
                pointer(SNOW_POINTER_EVENT_MOVE, (previous + point) / 2);
            pointer(SNOW_POINTER_EVENT_MOVE, point);
            const auto incremental = fixture.render(dpr);
            fixture.canvas.clearRenderState();
            require(fixture.render(dpr) == incremental,
                    "each growing pen-filter preview must match a fresh reference render");
            previous = point;
        }
        pointer(SNOW_POINTER_EVENT_UP, previous);
        const auto committed = fixture.render(dpr);
        fixture.canvas.clearRenderState();
        require(fixture.render(dpr) == committed,
                "committing a pen filter must retain every painted segment");
    }
}

} // namespace

int main(int argc, char** argv) {
    QApplication application(argc, argv);
    if (argc == 2 && QByteArray(argv[1]) == "--eraser-reuse-only") {
        referenceSceneEditsReuseImmutableEraserMasks();
        return 0;
    }
    filtersScaleTheirReferencePixels();
    sourceOrderingAndRetinaReferenceStayStable();
    contentChangesReplaceTheSurface();
    partialPaintsPreserveAlphaAndUnexposedPixels();
    changingTheReferenceGridRebuildsOnce();
    erasersRetainOriginalReferencePixelsAndExportThem();
    referenceSceneEditsReuseImmutableEraserMasks();
    accumulatedEditsMatchFreshReferencePixels();
    editsBeneathFiltersPreserveSamplingAtImageEdges();
    growingPenStrokesMatchFreshReferencePixels(
        SnowCanvasFilterRenderReference::Presentation::Replace, 0.75, 1.25);
    growingPenStrokesMatchFreshReferencePixels(
        SnowCanvasFilterRenderReference::Presentation::Composite, 1.5, 2.0);
    return 0;
}
