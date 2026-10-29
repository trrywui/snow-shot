// SPDX-License-Identifier: GPL-3.0-or-later
#include "snow_shot/presentation/imageeditingcanvas.h"
#include "snow_shot/presentation/imageeditingwindow.h"
#include "snow_shot/presentation/screenshotocrpresentation.h"
#include "snow_shot/presentation/screenshotpinnedtextpreparation.h"
#include "snow_shot/presentation/screenshotpinnedwindow.h"
#include "snow_shot/presentation/screenshotrecognitionsessioncontroller.h"
#include "snow_shot/presentation/screenshottoolpalette.h"
#include "snow_shot/storage/settingsadapters.h"

#include <QApplication>
#include <QClipboard>
#include <QElapsedTimer>
#include <QMimeData>
#include <QPointer>
#include <QScopeGuard>
#include <QScreen>
#include <QTextEdit>
#include <QThread>
#include <stdexcept>

#if SNOW_SHOT_ENABLE_TEXT_TRANSLATION
namespace {
using snow_shot::presentation::ImageEditingSource;
using snow_shot::presentation::ImageEditingWindow;
using Session = ScreenshotRecognitionSessionController;

void require(bool condition, const char* message) {
    if (!condition)
        throw std::runtime_error(message);
}

void awaitCondition(const std::function<bool()>& condition, const char* message) {
    QElapsedTimer timer;
    timer.start();
    while (!condition() && timer.elapsed() < 10000) {
        QApplication::processEvents();
        QThread::msleep(1);
    }
    require(condition(), message);
}

ScreenshotRecognitionResults recognitionFixture() {
    auto original = std::make_shared<ScreenshotOcrPresentation>();
    original->selection = QRect(0, 0, 100, 60);
    ScreenshotOcrLine line;
    line.text = QStringLiteral("Original text");
    line.quad = {QPointF(5, 5), QPointF(95, 5), QPointF(95, 25), QPointF(5, 25)};
    original->lines.append(line);
    original->prepareForRendering();
    auto translated = std::make_shared<ScreenshotOcrPresentation>();
    translated->selection = original->selection;
    translated->lines = original->lines;
    translated->lines[0].text = QStringLiteral("Translated text");
    translated->prepareForRendering();
    ScreenshotRecognitionResults results;
    results.key = QStringLiteral("image-editing-translation");
    results.text = ScreenshotOcrRecognitionResult{original};
    results.translatedText = std::move(translated);
    return results;
}

void editCachedTranslation(const ImageEditingSource& source) {
    ImageEditingWindow editor;
    editor.present(source);
    editor.palette()->textTranslationRequested();
    auto* session = editor.recognition();
    QPointer<QTextEdit> text = editor.canvas()->parentWidget()->findChild<QTextEdit*>();
    require(session && session->translating() && session->editing() &&
                !session->originalImageTranslationActive(),
            "independent cached translation uses editable text instead of the image overlay");
    require(text && text->isVisible() && !text->isReadOnly() &&
                text->toPlainText() == QStringLiteral("Translated text") &&
                session->textDraft() == text->toPlainText(),
            "cached translation displays the same text that export reads without a provider");
    require(session->findChildren<snow_shot::translation::TranslationJob*>().isEmpty(),
            "restoring a complete captured translation does not start another translation job");
    require(!editor.dirty(), "viewing a captured translation preserves the import checkpoint");
    session->setTextDraft(QStringLiteral("Edited translation"));
    require(text->toPlainText() == QStringLiteral("Edited translation"),
            "the captured translation document accepts independent edits");
    editor.palette()->copyRequested();
    require(QApplication::clipboard()->text() == text->toPlainText(),
            "Copy publishes the translation currently displayed in its editor");
    const snow_shot::storage::ScreenshotTranslationSettings settings;
    const auto restoreCopy =
        qScopeGuard([&] { static_cast<void>(settings.setCopyTranslationResultsAsImage(false)); });
    require(settings.setCopyTranslationResultsAsImage(true),
            "enable editor translation image copy");
    editor.palette()->copyRequested();
    require(QApplication::clipboard()->mimeData()->hasImage() &&
                !QApplication::clipboard()->mimeData()->hasText(),
            "editor Copy publishes the translated document as an image");
    auto cursor = text->textCursor();
    cursor.setPosition(0);
    cursor.setPosition(6, QTextCursor::KeepAnchor);
    text->setTextCursor(cursor);
    editor.palette()->copyRequested();
    require(QApplication::clipboard()->text() == QStringLiteral("Edited") &&
                !QApplication::clipboard()->mimeData()->hasImage(),
            "editor Copy preserves selected translation text");
    require(settings.setCopyTranslationResultsAsImage(false), "restore editor copy preference");
    editor.palette()->ocrRequested();
    require(!text, "switching recognition tools retires the preceding text editor widget");
    editor.palette()->textTranslationRequested();
    text = editor.canvas()->parentWidget()->findChild<QTextEdit*>();
    require(text && text->isVisible() && session->translating() &&
                !session->originalImageTranslationActive() &&
                text->toPlainText() == QStringLiteral("Edited translation"),
            "returning to cached translation preserves the edited translation document");
    session->resetTextEditing();
    require(text->toPlainText() == QStringLiteral("Translated text"),
            "Reset restores the captured translation as the editing baseline");
    editor.shutdown();
}

void liveAndPersistedTranslationImports() {
    QImage image(100, 60, QImage::Format_ARGB32_Premultiplied);
    image.fill(Qt::white);
    ScreenshotPinnedWindow::Config config;
    config.screen = QGuiApplication::primaryScreen();
    require(config.screen, "translation import fixture needs an offscreen screen");
    config.canvasSourceRect = QRectF(0, 0, 100, 60);
    config.contentCanvasRect = config.canvasSourceRect;
    config.surfaceCanvasRect = config.canvasSourceRect;
    config.imageSource = ScreenshotImageSource::fromImage(image, config.canvasSourceRect);
    config.initialWindowSize = image.size();
    config.nativeGeometry = QRect(QPoint(40, 40), image.size());
    config.automaticTextRecognition = false;
    config.recognitionResults = recognitionFixture();
    ScreenshotPinnedWindow pin;
    pin.setAttribute(Qt::WA_DeleteOnClose, false);
    bool published = false;
    require(pin.present(config, [&](bool success, QImage) { published = success; }),
            "translation source pin presents");
    awaitCondition([&] { return published; }, "translation source pin publishes its image");
    const auto live = pin.imageEditingSnapshot();
    const auto persisted = snow_shot::presentation::readImageEditingPinned(
        pin.persistenceSnapshot(), captureScreenshotPinnedTextPreparation());
    require(live.recognition.translatedText && persisted.recognition.translatedText,
            "live and persisted pins retain captured translation content for import");
    editCachedTranslation(live);
    pin.hide();
    editCachedTranslation(persisted);
}

void independentFreshTranslationKeepsTextPresentation() {
    auto cached = recognitionFixture();
    cached.translatedText.reset();
    QImage image(100, 60, QImage::Format_ARGB32_Premultiplied);
    image.fill(Qt::white);
    Session session(nullptr, nullptr, nullptr, {});
    session.setIndependentEditing(true);
    session.setTarget({cached.key, image, QRectF(0, 0, 100, 60)});
    session.seedRecognitionResults(cached);
    session.activate(Session::Mode::Text);
    session.beginTextTranslation();
    require(session.translating() && session.editing() &&
                !session.originalImageTranslationActive() && session.textDraft().isEmpty(),
            "a fresh independent translation keeps text editing even with image mode enabled");
}

void independentModeAfterSeedingKeepsCapturedText() {
    const auto cached = recognitionFixture();
    QImage image(100, 60, QImage::Format_ARGB32_Premultiplied);
    image.fill(Qt::white);
    Session session(nullptr, nullptr, nullptr, {});
    session.setTarget({cached.key, image, QRectF(0, 0, 100, 60)});
    session.seedRecognitionResults(cached);
    session.setIndependentEditing(true);
    session.activate(Session::Mode::Text);
    require(!session.activateCachedTextTranslation(),
            "cached overlay activation cannot bypass independent presentation mode");
    session.beginTextTranslation();
    require(session.editing() && !session.originalImageTranslationActive() &&
                session.textDraft() == QStringLiteral("Translated text"),
            "enabling independent editing after seeding retains the captured translation text");
}

void ordinaryHostsKeepCapturedOverlay() {
    const auto cached = recognitionFixture();
    QImage image(100, 60, QImage::Format_ARGB32_Premultiplied);
    image.fill(Qt::white);
    Session session(nullptr, nullptr, nullptr, {});
    session.setTarget({cached.key, image, QRectF(0, 0, 100, 60)});
    session.seedRecognitionResults(cached);
    session.activate(Session::Mode::Text);
    session.beginTextTranslation();
    const auto mime = session.recognitionClipboardMimeData();
    require(session.originalImageTranslationActive() && mime &&
                mime->text() == QStringLiteral("Translated text"),
            "ordinary recognition hosts continue displaying and exporting the captured overlay");
}
} // namespace
#endif

void runImageEditingTranslationTests() {
#if SNOW_SHOT_ENABLE_TEXT_TRANSLATION
    const snow_shot::storage::ScreenshotTranslationSettings settings;
    const bool originalImage = settings.originalImageTranslationEnabled();
    const auto restore = qScopeGuard(
        [&] { static_cast<void>(settings.setOriginalImageTranslationEnabled(originalImage)); });
    require(settings.setOriginalImageTranslationEnabled(true),
            "translation regression selects image translation for ordinary hosts");
    liveAndPersistedTranslationImports();
    independentModeAfterSeedingKeepsCapturedText();
    independentFreshTranslationKeepsTextPresentation();
    ordinaryHostsKeepCapturedOverlay();
#endif
}
