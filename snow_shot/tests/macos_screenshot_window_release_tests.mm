#include "snow_shot/platform/screenshotnative.h"

#import <AppKit/AppKit.h>
#import <IOSurface/IOSurface.h>
#import <QuartzCore/QuartzCore.h>

#include <QApplication>
#include <QColor>
#include <QEvent>
#include <QEventLoop>
#include <QPainter>
#include <QPointer>
#include <QProcess>
#include <QTimer>
#include <QWidget>
#include <QWindow>
#include <algorithm>
#include <array>
#include <cstdlib>
#include <cstdint>
#include <iostream>
#include <limits>

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(EXIT_FAILURE);
    }
}

class ScreenshotSurface final : public QWidget {
  public:
    ScreenshotSurface()
        : QWidget(nullptr, Qt::Tool | Qt::FramelessWindowHint | Qt::WindowStaysOnTopHint |
                               Qt::NoDropShadowWindowHint) {
        setAttribute(Qt::WA_TranslucentBackground);
        setAttribute(Qt::WA_NoSystemBackground);
        setAttribute(Qt::WA_ShowWithoutActivating);
        setGeometry(120, 120, 192, 128);
    }

    void releaseNativeSurface() {
        hide();
        setUpdatesEnabled(false);
        destroy(true, true);
    }

    QColor color = QColor(37, 101, 173);
    int paints = 0;

  protected:
    void paintEvent(QPaintEvent*) override {
        ++paints;
        QPainter painter(this);
        painter.fillRect(rect(), color);
    }
};

template <typename Predicate> void waitFor(Predicate ready, const char* message) {
    if (ready())
        return;
    // An idle dispatcher is not a presentation fence. Let Cocoa deliver expose,
    // paint and compositor events until the requested observable state exists.
    QEventLoop loop;
    QTimer poll;
    QTimer deadline;
    deadline.setSingleShot(true);
    bool completed = false;
    QObject::connect(&poll, &QTimer::timeout, &loop, [&] {
        if (ready()) {
            completed = true;
            loop.quit();
        }
    });
    QObject::connect(&deadline, &QTimer::timeout, &loop, &QEventLoop::quit);
    poll.start(1);
    deadline.start(5000);
    loop.exec();
    require(completed, message);
}

void retirementDoesNotMaterializeSurfaces() {
    using snow_shot::platform::retireScreenshotWindowPresentation;
    retireScreenshotWindowPresentation(nullptr);
    ScreenshotSurface surface;
    for (int attempt = 0; attempt != 3; ++attempt) {
        retireScreenshotWindowPresentation(&surface);
        require(surface.internalWinId() == 0 && surface.windowHandle() == nullptr,
                "retiring an unmaterialized screenshot must not create a native surface");
        const int previousPaints = surface.paints;
        surface.setUpdatesEnabled(true);
        surface.show();
        surface.repaint();
        waitFor([&] { return surface.paints > previousPaints; },
                "a reused screenshot must still paint");
        const auto nativeId = surface.internalWinId();
        const QPointer<QWindow> window = surface.windowHandle();
        const QRect geometry = surface.geometry();
        const int paints = surface.paints;
        surface.setUpdatesEnabled(false);
        retireScreenshotWindowPresentation(&surface);
        require(surface.isVisible() && !surface.updatesEnabled() &&
                    surface.internalWinId() == nativeId && surface.windowHandle() == window &&
                    surface.geometry() == geometry,
                "presentation retirement must preserve the QWidget until its owner hides it");
        QCoreApplication::postEvent(&surface, new QEvent(QEvent::UpdateRequest));
        QCoreApplication::processEvents();
        require(surface.paints == paints,
                "queued updates must not repaint a screenshot whose presentation is retired");
        surface.releaseNativeSurface();
        require(surface.internalWinId() == 0 && surface.windowHandle() == nullptr &&
                    window.isNull(),
                "screenshot completion must release its Qt native surface");
        retireScreenshotWindowPresentation(&surface);
        require(surface.internalWinId() == 0 && surface.windowHandle() == nullptr,
                "repeated retirement must not revive a destroyed native surface");
    }
}

CALayer* presentedSurfaceLayer(CALayer* layer) {
    if (!layer)
        return nil;
    id contents = layer.contents;
    if (contents && CFGetTypeID(reinterpret_cast<CFTypeRef>(contents)) == IOSurfaceGetTypeID())
        return layer;
    for (CALayer* child in layer.sublayers) {
        if (auto* surface = presentedSurfaceLayer(child))
            return surface;
    }
    return nil;
}

bool presentationIsEmpty(CALayer* layer) {
    if (!layer)
        return true;
    if (layer.contents)
        return false;
    for (CALayer* child in layer.sublayers) {
        if (!presentationIsEmpty(child))
            return false;
    }
    return true;
}

void clearModelContents(CALayer* layer) {
    if (!layer)
        return;
    layer.contents = nil;
    for (CALayer* child in layer.sublayers)
        clearModelContents(child);
}

QByteArray ownedUnmappedMemory() {
    // A retained NSWindow can own WindowServer pixels after the Qt view and its
    // mapped IOSurfaces are gone. Checking CALayer.contents cannot detect that.
    // Inspect this isolated native test process, not aggregate RSS or allocator
    // caches. vmmap ships with macOS; no screen capture permission is needed.
    QProcess vmmap;
    vmmap.start(QStringLiteral("/usr/bin/vmmap"),
                {QStringLiteral("-summary"), QString::number(QCoreApplication::applicationPid())});
    require(vmmap.waitForFinished(10000) && vmmap.exitStatus() == QProcess::NormalExit &&
                vmmap.exitCode() == 0,
            "vmmap must inspect the native fixture's retained memory");
    const QByteArray report = vmmap.readAllStandardOutput();
    require(report.contains("REGION TYPE") && report.contains("TOTAL"),
            "vmmap must return a complete memory region summary");
    for (const QByteArray& line : report.split('\n')) {
        if (line.trimmed().startsWith("owned unmapped ")) {
            std::cerr << "Owned-unmapped memory: " << line.constData() << '\n';
            return line.trimmed();
        }
    }
    return {};
}

quint64 ownedUnmappedCommittedBytes(const QByteArray& line) {
    if (line.isEmpty())
        return 0;
    std::array<quint64, 4> sizes{};
    size_t index = 0;
    for (QByteArray field : line.simplified().split(' ')) {
        if (field.isEmpty() || field.front() < '0' || field.front() > '9')
            continue;
        double factor = 1;
        switch (field.back()) {
        case 'K':
            factor = 1024;
            break;
        case 'M':
            factor = 1024 * 1024;
            break;
        case 'G':
            factor = 1024 * 1024 * 1024;
            break;
        default:
            break;
        }
        if (factor != 1)
            field.chop(1);
        bool valid = false;
        const double count = field.toDouble(&valid);
        require(valid && count >= 0 &&
                    count <= static_cast<double>(std::numeric_limits<qint64>::max()) / factor,
                "vmmap must provide a valid owned-unmapped byte count");
        sizes[index++] = static_cast<quint64>(count * factor);
        if (index == sizes.size()) {
            // Columns are virtual, resident, dirty and swapped. macOS 14 can
            // retain a virtual reservation after its screenshot pixels retire.
            // Dirty plus swapped counts committed ownership, including pages
            // displaced from RAM, rather than counting empty address space.
            return sizes[2] + sizes[3];
        }
    }
    require(false, "vmmap owned-unmapped row must contain all four memory sizes");
    return 0;
}

void ownedUnmappedReportUnits() {
    require(ownedUnmappedCommittedBytes({}) == 0 &&
                ownedUnmappedCommittedBytes("owned unmapped memory 512M 0K 0K 0K") == 0 &&
                ownedUnmappedCommittedBytes("owned unmapped memory 144K 0K 116K 4K") == 122880 &&
                ownedUnmappedCommittedBytes("owned unmapped 2M 0K 1.5M 16K") == 1589248,
            "owned-unmapped accounting must measure committed dirty and swapped bytes");
}

// Native focus/chrome bookkeeping can allocate a few pages while windows are
// replaced. This allowance is far below the real fixture frame, and the
// --omit-native-retirement negative control must still detect retained pixels.
constexpr quint64 kNativeBookkeepingAllowance = 64 * 1024;

void requireNoRetainedCompositorFrame(quint64 baseline) {
    waitFor(
        [&] {
            return ownedUnmappedCommittedBytes(ownedUnmappedMemory()) <=
                   baseline + kNativeBookkeepingAllowance;
        },
        "a retired native window must not retain a compositor frame above the warmup baseline "
        "and native bookkeeping allowance");
}

void requirePaintedPixels(CALayer* layer, const QColor& expected) {
    require(layer != nil, "a shown screenshot must present a real raster surface");
    IOSurfaceRef surface = reinterpret_cast<IOSurfaceRef>(layer.contents);
    require(IOSurfaceGetBytesPerElement(surface) == 4,
            "the screenshot fixture requires Qt's four-byte raster surface");
    require(IOSurfaceLock(surface, kIOSurfaceLockReadOnly, nullptr) == 0,
            "the presented screenshot surface must be readable");
    const auto* bytes = static_cast<const std::uint8_t*>(IOSurfaceGetBaseAddress(surface));
    const auto offset = (IOSurfaceGetHeight(surface) / 2) * IOSurfaceGetBytesPerRow(surface) +
                        (IOSurfaceGetWidth(surface) / 2) * 4;
    const QColor actual(bytes[offset + 2], bytes[offset + 1], bytes[offset], bytes[offset + 3]);
    require(IOSurfaceUnlock(surface, kIOSurfaceLockReadOnly, nullptr) == 0,
            "the presented screenshot surface must unlock");
    require(actual == expected, "surface reuse must present the new screenshot's pixels");
}

void retainedNativeWindowsDoNotRetainPresentation() {
    using namespace snow_shot::platform;
    // Keep the fixture application active while its last tool window is
    // repeatedly destroyed. Otherwise Cocoa may yield activation to the test
    // launcher between generations, independently of compositor retirement.
    QWidget focusOwner;
    focusOwner.setWindowTitle(QStringLiteral("Native screenshot fixture"));
    focusOwner.setGeometry(10, 10, 96, 64);
    focusOwner.show();
    focusOwner.activateWindow();
    waitFor([&] { return focusOwner.windowHandle()->isExposed(); },
            "the native fixture focus owner must be exposed");
    ScreenshotSurface surface;
    surface.resize(640, 480);
    // AppKit may keep process-wide drawing resources after native-window warmup.
    // Compare persistent ownership with that baseline rather than requiring an
    // empty vmmap category. Each actual screenshot frame must exceed this noise.
    // Unlike cooperative -activate, the interactive fixture must actually own
    // the foreground before asserting key-window retirement. This is test-only.
    [NSApp activateIgnoringOtherApps:YES];
    quint64 baseline = std::numeric_limits<quint64>::max();
    for (int sample = 0; sample != 3; ++sample) {
        @autoreleasepool {
            QCoreApplication::processEvents();
            baseline = std::min(baseline, ownedUnmappedCommittedBytes(ownedUnmappedMemory()));
        }
    }
    std::cout << "Warmup committed owned-unmapped baseline: " << baseline << " bytes" << std::endl;
    for (int generation = 0; generation != 3; ++generation) {
        NSWindow* retainedWindow = nil;
        int paints = 0;
        @autoreleasepool {
            std::cout << "Presenting retained window generation " << generation << std::endl;
            surface.color = generation % 2 == 0 ? QColor(37, 101, 173) : QColor(181, 73, 29);
            surface.setUpdatesEnabled(true);
            configureScreenshotOverlayWindow(&surface);
            surface.show();
            surface.repaint();
            NSView* view = reinterpret_cast<NSView*>(surface.internalWinId());
            NSWindow* window = view.window;
            [NSApp activateIgnoringOtherApps:YES];
            [window makeKeyAndOrderFront:nil];
            std::array<bool, 7> previousState{};
            bool reportedState = false;
            waitFor(
                [&] {
                    // Cocoa can deliver a queued activation change after Qt
                    // exposes a newly recreated tool window. Reassert focus
                    // while waiting for the actual presented frame.
                    if (!window.isKeyWindow) {
                        [NSApp activateIgnoringOtherApps:YES];
                        [window makeKeyAndOrderFront:nil];
                    }
                    CALayer* layer = presentedSurfaceLayer(view.layer);
                    const std::array<bool, 7> state{
                        NSApp.active != NO,
                        NSApp.keyWindow == window,
                        window.isKeyWindow != NO,
                        surface.windowHandle()->isExposed(),
                        layer != nil,
                        layer != nil && layer.presentationLayer != nil,
                        layer != nil && layer.presentationLayer.contents == layer.contents};
                    if (!reportedState || state != previousState) {
                        std::cout << "Native frame state (active/key/keyFlag/exposed/layer/"
                                     "presentation/contents):";
                        for (bool value : state)
                            std::cout << ' ' << value;
                        std::cout << "; activation policy "
                                  << static_cast<int>(NSApp.activationPolicy) << std::endl;
                        previousState = state;
                        reportedState = true;
                    }
                    return NSApp.keyWindow == window && window.isKeyWindow &&
                           surface.windowHandle()->isExposed() && layer != nil &&
                           layer.presentationLayer.contents == layer.contents;
                },
                "the focused screenshot must present its raster surface before retirement");
            requirePaintedPixels(presentedSurfaceLayer(view.layer), surface.color);
            require(IOSurfaceGetAllocSize(reinterpret_cast<IOSurfaceRef>(
                        presentedSurfaceLayer(view.layer).contents)) >
                        baseline + kNativeBookkeepingAllowance,
                    "the native test frame must exceed the warmup memory baseline");

            // Model AppKit retaining a retired previous key window. Retaining
            // only NSWindow deliberately leaves the pixel content's ownership
            // in the native layer tree and Qt backing store.
            retainedWindow = [window retain];
            const NSRect nativeFrame = window.frame;
            CALayer* frameLayer = window.contentView.superview.layer;
            paints = surface.paints;
            surface.setUpdatesEnabled(false);
            if (generation == 1) {
                // An earlier producer may have emptied the model tree without
                // committing it yet. Retirement must still flush that clear.
                clearModelContents(view.layer);
                clearModelContents(frameLayer);
            }
            const bool omitRetirement =
                QCoreApplication::arguments().contains(QStringLiteral("--omit-native-retirement"));
            if (omitRetirement) {
                // Keep the old compositor frame for the memory negative control.
            } else if (QCoreApplication::arguments().contains(
                           QStringLiteral("--omit-retirement-flush"))) {
                // Diagnostic negative control: keep the model-tree clear but
                // intentionally omit the production transaction flush.
                clearModelContents(view.layer);
                clearModelContents(frameLayer);
            } else {
                retireScreenshotWindowPresentation(&surface);
            }
            if (!omitRetirement)
                require(presentationIsEmpty(view.layer) && presentationIsEmpty(frameLayer),
                        "retirement must detach screenshot pixels before the first native hide");
            require(surface.isVisible() && !surface.updatesEnabled() && window.visible &&
                        window.isKeyWindow && NSApp.keyWindow == window &&
                        NSEqualRects(window.frame, nativeFrame),
                    "detaching presentation must preserve visibility, focus and geometry");
            QCoreApplication::postEvent(&surface, new QEvent(QEvent::UpdateRequest));
            if (generation == 2) {
                // Export may defer native destruction. Dispatch queued updates
                // while the retired surface still exists, then retire again.
                QCoreApplication::sendPostedEvents(&surface, QEvent::UpdateRequest);
                require(surface.paints == paints && presentationIsEmpty(view.layer) &&
                            presentationIsEmpty(frameLayer),
                        "queued Qt updates must not restore pixels on a retired presentation");
                retireScreenshotWindowPresentation(&surface);
            }
            // No event-loop turn between retirement and hide: an implicit
            // transaction commit here would mask a missing production flush.
            surface.releaseNativeSurface();
            require(retainedWindow.contentView == nil && !retainedWindow.visible,
                    "a retained retired native window must release its Qt content view");
            require(surface.internalWinId() == 0 && surface.windowHandle() == nullptr,
                    "native retirement must release the screenshot's Qt window");
        }
        // Observe persistent WindowServer ownership after temporary Cocoa
        // objects retire, while NSWindow itself remains explicitly retained.
        std::cout << "Checking retained window generation " << generation << std::endl;
        requireNoRetainedCompositorFrame(baseline);
        require(surface.paints == paints,
                "terminal surface retirement must not paint the screenshot again");
        [retainedWindow release];
    }
}
} // namespace

int main(int argc, char** argv) {
    QApplication app(argc, argv);
    app.setQuitOnLastWindowClosed(false);
    ownedUnmappedReportUnits();
    @autoreleasepool {
        retirementDoesNotMaterializeSurfaces();
    }
    if (QGuiApplication::platformName() == QStringLiteral("cocoa"))
        retainedNativeWindowsDoNotRetainPresentation();
    std::cout << "macOS screenshot window release tests passed\n";
    return EXIT_SUCCESS;
}
