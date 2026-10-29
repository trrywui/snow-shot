#include "snow_shot/presentation/crosshaircursor.h"
#include "snow_draw_engine_qt/snow_canvas_widget.h"
#include "../src/presentation/overlay/screenshotoverlaycursor.h"

#include <QApplication>
#include <QImage>
#include <cstdlib>
#include <iostream>

#ifdef Q_OS_WIN
#include "snow_capture.h"
#include <qt_windows.h>
#endif

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(1);
    }
}

void artworkAndScaling() {
#ifdef Q_OS_WIN
    SnowCaptureCursorImageView view{};
    require(snow_capture_crosshair_cursor_image(&view) != 0, "shared artwork must be available");
    require(view.width == 32 && view.height == 32 && view.hotspot_x == 16 && view.hotspot_y == 16 &&
                view.rgba_len == 32 * 32 * 4,
            "shared base image must retain its size and centered hotspot");
    for (qreal dpr : {1.0, 1.08, 1.25, 1.5, 1.52, 2.0}) {
        const auto cursor = snow_shot::presentation::crosshairCursor(dpr);
        require(cursor.shape() == Qt::BitmapCursor && cursor.hotSpot() == QPoint(16, 16),
                "custom crosshair must be a bitmap with the center hotspot");
        const auto image = cursor.pixmap().toImage().convertToFormat(QImage::Format_RGBA8888);
        const int physicalHotspot = qRound(16 * dpr);
        require(image.size() == QSize(physicalHotspot * 2, physicalHotspot * 2) &&
                    image.devicePixelRatio() == dpr,
                "crosshair raster must follow the display DPI");
        for (int y = 0; y < image.height(); ++y) {
            for (int x = 0; x < image.width(); ++x) {
                const auto sourceX = static_cast<quint64>(x) * view.width / image.width();
                const auto sourceY = static_cast<quint64>(y) * view.height / image.height();
                const auto* pixel = view.rgba + (sourceY * view.width + sourceX) * 4;
                require(image.pixelColor(x, y) == QColor(pixel[0], pixel[1], pixel[2], pixel[3]),
                        "Qt pixels must match the generator's nearest-neighbor mapping");
            }
        }
        require(cursor == snow_shot::presentation::crosshairCursor(dpr),
                "repeated crosshair requests must reuse the cursor");
        const int outline = physicalHotspot - qMax(1, image.width() / 32);
        require(image.pixelColor(physicalHotspot, physicalHotspot) == QColor(Qt::white) &&
                    image.pixelColor(outline, physicalHotspot) == QColor(Qt::white) &&
                    image.pixelColor(outline, outline) == QColor(Qt::black) &&
                    image.pixelColor(0, 0).alpha() == 0,
                "crosshair must have white fill, black outline and transparent corners");
    }
#else
    require(snow_shot::presentation::crosshairCursor().shape() == Qt::CrossCursor,
            "other platforms retain the native crosshair");
#endif
}

void canvasLayers() {
    SnowCanvasWidget canvas;
    snow_shot::presentation::configureCanvasCrosshairCursor(canvas);
    require(canvas.setCanvasTool(SnowCanvasTool::Shape), "activate a crosshair tool");
    const auto expected = snow_shot::presentation::crosshairCursor(canvas.devicePixelRatioF());
    require(canvas.cursor() == expected, "Snow Shot canvas tools must use the shared crosshair");
    canvas.setCursorForLayer(SnowCanvasCursorLayer::Host, QCursor(Qt::WaitCursor));
    require(canvas.cursor().shape() == Qt::WaitCursor,
            "non-crosshair host cursors retain priority");
    canvas.setCursorForLayer(SnowCanvasCursorLayer::CanvasTool, QCursor(Qt::IBeamCursor));
    canvas.clearCursorForLayer(SnowCanvasCursorLayer::Host);
    require(canvas.cursor().shape() == Qt::IBeamCursor, "clearing host reveals the latest tool");
    canvas.setCursorForLayer(SnowCanvasCursorLayer::Host, QCursor(Qt::CrossCursor));
    require(canvas.cursor() == expected, "host crosshairs use the same artwork as tool cursors");
}

void mixedDpiOverlayRevealsKeepThePointerCursor() {
    const QRect displays[]{QRect(-1920, 0, 1920, 1080), QRect(0, 0, 2560, 1440)};
    const qreal scales[]{1.0, 1.5};
    QCursor displayedCursor(Qt::ArrowCursor);
    QPoint cursorPosition;
    int commits = 0;
    const auto revealOrFocus = [&](int display) {
        snow_shot::presentation::detail::commitOverlayCursor(
            displays[display], cursorPosition, [&]() {
                ++commits;
                displayedCursor = snow_shot::presentation::crosshairCursor(scales[display]);
            });
    };
    const auto requirePointerCursor = [&](int owner) {
        require(displayedCursor == snow_shot::presentation::crosshairCursor(scales[owner]),
                "revealing or focusing another display must preserve the pointer's cursor scale");
    };

    for (int owner : {0, 1}) {
        cursorPosition = displays[owner].center();
        const int other = 1 - owner;
        revealOrFocus(owner);
        requirePointerCursor(owner);
        const int initialCommits = commits;
        // The pointer's overlay reveals first. The other display reveals on a
        // queued callback, interleaved with activation/focus callbacks.
        revealOrFocus(other);
        requirePointerCursor(owner);
        require(commits == initialCommits,
                "a secondary reveal must not install a native cursor for the wrong display");
        revealOrFocus(owner);
        revealOrFocus(other);
        requirePointerCursor(owner);

        // A queued activation must use the live pointer after it crosses displays.
        cursorPosition = displays[other].center();
        revealOrFocus(other);
        const int movedCommits = commits;
        revealOrFocus(owner);
        requirePointerCursor(other);
        require(commits == movedCommits,
                "an old keyboard owner must not replace the new pointer owner's cursor");
    }

    for (const QPoint& position : {QPoint(-1, 0), QPoint(0, 0), QPoint(2559, 1439)}) {
        cursorPosition = position;
        const int before = commits;
        revealOrFocus(0);
        revealOrFocus(1);
        const int owner = position.x() < 0 ? 0 : 1;
        requirePointerCursor(owner);
        require(commits == before + 1, "display boundaries must have exactly one cursor owner");
    }
    cursorPosition = QPoint(2560, 1440);
    const int before = commits;
    revealOrFocus(0);
    revealOrFocus(1);
    require(commits == before, "overlays must not install a cursor outside their capture geometry");
}

#ifdef Q_OS_WIN
void initialNativeCursor() {
    for (qreal dpr : {1.0, 1.08, 1.1, 1.25, 1.5, 1.52, 2.0}) {
        const HCURSOR previous = GetCursor();
        const bool applied = snow_shot::presentation::applyNativeCrosshairCursor(dpr);
        const HCURSOR first = GetCursor();
        ICONINFO info{};
        const bool inspected = applied && GetIconInfo(first, &info);
        BITMAP bitmap{};
        const bool sized = inspected && GetObjectW(info.hbmColor, sizeof(bitmap), &bitmap);
        const bool reused =
            snow_shot::presentation::applyNativeCrosshairCursor(dpr) && GetCursor() == first;
        SetCursor(previous);
        if (info.hbmColor)
            DeleteObject(info.hbmColor);
        if (info.hbmMask)
            DeleteObject(info.hbmMask);
        const int size = 2 * qRound(16 * dpr);
        require(applied && inspected && sized && bitmap.bmWidth == size &&
                    bitmap.bmHeight == size &&
                    info.xHotspot == static_cast<DWORD>((size + 1) / 2) &&
                    info.yHotspot == static_cast<DWORD>((size + 1) / 2) && reused,
                "initial native crosshair must be realized and cached before it is displayed");
    }
}
#endif
} // namespace

int main(int argc, char** argv) {
    QApplication app(argc, argv);
#ifdef Q_OS_WIN
    if (app.arguments().contains(QStringLiteral("--native-only"))) {
        initialNativeCursor();
        return 0;
    }
#endif
    artworkAndScaling();
    canvasLayers();
    mixedDpiOverlayRevealsKeepThePointerCursor();
    return 0;
}
