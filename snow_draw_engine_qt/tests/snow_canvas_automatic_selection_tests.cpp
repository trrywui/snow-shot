#include "../../test-support/canvas_quick_selection_test_support.h"
#include "snow_draw_engine_qt/snow_canvas_runtime.h"

#include <QKeyEvent>
#include <QFocusEvent>
#include <QJsonDocument>
#include <QJsonObject>

using canvas_quick_selection_test::mouse;
using canvas_quick_selection_test::require;

namespace {
void drawShape(SnowCanvasWidget& canvas, QPointF start = {60, 60}) {
    require(canvas.resetEditingState() && canvas.setCanvasTool(SnowCanvasTool::Shape),
            "prepare shape creation");
    mouse(canvas, QEvent::MouseButtonPress, start, Qt::LeftButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseMove, start + QPointF(100, 60), Qt::NoButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseButtonRelease, start + QPointF(100, 60), Qt::LeftButton,
          Qt::NoButton);
}

QJsonArray textRecords(const SnowCanvasRuntime& runtime) {
    QJsonArray texts;
    const auto elements = QJsonDocument::fromJson(runtime.serializeDocumentSession())
                              .object()
                              .value(QStringLiteral("document"))
                              .toObject()
                              .value(QStringLiteral("slots"))
                              .toArray();
    for (const auto& slot : elements) {
        const auto data = slot.toObject().value(QStringLiteral("data")).toObject();
        if (data.contains(QStringLiteral("Text"))) {
            auto text = data.value(QStringLiteral("Text")).toObject();
            text.insert(QStringLiteral("id"), slot.toObject().value(QStringLiteral("id")));
            texts.append(text);
        }
    }
    return texts;
}

void outsideClickCommitsAndDeselects() {
    for (const auto tool :
         {SnowCanvasTool::Text, SnowCanvasTool::Arrow, SnowCanvasTool::SpeechBubble,
          SnowCanvasTool::SerialNumber, SnowCanvasTool::Shape, SnowCanvasTool::Distance}) {
        for (const bool reopen : {false, true}) {
            for (const bool empty : {false, true}) {
                if (tool == SnowCanvasTool::Distance && empty)
                    continue;
                const bool useSelectTool = reopen && (tool == SnowCanvasTool::Arrow ||
                                                      tool == SnowCanvasTool::SpeechBubble ||
                                                      tool == SnowCanvasTool::Distance);
                SnowCanvasRuntimeConfig config;
                config.automaticallySelectNewlyCreatedElements = true;
                config.automaticSelectionExcludedTools.clear();
                SnowCanvasRuntime runtime(config);
                SnowCanvasWidget canvas(runtime);
                SnowCanvasWidget second(runtime);
                canvas.resize(800, 600);
                second.resize(800, 600);
                canvas.show();
                QApplication::processEvents();
                require(canvas.setCanvasTool(tool), "activate text dismissal fixture tool");
                const auto key = [&](int code, Qt::KeyboardModifiers modifiers = Qt::NoModifier,
                                     const QString& text = QString()) {
                    QKeyEvent event(QEvent::KeyPress, code, modifiers, text);
                    QApplication::sendEvent(&canvas, &event);
                };
                const auto click = [&](QPointF point) {
                    mouse(canvas, QEvent::MouseButtonPress, point, Qt::LeftButton, Qt::LeftButton);
                    mouse(canvas, QEvent::MouseButtonRelease, point, Qt::LeftButton, Qt::NoButton);
                };
                mouse(canvas, QEvent::MouseButtonPress, {200, 180}, Qt::LeftButton, Qt::LeftButton);
                if (tool == SnowCanvasTool::Text) {
                    mouse(canvas, QEvent::MouseButtonRelease, {200, 180}, Qt::LeftButton,
                          Qt::NoButton);
                } else {
                    const QPointF end(400, tool == SnowCanvasTool::SpeechBubble ||
                                                   tool == SnowCanvasTool::Shape
                                               ? 260
                                               : 180);
                    mouse(canvas, QEvent::MouseMove, end, Qt::NoButton, Qt::LeftButton);
                    mouse(canvas, QEvent::MouseButtonRelease, end, Qt::LeftButton, Qt::NoButton);
                }
                if (tool == SnowCanvasTool::Arrow)
                    require(canvas.editSelectedArrowText(), "begin selected arrow label");
                else if (tool == SnowCanvasTool::Shape)
                    click({300, 284});
                else if (tool == SnowCanvasTool::Distance)
                    click({290, 180});
                require(canvas.hasActiveTextEditing(), "fixture begins text editing");
                if (reopen || tool == SnowCanvasTool::Text) {
                    if (tool != SnowCanvasTool::Distance)
                        key(Qt::Key_A, Qt::NoModifier, QStringLiteral("original"));
                    key(Qt::Key_Return, Qt::ControlModifier);
                    require(!canvas.hasActiveTextEditing(), "keyboard commit closes the editor");
                    require(tool == SnowCanvasTool::Shape ||
                                runtime.selectedElementIds().size() == 1,
                            "keyboard commit retains the selected fixture");
                    if (useSelectTool)
                        require(canvas.setCanvasTool(SnowCanvasTool::Select),
                                "reopen existing labels with the select tool");
                    const auto text = textRecords(runtime).at(0).toObject();
                    if (tool == SnowCanvasTool::SerialNumber || tool == SnowCanvasTool::Shape) {
                        const auto id = text.value(QStringLiteral("id")).toObject();
                        SnowCanvasRuntimeEditor editor(runtime);
                        require(editor.select(
                                    static_cast<quint32>(id.value(QStringLiteral("index")).toInt()),
                                    static_cast<quint32>(
                                        id.value(QStringLiteral("generation")).toInt())),
                                "select bound text before reopening its editor");
                    }
                    const auto center = text.value(QStringLiteral("center")).toObject();
                    QPointF point = canvas.canvasToViewTransform().map(
                        QPointF(center.value(QStringLiteral("x")).toDouble(),
                                center.value(QStringLiteral("y")).toDouble()));
                    if (tool == SnowCanvasTool::Distance) {
                        click(point - QPointF(10, 0));
                    } else if (tool == SnowCanvasTool::Arrow ||
                               tool == SnowCanvasTool::SpeechBubble) {
                        mouse(canvas, QEvent::MouseButtonDblClick, point, Qt::LeftButton,
                              Qt::LeftButton);
                        mouse(canvas, QEvent::MouseButtonRelease, point, Qt::LeftButton,
                              Qt::NoButton);
                    } else {
                        click(point);
                    }
                }
                if (!canvas.hasActiveTextEditing() || runtime.selectedElementIds().size() != 1) {
                    std::cerr << "fixture: tool=" << static_cast<int>(tool) << ", reopen=" << reopen
                              << ", empty=" << empty
                              << ", editing=" << canvas.hasActiveTextEditing()
                              << ", selected=" << runtime.selectedElementIds().size() << '\n';
                }
                require(canvas.hasActiveTextEditing() && runtime.selectedElementIds().size() == 1,
                        "outside click fixture is selected and editing");
                key(Qt::Key_A, Qt::ControlModifier);
                key(Qt::Key_Backspace);
                const QString content = tool == SnowCanvasTool::Distance ? QStringLiteral("25")
                                                                         : QStringLiteral("edited");
                if (!empty)
                    key(Qt::Key_A, Qt::NoModifier, content);
                click({700, 500});
                if (canvas.hasActiveTextEditing() || !runtime.selectedElementIds().isEmpty()) {
                    std::cerr << "outside click: tool=" << static_cast<int>(tool)
                              << ", reopen=" << reopen << ", empty=" << empty << '\n';
                }
                require(!canvas.hasActiveTextEditing() && runtime.selectedElementIds().isEmpty(),
                        "one outside click must finish editing and deselect");
                require(second.canvasStyleToolbarState().selectedElementCount == 0 &&
                            canvas.canvasStyleToolbarState().selectedElementCount == 0 &&
                            canvas.canvasTool() == (useSelectTool ? SnowCanvasTool::Select : tool),
                        "dismissal updates every viewport and preserves the active tool");
                const auto texts = textRecords(runtime);
                require(empty ? texts.isEmpty()
                              : texts.size() == 1 && texts.at(0)
                                                         .toObject()
                                                         .value(QStringLiteral("text"))
                                                         .toString()
                                                         .startsWith(content),
                        "outside click commits content or removes the empty label");
                const auto committed = runtime.documentCheckpoint();
                require(canvas.undo() && canvas.redo() && runtime.documentCheckpoint() == committed,
                        "dismissing selection adds no document mutation after the text commit");
            }
        }
    }
}

void selectionAndRuntimeReplacement() {
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    SnowCanvasWidget second(runtime);
    canvas.resize(400, 300);
    second.resize(400, 300);
    canvas.show();
    second.show();
    QApplication::processEvents();
    drawShape(canvas);
    require(runtime.selectedElementIds().isEmpty(), "automatic selection defaults off");
    require(runtime.setAutomaticallySelectNewlyCreatedElements(true) &&
                runtime.setQuickSelectionDisabledTools({SnowCanvasTool::Shape}),
            "enable automatic selection independently of quick selection");
    require(runtime.selectedElementIds().isEmpty(), "enabling leaves existing elements alone");
    drawShape(canvas, {220, 180});
    require(runtime.selectedElementIds().size() == 1 &&
                canvas.canvasStyleToolbarState().selectedElementCount == 1 &&
                second.canvasStyleToolbarState().selectedElementCount == 1 &&
                canvas.canvasTool() == SnowCanvasTool::Shape,
            "creation selects the new shape in every viewport and preserves the tool");
    const auto selected = runtime.selectedElementIds();
    require(runtime.setAutomaticallySelectNewlyCreatedElements(false) &&
                runtime.selectedElementIds() == selected,
            "disabling preserves the current selection");
    require(runtime.setAutomaticallySelectNewlyCreatedElements(true), "restore preference");
    const QByteArray document = runtime.serializeDocumentSession();
    const QByteArray history = runtime.serializeDocumentHistory();
    for (int replacement = 0; replacement < 4; ++replacement) {
        if (replacement == 0)
            require(runtime.reset(), "reset runtime");
        else if (replacement == 1)
            require(runtime.restoreDocumentSession(document), "restore document");
        else if (replacement == 2)
            require(runtime.restoreDocumentHistory(history), "restore history");
        else {
            SnowCanvasRuntime source;
            require(runtime.cloneDocumentSessionFrom(source), "clone another document");
        }
        drawShape(canvas);
        require(runtime.selectedElementIds().size() == 1 &&
                    second.canvasStyleToolbarState().selectedElementCount == 1,
                "runtime replacement preserves the preference and shared selection");
    }
}

void automaticSelectionExclusions() {
    SnowCanvasRuntimeConfig config;
    config.automaticallySelectNewlyCreatedElements = true;
    SnowCanvasRuntime runtime(config);
    SnowCanvasWidget canvas(runtime);
    canvas.resize(400, 300);
    canvas.show();
    QApplication::processEvents();
    const auto draw = [&](SnowCanvasTool tool) {
        require(canvas.resetEditingState() && canvas.setCanvasTool(tool), "activate excluded tool");
        mouse(canvas, QEvent::MouseButtonPress, {60, 60}, Qt::LeftButton, Qt::LeftButton);
        mouse(canvas, QEvent::MouseMove, {160, 120}, Qt::NoButton, Qt::LeftButton);
        mouse(canvas, QEvent::MouseButtonRelease, {160, 120}, Qt::LeftButton, Qt::NoButton);
    };
    for (const auto tool : {SnowCanvasTool::FreeDraw, SnowCanvasTool::PenFilter}) {
        require(runtime.setAutomaticSelectionExcludedTools(config.automaticSelectionExcludedTools),
                "apply default exclusions");
        draw(tool);
        require(runtime.selectedElementIds().isEmpty(),
                "default exclusions leave Pen and Pen filter unselected");
        require(canvas.undo() && runtime.setAutomaticSelectionExcludedTools({}),
                "clear exclusions");
        draw(tool);
        require(runtime.selectedElementIds().size() == 1,
                "removing an exclusion enables automatic selection");
        require(canvas.undo(), "remove test stroke");
    }
    require(runtime.setAutomaticSelectionExcludedTools({SnowCanvasTool::Shape}), "exclude shape");
    const QByteArray document = runtime.serializeDocumentSession();
    const QByteArray history = runtime.serializeDocumentHistory();
    for (int replacement = 0; replacement < 4; ++replacement) {
        if (replacement == 0)
            require(runtime.reset(), "reset runtime with exclusions");
        else if (replacement == 1)
            require(runtime.restoreDocumentSession(document), "restore document with exclusions");
        else if (replacement == 2)
            require(runtime.restoreDocumentHistory(history), "restore history with exclusions");
        else {
            SnowCanvasRuntime source;
            require(runtime.cloneDocumentSessionFrom(source), "clone document with exclusions");
        }
        draw(SnowCanvasTool::Shape);
        require(runtime.selectedElementIds().isEmpty(),
                "custom exclusions survive runtime replacement");
        require(canvas.undo(), "remove excluded shape");
        draw(SnowCanvasTool::FreeDraw);
        require(runtime.selectedElementIds().size() == 1,
                "removed defaults survive runtime replacement");
    }
}

void selectedShapeHasWorkingHandles() {
    SnowCanvasRuntimeConfig config;
    config.automaticallySelectNewlyCreatedElements = true;
    SnowCanvasRuntime runtime(config);
    SnowCanvasWidget canvas(runtime);
    canvas.resize(400, 300);
    canvas.show();
    QApplication::processEvents();
    drawShape(canvas);
    const QByteArray original = runtime.documentCheckpoint();
    mouse(canvas, QEvent::MouseButtonPress, {160, 120}, Qt::LeftButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseMove, {190, 150}, Qt::NoButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseButtonRelease, {190, 150}, Qt::LeftButton, Qt::NoButton);
    require(runtime.selectedElementIds().size() == 1 && runtime.documentCheckpoint() != original,
            "newly selected shape can be resized immediately");
    require(canvas.undo() && runtime.documentCheckpoint() == original,
            "resizing is separate from the single creation history entry");
    require(canvas.undo() && !canvas.canvasHistoryState().canUndo &&
                runtime.selectedElementIds().isEmpty(),
            "automatic selection adds no history entry or stale selection");
}

void textCommitRespectsDefaultExclusion() {
    for (const bool excluded : {true, false}) {
        SnowCanvasRuntimeConfig config;
        config.automaticallySelectNewlyCreatedElements = true;
        if (!excluded)
            config.automaticSelectionExcludedTools.remove(SnowCanvasTool::Text);
        SnowCanvasRuntime runtime(config);
        SnowCanvasWidget canvas(runtime);
        canvas.resize(400, 300);
        canvas.show();
        QApplication::processEvents();
        require(canvas.setCanvasTool(SnowCanvasTool::Text), "activate text tool");
        mouse(canvas, QEvent::MouseButtonPress, {200, 150}, Qt::LeftButton, Qt::LeftButton);
        mouse(canvas, QEvent::MouseButtonRelease, {200, 150}, Qt::LeftButton, Qt::NoButton);
        QKeyEvent type(QEvent::KeyPress, Qt::Key_A, Qt::NoModifier, QStringLiteral("new text"));
        QApplication::sendEvent(&canvas, &type);
        mouse(canvas, QEvent::MouseButtonPress, {25, 40}, Qt::RightButton, Qt::RightButton);
        mouse(canvas, QEvent::MouseButtonRelease, {25, 40}, Qt::RightButton, Qt::NoButton);
        require(
            !canvas.hasActiveTextEditing() &&
                runtime.selectedElementIds().size() == (excluded ? 0 : 1) &&
                (excluded || canvas.canvasStyleToolbarState().source ==
                                 SnowCanvasStyleToolbarSource::SelectedText) &&
                canvas.canvasTool() == SnowCanvasTool::Text,
            "new text respects its default exclusion and can be opted into automatic selection");
    }
}

void serialNumberLabelSelectionPolicy() {
    enum class Finish { Commit, RightClickCommit, Cancel, SwitchTool, FocusLoss };
    for (const bool enabled : {false, true}) {
        for (const bool excluded : {false, true}) {
            for (const auto finish : {Finish::Commit, Finish::RightClickCommit, Finish::Cancel,
                                      Finish::SwitchTool, Finish::FocusLoss}) {
                SnowCanvasRuntimeConfig config;
                config.automaticallySelectNewlyCreatedElements = enabled;
                if (excluded)
                    config.automaticSelectionExcludedTools.insert(SnowCanvasTool::SerialNumber);
                SnowCanvasRuntime runtime(config);
                SnowCanvasWidget canvas(runtime);
                SnowCanvasWidget second(runtime);
                canvas.resize(400, 300);
                second.resize(400, 300);
                canvas.show();
                QApplication::processEvents();
                require(canvas.setCanvasTool(SnowCanvasTool::SerialNumber), "activate badge tool");
                mouse(canvas, QEvent::MouseButtonPress, {60, 60}, Qt::LeftButton, Qt::LeftButton);
                mouse(canvas, QEvent::MouseMove, {160, 120}, Qt::NoButton, Qt::LeftButton);
                mouse(canvas, QEvent::MouseButtonRelease, {160, 120}, Qt::LeftButton, Qt::NoButton);
                require(canvas.hasActiveTextEditing(), "badge drag starts label editing");
                const auto selected = runtime.selectedElementIds();
                require(selected.size() == (enabled && !excluded ? 1 : 0),
                        "badge creation respects automatic selection");
                QKeyEvent type(QEvent::KeyPress, Qt::Key_A, Qt::NoModifier,
                               QStringLiteral("label"));
                QApplication::sendEvent(&canvas, &type);
                require(runtime.setAutomaticallySelectNewlyCreatedElements(!enabled),
                        "change the policy while editing without changing live selection");
                if (finish == Finish::SwitchTool) {
                    require(canvas.setCanvasTool(SnowCanvasTool::Select),
                            "commit by changing tool");
                } else if (finish == Finish::FocusLoss) {
                    QFocusEvent focusOut(QEvent::FocusOut, Qt::OtherFocusReason);
                    QApplication::sendEvent(&canvas, &focusOut);
                } else if (finish == Finish::RightClickCommit) {
                    mouse(canvas, QEvent::MouseButtonPress, {25, 240}, Qt::RightButton,
                          Qt::RightButton);
                    mouse(canvas, QEvent::MouseButtonRelease, {25, 240}, Qt::RightButton,
                          Qt::NoButton);
                } else {
                    QKeyEvent finishEdit(QEvent::KeyPress,
                                         finish == Finish::Cancel ? Qt::Key_Escape : Qt::Key_Return,
                                         finish == Finish::Cancel ? Qt::NoModifier
                                                                  : Qt::ControlModifier);
                    QApplication::sendEvent(&canvas, &finishEdit);
                }
                // A blank right click explicitly resets the serial tool and clears selection.
                const auto expectedSelection =
                    finish == Finish::RightClickCommit ? QJsonArray{} : selected;
                const bool selectionPreserved =
                    !canvas.hasActiveTextEditing() &&
                    runtime.selectedElementIds() == expectedSelection &&
                    second.canvasStyleToolbarState().selectedElementCount ==
                        expectedSelection.size();
                if (!selectionPreserved) {
                    std::cerr << "serial label completion: enabled=" << enabled
                              << ", excluded=" << excluded
                              << ", finish=" << static_cast<int>(finish)
                              << ", before=" << selected.size()
                              << ", after=" << runtime.selectedElementIds().size() << '\n';
                }
                require(selectionPreserved,
                        "label commits preserve selection and respect explicit tool resets");
                if (!expectedSelection.isEmpty())
                    require(canvas.canvasStyleToolbarState().source ==
                                SnowCanvasStyleToolbarSource::SelectedSerialNumber,
                            "label editing does not replace the selected badge with its label");
                require(canvas.setCanvasTool(SnowCanvasTool::Text),
                        "activate explicit label editing");
                mouse(canvas, QEvent::MouseButtonPress, {160, 120}, Qt::LeftButton, Qt::LeftButton);
                mouse(canvas, QEvent::MouseButtonRelease, {160, 120}, Qt::LeftButton, Qt::NoButton);
                if (!canvas.hasActiveTextEditing() || runtime.selectedElementIds().size() != 1) {
                    std::cerr << "serial label case: enabled=" << enabled
                              << ", excluded=" << excluded
                              << ", finish=" << static_cast<int>(finish) << '\n';
                }
                require(canvas.hasActiveTextEditing() && runtime.selectedElementIds().size() == 1,
                        "serial labels can still be explicitly selected for editing");
                const auto explicitlySelected = runtime.selectedElementIds();
                require(selected.isEmpty() || explicitlySelected != selected,
                        "explicit editing selects the label instead of its badge");
                QKeyEvent commit(QEvent::KeyPress, Qt::Key_Return, Qt::ControlModifier);
                QApplication::sendEvent(&canvas, &commit);
                require(!canvas.hasActiveTextEditing() &&
                            runtime.selectedElementIds() == explicitlySelected,
                        "manual label editing preserves its explicit selection");
            }
        }
    }
}

void speechBubbleSelectionPolicy() {
    enum class Finish { EmptyCommit, Commit, RightClickCommit, Cancel, SwitchTool, FocusLoss };
    for (const bool enabled : {false, true}) {
        for (const bool excluded : {false, true}) {
            for (const auto finish : {Finish::EmptyCommit, Finish::Commit, Finish::RightClickCommit,
                                      Finish::Cancel, Finish::SwitchTool, Finish::FocusLoss}) {
                SnowCanvasRuntimeConfig config;
                config.automaticallySelectNewlyCreatedElements = enabled;
                if (!excluded) {
                    config.automaticSelectionExcludedTools.remove(SnowCanvasTool::SpeechBubble);
                }
                config.quickSelectionDisabledTools.insert(SnowCanvasTool::SpeechBubble);
                SnowCanvasRuntime runtime(config);
                SnowCanvasWidget canvas(runtime);
                SnowCanvasWidget second(runtime);
                canvas.resize(400, 300);
                second.resize(400, 300);
                canvas.show();
                QApplication::processEvents();
                require(canvas.setCanvasTool(SnowCanvasTool::SpeechBubble), "activate bubble tool");
                mouse(canvas, QEvent::MouseButtonPress, {60, 60}, Qt::LeftButton, Qt::LeftButton);
                mouse(canvas, QEvent::MouseMove, {160, 120}, Qt::NoButton, Qt::LeftButton);
                mouse(canvas, QEvent::MouseButtonRelease, {160, 120}, Qt::LeftButton, Qt::NoButton);
                require(canvas.hasActiveTextEditing(),
                        "unselected bubbles still start text editing");
                const auto expectedCount = enabled && !excluded ? 1u : 0u;
                const auto verifySelection = [&] {
                    require(runtime.selectedElementIds().size() == expectedCount &&
                                canvas.canvasStyleToolbarState().selectedElementCount ==
                                    expectedCount &&
                                second.canvasStyleToolbarState().selectedElementCount ==
                                    expectedCount,
                            "bubble creation and text editing respect automatic selection in every "
                            "viewport");
                };
                verifySelection();
                if (finish != Finish::EmptyCommit) {
                    QKeyEvent type(QEvent::KeyPress, Qt::Key_A, Qt::NoModifier,
                                   QStringLiteral("bubble text"));
                    QApplication::sendEvent(&canvas, &type);
                }
                if (finish == Finish::SwitchTool) {
                    require(canvas.setCanvasTool(SnowCanvasTool::Select),
                            "commit by changing tool");
                } else if (finish == Finish::FocusLoss) {
                    QFocusEvent focusOut(QEvent::FocusOut, Qt::OtherFocusReason);
                    QApplication::sendEvent(&canvas, &focusOut);
                } else if (finish == Finish::RightClickCommit) {
                    mouse(canvas, QEvent::MouseButtonPress, {25, 240}, Qt::RightButton,
                          Qt::RightButton);
                    mouse(canvas, QEvent::MouseButtonRelease, {25, 240}, Qt::RightButton,
                          Qt::NoButton);
                } else {
                    QKeyEvent finishEdit(QEvent::KeyPress,
                                         finish == Finish::Cancel ? Qt::Key_Escape : Qt::Key_Return,
                                         finish == Finish::Cancel ? Qt::NoModifier
                                                                  : Qt::ControlModifier);
                    QApplication::sendEvent(&canvas, &finishEdit);
                }
                require(!canvas.hasActiveTextEditing(), "finish bubble text editing");
                verifySelection();
                require(canvas.canvasTool() == (finish == Finish::SwitchTool
                                                    ? SnowCanvasTool::Select
                                                    : SnowCanvasTool::SpeechBubble),
                        "bubble text editing retains the tool");
                if (!expectedCount) {
                    mouse(canvas, QEvent::MouseButtonDblClick, {110, 90}, Qt::LeftButton,
                          Qt::LeftButton);
                    require(canvas.hasActiveTextEditing() &&
                                runtime.selectedElementIds().size() == 1,
                            "excluded bubbles can still be selected and edited manually");
                    QKeyEvent commit(QEvent::KeyPress, Qt::Key_Return, Qt::ControlModifier);
                    QApplication::sendEvent(&canvas, &commit);
                    require(!canvas.hasActiveTextEditing() &&
                                runtime.selectedElementIds().size() == 1,
                            "manual editing preserves explicit selection regardless of the policy");
                }
            }
        }
    }
}
} // namespace

int main(int argc, char** argv) {
    QApplication app(argc, argv);
    if (app.arguments().contains(QStringLiteral("--text-dismiss-only"))) {
        outsideClickCommitsAndDeselects();
        return 0;
    }
    outsideClickCommitsAndDeselects();
    selectionAndRuntimeReplacement();
    automaticSelectionExclusions();
    selectedShapeHasWorkingHandles();
    textCommitRespectsDefaultExclusion();
    serialNumberLabelSelectionPolicy();
    speechBubbleSelectionPolicy();
    return 0;
}
