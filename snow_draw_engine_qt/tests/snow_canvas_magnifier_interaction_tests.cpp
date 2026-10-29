#include "snow_draw_engine_qt/snow_canvas_runtime.h"
#include "snow_draw_engine_qt/snow_canvas_custom_renderer.h"
#include "snow_draw_engine_qt/snow_canvas_region_filter.h"
#include "snow_draw_engine_qt/snow_canvas_widget.h"

#include <QApplication>
#include <QFocusEvent>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QMouseEvent>
#include <QPaintEvent>
#include <QPainter>
#include <QPointer>
#include <QWheelEvent>
#include <QtMath>

#include <cmath>
#include <cstdlib>
#include <iostream>
#include <utility>
#include <vector>

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(EXIT_FAILURE);
    }
}

void mouse(SnowCanvasWidget& canvas, QEvent::Type type, QPointF point, Qt::MouseButton button,
           Qt::MouseButtons buttons, Qt::KeyboardModifiers modifiers = Qt::NoModifier) {
    QMouseEvent event(type, point, point, point, button, buttons, modifiers);
    QApplication::sendEvent(&canvas, &event);
    QApplication::processEvents();
}

void drag(SnowCanvasWidget& canvas, QPointF start, QPointF end) {
    mouse(canvas, QEvent::MouseButtonPress, start, Qt::LeftButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseMove, end, Qt::NoButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseButtonRelease, end, Qt::LeftButton, Qt::NoButton);
}

QJsonArray records(const SnowCanvasRuntime& runtime, const QString& kind) {
    const auto document = QJsonDocument::fromJson(runtime.serializeDocumentSession())
                              .object()
                              .value(QStringLiteral("document"))
                              .toObject();
    QJsonArray result;
    for (const auto& slot : document.value(QStringLiteral("slots")).toArray()) {
        const auto data = slot.toObject().value(QStringLiteral("data")).toObject();
        if (data.contains(kind)) {
            result.append(data.value(kind));
        }
    }
    return result;
}

QJsonArray magnifiers(const SnowCanvasRuntime& runtime) {
    return records(runtime, QStringLiteral("Magnifier"));
}

QJsonObject magnifier(const SnowCanvasRuntime& runtime) {
    const auto values = magnifiers(runtime);
    require(values.size() == 1, "a drag creates one compound magnifier annotation");
    return values.first().toObject();
}

QPointF center(const QJsonObject& value, const QString& field) {
    const auto point = value.value(field).toObject();
    return {point.value(QStringLiteral("x")).toDouble(),
            point.value(QStringLiteral("y")).toDouble()};
}

bool near(QPointF a, QPointF b) {
    return std::hypot(a.x() - b.x(), a.y() - b.y()) < 0.001;
}

void prepare(SnowCanvasWidget& canvas) {
    canvas.resize(800, 600);
    canvas.show();
    QApplication::processEvents();
    require(canvas.setCanvasTool(SnowCanvasTool::Magnifier), "activate Magnifier");
}

bool wheel(SnowCanvasWidget& canvas, int pixels, int angle = 0,
           Qt::KeyboardModifiers modifiers = Qt::NoModifier, bool native = false,
           Qt::MouseEventSource source = Qt::MouseEventSynthesizedBySystem,
           Qt::MouseButtons buttons = Qt::NoButton) {
    QWheelEvent event(QPointF(260, 190), canvas.mapToGlobal(QPoint(260, 190)), QPoint(0, pixels),
                      QPoint(0, angle), buttons, modifiers, Qt::NoScrollPhase, false, source);
    if (!native)
        return canvas.adjustMagnifierFromWheel(event);
    event.ignore();
    QApplication::sendEvent(&canvas, &event);
    return event.isAccepted();
}

void observeWheelCommits(SnowCanvasWidget& canvas, std::vector<SnowCanvasMagnifierEdit>& commits) {
    QObject::connect(&canvas, &SnowCanvasWidget::styleEditCommitted, &canvas,
                     [&commits](const SnowCanvasStyleEdit& edit) {
                         const auto* patch = std::get_if<SnowCanvasMagnifierEdit>(&edit);
                         require(patch != nullptr &&
                                     patch->properties == SnowCanvasMagnifierStylePropertyFactor,
                                 "wheel commits publish only the magnifier factor");
                         commits.push_back(*patch);
                     });
}

void selectMagnifier(SnowCanvasWidget& canvas, QPointF point = {260, 190}) {
    mouse(canvas, QEvent::MouseButtonPress, point, Qt::LeftButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseButtonRelease, point, Qt::LeftButton, Qt::NoButton);
    require(canvas.canvasStyleToolbarState().source ==
                SnowCanvasStyleToolbarSource::SelectedMagnifier,
            "source selection exposes the magnifier style");
}

void wheelAccumulationPublishesCompletedCreationEdits() {
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    prepare(canvas);
    canvas.setWheelZoomEnabled(false);
    const auto camera = canvas.canvasToViewTransform();
    const auto initial = canvas.canvasMagnifierStyle();
    std::vector<SnowCanvasMagnifierEdit> commits;
    observeWheelCommits(canvas, commits);
    for (int i = 0; i < 9; ++i)
        require(wheel(canvas, 10, 0, Qt::NoModifier, true), "precise native wheels are consumed");
    require(canvas.canvasMagnifierStyle() == initial && commits.empty(),
            "partial precise input neither changes defaults nor publishes a user edit");
    require(wheel(canvas, 10, 0, Qt::NoModifier, true) &&
                canvas.canvasMagnifierStyle().factor == 2.1 && commits.size() == 1 &&
                commits.back().target == SnowCanvasStyleEditTarget::Creation,
            "one hundred precise pixels commit one creation factor step with zoom disabled");
    require(wheel(canvas, 250) && canvas.canvasMagnifierStyle().factor == 2.3 &&
                wheel(canvas, 50) && canvas.canvasMagnifierStyle().factor == 2.4 &&
                commits.size() == 3,
            "large input keeps its remainder across completed factor commits");
    auto expected = initial;
    expected.factor = 2.4;
    require(canvas.canvasMagnifierStyle() == expected && canvas.canvasToViewTransform() == camera &&
                !canvas.canvasHistoryState().canUndo,
            "creation factor wheels preserve other properties, camera and document history");

    require(canvas.setCanvasMagnifierStyle(initial, SnowCanvasMagnifierStylePropertyFactor),
            "restore creation factor for fractional-angle input");
    const auto commitCount = commits.size();
    for (int i = 0; i < 5; ++i)
        require(wheel(canvas, 0, 20), "fractional angle wheels are consumed");
    require(canvas.canvasMagnifierStyle().factor == 2.0 && commits.size() == commitCount,
            "fractional angle input waits for a complete notch");
    require(wheel(canvas, 0, 20) && canvas.canvasMagnifierStyle().factor == 2.1 &&
                commits.size() == commitCount + 1,
            "one hundred twenty angle units commit one factor step");
}

void selectedWheelEditsRememberDefaultsAndPublishUndoableFactors() {
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    prepare(canvas);
    drag(canvas, {220, 160}, {300, 220});
    selectMagnifier(canvas);
    canvas.setWheelZoomEnabled(false);
    const auto original = magnifier(runtime);
    const auto camera = canvas.canvasToViewTransform();
    std::vector<SnowCanvasMagnifierEdit> commits;
    observeWheelCommits(canvas, commits);
    for (int i = 0; i < 10; ++i)
        require(wheel(canvas, 10, 0, Qt::NoModifier, true), "selected precise wheel is consumed");
    require(commits.size() == 1 && commits.back().target == SnowCanvasStyleEditTarget::Selection &&
                commits.back().style.factor == 2.1 &&
                magnifier(runtime).value(QStringLiteral("factor")).toDouble() == 2.1 &&
                canvas.canvasToViewTransform() == camera,
            "completed selected wheels publish selected factor edits without moving the camera");
    require(canvas.undo() && magnifier(runtime) == original && canvas.redo() &&
                canvas.canvasMagnifierStyle().factor == 2.1,
            "the completed selected factor step is independently undoable and redoable");
    require(canvas.resetEditingState() && canvas.setCanvasTool(SnowCanvasTool::Magnifier) &&
                canvas.canvasMagnifierStyle().factor == 2.1,
            "selected factor wheels remember future magnifier defaults across history");
}

void wheelCommitsPreserveTheirTargetWhenObserversDeleteTheCanvas() {
    for (const bool selected : {false, true}) {
        SnowCanvasRuntime runtime;
        QPointer<SnowCanvasWidget> canvas = new SnowCanvasWidget(runtime);
        prepare(*canvas);
        if (selected) {
            drag(*canvas, {220, 160}, {300, 220});
            selectMagnifier(*canvas);
        }
        const auto original = canvas->canvasMagnifierStyle();
        auto expected = original;
        expected.factor += 0.1;
        SnowCanvasStyleDefaults remembered;
        remembered.magnifier = original;
        QObject context;
        int commits = 0;
        QObject::connect(
            canvas, &SnowCanvasWidget::styleEditCommitted, &context,
            [&](const SnowCanvasStyleEdit& edit) {
                const auto* patch = std::get_if<SnowCanvasMagnifierEdit>(&edit);
                require(patch != nullptr && patch->style == expected &&
                            patch->properties == SnowCanvasMagnifierStylePropertyFactor &&
                            patch->target == (selected ? SnowCanvasStyleEditTarget::Selection
                                                       : SnowCanvasStyleEditTarget::Creation),
                        "wheel commits retain their explicit target before deletion");
                snowCanvasMergeStyleEdit(remembered, edit);
                ++commits;
                delete canvas.data();
            });
        require(!wheel(*canvas, 0, 120) && canvas.isNull() && commits == 1 &&
                    remembered.magnifier == expected,
                "wheel commits remember the factor and stop when an observer deletes the canvas");
        SnowCanvasWidget survivor(runtime);
        require(survivor.resetEditingState() && survivor.setCanvasTool(SnowCanvasTool::Magnifier) &&
                    survivor.canvasMagnifierStyle() == expected,
                "both wheel targets retain future defaults after the edited canvas is deleted");
    }
}

void wheelPublishesHomogenizedSelectionAtTheFactorLimit() {
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    prepare(canvas);
    drag(canvas, {220, 160}, {300, 220});
    drag(canvas, {500, 160}, {580, 220});
    selectMagnifier(canvas);
    auto style = canvas.canvasMagnifierStyle();
    style.factor = 10;
    require(canvas.setCanvasMagnifierStyle(style, SnowCanvasMagnifierStylePropertyFactor) &&
                canvas.setCanvasTool(SnowCanvasTool::Select),
            "give the primary magnifier a saturated factor before selecting both");
    selectMagnifier(canvas, {540, 190});
    mouse(canvas, QEvent::MouseButtonPress, {260, 190}, Qt::LeftButton, Qt::LeftButton,
          Qt::ShiftModifier);
    mouse(canvas, QEvent::MouseButtonRelease, {260, 190}, Qt::LeftButton, Qt::NoButton,
          Qt::ShiftModifier);
    const auto before = canvas.canvasStyleToolbarState();
    const auto magnifiersBefore = magnifiers(runtime);
    require(before.source == SnowCanvasStyleToolbarSource::SelectedMagnifier &&
                before.selectedElementCount == 2 && before.magnifierStyle.factor == 10 &&
                (before.magnifierStyleMixed & SnowCanvasMagnifierStylePropertyFactor) != 0,
            "the primary selected factor is saturated while the other selected factor differs");
    std::vector<SnowCanvasMagnifierEdit> commits;
    observeWheelCommits(canvas, commits);
    require(
        wheel(canvas, 0, 120, Qt::NoModifier, true) && canvas.canvasMagnifierStyle().factor == 10 &&
            (canvas.canvasStyleToolbarState().magnifierStyleMixed &
             SnowCanvasMagnifierStylePropertyFactor) == 0 &&
            commits.size() == 1 && commits.back().target == SnowCanvasStyleEditTarget::Selection &&
            commits.back().style.factor == 10,
        "homogenizing other selected factors publishes a user edit at the primary factor's limit");
    require(canvas.undo() && magnifiers(runtime) == magnifiersBefore &&
                (canvas.canvasStyleToolbarState().magnifierStyleMixed &
                 SnowCanvasMagnifierStylePropertyFactor) != 0,
            "undo restores every selected factor and its mixed toolbar state");
    require(canvas.resetEditingState() && canvas.setCanvasTool(SnowCanvasTool::Magnifier) &&
                canvas.canvasMagnifierStyle().factor == 10,
            "homogenizing selected factors remembers the explicit future magnifier factor");
}

void wheelRemaindersResetAtInputAndEditingBoundaries() {
    for (int transition = 0; transition < 7; ++transition) {
        SnowCanvasRuntime runtime;
        SnowCanvasWidget canvas(runtime);
        prepare(canvas);
        drag(canvas, {220, 160}, {300, 220});
        selectMagnifier(canvas);
        canvas.setWheelZoomEnabled(false);
        require(wheel(canvas, 40), "start an incomplete factor wheel");
        const auto camera = canvas.canvasToViewTransform();
        switch (transition) {
        case 0: {
            QFocusEvent event(QEvent::FocusOut);
            QApplication::sendEvent(&canvas, &event);
            break;
        }
        case 1:
            require(canvas.setCanvasTool(SnowCanvasTool::Watermark) &&
                        canvas.setCanvasTool(SnowCanvasTool::Magnifier),
                    "switch away from and back to magnifier");
            break;
        case 2: {
            auto style = canvas.canvasMagnifierStyle();
            style.strokeWidth = 4;
            require(canvas.setCanvasMagnifierStyle(style,
                                                   SnowCanvasMagnifierStylePropertyStrokeWidth) &&
                        canvas.undo() && canvas.redo(),
                    "undo and redo an unrelated magnifier property");
            break;
        }
        case 3:
            require(canvas.resetEditingState() && canvas.setCanvasTool(SnowCanvasTool::Magnifier),
                    "reset selection before restoring the magnifier target");
            selectMagnifier(canvas);
            break;
        case 4:
            require(canvas.setCanvasMagnifierStyle(canvas.canvasMagnifierStyle(),
                                                   SnowCanvasMagnifierStylePropertyFactor),
                    "an explicit factor patch resets incomplete input even without a value change");
            break;
        case 5:
            for (auto modifier : {Qt::ControlModifier, Qt::AltModifier, Qt::MetaModifier})
                require(!wheel(canvas, 10, 0, modifier),
                        "modified factor wheels remain available for normal host routing");
            require(canvas.canvasToViewTransform() == camera,
                    "resetting modified factor input cannot move the camera");
            break;
        case 6:
            require(wheel(canvas, 0, 60), "changing delta kind resets incomplete pixel input");
            require(canvas.canvasMagnifierStyle().factor == 2.0,
                    "different wheel units cannot complete each other's factor step");
            break;
        }
        require(wheel(canvas, 60) && canvas.canvasMagnifierStyle().factor == 2.0,
                "editing and input boundaries discard previous incomplete factor input");
        require(wheel(canvas, 40) && canvas.canvasMagnifierStyle().factor == 2.1,
                "fresh input completes exactly one factor step after a reset");
    }
}

void wheelDirectionLimitsAndCocoaMouseClassification() {
    SnowCanvasWidget canvas;
    prepare(canvas);
    require(wheel(canvas, 40) && wheel(canvas, -60) &&
                canvas.canvasMagnifierStyle().factor == 2.0 && wheel(canvas, -40) &&
                canvas.canvasMagnifierStyle().factor == 1.9,
            "direction reversal discards incomplete movement in the previous direction");
    std::vector<SnowCanvasMagnifierEdit> commits;
    observeWheelCommits(canvas, commits);
    for (const auto [factor, direction] : {std::pair{10.0, 1}, std::pair{1.0, -1}}) {
        auto style = canvas.canvasMagnifierStyle();
        style.factor = factor;
        require(canvas.setCanvasMagnifierStyle(style, SnowCanvasMagnifierStylePropertyFactor),
                "set a factor boundary");
        const auto commitCount = commits.size();
        require(wheel(canvas, direction * 250) && wheel(canvas, direction * 50) &&
                    canvas.canvasMagnifierStyle().factor == factor && commits.size() == commitCount,
                "saturated input neither accumulates hidden movement nor publishes empty edits");
        require(wheel(canvas, -direction * 60) && canvas.canvasMagnifierStyle().factor == factor &&
                    wheel(canvas, -direction * 40) &&
                    canvas.canvasMagnifierStyle().factor == factor - direction * 0.1,
                "one complete inward step responds immediately after saturation");
    }
#ifdef Q_OS_MACOS
    auto style = canvas.canvasMagnifierStyle();
    style.factor = 2.0;
    require(canvas.setCanvasMagnifierStyle(style, SnowCanvasMagnifierStylePropertyFactor),
            "restore creation factor for Cocoa wheel classification");
    require(wheel(canvas, 2, 120, Qt::NoModifier, true, Qt::MouseEventNotSynthesized) &&
                canvas.canvasMagnifierStyle().factor == 2.1,
            "ordinary Cocoa mouse notches use angle input rather than accelerated pixel estimates");
    require(wheel(canvas, 10, 120, Qt::NoModifier, true) &&
                canvas.canvasMagnifierStyle().factor == 2.1,
            "system-synthesized precise Cocoa scrolling keeps its actual pixel delta");
#endif
}

void compoundDraggingAndDefaultIsolation() {
    for (const auto shape : {SnowCanvasRectangleShape::Rectangle, SnowCanvasRectangleShape::Ellipse,
                             SnowCanvasRectangleShape::Diamond}) {
        SnowCanvasRuntime runtime;
        SnowCanvasWidget canvas(runtime);
        prepare(canvas);
        auto style = canvas.canvasMagnifierStyle();
        style.shape = shape;
        require(canvas.setCanvasMagnifierStyle(style), "set magnifier creation shape");
        drag(canvas, {220, 160}, {300, 220});
        const auto created = magnifier(runtime);
        const auto source = center(created, QStringLiteral("source_center"));
        const auto lens = center(created, QStringLiteral("magnified_center"));
        require(near(source, lens) && created.value(QStringLiteral("width")).toDouble() == 80 &&
                    created.value(QStringLiteral("height")).toDouble() == 60 &&
                    created.value(QStringLiteral("factor")).toDouble() == 2,
                "creation retains the entire source and initially aligns both regions");

        const QPointF delta(30, 15);
        const auto sourceView = canvas.canvasToViewTransform().map(source);
        drag(canvas, sourceView, sourceView + delta);
        const auto moved = magnifier(runtime);
        require(near(center(moved, QStringLiteral("source_center")), source + delta) &&
                    near(center(moved, QStringLiteral("magnified_center")), lens + delta),
                "the source wins overlap hit testing and moves both regions");
        require(canvas.undo() && magnifier(runtime) == created,
                "one undo restores the complete source drag");
        require(canvas.redo(), "redo restores the source drag");

        const auto movedLens = center(moved, QStringLiteral("magnified_center"));
        const auto lensView = canvas.canvasToViewTransform().map(movedLens + QPointF(60, 0));
        drag(canvas, lensView, lensView + QPointF(180, 0));
        const auto separated = magnifier(runtime);
        require(near(center(separated, QStringLiteral("source_center")), source + delta) &&
                    near(center(separated, QStringLiteral("magnified_center")),
                         movedLens + QPointF(180, 0)),
                "dragging the exposed magnified region preserves the source");

        const auto resizeHandle =
            canvas.canvasToViewTransform().map(source + delta + QPointF(40, 30));
        drag(canvas, resizeHandle, resizeHandle + QPointF(20, 10));
        const auto resized = magnifier(runtime);
        require(resized.value(QStringLiteral("width")).toDouble() == 100 &&
                    resized.value(QStringLiteral("height")).toDouble() == 70 &&
                    near(center(resized, QStringLiteral("magnified_center")),
                         center(separated, QStringLiteral("magnified_center"))) &&
                    resized.value(QStringLiteral("factor")).toDouble() == 2,
                "source resize handles preserve the magnified center and factor");
        require(canvas.undo() && magnifier(runtime) == separated,
                "source resizing is independently undoable");

        style.factor = 6;
        require(canvas.applyStyleEdit(
                    SnowCanvasMagnifierEdit{style, SnowCanvasMagnifierStylePropertyFactor,
                                            SnowCanvasStyleEditTarget::Creation}),
                "creation defaults can be replicated while an element is selected");
        require(magnifier(runtime).value(QStringLiteral("factor")).toDouble() == 2,
                "default replication cannot mutate a selected magnifier");
        style.factor = 3;
        require(canvas.commitStyleEdit(
                    SnowCanvasMagnifierEdit{style, SnowCanvasMagnifierStylePropertyFactor,
                                            SnowCanvasStyleEditTarget::Selection}),
                "selected magnification can be edited");
        require(magnifier(runtime).value(QStringLiteral("factor")).toDouble() == 3 &&
                    canvas.undo() && magnifier(runtime) == separated,
                "selected styles are undoable independently of creation defaults");
    }
}

void coincidentOneTimesLensGrip() {
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    prepare(canvas);
    auto style = canvas.canvasMagnifierStyle();
    style.factor = 1;
    require(canvas.setCanvasMagnifierStyle(style), "set one-times creation default");
    drag(canvas, {220, 160}, {300, 220});
    const auto original = magnifier(runtime);
    const auto source = center(original, QStringLiteral("source_center"));
    const auto sourceView = canvas.canvasToViewTransform().map(source);
    mouse(canvas, QEvent::MouseButtonPress, sourceView, Qt::LeftButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseButtonRelease, sourceView, Qt::LeftButton, Qt::NoButton);
    const auto grip = canvas.canvasToViewTransform().map(source + QPointF(0, 54));
    drag(canvas, grip, grip + QPointF(150, 0));
    const auto moved = magnifier(runtime);
    require(near(center(moved, QStringLiteral("source_center")), source) &&
                near(center(moved, QStringLiteral("magnified_center")), source + QPointF(150, 0)),
            "the external move grip separates a coincident one-times lens");
    require(canvas.undo() && magnifier(runtime) == original,
            "lens grip dragging remains a single undoable operation");
}

class DamageTrackingCanvas final : public SnowCanvasWidget {
  public:
    QRegion painted;

  protected:
    void paintEvent(QPaintEvent* event) override {
        painted += event->region();
        SnowCanvasWidget::paintEvent(event);
    }
};

class MutableImageRenderer final : public SnowCanvasCustomRenderer {
  public:
    QImage image;
    QRectF canvasRect;
    bool sourcesEnabled = true;
    std::uint64_t revision = 1;

    explicit MutableImageRenderer(QSize size) : image(size, QImage::Format_ARGB32_Premultiplied) {
        image.fill(Qt::blue);
    }

    std::uint64_t contentRevision() const override {
        return revision;
    }

    QList<SnowCanvasBaseImageSource> baseImageSources() const override {
        return sourcesEnabled ? QList<SnowCanvasBaseImageSource>{{image, canvasRect, {}}}
                              : QList<SnowCanvasBaseImageSource>{};
    }

    void renderBeforeCanvas(QPainter& painter, const SnowCanvasRenderContext& context) override {
        painter.save();
        painter.setWorldTransform(context.canvasToViewTransform, true);
        painter.drawImage(canvasRect, image);
        painter.restore();
    }
};

void prepareDamageCanvas(DamageTrackingCanvas& canvas, MutableImageRenderer& renderer,
                         double zoom = 1.0) {
    canvas.resize(400, 240);
    canvas.show();
    QApplication::processEvents();
    require(canvas.setViewportCamera(200, 120, zoom), "configure partial-image damage camera");
    renderer.canvasRect = canvas.canvasToViewTransform().inverted().mapRect(QRectF(canvas.rect()));
    canvas.setCustomRenderer(&renderer);
    QApplication::processEvents();
}

void addSeparatedLens(SnowCanvasWidget& canvas, const QRectF& source, const QPointF& delta) {
    require(canvas.setCanvasTool(SnowCanvasTool::Magnifier), "activate the damage-test lens");
    auto style = canvas.canvasMagnifierStyle();
    style.factor = 2;
    style.strokeWidth = 0;
    style.cornerRadii = {};
    style.connectionType = SnowCanvasMagnifierConnectionType::None;
    style.sourceOutline = false;
    require(canvas.setCanvasMagnifierStyle(style), "configure the damage-test lens");
    drag(canvas, source.topLeft(), source.bottomRight());
    mouse(canvas, QEvent::MouseButtonPress, source.center(), Qt::LeftButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseButtonRelease, source.center(), Qt::LeftButton, Qt::NoButton);
    const QPointF grip = source.center() + QPointF(0, source.height() + 24);
    drag(canvas, grip, grip + delta);
    require(canvas.resetEditingState(), "finish the separated damage-test lens");
}

void addBlur(SnowCanvasWidget& canvas, const QRectF& bounds) {
    require(canvas.setCanvasTool(SnowCanvasTool::RectangleFilter), "activate damage-test blur");
    require(canvas.setCanvasFilterStyle({SnowCanvasFilterType::GaussianBlur, 1, 1, 2},
                                        SnowCanvasFilterStylePropertyType |
                                            SnowCanvasFilterStylePropertyStrength),
            "configure damage-test blur support");
    drag(canvas, bounds.topLeft(), bounds.bottomRight());
    require(canvas.resetEditingState(), "finish the damage-test blur");
}

QImage fullRender(SnowCanvasWidget& canvas) {
    const qreal dpr = canvas.devicePixelRatioF();
    QImage output(QSize(qCeil(canvas.width() * dpr), qCeil(canvas.height() * dpr)),
                  QImage::Format_ARGB32_Premultiplied);
    output.setDevicePixelRatio(dpr);
    output.fill(Qt::transparent);
    QPainter painter(&output);
    canvas.render(&painter);
    return output;
}

int changedPixels(const QImage& before, const QImage& after, const QRect& bounds) {
    int changed = 0;
    const qreal dpr = before.devicePixelRatio();
    for (int y = 0; y < before.height(); ++y) {
        for (int x = 0; x < before.width(); ++x) {
            if (bounds.contains(QPoint(qFloor(x / dpr), qFloor(y / dpr))) &&
                before.pixel(x, y) != after.pixel(x, y))
                ++changed;
        }
    }
    return changed;
}

void requireChangedPixelsRepainted(const QImage& before, const QImage& after,
                                   const QRegion& painted) {
    int changed = 0;
    const qreal dpr = before.devicePixelRatio();
    for (int y = 0; y < before.height(); ++y) {
        for (int x = 0; x < before.width(); ++x) {
            if (before.pixel(x, y) == after.pixel(x, y))
                continue;
            ++changed;
            require(painted.contains(QPoint(qFloor(x / dpr), qFloor(y / dpr))),
                    "partial original-image damage must repaint every changed output pixel");
        }
    }
    require(changed > 0, "the partial original-image change must affect rendered pixels");
}

void magnifierSelectionDragsAllSelectedBodies() {
    for (bool dragSource : {false, true}) {
        SnowCanvasRuntime runtime;
        SnowCanvasWidget canvas(runtime);
        prepare(canvas);
        drag(canvas, {220, 160}, {300, 220});
        selectMagnifier(canvas);
        const auto source = center(magnifier(runtime), QStringLiteral("source_center"));
        const auto lensBody = canvas.canvasToViewTransform().map(source + QPointF(60, 0));
        drag(canvas, lensBody, lensBody + QPointF(180, 0));
        const auto originalMagnifier = magnifier(runtime);

        require(canvas.setCanvasTool(SnowCanvasTool::Shape), "create a mixed-selection shape");
        auto shapeStyle = canvas.canvasStyleToolbarState().shapeStyle;
        shapeStyle.fill = Qt::blue;
        require(canvas.setCanvasShapeStylePatch(shapeStyle, SnowCanvasShapeStylePropertyFillColor,
                                                SnowCanvasShapeKind::Rectangle),
                "make the selected rectangle body interactive");
        drag(canvas, {600, 320}, {680, 380});
        const auto rectangles = records(runtime, QStringLiteral("Rectangle"));
        require(rectangles.size() == 1, "the mixed-selection fixture contains one rectangle");
        const auto originalRectangle = rectangles.first().toObject();
        require(canvas.setCanvasTool(SnowCanvasTool::Select), "select both compound annotations");
        drag(canvas, {100, 60}, {760, 500});
        const auto selection = runtime.selectedElementIds();
        require(selection.size() == 2, "the marquee selects the magnifier and rectangle");

        const auto start = canvas.canvasToViewTransform().map(
            dragSource ? source : center(originalRectangle, QStringLiteral("center")));
        const QPointF delta(30, 15);
        mouse(canvas, QEvent::MouseButtonPress, start, Qt::LeftButton, Qt::LeftButton);
        mouse(canvas, QEvent::MouseMove, start + delta, Qt::NoButton, Qt::LeftButton);
        require(runtime.selectedElementIds() == selection &&
                    magnifier(runtime) == originalMagnifier &&
                    records(runtime, QStringLiteral("Rectangle")) == rectangles,
                "drag previews preserve the complete selection and committed document");
        mouse(canvas, QEvent::MouseButtonRelease, start + delta, Qt::LeftButton, Qt::NoButton);
        const auto movedMagnifier = magnifier(runtime);
        const auto movedRectangle =
            records(runtime, QStringLiteral("Rectangle")).first().toObject();
        require(runtime.selectedElementIds() == selection &&
                    near(center(movedMagnifier, QStringLiteral("source_center")), source + delta) &&
                    near(center(movedMagnifier, QStringLiteral("magnified_center")),
                         center(originalMagnifier, QStringLiteral("magnified_center")) + delta) &&
                    near(center(movedRectangle, QStringLiteral("center")),
                         center(originalRectangle, QStringLiteral("center")) + delta),
                "dragging either selected body moves the whole group and both magnifier regions");
        require(canvas.undo() && magnifier(runtime) == originalMagnifier &&
                    records(runtime, QStringLiteral("Rectangle")) == rectangles && canvas.redo() &&
                    magnifier(runtime) == movedMagnifier &&
                    records(runtime, QStringLiteral("Rectangle")).first().toObject() ==
                        movedRectangle,
                "the group drag is undone and redone as one complete operation");
    }
}

void wheelFactorEditsSurviveSourceAndLensDrags() {
    for (bool dragSource : {false, true}) {
        SnowCanvasRuntime runtime;
        SnowCanvasWidget canvas(runtime);
        prepare(canvas);
        drag(canvas, {220, 160}, {300, 220});
        selectMagnifier(canvas);
        canvas.setWheelZoomEnabled(false);
        const auto original = magnifier(runtime);
        const auto source = center(original, QStringLiteral("source_center"));
        const auto lens = center(original, QStringLiteral("magnified_center"));
        const auto camera = canvas.canvasToViewTransform();
        const auto start = camera.map(dragSource ? source : lens + QPointF(60, 0));
        std::vector<SnowCanvasMagnifierEdit> commits;
        observeWheelCommits(canvas, commits);
        mouse(canvas, QEvent::MouseButtonPress, start, Qt::LeftButton, Qt::LeftButton);
        mouse(canvas, QEvent::MouseMove, start + QPointF(20, 10), Qt::NoButton, Qt::LeftButton);
        require(wheel(canvas, 0, 120, Qt::NoModifier, true, Qt::MouseEventSynthesizedBySystem,
                      Qt::LeftButton),
                "native wheel input adjusts the factor during an active drag");
        const auto factorEdited = magnifier(runtime);
        require(
            factorEdited.value(QStringLiteral("factor")).toDouble() == 2.1 &&
                near(center(factorEdited, QStringLiteral("source_center")), source) &&
                near(center(factorEdited, QStringLiteral("magnified_center")), lens) &&
                commits.size() == 1 &&
                commits.back().target == SnowCanvasStyleEditTarget::Selection &&
                commits.back().style.factor == 2.1 && canvas.canvasMagnifierStyle().factor == 2.1 &&
                canvas.canvasToViewTransform() == camera,
            "the wheel commits only a selected factor edit while drag geometry stays transient");
        const QPointF delta(50, 25);
        mouse(canvas, QEvent::MouseMove, start + delta, Qt::NoButton, Qt::LeftButton);
        mouse(canvas, QEvent::MouseButtonRelease, start + delta, Qt::LeftButton, Qt::NoButton);
        const auto moved = magnifier(runtime);
        require(
            moved.value(QStringLiteral("factor")).toDouble() == 2.1 &&
                near(center(moved, QStringLiteral("source_center")),
                     dragSource ? source + delta : source) &&
                near(center(moved, QStringLiteral("magnified_center")), lens + delta) &&
                moved.value(QStringLiteral("width")) == original.value(QStringLiteral("width")) &&
                moved.value(QStringLiteral("height")) == original.value(QStringLiteral("height")) &&
                commits.size() == 1,
            "release preserves the wheel factor and commits the final source or lens geometry");
        require(canvas.undo() && magnifier(runtime) == factorEdited && canvas.undo() &&
                    magnifier(runtime) == original && canvas.redo() &&
                    magnifier(runtime) == factorEdited && canvas.redo() &&
                    magnifier(runtime) == moved,
                "factor and drag edits retain their independent undo and redo order");
        require(canvas.resetEditingState() && canvas.setCanvasTool(SnowCanvasTool::Magnifier) &&
                    canvas.canvasMagnifierStyle().factor == 2.1,
                "wheel edits during selected drags remember future magnifier defaults");
    }
}

void undoCreationCancelsLensPreviewAndCapture() {
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    prepare(canvas);
    const QImage empty = fullRender(canvas);
    drag(canvas, {220, 160}, {300, 220});
    selectMagnifier(canvas);
    const auto original = magnifier(runtime);
    const auto lens = center(original, QStringLiteral("magnified_center"));
    const auto start = canvas.canvasToViewTransform().map(lens + QPointF(60, 0));
    const auto end = start + QPointF(150, 20);
    mouse(canvas, QEvent::MouseButtonPress, start, Qt::LeftButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseMove, end, Qt::NoButton, Qt::LeftButton);
    require(fullRender(canvas) != empty && QWidget::mouseGrabber() == &canvas,
            "an active lens drag renders a preview and owns mouse capture");
    require(canvas.undo() && !runtime.hasDocumentContent() &&
                runtime.selectedElementIds().isEmpty() && fullRender(canvas) == empty &&
                QWidget::mouseGrabber() != &canvas,
            "undoing creation immediately removes the lens preview, selection and mouse capture");
    mouse(canvas, QEvent::MouseMove, end + QPointF(10, 10), Qt::NoButton, Qt::LeftButton);
    mouse(canvas, QEvent::MouseButtonRelease, end + QPointF(10, 10), Qt::LeftButton, Qt::NoButton);
    require(!runtime.hasDocumentContent() && fullRender(canvas) == empty &&
                !canvas.canvasHistoryState().canUndo && canvas.canvasHistoryState().canRedo,
            "late input cannot resurrect the removed lens or record a stale drag operation");
    require(canvas.redo() && magnifier(runtime) == original,
            "redo restores the original creation without the canceled lens displacement");
    selectMagnifier(canvas);
    drag(canvas, start, end);
    require(near(center(magnifier(runtime), QStringLiteral("magnified_center")),
                 lens + QPointF(150, 20)) &&
                canvas.undo() && magnifier(runtime) == original,
            "a new lens drag works and remains undoable after the canceled interaction");
}

void originalDamageClosesThroughLaterFilterPasses() {
    for (bool chained : {false, true}) {
        MutableImageRenderer renderer(QSize(400, 240));
        DamageTrackingCanvas canvas;
        prepareDamageCanvas(canvas, renderer);
        addSeparatedLens(canvas, QRectF(20, 80, 30, 30), QPointF(95, 0));
        addBlur(canvas, QRectF(90, 45, 100, 110));
        if (chained) {
            // This lens separates filter source passes, but its pristine source is unchanged.
            addSeparatedLens(canvas, QRectF(170, 80, 30, 30), QPointF(140, 0));
            addBlur(canvas, QRectF(205, 45, 70, 110));
        }
        const QImage before = fullRender(canvas);
        QApplication::processEvents();
        canvas.painted = {};
        {
            QPainter painter(&renderer.image);
            painter.fillRect(QRect(20, 80, 30, 30), Qt::red);
        }
        ++renderer.revision;
        canvas.updateBaseImageContent(QRegion(QRect(20, 80, 30, 30)));
        QApplication::processEvents();
        const QRegion painted = canvas.painted;
        const QImage after = fullRender(canvas);
        require(changedPixels(before, after, QRect(163, 65, 25, 70)) > 0,
                "the blur must propagate changed lens pixels outside the lens bounds");
        if (chained) {
            require(changedPixels(before, after, QRect(210, 65, 30, 70)) > 0,
                    "a later filter pass must sample the changed earlier blur output");
            require(!painted.contains(QPoint(325, 95)),
                    "expanded filter damage cannot invalidate a lens sampling unchanged originals");
        }
        requireChangedPixelsRepainted(before, after, painted);
        canvas.setCustomRenderer(nullptr);
    }
}

void siblingFiltersKeepTheirSharedEnteringDamage() {
    MutableImageRenderer renderer(QSize(400, 240));
    DamageTrackingCanvas canvas;
    prepareDamageCanvas(canvas, renderer);
    addSeparatedLens(canvas, QRectF(20, 80, 30, 30), QPointF(95, 0));
    addBlur(canvas, QRectF(140, 75, 80, 40));
    SnowCanvasRegionFilterParameters parameters;
    parameters.logicalSigma = 36.5; // Full-strength Gaussian, as configured by addBlur.
    parameters.devicePixelRatio = canvas.devicePixelRatioF();
    const int support =
        qCeil(snowCanvasRegionFilterSupportPixels(parameters) / parameters.devicePixelRatio);
    // The sibling must lie outside the original lens's sampling support, but
    // inside the first filter's expanded output. Reduced blur reconstruction
    // has wider support than three sigma, even where this image rounds to blue.
    const QRect sibling(170 + support, 75, 30, 40);
    addBlur(canvas, sibling);
    const QImage before = fullRender(canvas);
    QApplication::processEvents();
    canvas.painted = {};
    {
        QPainter painter(&renderer.image);
        painter.fillRect(QRect(20, 80, 30, 30), Qt::red);
    }
    ++renderer.revision;
    canvas.updateBaseImageContent(QRegion(QRect(20, 80, 30, 30)));
    QApplication::processEvents();
    const QRegion painted = canvas.painted;
    const QImage after = fullRender(canvas);
    require(changedPixels(before, after, QRect(170, 80, 40, 30)) > 0,
            "the first sibling filter must respond to changed lens pixels");
    require(changedPixels(before, after, sibling) == 0 && !painted.contains(sibling.center()),
            "sibling filters cannot consume or propagate each other's changed output");
    requireChangedPixelsRepainted(before, after, painted);
    canvas.setCustomRenderer(nullptr);
}

void sourcePixelSupportSurvivesZoomAndSourceReplacement() {
    for (double zoom : {1.0, 2.0}) {
        for (int change = 0; change < 3; ++change) {
            MutableImageRenderer renderer(QSize(40, 24));
            renderer.image.setPixelColor(5, 8, Qt::green);
            DamageTrackingCanvas canvas;
            prepareDamageCanvas(canvas, renderer, zoom);
            addSeparatedLens(canvas, QRectF(63.5, 80, 30, 30), QPointF(185, 0));
            const QImage before = fullRender(canvas);
            QApplication::processEvents();
            canvas.painted = {};
            if (change == 0) {
                renderer.image.setPixelColor(5, 8, Qt::red);
            } else if (change == 1) {
                renderer.image = QImage(QSize(400, 240), QImage::Format_ARGB32_Premultiplied);
                renderer.image.fill(Qt::blue);
            } else {
                renderer.sourcesEnabled = false;
            }
            ++renderer.revision;
            canvas.updateBaseImageContent(QRegion(QRect(50, 80, 10, 10)));
            QApplication::processEvents();
            const QRegion painted = canvas.painted;
            const QImage after = fullRender(canvas);
            require(
                changedPixels(before, after, QRect(234, 65, 61, 60)) > 0,
                "an original pixel outside the source outline contributes to lens interpolation");
            requireChangedPixelsRepainted(before, after, painted);
            canvas.setCustomRenderer(nullptr);
        }
    }
}
} // namespace

int main(int argc, char** argv) {
    qputenv("QT_QPA_PLATFORM", QByteArrayLiteral("offscreen"));
    QApplication app(argc, argv);
    wheelAccumulationPublishesCompletedCreationEdits();
    selectedWheelEditsRememberDefaultsAndPublishUndoableFactors();
    wheelCommitsPreserveTheirTargetWhenObserversDeleteTheCanvas();
    wheelPublishesHomogenizedSelectionAtTheFactorLimit();
    wheelRemaindersResetAtInputAndEditingBoundaries();
    wheelDirectionLimitsAndCocoaMouseClassification();
    wheelFactorEditsSurviveSourceAndLensDrags();
    if (app.arguments().contains(QStringLiteral("--magnifier-wheel-only")))
        return EXIT_SUCCESS;
    magnifierSelectionDragsAllSelectedBodies();
    undoCreationCancelsLensPreviewAndCapture();
    compoundDraggingAndDefaultIsolation();
    coincidentOneTimesLensGrip();
    originalDamageClosesThroughLaterFilterPasses();
    siblingFiltersKeepTheirSharedEnteringDamage();
    sourcePixelSupportSurvivesZoomAndSourceReplacement();
    return EXIT_SUCCESS;
}
