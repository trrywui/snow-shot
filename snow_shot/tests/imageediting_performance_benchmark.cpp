// SPDX-License-Identifier: GPL-3.0-or-later
#include "snow_shot/presentation/imageeditingwindow.h"
#include "snow_shot/presentation/imageeditingcanvas.h"
#include "snow_shot/presentation/screenshotexportartifact.h"
#include "snow_shot/storage/applicationstorage.h"
#include "../../test-support/memorysnapshot.h"
#include <QApplication>
#include <QFontDatabase>
#include <QDir>
#include <QElapsedTimer>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QTemporaryDir>
#include <QThread>
#include <QWheelEvent>
#include <QMouseEvent>
#include <iostream>
#include <stdexcept>
#include <algorithm>

using namespace snow_shot::presentation;
namespace {
void require(bool value, const char* message) {
    if (!value)
        throw std::runtime_error(message);
}
double milliseconds(const QElapsedTimer& timer) {
    return double(timer.nsecsElapsed()) / 1e6;
}
QJsonObject run(const QString& name, QSize size, int annotations, bool filtered = false) {
    ImageEditingSource source;
    source.image = QImage(size, QImage::Format_ARGB32_Premultiplied);
    source.image.fill(QColor(230, 240, 250, 160));
    source.canvasRect = QRectF(QPointF(), size);
    SnowCanvasRuntime runtime;
    if (annotations) {
        QJsonArray operations;
        for (int i = 0; i < annotations; ++i) {
            operations.append(QJsonObject{
                {QStringLiteral("type"), QStringLiteral("rectangle")},
                {QStringLiteral("bounds"), QJsonArray{(i * 31) % (size.width() - 100),
                                                      (i * 53) % (size.height() - 100), 80, 50}}});
            if (operations.size() == 100 || i + 1 == annotations) {
                require(
                    !runtime
                         .applyAnnotationTransaction(
                             QJsonDocument(QJsonObject{{QStringLiteral("version"), 1},
                                                       {QStringLiteral("operations"), operations}})
                                 .toJson())
                         .isEmpty(),
                    "annotation fixture");
                operations = {};
            }
        }
    }
    if (filtered) {
        require(
            !runtime
                 .applyAnnotationTransaction(QByteArrayLiteral(
                     R"({"version":1,"operations":[{"type":"rectangle_filter","bounds":[0,0,800,600],"style":{"filter":"gaussian_blur","strength":0.5}}]})"))
                 .isEmpty(),
            "filter fixture");
    }
    if (annotations || filtered)
        source.canvasSession = runtime.serializeDocumentSession();
    const auto before = snow::test_support::memorySnapshot();
    QElapsedTimer timer;
    double openMs = 0, cropMs = 0, undoMs = 0, redoMs = 0, exportMs = 0;
    double checkpointMs = 0, snapshotMs = 0;
    std::vector<double> cameraMs;
    std::vector<double> editMs;
    for (int pass = 0; pass < 3; ++pass) {
        timer.start();
        auto editor = std::make_unique<ImageEditingWindow>();
        editor->present(source);
        QApplication::processEvents();
        openMs += milliseconds(timer);
        for (int frame = 0; frame < 40; ++frame) {
            QWheelEvent wheel(QPointF(300, 200), editor->canvas()->mapToGlobal(QPoint(300, 200)),
                              {}, QPoint(0, frame % 2 ? 120 : -120), Qt::NoButton, Qt::NoModifier,
                              Qt::NoScrollPhase, false);
            timer.start();
            QApplication::sendEvent(editor->canvas(), &wheel);
            QMouseEvent press(QEvent::MouseButtonPress, QPointF(300, 200),
                              editor->canvas()->mapToGlobal(QPoint(300, 200)), Qt::MiddleButton,
                              Qt::MiddleButton, Qt::NoModifier);
            QMouseEvent move(QEvent::MouseMove, QPointF(305, 203),
                             editor->canvas()->mapToGlobal(QPoint(305, 203)), Qt::NoButton,
                             Qt::MiddleButton, Qt::NoModifier);
            QMouseEvent release(QEvent::MouseButtonRelease, QPointF(305, 203),
                                editor->canvas()->mapToGlobal(QPoint(305, 203)), Qt::MiddleButton,
                                Qt::NoButton, Qt::NoModifier);
            QApplication::sendEvent(editor->canvas(), &press);
            QApplication::sendEvent(editor->canvas(), &move);
            QApplication::sendEvent(editor->canvas(), &release);
            QApplication::processEvents();
            cameraMs.push_back(milliseconds(timer));
        }
        editor->canvas()->fit();
        QApplication::processEvents();
        for (int edit = 0; edit < 12; ++edit) {
            const QJsonObject operation{
                {QStringLiteral("type"), QStringLiteral("rectangle")},
                {QStringLiteral("bounds"), QJsonArray{20 + edit * 12, 30, 8, 8}}};
            const auto transaction =
                QJsonDocument(QJsonObject{{QStringLiteral("version"), 1},
                                          {QStringLiteral("operations"), QJsonArray{operation}}})
                    .toJson(QJsonDocument::Compact);
            timer.start();
            require(
                !editor->document()->runtime().applyAnnotationTransaction(transaction).isEmpty(),
                "small annotation edit");
            QApplication::processEvents();
            editMs.push_back(milliseconds(timer));
        }
        timer.start();
        for (int query = 0; query < 100; ++query)
            require(editor->dirty(), "edited content remains dirty");
        checkpointMs += milliseconds(timer) / 100;
        timer.start();
        require(editor->document()->crop(ImageEditingDocument::CropAxis::Rows, size.height() / 3,
                                         size.height() / 2),
                "crop");
        cropMs += milliseconds(timer);
        timer.start();
        require(editor->document()->undo(), "undo");
        undoMs += milliseconds(timer);
        timer.start();
        require(editor->document()->redo(), "redo");
        redoMs += milliseconds(timer);
        timer.start();
        const auto snapshot = editor->document()->snapshot();
        snapshotMs += milliseconds(timer);
        auto artifact = std::make_shared<ScreenshotExportArtifact>(
            ScreenshotExportSource::fromPinnedViewport({snapshot.canvasSession,
                                                        snapshot.image,
                                                        snapshot.canvasRect,
                                                        snapshot.image.size(),
                                                        {},
                                                        snapshot.smartErase}));
        bool completed = false, success = false;
        timer.start();
        require(artifact->requestImage(editor.get(),
                                       [&](auto result) {
                                           success = result.succeeded();
                                           completed = true;
                                       }),
                "start export");
        while (!completed && timer.elapsed() < 60000) {
            QApplication::processEvents();
            QThread::msleep(1);
        }
        require(completed && success, "export");
        exportMs += milliseconds(timer);
        editor->shutdown();
        editor.reset();
        artifact.reset();
        QApplication::sendPostedEvents(nullptr, QEvent::DeferredDelete);
        QApplication::processEvents();
    }
    std::sort(cameraMs.begin(), cameraMs.end());
    std::sort(editMs.begin(), editMs.end());
    const auto after = snow::test_support::memorySnapshot();
    return {{QStringLiteral("fixture"), name},
            {QStringLiteral("open_mean_ms"), openMs / 3},
            {QStringLiteral("pan_zoom_p95_ms"), cameraMs[size_t(cameraMs.size() * 0.95)]},
            {QStringLiteral("small_edit_p95_ms"), editMs[size_t(editMs.size() * 0.95)]},
            {QStringLiteral("dirty_check_mean_ms"), checkpointMs / 3},
            {QStringLiteral("snapshot_mean_ms"), snapshotMs / 3},
            {QStringLiteral("crop_mean_ms"), cropMs / 3},
            {QStringLiteral("undo_mean_ms"), undoMs / 3},
            {QStringLiteral("redo_mean_ms"), redoMs / 3},
            {QStringLiteral("export_mean_ms"), exportMs / 3},
            {QStringLiteral("peak_resident_bytes"), qint64(after.peakResidentBytes)},
            {QStringLiteral("retained_private_bytes_delta"),
             qint64(after.footprintBytes) - qint64(before.footprintBytes)}};
}
} // namespace
int main(int argc, char** argv) {
    QApplication app(argc, argv);
    app.setQuitOnLastWindowClosed(false);
    const int font = QFontDatabase::addApplicationFont(
        QStringLiteral(":/recording-test-fonts/SnowRecordingTestSans-Regular.ttf"));
    if (font >= 0)
        app.setFont(QFont(QFontDatabase::applicationFontFamilies(font).first(), 12));
    QTemporaryDir temporary;
    auto& storage = snow_shot::storage::ApplicationStorage::instance();
    if (!temporary.isValid() ||
        !storage
             .initialize({QDir(temporary.path()).filePath(QStringLiteral("bin")),
                          QDir(temporary.path()).filePath(QStringLiteral("settings")), 0})
             .success)
        return 1;
    try {
        QJsonArray report;
        report.append(run(QStringLiteral("4k"), QSize(3840, 2160), 0));
        report.append(run(QStringLiteral("tall"), QSize(2160, 16000), 0));
        report.append(run(QStringLiteral("annotation-heavy"), QSize(3840, 2160), 500));
        report.append(run(QStringLiteral("filtered-4k"), QSize(3840, 2160), 0, true));
        report.append(run(QStringLiteral("filtered-tall"), QSize(2160, 16000), 0, true));
        std::cout << QJsonDocument(report).toJson().constData();
    } catch (const std::exception& error) {
        std::cerr << error.what();
        return 1;
    }
    ScreenshotExportCoordinator::shared().shutdown();
    storage.shutdown();
    return 0;
}
