// SPDX-License-Identifier: GPL-3.0-or-later
#include "snow_shot/presentation/imageeditingdocument.h"
#include "snow_shot/presentation/imageeditingwindow.h"
#include "snow_shot/presentation/screenshotexportartifact.h"
#include "snow_shot/presentation/screenshotselectioncomposition.h"
#include "snow_shot/presentation/screenshotgeometry.h"
#include "snow_draw_engine_qt/snow_canvas_image.h"
#include "snow_draw_engine_qt/snow_canvas_widget.h"

#include <QApplication>
#include <QColorSpace>
#include <QEventLoop>
#include <QMouseEvent>
#include <QPainter>
#include <QScopeGuard>
#include <QTemporaryDir>
#include <QTimer>
#include <QUuid>
#include <stdexcept>

namespace {
using namespace snow_shot::presentation;
using namespace snow_shot::storage;

void require(bool condition, const char* message) {
    if (!condition)
        throw std::runtime_error(message);
}

void requirePixels(const QImage& actual, const QImage& expected, const char* message) {
    if (actual.isNull() || actual.size() != expected.size())
        throw std::runtime_error(QStringLiteral("%1: actual %2x%3, expected %4x%5")
                                     .arg(QString::fromUtf8(message))
                                     .arg(actual.width())
                                     .arg(actual.height())
                                     .arg(expected.width())
                                     .arg(expected.height())
                                     .toStdString());
    // PNG stores straight 8-bit RGBA. QColor's 16-bit unpremultiplication can differ after that
    // round trip even when every rendered premultiplied byte is identical. Compare those exact
    // rendered bytes, including alpha, without a visual tolerance.
    const auto actualPixels = actual.convertToFormat(QImage::Format_ARGB32_Premultiplied);
    const auto expectedPixels = expected.convertToFormat(QImage::Format_ARGB32_Premultiplied);
    for (int y = 0; y < expected.height(); ++y)
        for (int x = 0; x < expected.width(); ++x) {
            const QRgb a = actualPixels.pixel(x, y);
            const QRgb e = expectedPixels.pixel(x, y);
            if (a != e)
                throw std::runtime_error(
                    QStringLiteral("%1: pixel (%2,%3), source formats %4/%5, actual premultiplied "
                                   "ARGB %6, expected %7")
                        .arg(QString::fromUtf8(message))
                        .arg(x)
                        .arg(y)
                        .arg(actual.format())
                        .arg(expected.format())
                        .arg(QString::number(a, 16))
                        .arg(QString::number(e, 16))
                        .toStdString());
        }
}

QImage exportPixels(ScreenshotExportSource source) {
    ScreenshotExportArtifact artifact(std::move(source));
    QObject receiver;
    QEventLoop loop;
    QTimer timeout;
    timeout.setSingleShot(true);
    QObject::connect(&timeout, &QTimer::timeout, &loop, &QEventLoop::quit);
    bool completed = false;
    QImage output;
    require(artifact.requestImage(&receiver,
                                  [&](auto result) {
                                      completed = true;
                                      output = std::move(result.image);
                                      loop.quit();
                                  }),
            "history editor export starts");
    timeout.start(15000);
    if (!completed)
        loop.exec();
    require(completed && !output.isNull(), "history editor export completes");
    return output;
}

QImage exportSource(const ImageEditingSource& source) {
    ImageEditingDocument document;
    require(document.load(source), "history source loads into an independent editing document");
    const auto snapshot = document.snapshot();
    require(snapshot.error.isEmpty(), "history document prepares its export");
    return exportPixels(imageEditingExportSource(snapshot));
}

ImageEditingSource loadComposite(ImageEditingSource source) {
    auto composite = imageEditingCompositeSource(source);
    require(composite.isValid(), "styled pin prepares a faithful composite source");
    return completeImageEditingComposite(std::move(source), exportPixels(std::move(composite)));
}

CaptureHistoryDraft makeDraft(const SnowCanvasRuntime& runtime) {
    CaptureHistoryDraft draft;
    draft.id = QUuid::createUuid().toString(QUuid::WithoutBraces);
    draft.createdUtc = QDateTime::currentDateTimeUtc();
    draft.canvasBounds = QRect(-20, 10, 96, 64);
    draft.selection.rectangle = QRect(-10, 18, 64, 40);
    draft.selection.shadowColor = Qt::transparent;
    draft.canvasHistory = runtime.serializeDocumentHistory();
    CaptureHistoryDisplayDraft display;
    display.stableId = QStringLiteral("history-editor-source");
    display.name = QStringLiteral("History editor source");
    display.sourceCanvasRect = draft.canvasBounds;
    display.image = QImage(draft.canvasBounds.size(), QImage::Format_ARGB32_Premultiplied);
    display.image.setColorSpace(QColorSpace::SRgb);
    for (int y = 0; y < display.image.height(); ++y)
        for (int x = 0; x < display.image.width(); ++x)
            display.image.setPixelColor(x, y, QColor((x * 3) % 256, (y * 5) % 256, 71));
    draft.displays.append(std::move(display));
    return draft;
}

QImage composeHistoryResult(SnowCanvasRuntime& runtime, const CaptureHistoryDraft& draft) {
    const auto& selection = draft.selection;
    const auto pixels = selection.rectangle.size();
    const QList<CanvasExportSource> sources{
        {draft.displays.front().image, QRectF(draft.canvasBounds)}};
    ScreenshotResultStyle style{selection.cornerRadius, selection.shadowWidth,
                                selection.shadowColor};
    style.backgroundOpacityPercent = selection.backgroundOpacityPercent;
    if (selection.region)
        style.region = selection.region->translated(-selection.rectangle.topLeft());
    std::optional<ScreenshotLayoutCompactionPlan> plan;
    if (style.region && selection.layoutCompaction.enabled) {
        plan = makeScreenshotLayoutCompactionPlan(
            *style.region, pixels, 1.0, selection.layoutCompaction, selection.shadowWidth);
        require(plan->isValid() && plan->changed, "history fixture compacts separate regions");
        style = compactedScreenshotResultStyle(style, *plan);
    }
    if (style.region && runtime.hasDocumentContent())
        return renderAnnotatedScreenshotSelection(runtime, selection.rectangle, pixels, sources,
                                                  style, plan ? &*plan : nullptr);
    const auto content = runtime.renderToImage(selection.rectangle, pixels, sources);
    const auto background =
        renderScreenshotSelectionBackground(sources, selection.rectangle, pixels, style);
    return ScreenshotResultCompositor::compose(content, style, 1.0, 1.0, background);
}

void appearanceSurvivesHistoryEditing() {
    QTemporaryDir directory;
    require(directory.isValid(), "history appearance fixture directory");
    auto repository = makeCaptureHistoryRepository(directory.path());
    SnowCanvasRuntime runtime;
    require(
        !runtime
             .applyAnnotationTransaction(
                 R"({"version":1,"operations":[{"type":"rectangle","bounds":[-2,26,10,6],"style":{"fill":[255,0,0,255],"stroke":[0,0,0,0]}}]})")
             .isEmpty(),
        "history appearance fixture has an editable annotation");
    for (int variant = 0; variant < 7; ++variant) {
        auto draft = makeDraft(runtime);
        const QRect selection = draft.selection.rectangle;
        switch (variant) {
        case 0:
            draft.selection.region = ScreenshotRegionGeometry(QRegion(selection).subtracted(
                QRect(selection.topLeft() + QPoint(25, 12), QSize(12, 10))));
            break;
        case 1: {
            QPainterPath ellipse;
            ellipse.addEllipse(QRectF(selection));
            draft.selection.region =
                ScreenshotRegionGeometry::fromPath(ellipse, ScreenshotRegionType::Curve);
            break;
        }
        case 2:
            draft.selection.cornerRadius = 10;
            break;
        case 3:
            draft.selection.shadowWidth = 5;
            draft.selection.shadowColor = QColor(0, 0, 0, 180);
            break;
        case 4:
        case 5:
            draft.selection.region = ScreenshotRegionGeometry(
                QRegion(QRect(selection.topLeft(), QSize(20, selection.height()))) +
                QRect(selection.topLeft() + QPoint(44, 0), QSize(20, selection.height())));
            if (variant == 4)
                draft.selection.backgroundOpacityPercent = 50;
            else
                draft.selection.layoutCompaction = {true, 3};
            break;
        default:
            // These preferences alone do not change plain rectangular output.
            draft.selection.backgroundOpacityPercent = 50;
            draft.selection.layoutCompaction = {true, 3};
            draft.selection.lockAspectRatio = true;
            draft.selection.lockDragAspectRatio = true;
            break;
        }
        draft.resultImage = composeHistoryResult(runtime, draft);
        require(!draft.resultImage->isNull(), "history appearance fixture renders");
        const auto published = repository->publish(draft).get();
        require(published.storage.success, "history appearance fixture publishes");
        const auto stored = repository->loadResultImagePreservingRecord(published.record);
        require(stored.has_value(), "history appearance persists its composited pixels");
        requirePixels(*stored, *draft.resultImage,
                      "history PNG preserves the fixture's rendered appearance");
        const auto revision = repository->recordsSnapshot().revision;
        const auto imported = readImageEditingHistory(*repository, published.record);
        require(imported.error.isEmpty(), "history appearance imports successfully");
        if (variant == 6) {
            require(!imported.flattened && !imported.canvasHistory.isEmpty(),
                    "plain rectangle retains editable annotations and harmless preferences");
        } else {
            require(imported.flattened && imported.canvasHistory.isEmpty() &&
                        imported.canvasSession.isEmpty(),
                    "unsupported selection appearance explicitly opens its composited pixels");
            require(imported.image == *stored,
                    "history import preserves the stored decoded composite pixels exactly");
        }
        requirePixels(exportSource(imported), *stored,
                      "history editor export preserves selection appearance without duplicates");
        require(repository->recordsSnapshot().revision == revision,
                "history appearance import does not modify the source record");
    }

    SnowCanvasRuntime empty;
    auto decorated = makeDraft(empty);
    decorated.selection.shadowWidth = 4;
    decorated.selection.shadowColor = QColor(0, 0, 0, 180);
    decorated.resultImage = composeHistoryResult(empty, decorated);
    const auto decoratedRecord = repository->publish(decorated).get();
    require(decoratedRecord.storage.success, "unannotated decorated history fixture publishes");
    const auto storedDecoration =
        repository->loadResultImagePreservingRecord(decoratedRecord.record);
    require(storedDecoration.has_value(), "unannotated history persists its decorated image");
    requirePixels(*storedDecoration, *decorated.resultImage,
                  "history PNG preserves the unannotated fixture's rendered decoration");
    const auto importedDecoration = readImageEditingHistory(*repository, decoratedRecord.record);
    require(importedDecoration.flattened && importedDecoration.canvasHistory.isEmpty(),
            "unannotated history also explicitly imports its complete decorated image");
    require(importedDecoration.image == *storedDecoration,
            "unannotated import preserves the stored decoded decorated pixels exactly");
    requirePixels(exportSource(importedDecoration), *storedDecoration,
                  "unannotated history export preserves its decoration");

    auto unavailable = makeDraft(runtime);
    unavailable.selection.cornerRadius = 10;
    const auto published = repository->publish(std::move(unavailable)).get();
    require(published.storage.success, "history fixture without a composite publishes");
    const auto revision = repository->recordsSnapshot().revision;
    const auto failed = readImageEditingHistory(*repository, published.record);
    require(failed.image.isNull() && !failed.error.isEmpty() && failed.canvasHistory.isEmpty(),
            "missing appearance composite fails without exposing raw excluded pixels");
    require(repository->recordsSnapshot().revision == revision,
            "failed appearance import never removes the source record");
}

void historyResolutionUsesSelectedDisplays() {
    QTemporaryDir directory;
    require(directory.isValid(), "history resolution fixture directory");
    auto repository = makeCaptureHistoryRepository(directory.path());
    SnowCanvasRuntime runtime;
    for (int variant = 0; variant < 3; ++variant) {
        auto draft = makeDraft(runtime);
        draft.canvasBounds = QRect(0, 0, 200, 80);
        draft.selection.rectangle = variant == 0 ? QRect(0, 0, 60, 40) : QRect(80, 0, 80, 40);
        draft.displays.clear();
        for (int i = 0; i < 2; ++i) {
            CaptureHistoryDisplayDraft display;
            display.stableId = QString::number(i);
            display.name = display.stableId;
            display.sourceCanvasRect = QRect(i * 100, 0, 100, 80);
            display.canvasUsesPoints = true;
            display.backingScale = i == 0 ? 1.0 : variant == 2 ? 1.25 : 2.0;
            const QSize pixels = screenshotSelectionRenderedPixelSize(
                display.sourceCanvasRect->size(), display.backingScale);
            display.image = QImage(pixels, QImage::Format_ARGB32_Premultiplied);
            display.image.setColorSpace(QColorSpace::SRgb);
            display.image.fill(i == 0 ? Qt::red : Qt::blue);
            draft.displays.append(std::move(display));
        }
        // The exact recorded backing scale remains authoritative when source extents round up.
        if (variant == 2)
            draft.displays.back().image = draft.displays.back().image.scaled(QSize(126, 101));
        const auto published = repository->publish(draft).get();
        require(published.storage.success, "mixed-DPI history fixture publishes");
        const auto imported = readImageEditingHistory(*repository, published.record);
        const qreal expectedScale = variant == 0 ? 1.0 : variant == 1 ? 2.0 : 1.25;
        const QSize expected =
            screenshotSelectionRenderedPixelSize(draft.selection.rectangle.size(), expectedScale);
        require(imported.error.isEmpty() && !imported.flattened &&
                    imported.image.size() == expected &&
                    imported.canvasRect == QRectF(draft.selection.rectangle),
                "history import uses only selected display backing scales");
        require(exportSource(imported).size() == expected,
                "history editor export retains the selected display pixel resolution");
    }
}

void layeredImportsRetainInteriorCoverage() {
    QImage image(16, 12, QImage::Format_ARGB32_Premultiplied);
    image.fill(QColor(120, 60, 30, 170));
    const QRectF frame(-3.25, 2.5, 8, 6);
    const QRectF coverage(-1.75, 3.75, 4.5, 3.5);
    const QSize pixels(19, 15);
    const auto source = ScreenshotImageSource::fromLayers({{image, frame, coverage}});
    SnowCanvasRuntime runtime;
    const auto expected = runtime.renderToImage(frame, pixels, {{image, frame, coverage}});
    const auto actual = renderImageEditingBaseImage(source, frame, pixels);
    requirePixels(actual, expected, "editor import retains layer exclusions inside its output");
    require(actual.pixelColor(0, 0).alpha() == 0 &&
                actual.pixelColor(pixels.width() / 2, pixels.height() / 2).alpha() > 0,
            "interior layer coverage exposes only its included pixels");
}

void editableImportsPreserveResampledExportPixels() {
    QTemporaryDir directory;
    require(directory.isValid(), "resampled history fixture directory");
    auto repository = makeCaptureHistoryRepository(directory.path());
    SnowCanvasRuntime background;
    SnowCanvasRuntime annotations;
    require(
        !annotations
             .applyAnnotationTransaction(
                 R"({"version":1,"operations":[{"type":"rectangle","bounds":[-4,10,3,2],"style":{"fill":[250,30,70,160],"stroke":[0,0,0,0]}}]})")
             .isEmpty(),
        "resampled import fixture has an editable annotation");
    for (const qreal scale : {2.0, 1.25, 1.0}) {
        auto draft = makeDraft(annotations);
        draft.canvasBounds = QRect(-11, 5, 26, 14);
        draft.selection.rectangle = QRect(-7, 8, 16, 9);
        draft.cursorAvailable = draft.cursorVisible = true;
        draft.displays.clear();
        QList<CanvasExportSource> donors;
        QList<ScreenshotImageLayer> layers;
        for (int index = 0; index < 2; ++index) {
            CaptureHistoryDisplayDraft display;
            display.stableId = QString::number(index);
            display.name = display.stableId;
            display.sourceCanvasRect = QRect(-11 + index * 13, 5, 13, 14);
            display.canvasUsesPoints = true;
            display.backingScale = index == 0 ? 1.0 : scale;
            const auto size = screenshotSelectionRenderedPixelSize(display.sourceCanvasRect->size(),
                                                                   display.backingScale);
            display.image = QImage(size, QImage::Format_ARGB32);
            display.image.setColorSpace(QColorSpace::SRgb);
            for (int y = 0; y < size.height(); ++y)
                for (int x = 0; x < size.width(); ++x)
                    display.image.setPixelColor(
                        x, y,
                        QColor((x * 15 + y * 30) % 255, (x * 45 + index * 90) % 255,
                               (y * 60 + index * 30) % 255, (x + y) % 3 ? 255 : 85));
            const QRectF frame(*display.sourceCanvasRect);
            donors.append({display.image, frame, {}});
            layers.append({display.image, frame, frame.intersected(draft.selection.rectangle)});
            if (index == 0) {
                // A partly clipped cursor exercises source sampling at the selection edges.
                display.cursorPixelRect = QRect(2, 1, 6, 6);
                display.cursorPatch = QImage(6, 6, QImage::Format_ARGB32);
                for (int y = 0; y < 6; ++y)
                    for (int x = 0; x < 6; ++x)
                        display.cursorPatch.setPixelColor(
                            x, y, QColor(255, x * 45, y * 45, (x + y) % 2 ? 255 : 85));
                const QRectF cursor(frame.topLeft() + QPointF(2, 1), QSizeF(6, 6));
                donors.append({display.cursorPatch, cursor, {}});
                layers.append({display.cursorPatch, cursor,
                               cursor.intersected(draft.selection.rectangle), false});
            }
            draft.displays.append(std::move(display));
        }
        const auto bounds = QRectF(draft.selection.rectangle);
        const auto pixels =
            screenshotSelectionRenderedPixelSize(draft.selection.rectangle.size(), scale);
        const auto expectedBase = background.renderToImage(bounds, pixels, donors);
        const auto expected = annotations.renderToImage(bounds, pixels, donors);
        draft.resultImage = expected;
        const auto published = repository->publish(std::move(draft)).get();
        require(published.storage.success, "resampled history fixture publishes");
        const auto imported = readImageEditingHistory(*repository, published.record);
        require(imported.error.isEmpty() && !imported.flattened,
                "resampled history remains editable");
        requirePixels(imported.image, expectedBase,
                      "history import preserves export projection interpolation and cursor alpha");
        require(imported.image.colorSpace() == QColorSpace(QColorSpace::SRgb),
                "resampled history retains canonical capture color metadata");
        requirePixels(exportSource(imported), expected,
                      "untouched mixed-DPI history exports the original annotated pixels");
        ImageEditingDocument document;
        require(document.load(imported) && document.undo(),
                "resampled history retains editable annotation undo");
        requirePixels(exportSource(document.snapshot()), expectedBase,
                      "undo removes imported annotations without leaving baked pixels");

        ImageEditingSource layered;
        layered.layeredImage = ScreenshotImageSource::fromLayers(std::move(layers));
        layered.canvasRect = bounds;
        layered.pixelSize = pixels;
        layered.canvasSession = annotations.serializeDocumentSession();
        ImageEditingWindow window;
        QEventLoop loop;
        QTimer timeout;
        timeout.setSingleShot(true);
        QObject::connect(&timeout, &QTimer::timeout, &loop, &QEventLoop::quit);
        bool loaded = false;
        QObject::connect(&window, &ImageEditingWindow::loaded, &loop, [&](bool success) {
            loaded = success;
            loop.quit();
        });
        window.present(std::move(layered));
        timeout.start(15000);
        if (!loaded)
            loop.exec();
        require(loaded, "layered pin source finishes independent editor preparation");
        requirePixels(window.document()->image(), expectedBase,
                      "layered editor imports use the same projection and sampling as history");
        requirePixels(
            exportSource(window.document()->snapshot()), expected,
            "layered editor imports retain annotations without baking or resampling twice");
        window.shutdown();
    }
}

void historyMagnifiersRetainExteriorSamplingPixels() {
    ImageEditingSource source;
    source.image = QImage(200, 140, QImage::Format_ARGB32_Premultiplied);
    source.image.fill(Qt::blue);
    {
        QPainter painter(&source.image);
        painter.fillRect(QRect(0, 0, 100, 140), Qt::green);
    }
    source.canvasRect = QRectF(0, 0, 200, 140);
    ImageEditingDocument original;
    require(original.load(source), "magnifier history fixture opens");
    SnowCanvasWidget canvas(original.runtime());
    canvas.resize(200, 140);
    canvas.show();
    QApplication::processEvents();
    require(canvas.setViewportCamera(100, 70, 1) && canvas.setCanvasTool(SnowCanvasTool::Magnifier),
            "magnifier history fixture has an unscaled creation camera");
    const auto mouse = [&](QEvent::Type type, QPointF point, Qt::MouseButton button,
                           Qt::MouseButtons buttons) {
        QMouseEvent event(type, point, point, canvas.mapToGlobal(point), button, buttons,
                          Qt::NoModifier);
        QApplication::sendEvent(&canvas, &event);
    };
    const auto drag = [&](QPointF from, QPointF to) {
        mouse(QEvent::MouseButtonPress, from, Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseMove, to, Qt::NoButton, Qt::LeftButton);
        mouse(QEvent::MouseButtonRelease, to, Qt::LeftButton, Qt::NoButton);
    };
    drag({20, 50}, {40, 70});
    drag({30, 60}, {30, 60});
    drag({30, 104}, {160, 104});
    require(canvas.resetEditingStatePreservingTool(), "magnifier annotation is committed");

    QTemporaryDir directory;
    auto repository = makeCaptureHistoryRepository(directory.path());
    const QRect selection(100, 0, 100, 140);
    for (int variant = 0; variant < 3; ++variant) {
        CaptureHistoryDraft draft;
        draft.id = QUuid::createUuid().toString(QUuid::WithoutBraces);
        draft.createdUtc = QDateTime::currentDateTimeUtc();
        draft.canvasBounds = source.canvasRect.toRect();
        draft.selection.rectangle = selection;
        draft.selection.shadowColor = Qt::transparent;
        draft.canvasHistory = original.runtime().serializeDocumentHistory();
        QList<CanvasExportSource> donors;
        const bool separateDisplays = variant != 0;
        for (int index = 0; index < (separateDisplays ? 2 : 1); ++index) {
            CaptureHistoryDisplayDraft display;
            display.stableId = QString::number(index);
            display.name = display.stableId;
            display.sourceCanvasRect =
                separateDisplays ? QRect(index * 100, 0, 100, 140) : draft.canvasBounds;
            display.canvasUsesPoints = variant == 2;
            display.backingScale = index == 0 && variant == 2 ? 2.0 : 1.0;
            display.image = source.image.copy(*display.sourceCanvasRect);
            if (display.backingScale != 1)
                display.image = display.image.scaled(display.image.size() * display.backingScale);
            donors.append({display.image, QRectF(*display.sourceCanvasRect), {}});
            if (variant == 2 && index == 0) {
                display.cursorPixelRect = QRect(50, 110, 20, 20);
                display.cursorPatch = QImage(20, 20, QImage::Format_ARGB32_Premultiplied);
                display.cursorPatch.fill(Qt::yellow);
                donors.append({display.cursorPatch, QRectF(25, 55, 10, 10), {}});
                draft.cursorVisible = draft.cursorAvailable = true;
            }
            draft.displays.append(std::move(display));
        }
        const auto expected = original.runtime().renderToImage(selection, selection.size(), donors);
        require(expected.pixelColor(60, 60) == QColor(variant == 2 ? Qt::yellow : Qt::green),
                "the original magnifier samples an exterior display and its captured cursor");
        draft.resultImage = expected;
        const auto published = repository->publish(std::move(draft)).get();
        require(published.storage.success, "magnifier history publishes its complete donors");
        const auto imported = readImageEditingHistory(*repository, published.record);
        require(imported.error.isEmpty() && !imported.flattened &&
                    imported.image.size() == selection.size(),
                "exterior donors preserve editability without changing selected display DPI");
        requirePixels(exportSource(imported), expected,
                      "history export retains exterior magnifier sampling pixels");
        ImageEditingDocument document;
        require(document.load(imported), "magnifier working copy opens");
        require(document.crop(ImageEditingDocument::CropAxis::Rows, 120, 130) && document.undo(),
                "exterior donors survive mixed raster undo");
        requirePixels(exportPixels(imageEditingExportSource(document.snapshot())), expected,
                      "export snapshots retain exterior donors across crop and undo");
    }
}

void pinnedAppearanceSurvivesEditing() {
    SnowCanvasRuntime runtime;
    require(
        !runtime
             .applyAnnotationTransaction(
                 R"({"version":1,"operations":[{"type":"rectangle","bounds":[8,9,8,6],"style":{"fill":[255,0,0,255],"stroke":[0,0,0,0]}}]})")
             .isEmpty(),
        "styled pin fixture has an annotation");
    PinnedWindowRecord record;
    record.image = QImage(96, 64, QImage::Format_ARGB32_Premultiplied);
    record.image.fill(QColor(20, 80, 160));
    record.contentCanvasRect = QRectF(0, 0, 48, 32);
    record.canvasSession = runtime.serializeDocumentSession();
    record.originalFileName = QStringLiteral("styled-pin.png");
    const auto preparation = captureScreenshotPinnedTextPreparation();
    for (int variant = 0; variant < 4; ++variant) {
        ScreenshotResultStyle style;
        if (variant == 0)
            style.cornerRadius = 5;
        else if (variant == 1) {
            style.shadowWidth = 3;
            style.shadowColor = QColor(0, 0, 0, 180);
        } else {
            // Persisted and live styles use canvas units, even when the image
            // has two source pixels per canvas unit.
            style.region = QRegion(0, 0, 48, 32).subtracted(QRect(16, 10, 9, 8));
            if (variant == 3) {
                style.borderType = ScreenshotSelectionBorderType::Border;
                style.borderWidth = 3;
                style.shadowWidth = 11;
            }
        }
        record.resultStyle = encodeScreenshotResultStyle(style);
        const auto imported = readImageEditingPinned(record, preparation);
        require(imported.error.isEmpty() && imported.resultStyle.has_value(),
                "closed styled pin retains its pending appearance");
        auto expectedStyle = style;
        expectedStyle.cornerRadius *= 2;
        expectedStyle.shadowWidth *= 2;
        expectedStyle.borderWidth *= 2;
        if (style.region)
            expectedStyle.region = QRegion(0, 0, 96, 64).subtracted(QRect(32, 20, 18, 16));
        const auto content = runtime.renderToImage(record.contentCanvasRect, record.image.size(),
                                                   {{record.image, record.contentCanvasRect}});
        const auto expected = ScreenshotResultCompositor::compose(content, expectedStyle);
        const auto flattened = loadComposite(imported);
        require(flattened.flattened && flattened.canvasSession.isEmpty() &&
                    flattened.canvasHistory.isEmpty() && !flattened.resultStyle &&
                    flattened.recognition.isEmpty() &&
                    flattened.originalFileName == record.originalFileName,
                "styled pin composite discards baked editable state and retains its filename");
        requirePixels(flattened.image, expected, "closed pin retains its mask and effect pixels");
        requirePixels(exportSource(flattened), expected,
                      "styled pin editor export does not duplicate annotations or effects");
        if (variant >= 2) {
            const int padding = expectedStyle.effectWidth();
            require(flattened.image.pixelColor(40 + padding, 28 + padding).alpha() == 0,
                    "excluded pin pixels remain excluded when opening the editor");
        }
    }
    record.resultStyle.clear();
    const auto plain = readImageEditingPinned(record, preparation);
    require(!plain.resultStyle && !imageEditingCompositeSource(plain).isValid() &&
                !plain.canvasSession.isEmpty(),
            "plain pin annotations remain editable without a composite stage");
    record.resultStyle = QByteArrayLiteral("invalid style");
    const auto invalid = readImageEditingPinned(record, preparation);
    require(invalid.image.isNull() && !invalid.error.isEmpty(),
            "invalid pin appearance never falls back to raw donor pixels");
}

void pinnedTextUsesCapturedRenderingInputs() {
    const auto previousPalette = QApplication::palette();
    const auto restore = qScopeGuard([&] { QApplication::setPalette(previousPalette); });
    auto themed = previousPalette;
    const QColor background(25, 50, 74);
    themed.setColor(QPalette::Base, background);
    QApplication::setPalette(themed);
    const auto preparation = captureScreenshotPinnedTextPreparation();
    QApplication::setPalette(previousPalette);
    for (const bool html : {false, true}) {
        PinnedWindowRecord record;
        record.sourceKind = PinnedWindowSourceKind::ClipboardText;
        (html ? record.originalHtml : record.originalText) =
            html ? QStringLiteral("<p><b>Legacy</b> text</p>") : QStringLiteral("Legacy text");
        record.firstCreationTextDpi = 1.5;
        const auto imported = readImageEditingPinned(record, preparation);
        require(imported.error.isEmpty() && imported.image.pixelColor(0, 0) == background,
                "legacy text uses the GUI-captured theme even after that theme changes");
    }
    PinnedWindowRecord swatch;
    swatch.sourceKind = PinnedWindowSourceKind::ClipboardText;
    swatch.originalText = QStringLiteral("#336699");
    swatch.textStyle = PinnedTextStyle{};
    swatch.textStyle->colorSwatch = true;
    swatch.textStyle->backgroundColor = Qt::yellow;
    swatch.textStyle->textColor = Qt::red;
    swatch.textStyle->fontFamily = preparation.colorSwatchStyle.font.family();
    swatch.textStyle->fontSize = 16;
    swatch.textStyle->colorSwatchSize = QSize(342, 210);
    swatch.textStyle->maximumWidth = 342;
    swatch.firstCreationTextDpi = 1.5;
    auto expectedStyle = preparation.colorSwatchStyle;
    expectedStyle.font.setFamily(swatch.textStyle->fontFamily);
    const auto values = parsePinnedColorSwatch(swatch.originalText);
    require(values.has_value(), "color swatch fixture parses");
    const auto expected = renderPinnedColorSwatch(*values, expectedStyle, 1.5);
    const auto imported = readImageEditingPinned(swatch, preparation);
    require(imported.error.isEmpty(), "closed color swatch renders with captured theme inputs");
    requirePixels(imported.image,
                  snowCanvasTransformImage(expected.image, {}, Qt::SmoothTransformation),
                  "closed swatch uses the same current theme as normal pin restoration");
}
} // namespace

void runImageEditingSourceTests() {
    appearanceSurvivesHistoryEditing();
    historyResolutionUsesSelectedDisplays();
    layeredImportsRetainInteriorCoverage();
    editableImportsPreserveResampledExportPixels();
    historyMagnifiersRetainExteriorSamplingPixels();
    pinnedAppearanceSurvivesEditing();
    pinnedTextUsesCapturedRenderingInputs();
}
