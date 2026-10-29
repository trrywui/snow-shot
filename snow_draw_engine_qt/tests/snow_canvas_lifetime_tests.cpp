#include "snow_draw_engine_qt/snow_canvas_runtime.h"
#include "snow_draw_engine_qt/snow_canvas_widget.h"
#include "snow_canvas_runtime_access.h"
#include "snow_canvas_runtime_clients.h"
#include "snow_canvas_runtime_cleanup.h"
#include "snow_canvas_ffi_handles.h"
#include "snow_canvas_smart_erase.h"
#include "snow_canvas_viewport.h"

#include <QApplication>
#include <QCursor>
#include <QKeyEvent>
#include <QMouseEvent>
#include <QPointer>

#include <cstdlib>
#include <functional>
#include <iostream>
#include <memory>

namespace {

void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(1);
    }
}

class Observer final : public snow_canvas_runtime::Client {
  public:
    std::uint64_t runtimeViewportId() const override {
        return 1;
    }
    bool observesRuntimeViewport(std::uint64_t id) const override {
        return Client::observesRuntimeViewport(id) || (id != 0 && id == referenceViewportId);
    }
    void detachRuntimeForReplacement() override {
        attachment = nullptr;
        if (detach)
            detach();
    }
    void attachRuntime(SnowRuntime runtime) override {
        attachment = runtime;
    }
    void detachRuntimeOwner(SnowCanvasRuntime* owner) override {
        attachment = nullptr;
        ++ownerDetaches;
        if (ownerDetach)
            ownerDetach(*owner);
    }
    void clearRenderState() override {}
    void resetDocumentRetainedState() override {}
    bool resetEditingState(bool) override {
        if (reset)
            reset();
        return true;
    }
    void syncAfterEngineMutation() override {
        ++notifications;
        if (changed)
            changed();
    }
    void refreshStateFromEngine(bool) override {
        ++refreshes;
        syncAfterEngineMutation();
    }

    int notifications = 0;
    std::uint64_t referenceViewportId = 0;
    int refreshes = 0;
    int ownerDetaches = 0;
    std::function<void()> changed;
    std::function<void()> reset;
    std::function<void()> detach;
    std::function<void(SnowCanvasRuntime&)> ownerDetach;
    SnowRuntime attachment = nullptr;
};

void runtimeDestructionKeepsImplementationAvailableToDetachCallbacks() {
    auto runtime = std::make_unique<SnowCanvasRuntime>();
    const auto engine = snow_canvas_runtime::Access::retainHandle(*runtime);
    Observer observer;
    SnowCanvasWidget first(*runtime);
    SnowCanvasWidget second(*runtime);
    snow_canvas_runtime::Access::registerClient(*runtime, observer);
    observer.ownerDetach = [&](SnowCanvasRuntime& owner) {
        require(owner.isOwnerThread() && owner.isValid() &&
                    snow_canvas_runtime::Access::retainHandle(owner) == engine,
                "detach callbacks must retain the engine through its live runtime implementation");
    };

    runtime.reset();
    require(observer.ownerDetaches == 1 && !first.setViewportCamera(0, 0, 1) &&
                !second.setCanvasTool(SnowCanvasTool::Select),
            "runtime destruction must detach every surviving canvas before releasing its engine");
}

void runtimeRetirementDrainsNewRegistrations() {
    for (const bool destroyOwner : {false, true}) {
        auto runtime = std::make_unique<SnowCanvasRuntime>();
        const auto engineLease = snow_canvas_runtime::Access::retainHandle(*runtime);
        Observer first;
        Observer late;
        std::unique_ptr<SnowCanvasWidget> lateCanvas;
        snow_canvas_runtime::Access::registerClient(*runtime, first);
        first.ownerDetach = [&](SnowCanvasRuntime& owner) {
            snow_canvas_runtime::Access::registerClient(owner, late);
            lateCanvas = std::make_unique<SnowCanvasWidget>(owner);
            require(lateCanvas->setViewportCamera(0, 0, 1),
                    "a canvas created during retirement initially attaches to the live engine");
        };
        if (destroyOwner)
            runtime.reset();
        else
            runtime->destroyAsync();
        require(first.ownerDetaches == 1 && late.ownerDetaches == 1 && lateCanvas &&
                    !lateCanvas->setViewportCamera(0, 0, 1),
                "owner retirement must detach clients admitted by its callbacks");
        lateCanvas.reset();
    }
}

void runtimeRetirementHonorsReentrantRemovalAndOwnerDeletion() {
    for (const bool destroyOwner : {false, true}) {
        auto runtime = std::make_unique<SnowCanvasRuntime>();
        Observer first;
        Observer removed;
        Observer late;
        snow_canvas_runtime::Access::registerClient(*runtime, first);
        snow_canvas_runtime::Access::registerClient(*runtime, removed);
        first.ownerDetach = [&](SnowCanvasRuntime& owner) {
            snow_canvas_runtime::Access::unregisterClient(owner, first);
            snow_canvas_runtime::Access::unregisterClient(owner, removed);
            snow_canvas_runtime::Access::registerClient(owner, late);
            if (destroyOwner)
                runtime.reset();
        };
        runtime->destroyAsync();
        require(first.ownerDetaches == 1 && removed.ownerDetaches == 0 && late.ownerDetaches == 1,
                "retirement must cancel removed registrations and survive deletion in a callback");
    }
}

void runtimeRetirementDrainsClientsOfNestedReplacement() {
    SnowCanvasRuntime runtime;
    Observer first;
    Observer survivor;
    Observer late;
    std::unique_ptr<SnowCanvasWidget> lateCanvas;
    snow_canvas_runtime::Access::registerClient(runtime, first);
    snow_canvas_runtime::Access::registerClient(runtime, survivor);
    first.ownerDetach = [&](SnowCanvasRuntime& owner) {
        require(owner.reset(), "a detach observer can replace the remaining clients' runtime");
        snow_canvas_runtime::Access::registerClient(owner, late);
        lateCanvas = std::make_unique<SnowCanvasWidget>(owner);
    };
    runtime.destroyAsync();
    require(!runtime.isValid() && first.ownerDetaches == 1 && survivor.ownerDetaches == 1 &&
                late.ownerDetaches == 1 && lateCanvas && !lateCanvas->setViewportCamera(0, 0, 1),
            "retirement must detach a replacement engine and clients created during its callback");
}

void runtimeReplacementDrainsNewAndReregisteredClients() {
    for (const bool replaceRegistration : {false, true}) {
        SnowCanvasRuntime runtime;
        const auto retiredEngine = snow_canvas_runtime::Access::retainHandle(runtime);
        Observer first;
        Observer late;
        std::unique_ptr<SnowCanvasWidget> lateCanvas;
        snow_canvas_runtime::Access::registerClient(runtime, first);
        if (replaceRegistration)
            snow_canvas_runtime::Access::registerClient(runtime, late);
        first.detach = [&] {
            if (replaceRegistration)
                snow_canvas_runtime::Access::unregisterClient(runtime, late);
            snow_canvas_runtime::Access::registerClient(runtime, late);
            lateCanvas = std::make_unique<SnowCanvasWidget>(runtime);
        };
        require(runtime.reset(), "replacement must finish after admitting a late client");
        const auto replacement = snow_canvas_runtime::Access::handle(runtime);
        require(replacement != retiredEngine.get() && late.attachment == replacement &&
                    lateCanvas && lateCanvas->setViewportCamera(0, 0, 1),
                "replacement must detach and reattach registrations admitted during detachment");
        runtime.destroyAsync();
        require(first.ownerDetaches == 1 && late.ownerDetaches == 1 &&
                    !lateCanvas->setViewportCamera(0, 0, 1),
                "replacement clients must remain registered for subsequent retirement");
    }
}

void runtimeRetirementSupersedesPendingReplacement() {
    SnowCanvasRuntime runtime;
    Observer first;
    Observer survivor;
    snow_canvas_runtime::Access::registerClient(runtime, first);
    snow_canvas_runtime::Access::registerClient(runtime, survivor);
    first.detach = [&] { runtime.destroyAsync(); };
    require(
        !runtime.reset() && !runtime.isValid() && first.ownerDetaches == 1 &&
            survivor.ownerDetaches == 1,
        "retirement during detachment must cancel replacement instead of resurrecting an engine");
}

void registrationSnapshotsCancelRemovedObservers() {
    snow_canvas_runtime::ClientRegistry registry;
    Observer first;
    Observer replacement;
    Observer survivor;
    registry.registerClient(&first);
    registry.registerClient(&replacement);
    registry.registerClient(&survivor);
    bool changedRegistration = false;
    first.changed = [&] {
        if (!changedRegistration) {
            changedRegistration = true;
            registry.unregisterClient(&replacement);
            registry.registerClient(&replacement);
        }
    };
    registry.syncChangedViewports({1});
    require(first.notifications == 1 && replacement.notifications == 0 &&
                survivor.notifications == 1,
            "publication must cancel removed registrations and retain surviving observers");
    registry.syncChangedViewports({1});
    require(replacement.notifications == 1 && survivor.notifications == 2,
            "replacement registration must participate in subsequent publications");
}

void sourceViewportChangesNotifyTheirOwnerOnce() {
    snow_canvas_runtime::ClientRegistry registry;
    Observer retained;
    Observer direct;
    retained.referenceViewportId = 2;
    registry.registerClient(&retained);
    registry.registerClient(&direct);
    registry.syncChangedViewports({2});
    require(retained.notifications == 1 && retained.refreshes == 0 && direct.refreshes == 1,
            "a source-only change must synchronize its owner without syncing unrelated viewports");
    registry.syncChangedViewports({1, 2});
    require(retained.notifications == 2 && retained.refreshes == 0,
            "one mutation must notify a client once even when both its viewports changed");
    retained.referenceViewportId = 0;
    registry.syncChangedViewports({2});
    require(retained.refreshes == 1, "retiring a source viewport must remove the dependency");
}

void registryDestructionCancelsPublication() {
    auto registry = std::make_unique<snow_canvas_runtime::ClientRegistry>();
    Observer first;
    Observer remaining;
    registry->registerClient(&first);
    registry->registerClient(&remaining);
    first.changed = [&] { registry.reset(); };
    registry->syncChangedViewports({1});
    require(!registry && first.notifications == 1 && remaining.notifications == 0,
            "registry destruction must cancel remaining callbacks without accessing dead storage");
}

void changedViewportSnapshotsSurviveSourceMutation() {
    snow_canvas_runtime::ClientRegistry registry;
    Observer first;
    Observer survivor;
    std::vector<std::uint64_t> changedViewportIds{1};
    registry.registerClient(&first);
    registry.registerClient(&survivor);
    first.changed = [&] { changedViewportIds.clear(); };
    registry.syncChangedViewports(changedViewportIds);
    require(changedViewportIds.empty() && survivor.notifications == 1 && survivor.refreshes == 0,
            "later clients must still receive the owned viewport change after source mutation");
}

void widgetSignalsPermitOwnerDeletion() {
    const char* cases[] = {"active tool notification must permit synchronous owner deletion",
                           "aliased tool notification must permit synchronous owner deletion",
                           "text style notification must permit synchronous owner deletion",
                           "snap configuration notification must permit synchronous owner deletion",
                           "grid configuration notification must permit synchronous owner deletion",
                           "watermark preview notification must permit synchronous owner deletion",
                           "spotlight preview notification must permit synchronous owner deletion",
                           "history notification must permit synchronous owner deletion"};
    int currentCase = 0;
    const auto verify = [&](auto signal, const auto& prepare, const auto& change) {
        QPointer<SnowCanvasWidget> canvas = new SnowCanvasWidget;
        canvas->resize(320, 200);
        prepare(canvas.data());
        int notifications = 0;
        QObject context;
        QObject::connect(canvas, signal, &context, [&] {
            ++notifications;
            delete canvas.data();
        });
        change(canvas.data());
        require(canvas.isNull() && notifications == 1, cases[currentCase++]);
    };
    const auto unchanged = [](SnowCanvasWidget*) {};
    verify(
        &SnowCanvasWidget::activeToolChanged,
        [](auto* canvas) {
            require(canvas->setCanvasTool(SnowCanvasTool::Select),
                    "prepare a different tool before the notification fixture");
        },
        [](auto* canvas) { canvas->setCanvasTool(SnowCanvasTool::Shape); });
    verify(
        &SnowCanvasWidget::activeToolChanged,
        [](auto* canvas) { canvas->setCanvasTool(SnowCanvasTool::Line); },
        [](auto* canvas) { canvas->setCanvasTool(SnowCanvasTool::Arrow); });
    verify(
        &SnowCanvasWidget::styleToolbarStateChanged,
        [](auto* canvas) { canvas->setCanvasTool(SnowCanvasTool::Text); },
        [](auto* canvas) {
            auto style = canvas->canvasStyleToolbarState().textStyle;
            style.fontSize += 1;
            canvas->setCanvasTextStyle(style);
        });
    verify(&SnowCanvasWidget::snapConfigChanged, unchanged, [](auto* canvas) {
        auto config = canvas->canvasSnapConfig();
        config.enabled = !config.enabled;
        canvas->setCanvasSnapConfig(config);
    });
    verify(&SnowCanvasWidget::gridConfigChanged, unchanged, [](auto* canvas) {
        auto config = canvas->canvasGridConfig();
        config.enabled = !config.enabled;
        canvas->setCanvasGridConfig(config);
    });
    verify(&SnowCanvasWidget::watermarkPreviewApplied, unchanged, [](auto* canvas) {
        canvas->previewCanvasWatermarkConfig(canvas->canvasWatermarkConfig());
    });
    verify(&SnowCanvasWidget::spotlightPreviewApplied, unchanged, [](auto* canvas) {
        auto config = canvas->canvasSpotlightConfig();
        config.opacity = config.opacity == 0.5 ? 0.6 : 0.5;
        canvas->previewCanvasSpotlightConfig(config);
    });
    verify(
        &SnowCanvasWidget::historyStateChanged,
        [](auto* canvas) { canvas->setCanvasTool(SnowCanvasTool::Shape); },
        [](auto* canvas) {
            QPointer<SnowCanvasWidget> lifetime(canvas);
            QMouseEvent down(QEvent::MouseButtonPress, QPointF(20, 20), QPointF(20, 20),
                             Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
            QApplication::sendEvent(canvas, &down);
            if (!lifetime)
                return;
            QMouseEvent up(QEvent::MouseButtonRelease, QPointF(90, 70), QPointF(90, 70),
                           Qt::LeftButton, Qt::NoButton, Qt::NoModifier);
            QApplication::sendEvent(canvas, &up);
        });
}

void sharedRuntimePreservesSurvivingObservers() {
    SnowCanvasRuntime runtime;
    QPointer<SnowCanvasWidget> removed = new SnowCanvasWidget(runtime);
    SnowCanvasWidget survivor(runtime);
    QObject context;
    int survivorNotifications = 0;
    int documentNotifications = 0;
    QObject::connect(removed, &SnowCanvasWidget::historyStateChanged, &context,
                     [&] { delete removed.data(); });
    QObject::connect(&survivor, &SnowCanvasWidget::historyStateChanged, &context,
                     [&] { ++survivorNotifications; });
    runtime.setDocumentChangedHandler([&] { ++documentNotifications; });
    require(!runtime
                 .applyAnnotationTransaction(
                     R"({"version":1,"operations":[{"type":"rectangle","bounds":[8,8,24,24]}]})")
                 .isEmpty(),
            "shared runtime mutation must succeed");
    require(removed.isNull() && survivorNotifications == 1 && documentNotifications == 1,
            "removing one canvas must preserve surviving canvas and document observers");
    require(survivor.canvasHistoryState().canUndo,
            "surviving canvas must synchronize the published history");
}

void runtimeDeletionCancelsDocumentPublication() {
    auto runtime = std::make_unique<SnowCanvasRuntime>();
    SnowCanvasWidget canvas(*runtime);
    int documentNotifications = 0;
    runtime->setDocumentChangedHandler([&] { ++documentNotifications; });
    QObject::connect(&canvas, &SnowCanvasWidget::historyStateChanged, &canvas,
                     [&] { runtime.reset(); });
    const QByteArray result = runtime->applyAnnotationTransaction(
        R"({"version":1,"operations":[{"type":"rectangle","bounds":[8,8,24,24]}]})");
    require(!result.isEmpty() && !runtime && documentNotifications == 0,
            "runtime deletion during canvas publication must cancel its document callback");
}

void documentHandlerRetainsItsCallable() {
    auto runtime = std::make_unique<SnowCanvasRuntime>();
    int notifications = 0;
    runtime->setDocumentChangedHandler([&, captured = std::make_shared<int>(7)] {
        runtime.reset();
        require(*captured == 7, "document callback must outlive destruction of its owning runtime");
        ++notifications;
    });
    const QByteArray result = runtime->applyAnnotationTransaction(
        R"({"version":1,"operations":[{"type":"rectangle","bounds":[8,8,24,24]}]})");
    require(!result.isEmpty() && !runtime && notifications == 1,
            "document callback must allow synchronous runtime deletion");
}

void runtimeResetAllowsOwnerDeletion() {
    auto runtime = std::make_unique<SnowCanvasRuntime>();
    Observer observer;
    snow_canvas_runtime::Access::registerClient(*runtime, observer);
    observer.reset = [&] { runtime.reset(); };
    const bool reset = runtime->clearDocumentPreservingViewports();
    require(!reset && !runtime, "reset must cancel after a client deletes its runtime");
}

void coordinatorRemovalRetainsItsCallable() {
    std::unique_ptr<snow_canvas_smart_erase::Coordinator> coordinator;
    int notifications = 0;
    coordinator = std::make_unique<snow_canvas_smart_erase::Coordinator>(
        [&, captured = std::make_shared<int>(7)] {
            coordinator.reset();
            require(*captured == 7, "smart erase callback must survive owner deletion");
            ++notifications;
        });
    QImage image(8, 8, QImage::Format_ARGB32_Premultiplied);
    image.fill(Qt::white);
    SnowCanvasBaseImageSource source;
    source.image = image;
    source.canvasRect = QRectF(0, 0, 8, 8);
    source.coverage = source.canvasRect;
    coordinator->setSources(&notifications, {source}, false);
    coordinator->removeSources(&notifications);
    require(!coordinator && notifications == 1,
            "source removal must not continue through its deleted coordinator");
}

void keyResolverRetainsItsCallable() {
    QPointer<SnowCanvasWidget> canvas = new SnowCanvasWidget;
    int notifications = 0;
    canvas->setCommandKeyResolver([&, captured = std::make_shared<int>(7)](const QKeyEvent&) {
        delete canvas.data();
        require(*captured == 7, "key resolver must survive owner deletion");
        ++notifications;
        return Qt::Key_A;
    });
    QKeyEvent event(QEvent::KeyPress, Qt::Key_A, Qt::NoModifier);
    QApplication::sendEvent(canvas, &event);
    require(canvas.isNull() && notifications == 1,
            "key dispatch must cancel after its resolver deletes the canvas");
}

void presentationLeaseOutlivesRuntimeAndViewport() {
    auto runtime = std::make_unique<SnowCanvasRuntime>();
    auto engine = snow_canvas_runtime::Access::retainHandle(*runtime);
    SnowCanvasViewport viewport;
    require(viewport.create(engine.get(), snow_canvas_viewport::defaultEngineConfig()),
            "presentation lease fixture must create a viewport");
    ScopedPresentationUpdate presentation(engine.get());
    require(presentation.isActive(), "presentation lease fixture must begin a transaction");
    ScopedChangedViewportList changed;
    require(snow_viewport_set_surface_size(engine.get(), viewport.get(), 320, 200) == SNOW_OK,
            "presentation lease fixture must enqueue a viewport update");
    runtime.reset();
    snow_canvas_runtime::waitForRuntimeCleanup();
    viewport.reset();
    require(presentation.finish(changed),
            "presentation teardown must retain its engine and ignore a retired viewport");
    engine.reset();
    snow_canvas_runtime::waitForRuntimeCleanup();
}

void nativeCursorEventsAllowOwnerDeletion() {
    class DeletionFilter final : public QObject {
      public:
        std::function<void()> remove;
        bool eventFilter(QObject*, QEvent* event) override {
            if (event->type() == QEvent::CursorChange) {
                remove();
                return true;
            }
            return false;
        }
    };
    QPointer<SnowCanvasWidget> canvas = new SnowCanvasWidget;
    canvas->resize(320, 200);
    canvas->setCanvasTool(SnowCanvasTool::FreeDraw);
    canvas->unsetCursor();
    DeletionFilter filter;
    int notifications = 0;
    filter.remove = [&] {
        if (notifications != 0) {
            return;
        }
        ++notifications;
        delete canvas.data();
    };
    canvas->installEventFilter(&filter);
    QMouseEvent move(QEvent::MouseMove, QPointF(20, 20), QPointF(20, 20), Qt::NoButton,
                     Qt::NoButton, Qt::NoModifier);
    QApplication::sendEvent(canvas, &move);
    require(canvas.isNull() && notifications == 1,
            "native cursor dispatch inside a presentation scope must permit owner deletion");
}

void nestedToolChangeSupersedesEarlierRequest() {
    SnowCanvasWidget canvas;
    bool nested = false;
    QObject::connect(&canvas, &SnowCanvasWidget::activeToolChanged, &canvas, [&] {
        if (!nested) {
            nested = true;
            require(canvas.setCanvasTool(SnowCanvasTool::Arrow),
                    "nested tool selection must complete");
        }
    });
    require(!canvas.setCanvasTool(SnowCanvasTool::Line) && nested &&
                canvas.canvasTool() == SnowCanvasTool::Arrow,
            "earlier tool publication must not overwrite a later Line/Arrow alias request");

    QObject::disconnect(&canvas, nullptr, &canvas, nullptr);
    QCursor chosenCursor;
    QObject::connect(&canvas, &SnowCanvasWidget::activeToolChanged, &canvas, [&] {
        if (canvas.canvasTool() == SnowCanvasTool::Line) {
            require(canvas.setCanvasTool(SnowCanvasTool::FreeDraw),
                    "nested native tool selection must complete");
            chosenCursor = canvas.cursor();
        }
    });
    require(!canvas.setCanvasTool(SnowCanvasTool::Line) &&
                canvas.canvasTool() == SnowCanvasTool::FreeDraw && canvas.cursor() == chosenCursor,
            "an earlier alias request must not overwrite a nested tool's cursor");
}

void nestedSynchronizationPreservesPendingNotifications() {
    for (const bool changeOtherProperties : {false, true}) {
        SnowCanvasWidget canvas;
        require(canvas.setCanvasTool(SnowCanvasTool::Select),
                "prepare pending property notification fixture");
        const auto previousSnap = canvas.canvasSnapConfig();
        const auto previousGrid = canvas.canvasGridConfig();
        int toolNotifications = 0;
        int styleNotifications = 0;
        int snapNotifications = 0;
        int gridNotifications = 0;
        QObject::connect(&canvas, &SnowCanvasWidget::activeToolChanged, &canvas, [&] {
            ++toolNotifications;
            auto snap = previousSnap;
            auto grid = previousGrid;
            if (changeOtherProperties) {
                snap.enabled = !snap.enabled;
                grid.enabled = !grid.enabled;
            }
            require(canvas.setCanvasSnapConfig(snap) && canvas.setCanvasGridConfig(grid),
                    "nested independent config changes must succeed");
        });
        QObject::connect(&canvas, &SnowCanvasWidget::styleToolbarStateChanged, &canvas, [&] {
            ++styleNotifications;
            require(canvas.canvasTool() == SnowCanvasTool::Text,
                    "pending style notification must describe the current tool");
        });
        QObject::connect(&canvas, &SnowCanvasWidget::snapConfigChanged, &canvas,
                         [&] { ++snapNotifications; });
        QObject::connect(&canvas, &SnowCanvasWidget::gridConfigChanged, &canvas,
                         [&] { ++gridNotifications; });
        const bool selected = canvas.setCanvasTool(SnowCanvasTool::Text);
        require(styleNotifications == 1 && toolNotifications == 1,
                "nested no-op or unrelated synchronization must retain pending style notification");
        require(
            selected && canvas.canvasTool() == SnowCanvasTool::Text,
            "independent nested synchronization must not report successful tool change as failed");
        require(snapNotifications == (changeOtherProperties ? 1 : 0) &&
                    gridNotifications == (changeOtherProperties ? 1 : 0),
                "nested config publications must deliver each changed property once");
    }
}

void newerToolPublicationUsesCurrentState() {
    SnowCanvasWidget canvas;
    require(canvas.setCanvasTool(SnowCanvasTool::Select),
            "prepare superseding property notification fixture");
    bool nested = false;
    int styleNotifications = 0;
    QObject::connect(&canvas, &SnowCanvasWidget::activeToolChanged, &canvas, [&] {
        if (!nested) {
            nested = true;
            require(canvas.setCanvasTool(SnowCanvasTool::Shape),
                    "newer tool request must complete during property publication");
        }
    });
    QObject::connect(&canvas, &SnowCanvasWidget::styleToolbarStateChanged, &canvas, [&] {
        ++styleNotifications;
        require(canvas.canvasTool() == SnowCanvasTool::Shape,
                "superseded tool must not publish stale style state");
    });
    require(!canvas.setCanvasTool(SnowCanvasTool::Text) && nested &&
                canvas.canvasTool() == SnowCanvasTool::Shape && styleNotifications == 1,
            "real tool supersession must preserve only the current pending style notification");
}

void nestedStyleObserverPreservesHistoryNotification() {
    SnowCanvasWidget canvas;
    canvas.resize(320, 200);
    require(canvas.setCanvasTool(SnowCanvasTool::Shape), "prepare history publication fixture");
    const auto sendMouse = [&](QEvent::Type type, const QPointF& point, Qt::MouseButton button,
                               Qt::MouseButtons buttons) {
        QMouseEvent event(type, point, point, button, buttons, Qt::NoModifier);
        QApplication::sendEvent(&canvas, &event);
    };
    sendMouse(QEvent::MouseButtonPress, QPointF(20, 20), Qt::LeftButton, Qt::LeftButton);
    sendMouse(QEvent::MouseMove, QPointF(90, 70), Qt::NoButton, Qt::LeftButton);
    sendMouse(QEvent::MouseButtonRelease, QPointF(90, 70), Qt::LeftButton, Qt::NoButton);
    require(canvas.setCanvasTool(SnowCanvasTool::Select), "select history publication fixture");
    sendMouse(QEvent::MouseButtonPress, QPointF(20, 45), Qt::LeftButton, Qt::LeftButton);
    sendMouse(QEvent::MouseButtonRelease, QPointF(20, 45), Qt::LeftButton, Qt::NoButton);
    require(canvas.canvasStyleToolbarState().source ==
                    SnowCanvasStyleToolbarSource::SelectedRectangle &&
                canvas.canvasHistoryState().canUndo && !canvas.canvasHistoryState().canRedo,
            "history fixture must select the rectangle before undo");
    int styleNotifications = 0;
    int historyNotifications = 0;
    QObject::connect(&canvas, &SnowCanvasWidget::styleToolbarStateChanged, &canvas, [&] {
        ++styleNotifications;
        require(canvas.setCanvasSnapConfig(canvas.canvasSnapConfig()),
                "history fixture nested config synchronization must succeed");
    });
    QObject::connect(&canvas, &SnowCanvasWidget::historyStateChanged, &canvas,
                     [&] { ++historyNotifications; });
    require(canvas.undo(), "undo the selected rectangle");
    require(styleNotifications == 1 && historyNotifications == 1 &&
                !canvas.canvasHistoryState().canUndo && canvas.canvasHistoryState().canRedo &&
                canvas.canvasStyleToolbarState().source !=
                    SnowCanvasStyleToolbarSource::SelectedRectangle,
            "nested style observer must not consume a pending history notification");
}

void angleAndAutoFilterObserversPreserveOtherProperties() {
    {
        SnowCanvasWidget canvas;
        canvas.resize(600, 360);
        require(canvas.setCanvasTool(SnowCanvasTool::Angle),
                "prepare angle target publication fixture");
        int angleNotifications = 0;
        int historyNotifications = 0;
        QObject::connect(&canvas, &SnowCanvasWidget::angleAdjustmentTargetChanged, &canvas, [&] {
            ++angleNotifications;
            require(canvas.setCanvasGridConfig(canvas.canvasGridConfig()),
                    "angle observer can synchronize an unchanged config");
        });
        QObject::connect(&canvas, &SnowCanvasWidget::historyStateChanged, &canvas,
                         [&] { ++historyNotifications; });
        for (const QPointF point : {QPointF(500, 180), QPointF(300, 180), QPointF(300, 60)}) {
            QMouseEvent down(QEvent::MouseButtonPress, point, point, Qt::LeftButton, Qt::LeftButton,
                             Qt::NoModifier);
            QApplication::sendEvent(&canvas, &down);
            QMouseEvent up(QEvent::MouseButtonRelease, point, point, Qt::LeftButton, Qt::NoButton,
                           Qt::NoModifier);
            QApplication::sendEvent(&canvas, &up);
        }
        require(angleNotifications > 0 && historyNotifications == 1 &&
                    canvas.canvasHistoryState().canUndo,
                "nested angle observer must retain pending history publication");
    }
    {
        SnowCanvasRuntime runtime;
        SnowCanvasWidget canvas(runtime);
        SnowCanvasWidget survivor(runtime);
        int regionNotifications = 0;
        int gridNotifications = 0;
        QObject::connect(&survivor, &SnowCanvasWidget::autoFilterRegionsChanged, &survivor, [&] {
            ++regionNotifications;
            auto grid = survivor.canvasGridConfig();
            grid.enabled = !grid.enabled;
            require(survivor.setCanvasGridConfig(grid),
                    "auto filter observer can change an unrelated property");
        });
        QObject::connect(&survivor, &SnowCanvasWidget::gridConfigChanged, &survivor,
                         [&] { ++gridNotifications; });
        const SnowCanvasAutoFilterRecord record{
            QRectF(0, 0, 100, 100), {{1, QRectF(20, 30, 30, 20), QStringLiteral("text")}}};
        require(canvas.setAutoFilterRegions(record) && regionNotifications == 1 &&
                    gridNotifications == 1 && survivor.autoFilterRegions().has_value(),
                "auto filter and nested config publications must each reach surviving observers");
    }
}

void nativeCursorSynchronizationPreservesConsumedStyle() {
    class SynchronizationFilter final : public QObject {
      public:
        SnowCanvasWidget* canvas = nullptr;
        bool synchronized = false;
        bool eventFilter(QObject*, QEvent* event) override {
            if (event->type() == QEvent::CursorChange && !synchronized) {
                synchronized = true;
                require(canvas->setCanvasSnapConfig(canvas->canvasSnapConfig()),
                        "native cursor observer can synchronize unchanged config");
            }
            return false;
        }
    };
    SnowCanvasWidget canvas;
    canvas.resize(320, 200);
    require(canvas.setCanvasTool(SnowCanvasTool::FreeDraw),
            "prepare native cursor publication fixture");
    require(canvas.canvasStyleToolbarState().source ==
                SnowCanvasStyleToolbarSource::DefaultFreeDraw,
            "native cursor fixture must expose the active free draw style");
    QMouseEvent move(QEvent::MouseMove, QPointF(20, 20), QPointF(20, 20), Qt::NoButton,
                     Qt::NoButton, Qt::NoModifier);
    QApplication::sendEvent(&canvas, &move);
    SynchronizationFilter filter;
    filter.canvas = &canvas;
    canvas.installEventFilter(&filter);
    int styleNotifications = 0;
    QObject::connect(&canvas, &SnowCanvasWidget::styleToolbarStateChanged, &canvas,
                     [&] { ++styleNotifications; });
    auto style = canvas.canvasStyleToolbarState().shapeStyle;
    style.strokeWidth += 3;
    require(canvas.setCanvasShapeStylePatch(style, SnowCanvasShapeStylePropertyStrokeWidth,
                                            SnowCanvasShapeKind::FreeDraw),
            "native cursor observer must not fail a successful style update");
    if (!filter.synchronized || styleNotifications != 1 ||
        canvas.canvasStyleToolbarState().shapeStyle.strokeWidth != style.strokeWidth) {
        std::cerr << "native cursor fixture: synchronized=" << filter.synchronized
                  << " style=" << styleNotifications
                  << " width=" << canvas.canvasStyleToolbarState().shapeStyle.strokeWidth
                  << " requested=" << style.strokeWidth << '\n';
    }
    require(
        filter.synchronized && styleNotifications == 1 &&
            canvas.canvasStyleToolbarState().shapeStyle.strokeWidth == style.strokeWidth,
        "property changes must be retained before native cursor callbacks consume the snapshot");
}

void nestedRuntimeReplacementCancelsRetiredPublication() {
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    const SnowCanvasTool resetTool = canvas.canvasTool();
    int resets = 0;
    int staleStyleSignals = 0;
    const auto replaceRuntime =
        QObject::connect(&canvas, &SnowCanvasWidget::activeToolChanged, &canvas, [&] {
            ++resets;
            require(runtime.reset(), "nested runtime replacement must complete");
        });
    QObject::connect(&canvas, &SnowCanvasWidget::styleToolbarStateChanged, &canvas,
                     [&] { ++staleStyleSignals; });
    require(!canvas.setCanvasTool(SnowCanvasTool::Text) && resets == 1 &&
                canvas.canvasTool() == resetTool && staleStyleSignals == 0,
            "retired publication must stop after nested runtime replacement");
    QObject::disconnect(replaceRuntime);
    require(canvas.setCanvasTool(SnowCanvasTool::Text) && staleStyleSignals == 1,
            "replacement attachment must resume publication without retired pending flags");
}

void replacementAttachmentPublishesInsideRetiredObserver() {
    SnowCanvasRuntime runtime;
    SnowCanvasWidget canvas(runtime);
    require(canvas.setCanvasTool(SnowCanvasTool::Select),
            "prepare immediate replacement mutation fixture");
    bool replaced = false;
    int toolNotifications = 0;
    int styleNotifications = 0;
    QObject::connect(&canvas, &SnowCanvasWidget::activeToolChanged, &canvas, [&] {
        ++toolNotifications;
        if (!replaced) {
            replaced = true;
            require(runtime.reset() && canvas.setCanvasTool(SnowCanvasTool::FreeDraw),
                    "replacement can publish its new tool before retired observer returns");
        }
    });
    QObject::connect(&canvas, &SnowCanvasWidget::styleToolbarStateChanged, &canvas, [&] {
        ++styleNotifications;
        require(canvas.canvasTool() == SnowCanvasTool::FreeDraw,
                "replacement observer must see fresh style state");
    });
    require(!canvas.setCanvasTool(SnowCanvasTool::Text) && replaced &&
                canvas.canvasTool() == SnowCanvasTool::FreeDraw && toolNotifications == 2 &&
                styleNotifications == 1,
            "retired publisher must not strand the replacement attachment's pending signals");
}

void nestedRuntimeReplacementSupersedesEarlierReplacement() {
    SnowCanvasRuntime runtime;
    Observer observer;
    Observer survivor;
    snow_canvas_runtime::Access::registerClient(runtime, observer);
    snow_canvas_runtime::Access::registerClient(runtime, survivor);
    bool nested = false;
    SnowRuntime chosen = nullptr;
    observer.detach = [&] {
        if (!nested) {
            nested = true;
            require(runtime.reset(), "nested replacement must succeed");
            chosen = snow_canvas_runtime::Access::handle(runtime);
        }
    };
    require(!runtime.reset() && nested && snow_canvas_runtime::Access::handle(runtime) == chosen &&
                survivor.attachment == chosen,
            "earlier replacement must not overwrite the runtime selected during detachment");
    snow_canvas_runtime::Access::unregisterClient(runtime, observer);
    snow_canvas_runtime::Access::unregisterClient(runtime, survivor);
}

} // namespace

int main(int argc, char** argv) {
    QApplication app(argc, argv);
    runtimeDestructionKeepsImplementationAvailableToDetachCallbacks();
    runtimeRetirementDrainsNewRegistrations();
    runtimeRetirementHonorsReentrantRemovalAndOwnerDeletion();
    runtimeRetirementDrainsClientsOfNestedReplacement();
    runtimeReplacementDrainsNewAndReregisteredClients();
    runtimeRetirementSupersedesPendingReplacement();
    registrationSnapshotsCancelRemovedObservers();
    sourceViewportChangesNotifyTheirOwnerOnce();
    registryDestructionCancelsPublication();
    changedViewportSnapshotsSurviveSourceMutation();
    widgetSignalsPermitOwnerDeletion();
    sharedRuntimePreservesSurvivingObservers();
    runtimeDeletionCancelsDocumentPublication();
    documentHandlerRetainsItsCallable();
    runtimeResetAllowsOwnerDeletion();
    coordinatorRemovalRetainsItsCallable();
    keyResolverRetainsItsCallable();
    presentationLeaseOutlivesRuntimeAndViewport();
    nativeCursorEventsAllowOwnerDeletion();
    nestedToolChangeSupersedesEarlierRequest();
    nestedSynchronizationPreservesPendingNotifications();
    newerToolPublicationUsesCurrentState();
    nestedStyleObserverPreservesHistoryNotification();
    angleAndAutoFilterObserversPreserveOtherProperties();
    nativeCursorSynchronizationPreservesConsumedStyle();
    nestedRuntimeReplacementCancelsRetiredPublication();
    replacementAttachmentPublishesInsideRetiredObserver();
    nestedRuntimeReplacementSupersedesEarlierReplacement();
    snow_canvas_runtime::waitForRuntimeCleanup();
    std::cout << "Canvas lifetime tests passed\n";
    return 0;
}
