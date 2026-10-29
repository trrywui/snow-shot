// SPDX-License-Identifier: GPL-3.0-or-later
#include "snow_shot/presentation/imageeditingdocument.h"
#include "snow_shot/presentation/screenshotexportartifact.h"
#include <QApplication>
#include <QElapsedTimer>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QThread>
#include <cmath>
#include <stdexcept>

namespace {
using namespace snow_shot::presentation;
void require(bool condition, const char* message) {
    if (!condition)
        throw std::runtime_error(message);
}
ImageEditingSource source() {
    ImageEditingSource result;
    result.image = QImage(32, 24, QImage::Format_ARGB32_Premultiplied);
    result.image.fill(QColor(21, 72, 124, 160));
    result.canvasRect = QRectF(-8, 12, 32, 24);
    return result;
}
void rejectedReplacementPreservesDocument() {
    ImageEditingDocument document;
    const auto original = source();
    require(document.load(original), "atomic-load fixture opens");
    require(!document.runtime()
                 .applyAnnotationTransaction(QByteArrayLiteral(
                     R"({"version":1,"operations":[{"type":"rectangle","bounds":[0,16,8,4]}]})"))
                 .isEmpty(),
            "atomic-load fixture has annotations");
    require(document.crop(ImageEditingDocument::CropAxis::Rows, 2, 4),
            "atomic-load fixture has raster history");
    const auto checkpoint = document.checkpoint();
    const auto image = document.image();
    const auto history = document.runtime().serializeDocumentSession();
    for (const bool session : {false, true}) {
        auto invalid = original;
        if (session)
            invalid.canvasSession = QByteArrayLiteral("invalid session");
        else
            invalid.canvasHistory = QByteArrayLiteral("invalid history");
        require(!document.load(invalid), "malformed replacement is rejected");
        require(document.image() == image && document.checkpoint() == checkpoint &&
                    document.runtime().serializeDocumentSession() == history,
                "failed replacement preserves raster annotations and mixed undo history");
    }
    require(document.undo() && document.undo() && document.image() == original.image &&
                !document.dirty(),
            "mixed history still works after a rejected replacement");
}
void historicalEditsDoNotLimitContentOperations() {
    ImageEditingDocument document;
    const auto original = source();
    require(document.load(original), "history-pressure fixture opens");
    const auto checkpoint = document.checkpoint();
    QJsonArray points;
    for (int i = 0; i < 8192; ++i)
        points.append(QJsonArray{-6.0 + double(i) / 8192 * 28.0, 23.0 + std::sin(i * 0.07) * 10.0});
    const auto transaction =
        QJsonDocument(QJsonObject{{QStringLiteral("version"), 1},
                                  {QStringLiteral("operations"),
                                   QJsonArray{QJsonObject{
                                       {QStringLiteral("type"), QStringLiteral("freehand")},
                                       {QStringLiteral("points"), points}}}}})
            .toJson(QJsonDocument::Compact);
    for (int round = 0; round < 24; ++round) {
        require(!document.runtime().applyAnnotationTransaction(transaction).isEmpty(),
                "large freehand stroke is created");
        SnowCanvasRuntimeEditor editor(document.runtime());
        require(editor.deleteAllElements(), "large freehand stroke is deleted");
    }
    require(document.runtime().serializeDocumentSession().isEmpty(),
            "fixture exceeds the persisted full-session size limit");
    require(!document.runtime().hasDocumentContent() && document.runtime().canUndo() &&
                document.image() == original.image && document.checkpoint() == checkpoint &&
                !document.dirty(),
            "historical edits do not change current pixels or the saved checkpoint");
    const auto snapshot = document.snapshot();
    require(snapshot.error.isEmpty() && !snapshot.canvasSession.isEmpty(),
            "current-content export snapshot remains available beyond the history limit");
    SnowCanvasRuntime restored;
    require(restored.restoreDocumentSession(snapshot.canvasSession) && !restored.canUndo() &&
                !restored.canRedo() && !restored.hasDocumentContent(),
            "render snapshot restores current content without historical edits");
    auto artifact = std::make_shared<ScreenshotExportArtifact>(imageEditingExportSource(snapshot));
    bool completed = false;
    ScreenshotExportImageResult result;
    require(artifact->requestImage(&document,
                                   [&](auto value) {
                                       result = std::move(value);
                                       completed = true;
                                   }),
            "history-pressure image export starts");
    QElapsedTimer timer;
    timer.start();
    while (!completed && timer.elapsed() < 15000) {
        QApplication::processEvents();
        QThread::msleep(1);
    }
    require(completed && result.succeeded() &&
                result.image.convertToFormat(QImage::Format_ARGB32) ==
                    original.image.convertToFormat(QImage::Format_ARGB32),
            "history-pressure export preserves full-resolution base pixels and alpha");
    require(document.undo() && document.dirty() && document.redo() && !document.dirty(),
            "creating render snapshots preserves the editor's undo and redo history");
}

void samplingContextDoesNotRevealOriginalPixelsAfterCrop() {
    auto imported = source();
    imported.image.fill(Qt::transparent);
    QImage donor(64, 48, QImage::Format_ARGB32_Premultiplied);
    donor.fill(Qt::red);
    imported.samplingSources = {{donor, QRectF(-16, 0, 64, 48), {}}};
    ImageEditingDocument document;
    require(document.load(imported), "transparent document imports exterior sampling context");
    require(document.crop(ImageEditingDocument::CropAxis::Rows, 4, 8),
            "transparent sampling fixture crops rows");
    require(
        !document.runtime()
             .applyAnnotationTransaction(QByteArrayLiteral(
                 R"({"version":1,"operations":[{"type":"rectangle","bounds":[-2,18,2,2],"style":{"fill":[255,0,0,255],"stroke":[0,0,0,0]}}]})"))
             .isEmpty(),
        "transparent sampling fixture includes ordinary drawing for tiled export");
    QList<CanvasExportSource> sources;
    for (const auto& layer : document.baseImageSources())
        sources.append({layer.image, layer.canvasRect, layer.coverage});
    const auto image =
        document.runtime().renderToImage(imported.canvasRect, imported.image.size(), sources);
    require(image.pixelColor(4, 4).alpha() == 0 && image.pixelColor(4, 22).alpha() == 0,
            "transparent edited pixels and removed strips never expose retained original donors");
    const auto tiled = document.runtime().renderTilesToImage(
        imported.canvasRect, imported.image.size(), imported.image.size(),
        {{QRect(QPoint(), imported.image.size()), QPoint()}}, sources);
    require(tiled == image, "ordinary tiled drawing exports preserve donor coverage and alpha");
    const auto exterior =
        document.runtime().renderToImage(QRectF(-16, 0, 4, 4), QSize(4, 4), sources);
    require(exterior.pixelColor(1, 1) == QColor(Qt::red),
            "sampling pixels outside the original editable bounds remain available");
}

void cropHistoryPreservesPixelsAndCancellation() {
    for (const auto axis :
         {ImageEditingDocument::CropAxis::Rows, ImageEditingDocument::CropAxis::Columns}) {
        auto original = source();
        for (int y = 0; y < original.image.height(); ++y)
            for (int x = 0; x < original.image.width(); ++x)
                original.image.setPixelColor(x, y, QColor(x * 7, y * 9, (x + y) * 3, 180));
        ImageEditingDocument document;
        require(document.load(original), "crop history fixture opens");
        const auto saved = document.checkpoint();
        require(document.crop(axis, 5, 11), "crop removes the requested strip");
        const auto cropped = document.image();
        const auto edited = document.checkpoint();
        for (int round = 0; round < 3; ++round) {
            for (const bool redo : {false, true}) {
                const auto before = document.image();
                auto cancelled = document.historyTransition(redo);
                ImageEditingDocument::prepareHistoryTransition(cancelled, [] { return true; });
                require(!document.applyHistoryTransition(std::move(cancelled)) &&
                            document.image() == before,
                        "cancelled pixel preparation leaves mixed history untouched");
                require(redo ? document.redo() : document.undo(),
                        "crop history transition succeeds");
                require(document.image() == (redo ? cropped : original.image) &&
                            document.checkpoint() == (redo ? edited : saved) &&
                            document.dirty() == redo,
                        "repeated undo and redo preserve every pixel and saved-content identity");
            }
        }
    }
}

void contentCheckpointsFollowReplacementAndSavedRevisions() {
    ImageEditingDocument document;
    require(document.load(source()), "checkpoint fixture opens");
    const auto empty = document.checkpoint();
    require(!document.runtime()
                 .applyAnnotationTransaction(QByteArrayLiteral(
                     R"({"version":1,"operations":[{"type":"rectangle","bounds":[0,16,8,4]}]})"))
                 .isEmpty(),
            "checkpoint fixture adds content");
    const auto first = document.checkpoint();
    require(first != empty && document.dirty(), "content mutation changes the checkpoint");
    document.markExported(first);
    require(document.undo() && document.checkpoint() == empty && document.dirty(),
            "undo remains dirty relative to exported content");
    require(document.redo() && document.checkpoint() == first && !document.dirty(),
            "redo restores exported content identity");
    auto replacement = source();
    SnowCanvasRuntime other;
    require(!other
                 .applyAnnotationTransaction(QByteArrayLiteral(
                     R"({"version":1,"operations":[{"type":"rectangle","bounds":[4,20,6,3]}]})"))
                 .isEmpty(),
            "replacement has different content at the same local revision");
    replacement.canvasSession = other.serializeDocumentSnapshot();
    require(document.load(replacement) && document.checkpoint() != first && !document.dirty(),
            "runtime replacement invalidates cached content identity");
    document.markExported(first);
    require(document.dirty(), "late export completion cannot mark newer content clean");

    const auto replacementCheckpoint = document.runtime().documentCheckpoint();
    SnowCanvasRuntime alternative;
    require(!alternative
                 .applyAnnotationTransaction(QByteArrayLiteral(
                     R"({"version":1,"operations":[{"type":"rectangle","bounds":[1,2,3,4]}]})"))
                 .isEmpty(),
            "in-place replacement fixture has distinct content");
    require(document.runtime().restoreDocumentHistoryPreservingEditorStyles(
                alternative.serializeDocumentHistory()) &&
                document.runtime().documentCheckpoint() == alternative.documentCheckpoint() &&
                document.runtime().documentCheckpoint() != replacementCheckpoint,
            "in-place history replacement invalidates canonical bytes even if revisions collide");
    const auto beforeClear = document.runtime().documentCheckpoint();
    require(document.runtime().clearDocumentPreservingViewports() &&
                !document.runtime().hasDocumentContent() &&
                document.runtime().documentCheckpoint() != beforeClear,
            "clearing a document invalidates canonical bytes before publishing its new state");
}
QImage rendered(ScreenshotExportSource source) {
    QObject receiver;
    auto artifact = std::make_shared<ScreenshotExportArtifact>(std::move(source));
    bool completed = false;
    ScreenshotExportImageResult result;
    require(artifact->requestImage(&receiver,
                                   [&](auto value) {
                                       result = std::move(value);
                                       completed = true;
                                   }),
            "cached export starts");
    QElapsedTimer timer;
    timer.start();
    while (!completed && timer.elapsed() < 15000) {
        QApplication::processEvents();
        QThread::msleep(1);
    }
    require(completed && result.succeeded(), "cached export completes");
    return result.image;
}

void exportsShareOnlyUnchangedImmutablePixels() {
    ImageEditingDocument document;
    const auto original = source();
    require(document.load(original), "reusable export fixture opens");
    require(rendered(document.exportSource()) == document.image(),
            "image-only export preserves original pixels and alpha");
    require(
        !document.runtime()
             .applyAnnotationTransaction(QByteArrayLiteral(
                 R"({"version":1,"operations":[{"type":"rectangle","bounds":[0,16,8,4],"style":{"fill":[255,0,0,255]}}]})"))
             .isEmpty(),
        "reusable export fixture adds an annotation");
    const auto frozen = document.exportSource();
    const auto first = rendered(frozen);
    document.markExported(document.checkpoint());
    const auto repeated = rendered(document.exportSource());
    require(first.cacheKey() == repeated.cacheKey(),
            "separate unchanged-content exports share the same rendered raster");
    auto cancelled = std::make_shared<ScreenshotExportArtifact>(document.exportSource());
    cancelled->cancel();
    require(rendered(document.exportSource()) == first,
            "cancelling an export cannot poison another consumer of cached pixels");
    require(document.crop(ImageEditingDocument::CropAxis::Columns, 2, 6),
            "reusable export fixture crops columns");
    require(rendered(document.exportSource()).size() == document.image().size() &&
                rendered(frozen) == first,
            "a crop invalidates current output while in-flight snapshots retain their pixels");
    require(document.undo() && rendered(document.exportSource()) == first,
            "undo re-renders the correct complete content");
}
} // namespace
void runImageEditingDocumentTests() {
    rejectedReplacementPreservesDocument();
    historicalEditsDoNotLimitContentOperations();
    samplingContextDoesNotRevealOriginalPixelsAfterCrop();
    cropHistoryPreservesPixelsAndCancellation();
    contentCheckpointsFollowReplacementAndSavedRevisions();
    exportsShareOnlyUnchangedImmutablePixels();
}
