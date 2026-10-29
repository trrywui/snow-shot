#include "snow_draw_engine_qt/snow_canvas_runtime.h"
#include "snow_draw_engine_qt/snow_canvas_widget.h"
#include "snow_canvas_ffi_handles.h"
#include "snow_canvas_display_item.h"
#include "snow_canvas_commands.h"
#include "snow_canvas_runtime_access.h"
#include "snow_canvas_text.h"
#include "snow_canvas_text_layout.h"
#include "snow_canvas_text_measurement.h"
#include "snow_canvas_viewport.h"
#include "snow_canvas_renderer.h"
#include "snow_canvas_stroke_rendering.h"

#include <QApplication>
#include <QInputMethodEvent>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QKeyEvent>
#include <QLineF>
#include <QMouseEvent>
#include <QPainter>
#include <vector>

#include <cmath>
#include <cstdlib>
#include <iostream>

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(1);
    }
}

void mouse(SnowCanvasWidget& canvas, QEvent::Type type, QPointF point, Qt::MouseButton button,
           Qt::MouseButtons buttons) {
    QMouseEvent event(type, point, point, point, button, buttons, Qt::NoModifier);
    QApplication::sendEvent(&canvas, &event);
}

void key(SnowCanvasWidget& canvas, int code, Qt::KeyboardModifiers modifiers = Qt::NoModifier,
         const QString& text = {}) {
    QKeyEvent event(QEvent::KeyPress, code, modifiers, text);
    QApplication::sendEvent(&canvas, &event);
}

QJsonArray records(const SnowCanvasRuntime& runtime, const QString& kind) {
    const auto elements = QJsonDocument::fromJson(runtime.serializeDocumentSession())
                              .object()
                              .value(QStringLiteral("document"))
                              .toObject()
                              .value(QStringLiteral("slots"))
                              .toArray();
    QJsonArray result;
    for (const auto& slot : elements) {
        if (slot.toObject().value(QStringLiteral("data")).toObject().contains(kind)) {
            result.append(slot);
        }
    }
    return result;
}

QJsonObject payload(const SnowCanvasRuntime& runtime, const QString& kind) {
    const auto found = records(runtime, kind);
    require(found.size() == 1, "expected exactly one element of requested kind");
    return found[0].toObject().value(QStringLiteral("data")).toObject().value(kind).toObject();
}

QPointF point(const QJsonObject& object) {
    return {object.value(QStringLiteral("x")).toDouble(),
            object.value(QStringLiteral("y")).toDouble()};
}

void requirePreservedTailGeometry(const QJsonObject& before, const QJsonObject& after) {
    const auto canvasTip = [](const QJsonObject& bubble, const QJsonObject& tail) {
        const auto body = bubble.value(QStringLiteral("body")).toObject();
        const auto center = point(body.value(QStringLiteral("center")).toObject());
        const auto tip = point(tail.value(QStringLiteral("tip")).toObject());
        const double rotation = body.value(QStringLiteral("rotation")).toDouble();
        return center + QPointF(tip.x() * std::cos(rotation) - tip.y() * std::sin(rotation),
                                tip.x() * std::sin(rotation) + tip.y() * std::cos(rotation));
    };
    const auto originalTails = before.value(QStringLiteral("tails")).toArray();
    const auto updatedTails = after.value(QStringLiteral("tails")).toArray();
    require(originalTails.size() == updatedTails.size(), "shape switches preserve every tail");
    for (qsizetype index = 0; index < originalTails.size(); ++index) {
        const auto original = originalTails[index].toObject();
        const auto updated = updatedTails[index].toObject();
        require(original.value(QStringLiteral("id")) == updated.value(QStringLiteral("id")) &&
                    original.value(QStringLiteral("base_width")) ==
                        updated.value(QStringLiteral("base_width")) &&
                    QLineF(canvasTip(before, original), canvasTip(after, updated)).length() < 1e-6,
                "shape switches and text reflow preserve tail IDs, canvas endpoints and widths");
    }
}

QPointF centerView(const SnowCanvasWidget& canvas, const QJsonObject& body) {
    return canvas.canvasToViewTransform().map(
        point(body.value(QStringLiteral("center")).toObject()));
}

QPointF addHandle(const SnowCanvasWidget& canvas, const SnowCanvasRuntime& runtime) {
    const auto body =
        payload(runtime, QStringLiteral("SpeechBubble")).value(QStringLiteral("body")).toObject();
    const auto center = centerView(canvas, body);
    const double angle = body.value(QStringLiteral("rotation")).toDouble();
    const double zoom =
        std::hypot(canvas.canvasToViewTransform().m11(), canvas.canvasToViewTransform().m12());
    const double distance = body.value(QStringLiteral("height")).toDouble() * zoom / 2.0 + 24.0;
    return center + QPointF(-std::sin(angle) * distance, std::cos(angle) * distance);
}

void prepare(SnowCanvasWidget& canvas) {
    canvas.resize(800, 700);
    canvas.show();
    QApplication::processEvents();
    require(canvas.setViewportCamera(0, 0, 1), "set deterministic camera");
}

void create(SnowCanvasWidget& canvas, SnowCanvasRuntime& runtime,
            SnowCanvasRectangleShape shape = SnowCanvasRectangleShape::Rectangle,
            QPointF start = {130, 100}, QPointF end = {360, 160},
            SnowCanvasStrokeRenderingStyle rendering = SnowCanvasStrokeRenderingStyle::Architect) {
    require(runtime.setAutomaticallySelectNewlyCreatedElements(true) &&
                runtime.setAutomaticSelectionExcludedTools({}),
            "enable selection for bubble editing and handle tests");
    require(canvas.setCanvasTool(SnowCanvasTool::SpeechBubble), "activate speech bubble");
    SnowCanvasShapeStyle style;
    style.shape = shape;
    style.strokeRenderingStyle = rendering;
    require(canvas.setCanvasShapeStylePatch(style,
                                            SnowCanvasShapeStylePropertyShape |
                                                SnowCanvasShapeStylePropertyStrokeRenderingStyle,
                                            SnowCanvasShapeKind::SpeechBubble),
            "set bubble shape");
    mouse(canvas, QEvent::MouseButtonPress, start, Qt::LeftButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseMove, end - QPointF(8, 6), Qt::NoButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseButtonRelease, end, Qt::LeftButton, Qt::NoButton);
    require(records(runtime, QStringLiteral("SpeechBubble")).size() == 1, "drag creates bubble");
    const QString expectedShape =
        shape == SnowCanvasRectangleShape::ThoughtBubble ? QStringLiteral("ThoughtBubble")
        : shape == SnowCanvasRectangleShape::Ellipse     ? QStringLiteral("Ellipse")
        : shape == SnowCanvasRectangleShape::Diamond     ? QStringLiteral("Diamond")
                                                         : QStringLiteral("Rectangle");
    require(payload(runtime, QStringLiteral("SpeechBubble"))
                    .value(QStringLiteral("body"))
                    .toObject()
                    .value(QStringLiteral("highlight_shape"))
                    .toString() == expectedShape,
            "creation uses the requested bubble shape");
    require(canvas.hasActiveTextEditing(), "creation immediately starts text editing");
    require(canvas.canvasStyleToolbarState().source == SnowCanvasStyleToolbarSource::SelectedText,
            "new bubble draft exposes text controls");
    require(runtime.selectedElementIds().size() == 1, "new bubble is selected");
}

void commit(SnowCanvasWidget& canvas) {
    key(canvas, Qt::Key_Return, Qt::ControlModifier);
    require(!canvas.hasActiveTextEditing(), "text commits on control-enter");
    require(canvas.canvasStyleToolbarState().source ==
                SnowCanvasStyleToolbarSource::SelectedSpeechBubble,
            "committed bubble exposes appearance controls");
}

void open(SnowCanvasWidget& canvas, const SnowCanvasRuntime& runtime) {
    const auto body =
        payload(runtime, QStringLiteral("SpeechBubble")).value(QStringLiteral("body")).toObject();
    mouse(canvas, QEvent::MouseButtonDblClick, centerView(canvas, body), Qt::LeftButton,
          Qt::LeftButton);
    require(canvas.hasActiveTextEditing(), "double-click edits bubble text");
}

void textAlignmentDefaultsToCenter() {
    for (auto shape :
         {SnowCanvasRectangleShape::Rectangle, SnowCanvasRectangleShape::Ellipse,
          SnowCanvasRectangleShape::Diamond, SnowCanvasRectangleShape::ThoughtBubble}) {
        SnowCanvasRuntime runtime;
        SnowCanvasWidget canvas(runtime);
        prepare(canvas);
        SnowCanvasTextStyle defaults;
        defaults.horizontalAlign = SnowCanvasTextHorizontalAlign::Right;
        defaults.verticalAlign = SnowCanvasTextVerticalAlign::Bottom;
        defaults.fontSize = 23;
        defaults.bold = true;
        require(canvas.setCanvasTextStyle(defaults), "set general text defaults");
        defaults = canvas.canvasStyleToolbarState().textStyle;
        create(canvas, runtime, shape);
        auto expected = defaults;
        expected.horizontalAlign = SnowCanvasTextHorizontalAlign::Center;
        expected.verticalAlign = SnowCanvasTextVerticalAlign::Center;
        require(canvas.canvasStyleToolbarState().textStyle == expected,
                "new bubble draft centers text while inheriting other text defaults");
        key(canvas, Qt::Key_A, Qt::NoModifier, QStringLiteral("centered bubble"));
        commit(canvas);
        const auto text = payload(runtime, QStringLiteral("Text"));
        require(
            text.value(QStringLiteral("horizontal_align")).toString() == QStringLiteral("Center") &&
                text.value(QStringLiteral("vertical_align")).toString() == QStringLiteral("Center"),
            "committed bubble text uses centered alignment");
        require(canvas.canvasStyleToolbarState().textStyle == defaults,
                "creating bubble text preserves general text defaults");
        open(canvas, runtime);
        require(canvas.canvasStyleToolbarState().textStyle == expected,
                "reopened bubble text keeps centered alignment");
        require(canvas.setCanvasTextStyle(defaults, SnowCanvasTextStyleMixedHorizontalAlign |
                                                        SnowCanvasTextStyleMixedVerticalAlign),
                "change bubble text alignment explicitly");
        commit(canvas);
        open(canvas, runtime);
        require(canvas.canvasStyleToolbarState().textStyle == defaults,
                "reopening bubble text preserves its explicit alignment");
        key(canvas, Qt::Key_Escape);
    }
}

void wrappingGrowthAndCancellation() {
    for (auto shape :
         {SnowCanvasRectangleShape::Rectangle, SnowCanvasRectangleShape::Ellipse,
          SnowCanvasRectangleShape::Diamond, SnowCanvasRectangleShape::ThoughtBubble}) {
        SnowCanvasRuntime runtime;
        SnowCanvasWidget canvas(runtime);
        prepare(canvas);
        create(canvas, runtime, shape, {130, 100}, {360, 160},
               SnowCanvasStrokeRenderingStyle::Artist);
        const auto original = payload(runtime, QStringLiteral("SpeechBubble"));
        require(original.value(QStringLiteral("tails")).toArray().isEmpty(), "no default tail");
        const auto originalBody = original.value(QStringLiteral("body")).toObject();
        const double top = originalBody.value(QStringLiteral("center"))
                               .toObject()
                               .value(QStringLiteral("y"))
                               .toDouble() -
                           originalBody.value(QStringLiteral("height")).toDouble() / 2.0;
        const QString text = QString::fromUtf8("中文👩‍💻 é wrapping ").repeated(12);
        key(canvas, Qt::Key_A, Qt::NoModifier, text);
        require(payload(runtime, QStringLiteral("SpeechBubble")) == original,
                "live growth must not change committed bubble");
        commit(canvas);
        const auto grown = payload(runtime, QStringLiteral("SpeechBubble"));
        const auto body = grown.value(QStringLiteral("body")).toObject();
        const auto label = payload(runtime, QStringLiteral("Text"));
        require(label.value(QStringLiteral("text")).toString() == text,
                "bound text preserves Unicode");
        require(body.value(QStringLiteral("height")).toDouble() >
                    originalBody.value(QStringLiteral("height")).toDouble(),
                "text grows short body");
        const double finalTop = point(body.value(QStringLiteral("center")).toObject()).y() -
                                body.value(QStringLiteral("height")).toDouble() / 2.0;
        require(std::abs(finalTop - top) < 1e-7, "automatic height keeps top fixed");
        require(label.value(QStringLiteral("content_height")).toDouble() <=
                    label.value(QStringLiteral("height")).toDouble() + 1e-7,
                "all wrapped lines fit text container");
        require(canvas.undo(), "undo committed label and growth");
        require(payload(runtime, QStringLiteral("SpeechBubble")) == original &&
                    records(runtime, QStringLiteral("Text")).isEmpty(),
                "label binding and growth undo together");
        require(canvas.redo(), "redo bubble label");
        open(canvas, runtime);
        key(canvas, Qt::Key_A, Qt::ControlModifier);
        key(canvas, Qt::Key_X, Qt::NoModifier, QStringLiteral("x"));
        commit(canvas);
        require(payload(runtime, QStringLiteral("SpeechBubble")).value(QStringLiteral("body")) ==
                    grown.value(QStringLiteral("body")),
                "shorter text never auto-shrinks body");
        const auto beforeCancel = payload(runtime, QStringLiteral("SpeechBubble"));
        open(canvas, runtime);
        key(canvas, Qt::Key_A, Qt::NoModifier, QStringLiteral("long text ").repeated(300));
        require(canvas.cancelActiveTextEditing(), "explicit cancellation succeeds");
        require(payload(runtime, QStringLiteral("SpeechBubble")) == beforeCancel &&
                    payload(runtime, QStringLiteral("Text")).value(QStringLiteral("text")) ==
                        QStringLiteral("x"),
                "cancellation restores body and label");
    }
}

QPointF tailTipCanvas(const QJsonObject& bubble) {
    const auto body = bubble.value(QStringLiteral("body")).toObject();
    const auto tails = bubble.value(QStringLiteral("tails")).toArray();
    require(tails.size() == 1, "fixed tip fixture retains one tail");
    const auto tip = point(tails[0].toObject().value(QStringLiteral("tip")).toObject());
    const double angle = body.value(QStringLiteral("rotation")).toDouble();
    return point(body.value(QStringLiteral("center")).toObject()) +
           QPointF(tip.x() * std::cos(angle) - tip.y() * std::sin(angle),
                   tip.x() * std::sin(angle) + tip.y() * std::cos(angle));
}

void bodyMovementAndRotationKeepTailTipFixed() {
    for (auto shape :
         {SnowCanvasRectangleShape::Rectangle, SnowCanvasRectangleShape::Ellipse,
          SnowCanvasRectangleShape::Diamond, SnowCanvasRectangleShape::ThoughtBubble}) {
        SnowCanvasRuntime runtime;
        SnowCanvasWidget canvas(runtime);
        prepare(canvas);
        create(canvas, runtime, shape, {130, 100}, {360, 260});
        key(canvas, Qt::Key_A, Qt::NoModifier, QStringLiteral("anchor"));
        commit(canvas);
        const auto add = addHandle(canvas, runtime);
        mouse(canvas, QEvent::MouseButtonPress, add, Qt::LeftButton, Qt::LeftButton);
        mouse(canvas, QEvent::MouseButtonRelease, add + QPointF(180, 180), Qt::LeftButton,
              Qt::NoButton);
        const auto original = payload(runtime, QStringLiteral("SpeechBubble"));
        const auto tip = tailTipCanvas(original);
        for (bool rotate : {false, true}) {
            const auto before = payload(runtime, QStringLiteral("SpeechBubble"));
            const auto body = before.value(QStringLiteral("body")).toObject();
            const auto center = centerView(canvas, body);
            const auto start =
                rotate
                    ? center * 2.0 - addHandle(canvas, runtime)
                    : center - QPointF(0, body.value(QStringLiteral("height")).toDouble() * 0.35);
            const auto target =
                rotate ? center + QPointF(-(start.y() - center.y()), start.x() - center.x())
                       : start - QPointF(35, 25);
            mouse(canvas, QEvent::MouseButtonPress, start, Qt::LeftButton, Qt::LeftButton);
            mouse(canvas, QEvent::MouseMove, (start + target) / 2.0, Qt::NoButton, Qt::LeftButton);
            mouse(canvas, QEvent::MouseButtonRelease, target, Qt::LeftButton, Qt::NoButton);
            const auto after = payload(runtime, QStringLiteral("SpeechBubble"));
            require(after.value(QStringLiteral("body")) != before.value(QStringLiteral("body")),
                    "body gesture changes the bubble");
            require(QLineF(tailTipCanvas(after), tip).length() < 1e-6,
                    "moving and rotating the body keeps the canvas tail tip fixed");
            require(canvas.undo() && payload(runtime, QStringLiteral("SpeechBubble")) == before,
                    "undo restores body and fixed tail");
            require(canvas.redo() && payload(runtime, QStringLiteral("SpeechBubble")) == after,
                    "redo restores body and fixed tail");
        }
        const auto session = runtime.serializeDocumentSession();
        require(runtime.restoreDocumentSession(session), "restore fixed tail session");
        require(
            QLineF(tailTipCanvas(payload(runtime, QStringLiteral("SpeechBubble"))), tip).length() <
                1e-6,
            "fixed canvas tail tip survives session restoration");
    }
}

void tailGesturesAndLifecycle() {
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    prepare(canvas);
    create(canvas, runtime, SnowCanvasRectangleShape::Rectangle, {130, 100}, {360, 260},
           SnowCanvasStrokeRenderingStyle::Cartoonist);
    key(canvas, Qt::Key_A, Qt::NoModifier, QStringLiteral("bubble"));
    commit(canvas);
    const QPointF handle = addHandle(canvas, runtime);
    mouse(canvas, QEvent::MouseButtonPress, handle, Qt::LeftButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseMove, handle + QPointF(70, 100), Qt::NoButton, Qt::LeftButton);
    const QPointF final = handle + QPointF(85, 120);
    mouse(canvas, QEvent::MouseButtonRelease, final, Qt::LeftButton, Qt::NoButton);
    auto bubble = payload(runtime, QStringLiteral("SpeechBubble"));
    require(bubble.value(QStringLiteral("tails")).toArray().size() == 1,
            "add handle creates and drags tail in one gesture");
    const auto tail = bubble.value(QStringLiteral("tails")).toArray()[0].toObject();
    const auto body = bubble.value(QStringLiteral("body")).toObject();
    const QPointF tipCanvas = point(body.value(QStringLiteral("center")).toObject()) +
                              point(tail.value(QStringLiteral("tip")).toObject());
    require(QLineF(canvas.canvasToViewTransform().map(tipCanvas), final).length() < 1e-6,
            "tail includes final release coordinate");
    require(canvas.undo() && payload(runtime, QStringLiteral("SpeechBubble"))
                                 .value(QStringLiteral("tails"))
                                 .toArray()
                                 .isEmpty(),
            "one undo removes add-and-drag gesture");
    require(canvas.redo(), "redo tail");
    mouse(canvas, QEvent::MouseButtonPress, final, Qt::LeftButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseMove, centerView(canvas, body), Qt::NoButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseButtonRelease, centerView(canvas, body), Qt::LeftButton,
          Qt::NoButton);
    require(payload(runtime, QStringLiteral("SpeechBubble"))
                .value(QStringLiteral("tails"))
                .toArray()
                .isEmpty(),
            "tip inside body deletes tail");
    require(canvas.undo(), "undo tail deletion");
    for (int index = 0; index < 2; ++index) {
        const auto add = addHandle(canvas, runtime);
        mouse(canvas, QEvent::MouseButtonPress, add, Qt::LeftButton, Qt::LeftButton);
        mouse(canvas, QEvent::MouseButtonRelease, add, Qt::LeftButton, Qt::NoButton);
    }
    require(payload(runtime, QStringLiteral("SpeechBubble"))
                    .value(QStringLiteral("tails"))
                    .toArray()
                    .size() == 3,
            "repeated click additions remain reachable");
    SnowCanvasViewport observer;
    const auto engine = snow_canvas_runtime::Access::handle(runtime);
    require(observer.create(engine, snow_canvas_viewport::defaultEngineConfig()),
            "create observer");
    require(snow_viewport_set_surface_size(engine, observer.get(), 800, 700) == SNOW_OK,
            "size observer surface");
    const auto ownerId = records(runtime, QStringLiteral("SpeechBubble"))[0]
                             .toObject()
                             .value(QStringLiteral("id"))
                             .toObject();
    require(
        snow_canvas_commands::selectElement(
            engine, observer.get(),
            SnowElementId{
                static_cast<std::uint32_t>(ownerId.value(QStringLiteral("index")).toInt()),
                static_cast<std::uint32_t>(ownerId.value(QStringLiteral("generation")).toInt())})
            .success,
        "select bubble in observer viewport");
    ScopedPatchHandle patch;
    require(snow_viewport_acquire_patch(engine, observer.get(), nullptr, patch.outParam()) ==
                SNOW_OK,
            "read bubble control overlay");
    const SnowOverlayDisplayItem* overlay = nullptr;
    std::uint32_t count = 0;
    require(snow_patch_get_overlay_items(patch.get(), &overlay, &count) == SNOW_OK,
            "read controls");
    int tipHandles = 0;
    int baseHandles = 0;
    for (std::uint32_t index = 0; index < count; ++index) {
        tipHandles += overlay[index].rect_kind == SNOW_OVERLAY_RECT_SPEECH_BUBBLE_TAIL_TIP_HANDLE;
        baseHandles += overlay[index].rect_kind == SNOW_OVERLAY_RECT_SPEECH_BUBBLE_TAIL_BASE_HANDLE;
    }
    require(tipHandles == 3 && baseHandles == 6, "each tail has three controls");
    const auto session = runtime.serializeDocumentSession();
    SnowCanvasRuntime restored;
    require(restored.restoreDocumentSession(session), "restore bubble session");
    require(payload(restored, QStringLiteral("SpeechBubble")) ==
                payload(runtime, QStringLiteral("SpeechBubble")),
            "tails and label links survive restore");
    const auto originalBody =
        payload(runtime, QStringLiteral("SpeechBubble")).value(QStringLiteral("body")).toObject();
    require(originalBody.value(QStringLiteral("stroke_rendering_style")) ==
                QStringLiteral("cartoonist"),
            "tail edits preserve hand-drawn style");
    const auto templateBytes = runtime.serializeSelectedDrawTemplate();
    require(QJsonDocument::fromJson(templateBytes)
                    .object()
                    .value(QStringLiteral("schemaVersion"))
                    .toInt() == 7,
            "bubble draw templates use the current binding schema");
    SnowCanvasRuntime templated;
    SnowCanvasWidget templateCanvas(templated);
    prepare(templateCanvas);
    require(templateCanvas.insertDrawTemplate(templateBytes, QPointF(20, 30)),
            "insert bubble draw template");
    const auto inserted = payload(templated, QStringLiteral("SpeechBubble"));
    const auto insertedBody = inserted.value(QStringLiteral("body")).toObject();
    require(insertedBody.value(QStringLiteral("stroke_rendering_style")) ==
                    originalBody.value(QStringLiteral("stroke_rendering_style")) &&
                insertedBody.value(QStringLiteral("stroke_rendering_seed")) ==
                    originalBody.value(QStringLiteral("stroke_rendering_seed")),
            "bubble templates preserve style and stable seed");
    require(records(templated, QStringLiteral("Text")).size() == 1 &&
                inserted.value(QStringLiteral("tails")) ==
                    payload(runtime, QStringLiteral("SpeechBubble")).value(QStringLiteral("tails")),
            "template insertion retains local tails and owned text");
    require(templateCanvas.undo() && records(templated, QStringLiteral("SpeechBubble")).isEmpty() &&
                records(templated, QStringLiteral("Text")).isEmpty(),
            "template body and text undo together");
    require(templateCanvas.redo(), "redo bubble template");
    require(canvas.duplicateSelected(), "duplicate bound bubble");
    for (const auto& record : records(runtime, QStringLiteral("SpeechBubble"))) {
        const auto duplicatedBody = record.toObject()
                                        .value(QStringLiteral("data"))
                                        .toObject()
                                        .value(QStringLiteral("SpeechBubble"))
                                        .toObject()
                                        .value(QStringLiteral("body"))
                                        .toObject();
        require(duplicatedBody.value(QStringLiteral("stroke_rendering_style")) ==
                        originalBody.value(QStringLiteral("stroke_rendering_style")) &&
                    duplicatedBody.value(QStringLiteral("stroke_rendering_seed")) ==
                        originalBody.value(QStringLiteral("stroke_rendering_seed")),
                "bubble duplication preserves exact style and seed");
    }
    require(records(runtime, QStringLiteral("SpeechBubble")).size() == 2 &&
                records(runtime, QStringLiteral("Text")).size() == 2,
            "duplicate body and label together");
    require(canvas.deleteSelected(), "delete duplicate");
    require(records(runtime, QStringLiteral("SpeechBubble")).size() == 1 &&
                records(runtime, QStringLiteral("Text")).size() == 1,
            "delete bound label with owner");
}

void handDrawnStylesReachEveryBubbleContourAndExport() {
    for (auto shape :
         {SnowCanvasRectangleShape::Rectangle, SnowCanvasRectangleShape::Ellipse,
          SnowCanvasRectangleShape::Diamond, SnowCanvasRectangleShape::ThoughtBubble}) {
        SnowCanvasRuntime runtime;
        SnowCanvasWidget canvas(runtime);
        prepare(canvas);
        create(canvas, runtime, shape, {230, 180}, {470, 330},
               SnowCanvasStrokeRenderingStyle::Cartoonist);
        key(canvas, Qt::Key_A, Qt::NoModifier, QStringLiteral("styled bubble"));
        commit(canvas);
        for (const QPointF offset : {QPointF(-60, 80), QPointF(80, 95)}) {
            const auto handle = addHandle(canvas, runtime);
            mouse(canvas, QEvent::MouseButtonPress, handle, Qt::LeftButton, Qt::LeftButton);
            mouse(canvas, QEvent::MouseButtonRelease, handle + offset, Qt::LeftButton,
                  Qt::NoButton);
        }
        SnowCanvasShapeStyle fill;
        fill.fill = QColor(30, 80, 220, 128);
        fill.fillStyle = SnowCanvasFillStyle::CrossLine;
        require(canvas.setCanvasShapeStylePatch(fill,
                                                SnowCanvasShapeStylePropertyFillColor |
                                                    SnowCanvasShapeStylePropertyFillStyle,
                                                SnowCanvasShapeKind::SpeechBubble),
                "give every styled bubble a translucent patterned fill");
        const auto engine = snow_canvas_runtime::Access::handle(runtime);
        SnowCanvasViewport viewport;
        require(viewport.create(engine, snow_canvas_viewport::defaultEngineConfig()) &&
                    snow_viewport_set_surface_size(engine, viewport.get(), 800, 700) == SNOW_OK,
                "create styled bubble scene observer");
        SnowCanvasDisplayCache cache;
        QImage previous;
        const auto seed = payload(runtime, QStringLiteral("SpeechBubble"))
                              .value(QStringLiteral("body"))
                              .toObject()
                              .value(QStringLiteral("stroke_rendering_seed"));
        for (auto rendering :
             {SnowCanvasStrokeRenderingStyle::Artist, SnowCanvasStrokeRenderingStyle::Cartoonist,
              SnowCanvasStrokeRenderingStyle::Architect}) {
            SnowCanvasShapeStyle style;
            style.strokeRenderingStyle = rendering;
            const auto before = payload(runtime, QStringLiteral("SpeechBubble"));
            require(canvas.setCanvasShapeStylePatch(
                        style, SnowCanvasShapeStylePropertyStrokeRenderingStyle,
                        SnowCanvasShapeKind::SpeechBubble),
                    "edit bubble rendering style");
            const auto after = payload(runtime, QStringLiteral("SpeechBubble"));
            require(after.value(QStringLiteral("body"))
                                .toObject()
                                .value(QStringLiteral("stroke_rendering_seed")) == seed &&
                        after.value(QStringLiteral("tails")) ==
                            before.value(QStringLiteral("tails")),
                    "style edits preserve bubble seed and all tails");
            require(canvas.undo() && payload(runtime, QStringLiteral("SpeechBubble")) == before &&
                        canvas.redo() && payload(runtime, QStringLiteral("SpeechBubble")) == after,
                    "rendering style edits undo and redo in one step");
            for (auto pattern : {SnowCanvasStrokeStyle::Solid, SnowCanvasStrokeStyle::Dashed,
                                 SnowCanvasStrokeStyle::Dotted}) {
                style.strokeStyle = pattern;
                require(canvas.setCanvasShapeStylePatch(style,
                                                        SnowCanvasShapeStylePropertyStrokeStyle,
                                                        SnowCanvasShapeKind::SpeechBubble),
                        "set styled bubble pattern");
                const QRectF area(-400, -350, 800, 700);
                const QImage image = runtime.renderToImage(area, QSize(800, 700), {});
                require(!image.isNull() &&
                            image == runtime.renderToImage(area, QSize(800, 700), {}),
                        "each bubble style and pattern exports deterministically");
                require(cache.sync(engine, viewport.get()), "sync styled bubble scene");
                QImage scene(image.size(), image.format());
                scene.fill(Qt::transparent);
                QPainter painter(&scene);
                painter.setRenderHint(QPainter::Antialiasing);
                // Match the export painter's sampling of textured fills.
                painter.setRenderHint(QPainter::SmoothPixmapTransform);
                snow_canvas_renderer::SceneRenderRequest request;
                request.painter = &painter;
                request.displayInfo = &cache.sceneInfo();
                request.sceneItems = cache.sceneItems();
                request.sceneItemCount = cache.sceneItemCount();
                request.exposedRegion = QRegion(scene.rect());
                snow_canvas_renderer::renderSceneItems(request);
                painter.end();
                require(scene == image,
                        "bubble viewport scene and export use identical styled contours");
                if (pattern == SnowCanvasStrokeStyle::Solid) {
                    require(previous.isNull() || previous != image,
                            "all bubble rendering styles have distinct ink");
                    previous = image;
                    if (rendering != SnowCanvasStrokeRenderingStyle::Architect) {
                        const auto originalStroke =
                            canvas.canvasStyleToolbarState().shapeStyle.stroke;
                        style.stroke = QColor(210, 30, 50);
                        snow_canvas_stroke_rendering::takeDiagnostics();
                        require(canvas.setCanvasShapeStylePatch(
                                    style, SnowCanvasShapeStylePropertyStrokeColor,
                                    SnowCanvasShapeKind::SpeechBubble) &&
                                    cache.sync(engine, viewport.get()),
                                "update bubble color through the runtime");
                        const auto touchInk = [&cache] {
                            for (std::uint32_t i = 0; i < cache.sceneItemCount(); ++i)
                                if (cache.sceneItems()[i].kind ==
                                    SNOW_SCENE_DISPLAY_ITEM_SPEECH_BUBBLE)
                                    static_cast<void>(snow_canvas_stroke_rendering::geometry(
                                        cache.sceneItems()[i]));
                        };
                        touchInk();
                        require(canvas.setSelectedOpacity(0.5) &&
                                    cache.sync(engine, viewport.get()),
                                "update bubble opacity through the runtime");
                        touchInk();
                        const auto warm = snow_canvas_stroke_rendering::takeDiagnostics();
                        require(warm.geometryBuilds == 0 && warm.strokeBuilds == 0,
                                "runtime material patches retain cached bubble ink");
                        style.stroke = originalStroke;
                        require(canvas.setCanvasShapeStylePatch(
                                    style, SnowCanvasShapeStylePropertyStrokeColor,
                                    SnowCanvasShapeKind::SpeechBubble) &&
                                    canvas.setSelectedOpacity(1.0),
                                "restore bubble material after cache check");
                    }
                }
            }
        }
    }
}

void patternedAndTransparentTailExports() {
    for (auto shape :
         {SnowCanvasRectangleShape::Rectangle, SnowCanvasRectangleShape::Ellipse,
          SnowCanvasRectangleShape::Diamond, SnowCanvasRectangleShape::ThoughtBubble}) {
        for (auto fillStyle : {SnowCanvasFillStyle::Border, SnowCanvasFillStyle::Solid,
                               SnowCanvasFillStyle::Line, SnowCanvasFillStyle::CrossLine}) {
            SnowCanvasRuntime runtime;
            SnowCanvasWidget canvas(runtime);
            prepare(canvas);
            create(canvas, runtime, shape, {160, 170}, {400, 340});
            require(canvas.cancelActiveTextEditing(), "cancel the empty initial draft");
            SnowCanvasShapeStyle style;
            style.fill = QColor(220, 30, 30);
            style.stroke = Qt::black;
            style.fillStyle = fillStyle;
            require(canvas.setCanvasShapeStylePatch(style,
                                                    SnowCanvasShapeStylePropertyFillColor |
                                                        SnowCanvasShapeStylePropertyFillStyle |
                                                        SnowCanvasShapeStylePropertyStrokeColor,
                                                    SnowCanvasShapeKind::SpeechBubble),
                    "configure bubble fill export");
            const auto handle = addHandle(canvas, runtime);
            const auto tip = handle + QPointF(60, 100);
            mouse(canvas, QEvent::MouseButtonPress, handle, Qt::LeftButton, Qt::LeftButton);
            mouse(canvas, QEvent::MouseButtonRelease, tip, Qt::LeftButton, Qt::NoButton);
            auto image = runtime.renderToImage(QRectF(-400, -350, 800, 700), QSize(800, 700), {});
            require(!image.isNull(), "export styled bubble with tail");
            bool tailPainted = false;
            const QPoint sample =
                shape == SnowCanvasRectangleShape::ThoughtBubble ? tip.toPoint() : QPoint(310, 395);
            for (int y = sample.y() - 20; y <= sample.y() + 20; ++y) {
                for (int x = sample.x() - 20; x <= sample.x() + 20; ++x) {
                    tailPainted |= image.pixelColor(x, y).alpha() != 0;
                }
            }
            require(tailPainted, "export includes tail-only bounds for each fill pattern");
            for (int y = 0; y < image.height(); ++y) {
                for (int x = 0; x < image.width(); ++x) {
                    const auto pixel = image.pixelColor(x, y);
                    require(pixel.alpha() == 0 || pixel.blue() <= pixel.red() + 20,
                            "annotation handles are excluded from export");
                }
            }
            style.fill = Qt::transparent;
            require(canvas.setCanvasShapeStylePatch(style, SnowCanvasShapeStylePropertyFillColor,
                                                    SnowCanvasShapeKind::SpeechBubble),
                    "configure transparent bubble fill");
            image = runtime.renderToImage(QRectF(-400, -350, 800, 700), QSize(800, 700), {});
            require(image.pixelColor(280, 255).alpha() == 0,
                    "transparent body fill leaves the center unpainted");
            require(canvas.setCanvasTool(SnowCanvasTool::Select), "switch to selection");
            mouse(canvas, QEvent::MouseButtonPress, {720, 650}, Qt::LeftButton, Qt::LeftButton);
            mouse(canvas, QEvent::MouseButtonRelease, {720, 650}, Qt::LeftButton, Qt::NoButton);
            mouse(canvas, QEvent::MouseButtonDblClick, sample, Qt::LeftButton, Qt::LeftButton);
            require(canvas.hasActiveTextEditing(), "tail-only double-click opens bound editing");
            require(canvas.cancelActiveTextEditing(), "cancel tail text edit");
        }
    }
}

void graphemeCacheAndNarrowText() {
    SnowTextElementInfo info{};
    info.font_size = 30;
    auto item = snow_canvas_text::defaultPreviewItem(info);
    item.text_italic = 1;
    snow_canvas_text_measurement::BubbleTextLayoutCache cache(8192);
    const QString text = QString::fromUtf8("漢👩‍💻éfj");
    const auto glyphs = cache.graphemes(text, QFont(), item);
    require(glyphs.minimumWidth > 1 && glyphs.inkReserve > 0,
            "measure shaped clusters and reserves");
    cache.wrapped(text, QFont(), item, glyphs.minimumWidth);
    const auto measured = cache.measurementCount();
    item.center_x += 100;
    item.height += 100;
    cache.graphemes(text, QFont(), item);
    cache.wrapped(text, QFont(), item, glyphs.minimumWidth);
    require(cache.measurementCount() == measured,
            "geometry unrelated to width reuses text metrics");
    for (int index = 0; index < 100; ++index) {
        cache.graphemes(text + QString::number(index), QFont(), item);
    }
    require(cache.retainedBytes() <= 8192, "bubble metric caches obey memory budget");
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    prepare(canvas);
    create(canvas, runtime, SnowCanvasRectangleShape::Diamond, {100, 100}, {112, 150});
    key(canvas, Qt::Key_A, Qt::NoModifier, text);
    commit(canvas);
    const auto label = payload(runtime, QStringLiteral("Text"));
    item.font_size = label.value(QStringLiteral("font_size")).toDouble();
    item.text_italic = label.value(QStringLiteral("italic")).toBool() ? 1 : 0;
    item.text_bold = label.value(QStringLiteral("bold")).toBool() ? 1 : 0;
    item.setFontFamilyUtf8(label.value(QStringLiteral("font_family")).toString().toUtf8());
    const auto actualGlyphs = cache.graphemes(text, canvas.font(), item);
    require(label.value(QStringLiteral("width")).toDouble() >= actualGlyphs.minimumWidth - 1e-6,
            "minimum body width permits at least one grapheme column");
}

void imeAndRotatedGrowthAndExport() {
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    prepare(canvas);
    create(canvas, runtime, SnowCanvasRectangleShape::Ellipse, {160, 160}, {420, 240});
    QInputMethodEvent preedit(QString::fromUtf8("你好"), {});
    QApplication::sendEvent(&canvas, &preedit);
    require(records(runtime, QStringLiteral("Text")).isEmpty(), "IME preedit is presentation only");
    QInputMethodEvent input;
    input.setCommitString(QString::fromUtf8("你好世界"));
    QApplication::sendEvent(&canvas, &input);
    commit(canvas);
    require(payload(runtime, QStringLiteral("Text")).value(QStringLiteral("text")).toString() ==
                QString::fromUtf8("你好世界"),
            "IME commits complete Unicode text");

    auto saved = QJsonDocument::fromJson(runtime.serializeDocumentSession()).object();
    auto document = saved.value(QStringLiteral("document")).toObject();
    auto elements = document.value(QStringLiteral("slots")).toArray();
    constexpr double rotation = 0.6;
    for (qsizetype index = 0; index < elements.size(); ++index) {
        auto record = elements[index].toObject();
        auto data = record.value(QStringLiteral("data")).toObject();
        if (data.contains(QStringLiteral("SpeechBubble"))) {
            auto value = data.value(QStringLiteral("SpeechBubble")).toObject();
            auto body = value.value(QStringLiteral("body")).toObject();
            body[QStringLiteral("rotation")] = rotation;
            value[QStringLiteral("body")] = body;
            data[QStringLiteral("SpeechBubble")] = value;
        } else if (data.contains(QStringLiteral("Text"))) {
            auto value = data.value(QStringLiteral("Text")).toObject();
            value[QStringLiteral("rotation")] = rotation;
            data[QStringLiteral("Text")] = value;
        }
        record[QStringLiteral("data")] = data;
        elements[index] = record;
    }
    document[QStringLiteral("slots")] = elements;
    saved[QStringLiteral("document")] = document;
    SnowCanvasRuntime empty;
    saved[QStringLiteral("history")] = QJsonDocument::fromJson(empty.serializeDocumentSession())
                                           .object()
                                           .value(QStringLiteral("history"));
    require(runtime.restoreDocumentSession(QJsonDocument(saved).toJson()),
            "restore rotated bubble");
    const auto before =
        payload(runtime, QStringLiteral("SpeechBubble")).value(QStringLiteral("body")).toObject();
    const auto oldCenter = point(before.value(QStringLiteral("center")).toObject());
    const QPointF up(std::sin(rotation), -std::cos(rotation));
    const auto top = oldCenter + up * (before.value(QStringLiteral("height")).toDouble() / 2.0);
    open(canvas, runtime);
    SnowCanvasTextStyle style = canvas.canvasStyleToolbarState().textStyle;
    style.fontSize = 36;
    style.italic = true;
    require(canvas.setCanvasTextStyle(style, SnowCanvasTextStyleMixedFontSize |
                                                 SnowCanvasTextStyleMixedItalic),
            "style rotated text");
    key(canvas, Qt::Key_A, Qt::NoModifier,
        QString::fromUtf8("漢字 é 👩‍💻 ").repeated(40));
    commit(canvas);
    const auto grown =
        payload(runtime, QStringLiteral("SpeechBubble")).value(QStringLiteral("body")).toObject();
    const auto newTop = point(grown.value(QStringLiteral("center")).toObject()) +
                        up * (grown.value(QStringLiteral("height")).toDouble() / 2.0);
    require(QLineF(top, newTop).length() < 1e-6, "rotated growth preserves local top edge");
    require(
        std::abs(
            payload(runtime, QStringLiteral("Text")).value(QStringLiteral("rotation")).toDouble() -
            rotation) < 1e-9,
        "label follows bubble rotation");
    const auto history = runtime.serializeDocumentHistory();
    SnowCanvasRuntime restored;
    require(restored.restoreDocumentHistory(history), "restore bubble document history");
    const QRectF area(-500, -500, 1000, 2000);
    const auto image = runtime.renderToImage(area, QSize(500, 1000), {});
    const auto restoredImage = restored.renderToImage(area, QSize(500, 1000), {});
    require(!image.isNull() && image == restoredImage,
            "export is stable across history restoration");
    require(image != empty.renderToImage(area, QSize(500, 1000), {}),
            "export paints the bubble rather than only the background");
    bool painted = false;
    for (int y = 0; y < image.height() && !painted; ++y) {
        for (int x = 0; x < image.width(); ++x) {
            if (image.pixelColor(x, y).alpha() != 0) {
                painted = true;
                break;
            }
        }
    }
    require(painted, "bubble export contains visible shape and glyphs");
}

void manualResizeKeepsTextMinimums() {
    for (auto shape :
         {SnowCanvasRectangleShape::Rectangle, SnowCanvasRectangleShape::Ellipse,
          SnowCanvasRectangleShape::Diamond, SnowCanvasRectangleShape::ThoughtBubble}) {
        SnowCanvasRuntime runtime;
        SnowCanvasWidget canvas(runtime);
        prepare(canvas);
        create(canvas, runtime, shape, {160, 160}, {420, 260});
        const QString text = QString::fromUtf8("漢字 👩‍💻 é fj");
        key(canvas, Qt::Key_A, Qt::NoModifier, text);
        commit(canvas);
        const auto before = payload(runtime, QStringLiteral("SpeechBubble"))
                                .value(QStringLiteral("body"))
                                .toObject();
        const auto center = centerView(canvas, before);
        const double width = before.value(QStringLiteral("width")).toDouble();
        const double height = before.value(QStringLiteral("height")).toDouble();
        const auto handle = center + QPointF(width / 2.0 + 4.0, height / 2.0 + 4.0);
        const auto target = center - QPointF(width / 2.0, height / 2.0) + QPointF(5, 5);
        mouse(canvas, QEvent::MouseButtonPress, handle, Qt::LeftButton, Qt::LeftButton);
        mouse(canvas, QEvent::MouseMove, target + QPointF(10, 10), Qt::NoButton, Qt::LeftButton);
        mouse(canvas, QEvent::MouseButtonRelease, target, Qt::LeftButton, Qt::NoButton);
        const auto body = payload(runtime, QStringLiteral("SpeechBubble"))
                              .value(QStringLiteral("body"))
                              .toObject();
        const auto label = payload(runtime, QStringLiteral("Text"));
        require(body.value(QStringLiteral("width")).toDouble() < width,
                "resize changes body rather than moving selection");
        SnowTextElementInfo info{};
        info.font_size = label.value(QStringLiteral("font_size")).toDouble();
        auto item = snow_canvas_text::defaultPreviewItem(info);
        const auto glyphs =
            snow_canvas_text_layout::measureGraphemeMetrics(text, canvas.font(), item);
        require(label.value(QStringLiteral("width")).toDouble() >= glyphs.minimumWidth - 1e-6,
                "manual resize preserves a full grapheme column");
        require(label.value(QStringLiteral("content_height")).toDouble() <=
                    label.value(QStringLiteral("height")).toDouble() + 1e-6,
                "manual resize preserves all wrapped text lines");
        require(info.font_size == 30.0, "body resizing never scales text font");
    }
}

void emptyCommitKeepsGrowthAndFontChangesRemeasure() {
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    prepare(canvas);
    create(canvas, runtime, SnowCanvasRectangleShape::Rectangle, {130, 100}, {390, 150});
    const auto original = payload(runtime, QStringLiteral("SpeechBubble"));
    key(canvas, Qt::Key_A, Qt::NoModifier,
        QStringLiteral("Text that needs many lines ").repeated(80));
    key(canvas, Qt::Key_A, Qt::ControlModifier);
    key(canvas, Qt::Key_Delete);
    commit(canvas);
    const auto grown = payload(runtime, QStringLiteral("SpeechBubble"));
    require(records(runtime, QStringLiteral("Text")).isEmpty(), "empty commit creates no label");
    require(
        grown.value(QStringLiteral("body")).toObject().value(QStringLiteral("height")).toDouble() >
            5.0 * original.value(QStringLiteral("body"))
                      .toObject()
                      .value(QStringLiteral("height"))
                      .toDouble(),
        "empty commit keeps previously grown draft geometry");
    require(canvas.undo() && payload(runtime, QStringLiteral("SpeechBubble")) == original,
            "empty draft growth remains a separate undo step");
    require(canvas.redo(), "redo empty draft growth");
    open(canvas, runtime);
    const QString text = QStringLiteral("Wide words iii WWW ").repeated(10);
    key(canvas, Qt::Key_A, Qt::NoModifier, text);
    QFont font = canvas.font();
    font.setLetterSpacing(QFont::AbsoluteSpacing, 5.0);
    canvas.setFont(font);
    commit(canvas);
    auto label = payload(runtime, QStringLiteral("Text"));
    SnowTextElementInfo info{};
    info.font_size = label.value(QStringLiteral("font_size")).toDouble();
    auto item = snow_canvas_text::defaultPreviewItem(info);
    auto measured = snow_canvas_text_layout::measureWrappedTextLayout(
        text, canvas.font(), item, label.value(QStringLiteral("width")).toDouble());
    require(std::abs(measured.layout.height() -
                     label.value(QStringLiteral("content_height")).toDouble()) < 1e-6,
            "font changes during editing persist refreshed draft metrics on commit");

    font.setLetterSpacing(QFont::AbsoluteSpacing, 9.0);
    canvas.setFont(font);
    QApplication::processEvents();
    const auto engine = snow_canvas_runtime::Access::handle(runtime);
    SnowCanvasViewport observer;
    require(observer.create(engine, snow_canvas_viewport::defaultEngineConfig()),
            "create font metric observer");
    require(snow_viewport_set_surface_size(engine, observer.get(), 800, 700) == SNOW_OK,
            "size font metric observer");
    const auto textCenter = point(label.value(QStringLiteral("center")).toObject());
    require(snow_viewport_set_camera(engine, observer.get(), textCenter.x(), textCenter.y(), 1.0) ==
                SNOW_OK,
            "observe the center of the retained tall text container");
    ScopedPatchHandle patch;
    require(snow_viewport_acquire_patch(engine, observer.get(), nullptr, patch.outParam()) ==
                SNOW_OK,
            "read remeasured presentation");
    const SnowSceneDisplayItem* scene = nullptr;
    std::uint32_t count = 0;
    require(snow_patch_get_scene_items(patch.get(), &scene, &count) == SNOW_OK,
            "read remeasured text");
    bool found = false;
    for (std::uint32_t index = 0; index < count; ++index) {
        if (scene[index].kind != SNOW_SCENE_DISPLAY_ITEM_TEXT) {
            continue;
        }
        measured = snow_canvas_text_layout::measureWrappedTextLayout(
            text, canvas.font(), scene[index], scene[index].width);
        require(std::abs(measured.layout.height() - scene[index].content_height) < 1e-6,
                "host font changes immediately refresh presentation metrics");
        found = true;
    }
    require(found, "remeasured text remains visible");
    SnowCanvasShapeStyle bodyStyle;
    bodyStyle.stroke = Qt::red;
    require(canvas.setCanvasShapeStylePatch(bodyStyle, SnowCanvasShapeStylePropertyStrokeColor,
                                            SnowCanvasShapeKind::SpeechBubble),
            "persist font metrics in the next bubble transaction");
    label = payload(runtime, QStringLiteral("Text"));
    require(std::abs(measured.layout.height() -
                     label.value(QStringLiteral("content_height")).toDouble()) < 1e-6,
            "the originating transaction stores refreshed presentation metrics");
}

void shapeStyleReflowCommitsWithItsUndoStep() {
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    prepare(canvas);
    create(canvas, runtime, SnowCanvasRectangleShape::Rectangle, {160, 170}, {400, 250});
    const QString text =
        QStringLiteral("Words that wrap differently inside each body ").repeated(4);
    key(canvas, Qt::Key_A, Qt::NoModifier, text);
    commit(canvas);
    const auto add = addHandle(canvas, runtime);
    mouse(canvas, QEvent::MouseButtonPress, add, Qt::LeftButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseButtonRelease, add, Qt::LeftButton, Qt::NoButton);
    const auto original = payload(runtime, QStringLiteral("SpeechBubble"));
    const auto originalText = payload(runtime, QStringLiteral("Text"));
    SnowCanvasShapeStyle style;
    style.shape = SnowCanvasRectangleShape::Diamond;
    style.strokeWidth = 8.0;
    require(canvas.setCanvasShapeStylePatch(
                style, SnowCanvasShapeStylePropertyShape | SnowCanvasShapeStylePropertyStrokeWidth,
                SnowCanvasShapeKind::SpeechBubble),
            "change bubble contour and padding");
    const auto changed = payload(runtime, QStringLiteral("SpeechBubble"));
    const auto body = changed.value(QStringLiteral("body")).toObject();
    const auto label = payload(runtime, QStringLiteral("Text"));
    const double contentWidth =
        body.value(QStringLiteral("width")).toDouble() / 2.0 -
        2.0 * (9.0 + label.value(QStringLiteral("text_ink_reserve")).toDouble());
    require(std::abs(contentWidth - label.value(QStringLiteral("width")).toDouble()) < 1e-6,
            "shape style transaction persists the new safe content width");
    SnowTextElementInfo info{};
    info.font_size = label.value(QStringLiteral("font_size")).toDouble();
    const auto item = snow_canvas_text::defaultPreviewItem(info);
    const auto measured =
        snow_canvas_text_layout::measureWrappedTextLayout(text, canvas.font(), item, contentWidth);
    require(std::abs(measured.layout.height() -
                     label.value(QStringLiteral("content_height")).toDouble()) < 1e-6 &&
                label.value(QStringLiteral("height")).toDouble() >= measured.layout.height(),
            "shape style transaction persists wrapped layout and required growth");
    require(changed.value(QStringLiteral("tails")).toArray().isEmpty(),
            "style-induced growth deletes swallowed tails in the same transaction");
    require(canvas.undo() && payload(runtime, QStringLiteral("SpeechBubble")) == original &&
                payload(runtime, QStringLiteral("Text")) == originalText,
            "one undo restores shape, padding, dimensions, text layout, and tails");
    require(canvas.redo() && payload(runtime, QStringLiteral("SpeechBubble")) == changed &&
                payload(runtime, QStringLiteral("Text")) == label,
            "one redo replays the complete style and reflow transaction");
    open(canvas, runtime);
    SnowCanvasTextStyle textStyle;
    textStyle.fontSize = 80;
    textStyle.bold = true;
    textStyle.italic = true;
    textStyle.strokeWidth = 6;
    textStyle.stroke = Qt::black;
    require(canvas.setCanvasTextStyle(textStyle, SnowCanvasTextStyleMixedFontSize |
                                                     SnowCanvasTextStyleMixedBold |
                                                     SnowCanvasTextStyleMixedItalic |
                                                     SnowCanvasTextStyleMixedStrokeWidth |
                                                     SnowCanvasTextStyleMixedStroke),
            "format an existing bound draft");
    commit(canvas);
    const auto formattedBody = payload(runtime, QStringLiteral("SpeechBubble"));
    const auto formattedText = payload(runtime, QStringLiteral("Text"));
    require(formattedText.value(QStringLiteral("font_size")).toDouble() == 80.0 &&
                formattedText.value(QStringLiteral("bold")).toBool() &&
                formattedText.value(QStringLiteral("italic")).toBool() &&
                formattedText.value(QStringLiteral("height")).toDouble() >=
                    formattedText.value(QStringLiteral("content_height")).toDouble(),
            "font and text stroke edits commit fitting wrapped geometry");
    require(canvas.undo() && payload(runtime, QStringLiteral("SpeechBubble")) == changed &&
                payload(runtime, QStringLiteral("Text")) == label,
            "one undo restores formatting and its growth");
    require(canvas.redo() && payload(runtime, QStringLiteral("SpeechBubble")) == formattedBody &&
                payload(runtime, QStringLiteral("Text")) == formattedText,
            "one redo restores formatted growth");
}
void disconnectedContoursAcrossChunkBoundaries() {
    for (int segments : {62, 63, 64}) {
        for (bool curved : {false, true}) {
            std::vector<SnowArrowPathCommand> commands;
            QPainterPath expected;
            expected.setFillRule(Qt::WindingFill);
            for (int contour = 0; contour < 2; ++contour) {
                const double cx = 40.0 + contour * 90.0;
                for (int vertex = 0; vertex <= segments; ++vertex) {
                    const double angle =
                        vertex == segments ? 0.0 : 2.0 * 3.14159265358979323846 * vertex / segments;
                    const QPointF point(cx + 20.0 * std::cos(angle), 40.0 + 20.0 * std::sin(angle));
                    SnowArrowPathCommand command{};
                    command.kind = vertex == 0 ? SNOW_ARROW_PATH_COMMAND_MOVE_TO
                                               : SNOW_ARROW_PATH_COMMAND_LINE_TO;
                    command.point = {point.x(), point.y()};
                    if (vertex != 0 && curved) {
                        const double step = 2.0 * 3.14159265358979323846 / segments;
                        const double previous = (vertex - 1) * step;
                        const double tangent = 20.0 * 4.0 / 3.0 * std::tan(step / 4.0);
                        command.kind = SNOW_ARROW_PATH_COMMAND_CUBIC_TO;
                        command.control1 = {
                            cx + 20.0 * std::cos(previous) - tangent * std::sin(previous),
                            40.0 + 20.0 * std::sin(previous) + tangent * std::cos(previous)};
                        command.control2 = {point.x() + tangent * std::sin(angle),
                                            point.y() - tangent * std::cos(angle)};
                        expected.cubicTo(command.control1.x, command.control1.y, command.control2.x,
                                         command.control2.y, point.x(), point.y());
                    } else if (vertex == 0) {
                        expected.moveTo(point);
                    } else {
                        expected.lineTo(point);
                    }
                    commands.push_back(command);
                }
                expected.closeSubpath();
            }
            std::vector<SnowPathChunk> chunks;
            for (std::size_t offset = 0; offset < commands.size(); offset += 64) {
                SnowPathChunk chunk{};
                chunk.stable_id = chunks.size() + 1;
                chunk.command_start = static_cast<std::uint32_t>(offset);
                chunk.command_offset = static_cast<std::uint32_t>(offset);
                chunk.command_count =
                    static_cast<std::uint32_t>(std::min<std::size_t>(64, commands.size() - offset));
                if (offset != 0) {
                    chunk.start_x = commands[offset - 1].point.x;
                    chunk.start_y = commands[offset - 1].point.y;
                }
                chunk.max_x = 160;
                chunk.max_y = 80;
                chunks.push_back(chunk);
            }
            SnowPathChunkRange range{};
            range.insert_chunk_count = static_cast<std::uint32_t>(chunks.size());
            SnowCanvasSceneItem item;
            item.kind = SNOW_SCENE_DISPLAY_ITEM_SPEECH_BUBBLE;
            require(item.applyPathGeometryPatch(
                        0, 1, &range, 1, chunks.data(), static_cast<std::uint32_t>(chunks.size()),
                        commands.data(), static_cast<std::uint32_t>(commands.size()), true, true),
                    "accept closed contours spanning 64-command chunks");
            const auto paint = [](const QPainterPath& path, double zoom = 1.0,
                                  double rotation = 0.0) {
                QImage image(qCeil(170 * zoom), qCeil(80 * zoom),
                             QImage::Format_ARGB32_Premultiplied);
                image.fill(Qt::transparent);
                QPainter painter(&image);
                painter.scale(zoom, zoom);
                painter.translate(85, 40);
                painter.rotate(rotation);
                painter.translate(-85, -40);
                painter.setRenderHint(QPainter::Antialiasing);
                painter.setPen(QPen(Qt::black, 3));
                painter.setBrush(QColor(80, 120, 240, 128));
                painter.drawPath(path);
                return image;
            };
            const auto actual = paint(item.aggregateClosedPath());
            require(actual == paint(expected),
                    "chunk assembly preserves closed contours without connecting lines or seams");
            require(actual.pixelColor(85, 40).alpha() == 0 &&
                        actual.pixelColor(40, 40).alpha() == 128 &&
                        actual.pixelColor(130, 40).alpha() == 128,
                    "separate circles retain their fill and transparent gap");
            for (double zoom : {0.5, 1.25, 2.0})
                for (double rotation : {0.0, 21.0})
                    require(
                        paint(item.aggregateClosedPath(), zoom, rotation) ==
                            paint(expected, zoom, rotation),
                        "disconnected lines and cubics retain their joins under zoom and rotation");
        }
    }
}

void thoughtBubbleStyleSwitchPreservesTrailsAndText() {
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    prepare(canvas);
    create(canvas, runtime, SnowCanvasRectangleShape::ThoughtBubble, {160, 140}, {440, 320});
    key(canvas, Qt::Key_A, Qt::NoModifier, QStringLiteral("Thinking..."));
    commit(canvas);
    SnowCanvasShapeStyle style;
    style.shape = SnowCanvasRectangleShape::ThoughtBubble;
    style.fill = QColor(230, 244, 255);
    style.stroke = QColor(22, 119, 255);
    style.strokeWidth = 3;
    style.cornerRadii = {7, 8, 9, 10};
    require(canvas.setCanvasShapeStylePatch(style,
                                            SnowCanvasShapeStylePropertyFillColor |
                                                SnowCanvasShapeStylePropertyStrokeColor |
                                                SnowCanvasShapeStylePropertyStrokeWidth |
                                                SnowCanvasShapeStylePropertyCornerRadius,
                                            SnowCanvasShapeKind::SpeechBubble),
            "set thought bubble appearance");
    const auto handle = addHandle(canvas, runtime);
    mouse(canvas, QEvent::MouseButtonPress, handle, Qt::LeftButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseButtonRelease, handle + QPointF(-90, 130), Qt::LeftButton,
          Qt::NoButton);
    const auto original = payload(runtime, QStringLiteral("SpeechBubble"));
    const auto text = payload(runtime, QStringLiteral("Text"));
    if (const auto path = qEnvironmentVariable("SNOW_THOUGHT_BUBBLE_PREVIEW"); !path.isEmpty()) {
        require(runtime.renderToImage(QRectF(-400, -350, 800, 700), QSize(800, 700), {}).save(path),
                "save thought bubble visual inspection export");
    }
    style.shape = SnowCanvasRectangleShape::Diamond;
    require(canvas.setCanvasShapeStylePatch(style, SnowCanvasShapeStylePropertyShape,
                                            SnowCanvasShapeKind::SpeechBubble),
            "switch thought bubble to diamond");
    const auto diamond = payload(runtime, QStringLiteral("SpeechBubble"));
    requirePreservedTailGeometry(original, diamond);
    style.shape = SnowCanvasRectangleShape::ThoughtBubble;
    require(canvas.setCanvasShapeStylePatch(style, SnowCanvasShapeStylePropertyShape,
                                            SnowCanvasShapeKind::SpeechBubble),
            "switch back to thought bubble");
    const auto restored = payload(runtime, QStringLiteral("SpeechBubble"));
    requirePreservedTailGeometry(original, restored);
    require(
        restored.value(QStringLiteral("body")).toObject().value(QStringLiteral("corner_radii")) ==
                original.value(QStringLiteral("body"))
                    .toObject()
                    .value(QStringLiteral("corner_radii")) &&
            payload(runtime, QStringLiteral("Text")).value(QStringLiteral("text")) ==
                text.value(QStringLiteral("text")),
        "switching back retains requested radii, trails and label");
    require(canvas.undo() && payload(runtime, QStringLiteral("SpeechBubble")) == diamond,
            "undo restores the previous bubble type and geometry");
    require(canvas.redo() && payload(runtime, QStringLiteral("SpeechBubble")) == restored,
            "redo restores the thought bubble");
}

} // namespace

int main(int argc, char** argv) {
    qputenv("QT_QPA_PLATFORM", "offscreen");
    QApplication application(argc, argv);
    if (application.arguments().contains(QStringLiteral("--border-fill-only"))) {
        patternedAndTransparentTailExports();
        return 0;
    }
    disconnectedContoursAcrossChunkBoundaries();
    handDrawnStylesReachEveryBubbleContourAndExport();
    thoughtBubbleStyleSwitchPreservesTrailsAndText();
    textAlignmentDefaultsToCenter();
    wrappingGrowthAndCancellation();
    tailGesturesAndLifecycle();
    bodyMovementAndRotationKeepTailTipFixed();
    patternedAndTransparentTailExports();
    graphemeCacheAndNarrowText();
    imeAndRotatedGrowthAndExport();
    manualResizeKeepsTextMinimums();
    emptyCommitKeepsGrowthAndFontChangesRemeasure();
    shapeStyleReflowCommitsWithItsUndoStep();
    return 0;
}
