#include "snow_shot/app/mcp/mcpstylepatch.h"
#include "snow_shot/presentation/screenshotcanvastoolstyles.h"
#include "snow_shot/presentation/screenshotdefaultstyles.h"
#include "snow_shot/presentation/screenshottoolbarlayoutmodel.h"
#include "snow_shot/storage/applicationstorage.h"
#include "snow_draw_engine_qt/snow_canvas_runtime.h"

#include <QApplication>
#include <QDir>
#include <QJsonDocument>
#include <QScopeGuard>
#include <QTemporaryDir>

#include <cstdlib>
#include <iostream>

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(1);
    }
}

void defaultsPersistIndependently() {
    using namespace snow_shot::presentation;
    const auto initial = screenshotCanvasStyleDefaults();
    require(initial.speechBubble == initial.rectangle,
            "Speech Bubble starts with the Shape appearance");
    auto changed = initial.speechBubble;
    changed.shape = SnowCanvasRectangleShape::ThoughtBubble;
    changed.stroke = QColor(12, 34, 56);
    changed.strokeWidth = 4;
    changed.fill = QColor(240, 230, 220, 200);
    changed.fillStyle = SnowCanvasFillStyle::CrossLine;
    changed.cornerRadii = {7, 8, 9, 10};
    changed.strokeRenderingStyle = SnowCanvasStrokeRenderingStyle::Cartoonist;
    constexpr quint32 properties =
        SnowCanvasShapeStylePropertyShape | SnowCanvasShapeStylePropertyStrokeColor |
        SnowCanvasShapeStylePropertyStrokeWidth | SnowCanvasShapeStylePropertyFillColor |
        SnowCanvasShapeStylePropertyFillStyle | SnowCanvasShapeStylePropertyCornerRadius |
        SnowCanvasShapeStylePropertyStrokeRenderingStyle;
    require(persistScreenshotCanvasStyleEdit(
                SnowCanvasShapeEdit{changed, properties, SnowCanvasShapeKind::SpeechBubble}),
            "persist independent Speech Bubble appearance");
    auto partial = initial.speechBubble;
    partial.strokeWidth = 6;
    require(
        persistScreenshotCanvasStyleEdit(SnowCanvasShapeEdit{
            partial, SnowCanvasShapeStylePropertyStrokeWidth, SnowCanvasShapeKind::SpeechBubble}),
        "merge a Speech Bubble width edit");
    changed.strokeWidth = 6;
    const auto restored = screenshotCanvasToolStyleDefaults();
    require(restored.speechBubble == changed && restored.rectangle == initial.rectangle &&
                restored.text == initial.text,
            "partial Speech Bubble edits preserve other appearance and Shape/Text defaults");
    SnowCanvasWidget canvas;
    applyScreenshotCanvasToolStyles(canvas, restored);
    require(canvas.setCanvasTool(SnowCanvasTool::SpeechBubble) &&
                canvas.canvasStyleToolbarState().source ==
                    SnowCanvasStyleToolbarSource::DefaultSpeechBubble &&
                canvas.canvasStyleToolbarState().shapeStyle == changed,
            "new canvases restore independent Speech Bubble defaults");
}

void toolParticipatesInConfiguration() {
    using namespace snow_shot;
    const auto defaults = presentation::toolbar_layout::defaultPositions();
    const auto bubble = QStringLiteral("speech-bubble");
    const auto serial = QStringLiteral("serial-number");
    const QStringList group{bubble, serial};
    const auto stack =
        presentation::toolbar_layout::stackPresentation(group, [](const QString&) { return true; });
    require(defaults.contains(group) && !defaults.contains({bubble}) &&
                stack.entryItemId() == serial &&
                stack.popoverItemIds == QStringList{serial, bubble},
            "Speech Bubble shares the default Serial Number group with Serial Number first");
    const auto shortcuts = storage::DrawingShortcutSettings().allShortcuts();
    require(shortcuts.contains(QStringLiteral("speech_bubble")) &&
                shortcuts.value(QStringLiteral("speech_bubble")).isEmpty(),
            "Speech Bubble shortcuts start unassigned");
    require(storage::DrawingShortcutSettings().setShortcuts(QStringLiteral("speech_bubble"),
                                                            {QStringLiteral("Ctrl+Alt+F12")}),
            "Speech Bubble supports configurable shortcuts");
    require(presentation::screenshotQuickSelectionDisabledTools({bubble}).contains(
                SnowCanvasTool::SpeechBubble),
            "Speech Bubble participates in quick selection exclusions");
    storage::ScreenshotToolbarSettings settings;
    constexpr auto kind = storage::ScreenshotToolbarLayoutKind::DrawingTools;
    require(settings.layout(kind).positions == defaults,
            "new configurations use the same grouped drawing toolbar defaults");
    auto withoutBubble = defaults;
    for (auto& position : withoutBubble)
        position.removeAll(bubble);
    const auto save = [&](const QVector<QStringList>& positions, const QStringList& hidden = {}) {
        QJsonArray saved;
        for (const auto& position : positions)
            saved.append(QJsonArray::fromStringList(position));
        return storage::ApplicationStorage::instance().configuration().setValue(
            QStringLiteral("screenshot_toolbar/layout"),
            QJsonObject{{QStringLiteral("positions"), saved},
                        {QStringLiteral("hidden"), QJsonArray::fromStringList(hidden)}});
    };
    auto completed = withoutBubble;
    completed.append({bubble});
    require(save(withoutBubble) && settings.layout(kind).positions == completed &&
                presentation::toolbar_layout::normalizedLayout({withoutBubble, {}}).positions ==
                    completed,
            "missing Speech Bubble uses generic layout completion without rewriting saved groups");
    auto standalone = withoutBubble;
    standalone.insert(standalone.indexOf({QStringLiteral("shape")}) + 1, {bubble});
    require(save(standalone) && settings.layout(kind).positions == standalone &&
                presentation::toolbar_layout::normalizedLayout({standalone, {}}).positions ==
                    standalone,
            "explicit standalone Speech Bubble uses ordinary layout handling without regrouping");
    const QVector<QStringList> custom{{QStringLiteral("text"), QStringLiteral("shape")},
                                      {QStringLiteral("filter"), serial},
                                      {QStringLiteral("undo")},
                                      {QStringLiteral("redo")}};
    require(save(custom), "save a custom toolbar");
    const auto normalized = settings.layout(kind);
    require(normalized.positions.contains(custom.first()) &&
                normalized.positions.contains(custom.at(1)) &&
                normalized.positions.contains({bubble}),
            "generic completion preserves custom stacks without inserting Speech Bubble into them");
    require(presentation::toolbar_layout::normalizedLayout({custom, {}}) == normalized &&
                presentation::toolbar_layout::normalizedLayout(normalized) == normalized,
            "storage and presentation agree on idempotent toolbar normalization");
    require(save(withoutBubble, {bubble}) &&
                settings.layout(kind) ==
                    storage::ScreenshotToolbarLayout{withoutBubble, {bubble}} &&
                presentation::toolbar_layout::normalizedLayout({withoutBubble, {bubble}}) ==
                    settings.layout(kind),
            "explicitly hidden Speech Bubble remains hidden");
    auto relocated = withoutBubble;
    relocated.prepend({bubble});
    require(save(relocated) && settings.layout(kind).positions == relocated &&
                presentation::toolbar_layout::normalizedLayout({relocated, {}}).positions ==
                    relocated,
            "an explicit standalone Speech Bubble placement stays in its custom position");
    auto hiddenSerial = withoutBubble;
    hiddenSerial.removeAll({serial});
    require(save(hiddenSerial, {serial}) && settings.layout(kind).hidden.contains(serial) &&
                settings.layout(kind).positions.contains({bubble}) &&
                presentation::toolbar_layout::normalizedLayout({hiddenSerial, {serial}}) ==
                    settings.layout(kind),
            "Speech Bubble stays available when Serial Number is explicitly hidden");
    require(settings.setLastDrawingTool(bubble) && settings.lastDrawingTool() == bubble,
            "Speech Bubble can be remembered across annotation sessions");
}

void mcpRoutesShapeStylesExplicitly() {
    using namespace snow_shot::app::mcp;
    require(mcpCanvasTools().value(QStringLiteral("speech_bubble")) == SnowCanvasTool::SpeechBubble,
            "MCP selects the Speech Bubble tool");
    SnowCanvasRuntime runtime;
    SnowCanvasRuntimeEditor editor(runtime, SnowCanvasTool::SpeechBubble);
    require(editor.isValid(), "create a headless Speech Bubble editor");
    const auto apply = [&](const QJsonObject& patch) {
        return mcpStylePatch(editor, editor,
                             {{QStringLiteral("target"), QStringLiteral("speech_bubble")},
                              {QStringLiteral("style"), patch}});
    };
    require(apply({{QStringLiteral("shape"), QStringLiteral("ellipse")},
                   {QStringLiteral("stroke_width"), 5},
                   {QStringLiteral("fill_style"), QStringLiteral("line")},
                   {QStringLiteral("stroke_rendering_style"), QStringLiteral("artist")},
                   {QStringLiteral("corner_radii"), QJsonArray{1, 2, 3, 4}}}),
            "MCP applies Speech Bubble shape properties");
    const auto accepted = editor.canvasStyleToolbarState().shapeStyle;
    require(accepted.shape == SnowCanvasRectangleShape::Ellipse && accepted.strokeWidth == 5 &&
                accepted.fillStyle == SnowCanvasFillStyle::Line &&
                accepted.strokeRenderingStyle == SnowCanvasStrokeRenderingStyle::Artist &&
                accepted.cornerRadii == SnowCanvasCornerRadii{1, 2, 3, 4},
            "MCP style patches reach Speech Bubble defaults instead of Spotlight");
    require(apply({{QStringLiteral("shape"), QStringLiteral("thought_bubble")}}) &&
                editor.canvasStyleToolbarState().shapeStyle.shape ==
                    SnowCanvasRectangleShape::ThoughtBubble,
            "MCP accepts Thought Bubble for Speech Bubble");
    const auto thought = editor.canvasStyleToolbarState().shapeStyle;
    for (const QString& target :
         {QStringLiteral("rectangle"), QStringLiteral("magnifier"), QStringLiteral("spotlight")}) {
        require(!mcpStylePatch(
                    editor, editor,
                    {{QStringLiteral("target"), target},
                     {QStringLiteral("style"),
                      QJsonObject{{QStringLiteral("shape"), QStringLiteral("thought_bubble")}}}}) &&
                    editor.canvasStyleToolbarState().shapeStyle == thought,
                "Thought Bubble rejects unrelated targets without changing state");
    }
    require(apply({{QStringLiteral("shape"), QStringLiteral("ellipse")}}),
            "restore the accepted ellipse appearance");
    for (const auto& invalid :
         {QJsonObject{{QStringLiteral("shape"), QStringLiteral("triangle")}},
          QJsonObject{{QStringLiteral("stroke_rendering_style"), QStringLiteral("rough")}},
          QJsonObject{{QStringLiteral("stroke_width"), -1}},
          QJsonObject{{QStringLiteral("font_size"), 42}}})
        require(!apply(invalid) && editor.canvasStyleToolbarState().shapeStyle == accepted,
                "invalid Speech Bubble styles reject without modifying defaults");
    require(editor.setActiveTool(SnowCanvasTool::Shape) &&
                editor.canvasStyleToolbarState().shapeStyle != accepted,
            "MCP edits preserve independent Shape defaults");
}
} // namespace

int main(int argc, char** argv) {
    QApplication application(argc, argv);
    QTemporaryDir directory;
    require(directory.isValid() && QDir().mkpath(directory.filePath(QStringLiteral("bin"))),
            "create isolated test storage");
    require(snow_shot::storage::ApplicationStorage::instance()
                .initialize({directory.filePath(QStringLiteral("bin")), directory.path(), 60000})
                .success,
            "initialize isolated Speech Bubble configuration");
    const auto shutdownStorage =
        qScopeGuard([] { snow_shot::storage::ApplicationStorage::instance().shutdown(); });
    defaultsPersistIndependently();
    toolParticipatesInConfiguration();
    mcpRoutesShapeStylesExplicitly();
    return 0;
}
