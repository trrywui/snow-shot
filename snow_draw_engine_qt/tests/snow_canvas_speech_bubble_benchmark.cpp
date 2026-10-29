#include "snow_draw_engine_qt/snow_canvas_runtime.h"
#include "snow_draw_engine_qt/snow_canvas_widget.h"
#include "snow_canvas_text.h"
#include "snow_canvas_runtime_access.h"
#include "snow_canvas_viewport.h"
#include "snow_canvas_ffi_handles.h"
#include "snow_canvas_commands.h"
#include "snow_canvas_text_measurement.h"
#include "snow_canvas_text_render.h"

#include <QApplication>
#include <QElapsedTimer>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QKeyEvent>
#include <QLineF>
#include <QMouseEvent>

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <iomanip>
#include <iostream>
#include <memory>
#include <vector>

namespace {
constexpr double kPi = 3.14159265358979323846;
constexpr double kFixtureTailHalfAngle = 0.08;
void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(1);
    }
}

void mouse(SnowCanvasWidget& canvas, QEvent::Type type, const QPointF& point,
           Qt::MouseButton button, Qt::MouseButtons buttons) {
    QMouseEvent event(type, point, point, point, button, buttons, Qt::NoModifier);
    QApplication::sendEvent(&canvas, &event);
}

void key(SnowCanvasWidget& canvas, int code, Qt::KeyboardModifiers modifiers = Qt::NoModifier,
         const QString& text = {}) {
    QKeyEvent event(QEvent::KeyPress, code, modifiers, text);
    QApplication::sendEvent(&canvas, &event);
}

QJsonObject session(const SnowCanvasRuntime& runtime) {
    const auto bytes = runtime.serializeDocumentSession();
    require(!bytes.isEmpty(), "benchmark document must fit the session capacity");
    return QJsonDocument::fromJson(bytes).object();
}

QJsonObject payload(const SnowCanvasRuntime& runtime, const QJsonObject& id, const QString& kind) {
    const auto records = session(runtime)
                             .value(QStringLiteral("document"))
                             .toObject()
                             .value(QStringLiteral("slots"))
                             .toArray();
    for (const auto& record : records) {
        const auto value = record.toObject();
        if (value.value(QStringLiteral("id")).toObject() == id) {
            const auto data = value.value(QStringLiteral("data")).toObject();
            require(data.contains(kind), "benchmark element must have the expected kind");
            return data.value(kind).toObject();
        }
    }
    require(false, "benchmark element must exist");
    return {};
}

QPointF point(const QJsonObject& value) {
    return {value.value(QStringLiteral("x")).toDouble(),
            value.value(QStringLiteral("y")).toDouble()};
}

QPointF bodyCenter(const QJsonObject& bubble) {
    return point(
        bubble.value(QStringLiteral("body")).toObject().value(QStringLiteral("center")).toObject());
}

QPointF localToView(const SnowCanvasWidget& canvas, const QJsonObject& bubble,
                    const QPointF& local) {
    // Fixtures are unrotated; benchmark resize and tail gestures keep rotation unchanged.
    return canvas.canvasToViewTransform().map(bodyCenter(bubble) + local);
}

QPointF contourPoint(const QJsonObject& bubble, double angle) {
    const auto body = bubble.value(QStringLiteral("body")).toObject();
    const double halfWidth = body.value(QStringLiteral("width")).toDouble() / 2;
    const double halfHeight = body.value(QStringLiteral("height")).toDouble() / 2;
    const double cosine = std::cos(angle);
    const double sine = std::sin(angle);
    const double radius = std::min(halfWidth / std::max(std::abs(cosine), 1e-12),
                                   halfHeight / std::max(std::abs(sine), 1e-12));
    return {cosine * radius, sine * radius};
}

struct TextCase {
    const char* name;
    QString text;
    QString typed;
};

struct Fixture {
    std::unique_ptr<SnowCanvasRuntime> runtime;
    QJsonObject owner;
};

Fixture prepare(int count, int tails, const TextCase& textCase, bool thoughtBubble) {
    Fixture fixture{std::make_unique<SnowCanvasRuntime>(), {}};
    require(fixture.runtime->setAutomaticallySelectNewlyCreatedElements(true) &&
                fixture.runtime->setAutomaticSelectionExcludedTools({}),
            "enable deterministic bubble selection");
    {
        SnowCanvasWidget canvas(*fixture.runtime);
        canvas.resize(1280, 720);
        canvas.show();
        QApplication::processEvents();
        require(canvas.setViewportCamera(0, 0, 1), "set fixture camera");
        require(canvas.setCanvasTool(SnowCanvasTool::SpeechBubble), "activate Speech Bubble");
        SnowCanvasShapeStyle style;
        style.shape = thoughtBubble ? SnowCanvasRectangleShape::ThoughtBubble
                                    : SnowCanvasRectangleShape::Rectangle;
        style.cornerRadii = {0, 0, 0, 0};
        require(canvas.setCanvasShapeStylePatch(style,
                                                SnowCanvasShapeStylePropertyShape |
                                                    SnowCanvasShapeStylePropertyCornerRadius,
                                                SnowCanvasShapeKind::SpeechBubble),
                "set deterministic benchmark contour");
        SnowCanvasTextStyle textStyle;
        textStyle.fontSize = 18;
        require(canvas.setCanvasTextStyle(textStyle), "set benchmark font size");
        mouse(canvas, QEvent::MouseButtonPress, {120, 80}, Qt::LeftButton, Qt::LeftButton);
        mouse(canvas, QEvent::MouseMove, {715, 415}, Qt::NoButton, Qt::LeftButton);
        mouse(canvas, QEvent::MouseButtonRelease, {720, 420}, Qt::LeftButton, Qt::NoButton);
        require(canvas.hasActiveTextEditing(), "drag creation must open the bound text editor");
        key(canvas, Qt::Key_A, Qt::NoModifier, textCase.text);
        key(canvas, Qt::Key_Return, Qt::ControlModifier);
        require(!canvas.hasActiveTextEditing(), "commit benchmark text");
        const auto selected = fixture.runtime->selectedElementIds();
        require(selected.size() == 1, "benchmark bubble is selected after creation");
        fixture.owner = selected.first().toObject();
    }
    // Add a deterministic fan of tails outside the body before producing scene density.
    // Only the fixture is imported; every timed mutation below uses real Qt input.
    auto saved = session(*fixture.runtime);
    auto document = saved.value(QStringLiteral("document")).toObject();
    auto records = document.value(QStringLiteral("slots")).toArray();
    for (qsizetype index = 0; index < records.size(); ++index) {
        auto record = records.at(index).toObject();
        if (record.value(QStringLiteral("id")).toObject() != fixture.owner)
            continue;
        auto data = record.value(QStringLiteral("data")).toObject();
        auto bubble = data.value(QStringLiteral("SpeechBubble")).toObject();
        const auto body = bubble.value(QStringLiteral("body")).toObject();
        const double radius = std::max(body.value(QStringLiteral("width")).toDouble(),
                                       body.value(QStringLiteral("height")).toDouble()) *
                              (thoughtBubble ? 1.25 : 0.65);
        QJsonArray values;
        for (int tail = 0; tail < tails; ++tail) {
            const double angle = kPi / 3 + 2 * kPi * tail / tails;
            values.append(QJsonObject{
                {QStringLiteral("id"), tail + 1},
                {QStringLiteral("tip"),
                 QJsonObject{{QStringLiteral("x"), std::cos(angle) * radius},
                             {QStringLiteral("y"), std::sin(angle) * radius}}},
                {QStringLiteral("base_width"),
                 thoughtBubble ? 32.0
                               : QLineF(contourPoint(bubble, angle - kFixtureTailHalfAngle),
                                        contourPoint(bubble, angle + kFixtureTailHalfAngle))
                                     .length()}});
        }
        bubble.insert(QStringLiteral("tails"), values);
        data.insert(QStringLiteral("SpeechBubble"), bubble);
        record.insert(QStringLiteral("data"), data);
        records[index] = record;
    }
    document.insert(QStringLiteral("slots"), records);
    saved.insert(QStringLiteral("document"), document);
    SnowCanvasRuntime empty;
    saved.insert(QStringLiteral("history"), session(empty).value(QStringLiteral("history")));
    require(fixture.runtime->restoreDocumentSession(QJsonDocument(saved).toJson()),
            "restore the benchmark fan of tails");
    {
        SnowCanvasRuntimeEditor editor(*fixture.runtime);
        require(editor.isValid(), "create density editor");
        const auto index =
            static_cast<quint32>(fixture.owner.value(QStringLiteral("index")).toInteger());
        const auto generation =
            static_cast<quint32>(fixture.owner.value(QStringLiteral("generation")).toInteger());
        for (int copy = 1; copy < count; ++copy) {
            require(editor.select(index, generation) &&
                        editor.duplicateSelected(QPointF(5000 + copy * 32, (copy % 20) * 32)),
                    "duplicateSelected must create offscreen density with bound labels");
        }
        require(editor.select(index, generation), "restore benchmark owner selection");
    }
    const auto all = session(*fixture.runtime)
                         .value(QStringLiteral("document"))
                         .toObject()
                         .value(QStringLiteral("slots"))
                         .toArray();
    int actual = 0;
    for (const auto& record : all)
        if (record.toObject()
                .value(QStringLiteral("data"))
                .toObject()
                .contains(QStringLiteral("SpeechBubble")))
            ++actual;
    require(actual == count, "benchmark must create the requested number of bubbles");
    return fixture;
}

void checkBoundedMeasurementReuse(const QString& text) {
    SnowTextElementInfo info{};
    info.font_size = 18;
    auto item = snow_canvas_text::defaultPreviewItem(info);
    constexpr int capacity = 8192;
    snow_canvas_text_measurement::BubbleTextLayoutCache cache(capacity);
    cache.graphemes(text, QFont(), item);
    cache.wrapped(text, QFont(), item, 540);
    const auto before = cache.measurementCount();
    item.center_x += 100;
    item.height += 100;
    cache.graphemes(text, QFont(), item);
    cache.wrapped(text, QFont(), item, 540);
    require(cache.measurementCount() == before,
            "body movement and tail geometry must reuse cached typography at equal width");
    for (int index = 0; index < 128; ++index) {
        const auto unique = text + QString::number(index);
        cache.graphemes(unique, QFont(), item);
        cache.wrapped(unique, QFont(), item, 540 + index % 7);
    }
    require(cache.retainedBytes() <= capacity, "bubble metric cache must obey its byte budget");
}

enum class Operation { Typing, BodyResize, TipDrag, BaseDrag };

const char* name(Operation operation) {
    switch (operation) {
    case Operation::Typing:
        return "typing";
    case Operation::BodyResize:
        return "body-resize";
    case Operation::TipDrag:
        return "tip-drag";
    case Operation::BaseDrag:
        return "base-drag";
    }
    return "unknown";
}

double percentile(std::vector<double> values, double fraction) {
    std::sort(values.begin(), values.end());
    const auto index =
        static_cast<std::size_t>(std::ceil(static_cast<double>(values.size()) * fraction) - 1);
    return values.at(std::min(index, values.size() - 1));
}

QPointF thoughtBaseHandle(SnowCanvasRuntime& runtime, const SnowCanvasWidget& canvas,
                          const QJsonObject& owner) {
    const auto engine = snow_canvas_runtime::Access::handle(runtime);
    SnowCanvasViewport observer;
    require(observer.create(engine, snow_canvas_viewport::defaultEngineConfig()),
            "create control observer");
    require(snow_viewport_set_surface_size(engine, observer.get(), 4096, 4096) == SNOW_OK &&
                snow_viewport_set_camera(engine, observer.get(), 0, 0, 1) == SNOW_OK,
            "configure control observer");
    require(snow_canvas_commands::selectElement(
                engine, observer.get(),
                SnowElementId{
                    static_cast<std::uint32_t>(owner.value(QStringLiteral("index")).toInt()),
                    static_cast<std::uint32_t>(owner.value(QStringLiteral("generation")).toInt())})
                .success,
            "select fixture for control inspection");
    ScopedPatchHandle patch;
    require(snow_viewport_acquire_patch(engine, observer.get(), nullptr, patch.outParam()) ==
                SNOW_OK,
            "inspect real circle width controls");
    const SnowOverlayDisplayItem* overlay = nullptr;
    std::uint32_t count = 0;
    require(snow_patch_get_overlay_items(patch.get(), &overlay, &count) == SNOW_OK,
            "read circle width controls");
    for (std::uint32_t i = 0; i < count; ++i) {
        if (overlay[i].rect_kind == SNOW_OVERLAY_RECT_SPEECH_BUBBLE_TAIL_BASE_HANDLE)
            return canvas.canvasToViewTransform().map(
                QPointF(overlay[i].center_x, overlay[i].center_y));
    }
    require(false, "Thought Bubble has a width control");
    return {};
}

void run(const Fixture& fixture, int count, int tails, const TextCase& textCase,
         Operation operation, int iterations, int repeat) {
    SnowCanvasRuntime runtime;
    require(runtime.cloneDocumentSessionFrom(*fixture.runtime), "clone the density fixture");
    SnowCanvasWidget canvas(runtime);
    canvas.resize(1280, 720);
    canvas.show();
    QApplication::processEvents();
    require(canvas.setViewportCamera(0, 0, 1), "set benchmark camera");
    require(canvas.setCanvasTool(SnowCanvasTool::Select), "activate selection tool");
    const auto original = payload(runtime, fixture.owner, QStringLiteral("SpeechBubble"));
    const auto originalBody = original.value(QStringLiteral("body")).toObject();
    const bool thoughtBubble =
        originalBody.value(QStringLiteral("highlight_shape")) == QStringLiteral("ThoughtBubble");
    const auto textId = original.value(QStringLiteral("text_element_id")).toObject();
    const auto originalText = payload(runtime, textId, QStringLiteral("Text"));
    const QPointF selectPoint = localToView(
        canvas, original, {-originalBody.value(QStringLiteral("width")).toDouble() / 2, 0});
    mouse(canvas, QEvent::MouseButtonPress, selectPoint, Qt::LeftButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseButtonRelease, selectPoint, Qt::LeftButton, Qt::NoButton);
    require(runtime.selectedElementIds().size() == 1 &&
                runtime.selectedElementIds().first().toObject() == fixture.owner,
            "benchmark must select its active bubble rather than an offscreen duplicate");
    QPointF handle;
    const auto originalTail = original.value(QStringLiteral("tails")).toArray().first().toObject();
    const auto tip = point(originalTail.value(QStringLiteral("tip")).toObject());
    const double axis = std::atan2(tip.y(), tip.x());
    if (operation == Operation::Typing) {
        const auto center = localToView(canvas, original, {});
        mouse(canvas, QEvent::MouseButtonDblClick, center, Qt::LeftButton, Qt::LeftButton);
        require(canvas.hasActiveTextEditing(), "typing benchmark must enter bound text editing");
        key(canvas, Qt::Key_End, Qt::ControlModifier);
    } else {
        if (operation == Operation::BodyResize)
            handle = localToView(canvas, original,
                                 {originalBody.value(QStringLiteral("width")).toDouble() / 2, 0});
        else if (operation == Operation::TipDrag)
            handle = localToView(canvas, original, tip);
        else if (thoughtBubble)
            handle = thoughtBaseHandle(runtime, canvas, fixture.owner);
        else
            handle =
                localToView(canvas, original, contourPoint(original, axis - kFixtureTailHalfAngle));
        mouse(canvas, QEvent::MouseButtonPress, handle, Qt::LeftButton, Qt::LeftButton);
    }
    std::vector<double> inputs, paints, frames;
    inputs.reserve(static_cast<std::size_t>(iterations));
    paints.reserve(static_cast<std::size_t>(iterations));
    frames.reserve(static_cast<std::size_t>(iterations));
    snow_canvas_text_render::clearRenderCacheForCurrentThread();
    constexpr int warmup = 24;
    QPointF finalPosition = handle;
    for (int frame = 0; frame < iterations + warmup; ++frame) {
        QElapsedTimer timer;
        timer.start();
        if (operation == Operation::Typing) {
            key(canvas, Qt::Key_A, Qt::NoModifier, textCase.typed);
        } else {
            if (operation == Operation::BodyResize)
                finalPosition = handle + QPointF(20 + frame % 41, 0);
            else if (operation == Operation::TipDrag)
                finalPosition = handle + QPointF(20 + frame % 31, 10 + frame % 29);
            else if (thoughtBubble)
                finalPosition =
                    handle + QPointF(-std::sin(axis), std::cos(axis)) * (10 + frame % 17);
            else
                finalPosition = localToView(
                    canvas, original, contourPoint(original, axis - (0.13 + 0.005 * (frame % 17))));
            mouse(canvas, QEvent::MouseMove, finalPosition, Qt::NoButton, Qt::LeftButton);
        }
        const double inputMs = static_cast<double>(timer.nsecsElapsed()) / 1e6;
        // Match the arrow benchmark's ordinary event-driven frame completion.
        QApplication::processEvents();
        const double frameMs = static_cast<double>(timer.nsecsElapsed()) / 1e6;
        const double paintMs = frameMs - inputMs;
        if (frame >= warmup) {
            inputs.push_back(inputMs);
            paints.push_back(paintMs);
            frames.push_back(frameMs);
        }
    }
    if (operation == Operation::Typing)
        key(canvas, Qt::Key_Return, Qt::ControlModifier);
    else
        mouse(canvas, QEvent::MouseButtonRelease, finalPosition, Qt::LeftButton, Qt::NoButton);
    const auto after = payload(runtime, fixture.owner, QStringLiteral("SpeechBubble"));
    const auto afterBody = after.value(QStringLiteral("body")).toObject();
    const auto afterText = payload(runtime, textId, QStringLiteral("Text"));
    const auto afterTail = after.value(QStringLiteral("tails")).toArray().first().toObject();
    require(after.value(QStringLiteral("tails")).toArray().size() == tails,
            "benchmark gestures preserve all outside tails");
    require(afterText.value(QStringLiteral("font_size")) ==
                originalText.value(QStringLiteral("font_size")),
            "typing and geometry edits must never scale the bound font");
    if (operation == Operation::Typing) {
        require(afterText.value(QStringLiteral("text")).toString().size() > textCase.text.size(),
                "typing must mutate the bound text instead of sending ignored keys");
    } else if (operation == Operation::BodyResize) {
        require(afterBody.value(QStringLiteral("width")).toDouble() >
                        originalBody.value(QStringLiteral("width")).toDouble() &&
                    afterText.value(QStringLiteral("width")).toDouble() >
                        originalText.value(QStringLiteral("width")).toDouble(),
                "resize must change the body and the bound text's wrap width");
        const double left = point(afterBody.value(QStringLiteral("center")).toObject()).x() -
                            afterBody.value(QStringLiteral("width")).toDouble() / 2;
        const double originalLeft =
            bodyCenter(original).x() - originalBody.value(QStringLiteral("width")).toDouble() / 2;
        require(std::abs(left - originalLeft) < 1e-7, "right resize keeps the opposite edge fixed");
    } else if (operation == Operation::TipDrag) {
        require(afterTail.value(QStringLiteral("tip")) !=
                        originalTail.value(QStringLiteral("tip")) &&
                    afterTail.value(QStringLiteral("base_width")) ==
                        originalTail.value(QStringLiteral("base_width")),
                "tip input must move the tip without changing base width");
        require(QLineF(localToView(canvas, after,
                                   point(afterTail.value(QStringLiteral("tip")).toObject())),
                       finalPosition)
                        .length() < 1e-6,
                "tip must include the final release coordinate");
    } else {
        require(afterTail.value(QStringLiteral("tip")) ==
                        originalTail.value(QStringLiteral("tip")) &&
                    afterTail.value(QStringLiteral("base_width")) !=
                        originalTail.value(QStringLiteral("base_width")),
                "base input must widen the symmetric attachment instead of moving the bubble");
        require(afterBody == originalBody, "base drags must preserve body geometry");
    }
    const auto cache = snow_canvas_text_render::renderCacheDiagnosticsForCurrentThread();
    require(cache.builds > 0, "benchmark frames must actually paint their bound text");
    require(cache.entries <= snow_canvas_text_render::kRenderCacheEntryLimit &&
                cache.retainedCharacters <= snow_canvas_text_render::kRenderCacheCharacterLimit &&
                cache.estimatedBytes <= snow_canvas_text_render::kRenderCacheByteLimit,
            "painted text layout cache must remain within every retention limit");
    if (operation == Operation::TipDrag || operation == Operation::BaseDrag)
        require(cache.hits > cache.builds, "tail geometry must reuse painted text layouts");
    std::cout << name(operation) << ',' << count << ',' << tails << ',' << textCase.name << ','
              << repeat << ',' << percentile(inputs, 0.5) << ',' << percentile(inputs, 0.95) << ','
              << percentile(paints, 0.5) << ',' << percentile(paints, 0.95) << ','
              << percentile(frames, 0.5) << ',' << percentile(frames, 0.95) << ',' << cache.entries
              << ',' << cache.estimatedBytes << ',' << cache.hits << ',' << cache.builds << '\n';
}
} // namespace

int main(int argc, char** argv) {
    qputenv("QT_QPA_PLATFORM", "offscreen");
#ifdef Q_OS_WIN
    qputenv("QT_QPA_FONTDIR", qgetenv("WINDIR") + "/Fonts");
#endif
    QApplication application(argc, argv);
    const int iterations = argc > 1 ? std::max(20, std::atoi(argv[1])) : 100;
    const int repeats = argc > 2 ? std::max(1, std::atoi(argv[2])) : 3;
    const bool smoke = application.arguments().contains(QStringLiteral("--smoke"));
    const bool thoughtBubble = application.arguments().contains(QStringLiteral("--thought-bubble"));
    const std::vector<int> counts = smoke ? std::vector<int>{1} : std::vector<int>{1, 100, 1000};
    const std::vector<int> tailCounts = smoke ? std::vector<int>{4} : std::vector<int>{1, 4, 16};
    const std::vector<TextCase> cases{
        {"short-ascii", QStringLiteral("Speech bubble"), QStringLiteral("a")},
        {"long-ascii",
         QStringLiteral("A long speech bubble label wraps within its body. ").repeated(10),
         QStringLiteral("a")},
        {"short-cjk", QString::fromUtf8("对话气泡自动换行"), QString::fromUtf8("漢")},
        {"long-cjk", QString::fromUtf8("对话气泡文本自动换行并保持可读性。").repeated(18),
         QString::fromUtf8("漢")},
    };
    std::cout << std::fixed << std::setprecision(6)
              << "scenario,scene_bubbles,tails,text_case,repeat,input_p50_ms,input_p95_ms,paint_"
                 "p50_ms,paint_p95_ms,frame_p50_ms,frame_p95_ms,cache_entries,cache_bytes,cache_"
                 "hits,cache_builds\n";
    for (const auto& textCase : cases) {
        checkBoundedMeasurementReuse(textCase.text);
        for (int count : counts) {
            for (int tails : tailCounts) {
                const auto fixture = prepare(count, tails, textCase, thoughtBubble);
                for (int repeat = 0; repeat < repeats; ++repeat)
                    for (auto operation : {Operation::Typing, Operation::BodyResize,
                                           Operation::TipDrag, Operation::BaseDrag})
                        run(fixture, count, tails, textCase, operation, iterations, repeat);
            }
        }
    }
}
