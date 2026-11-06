#include "snow_shot/runtime/runtimeactivitytracker.h"
#include "screenshotcaptureworker.h"
#include "snow_shot/diagnostics/diagnostics.h"
#include "captureframeimage.h"
#include "captureframegeometry.h"

#include "screenshotcaptureperfinstrumentation.h"
#include "snow_shot/presentation/screenshotcapturecoordinator.h"
#include "snow_shot/presentation/capture/screenshotcapturepolicy.h"
#include "snow_shot/storage/settingsadapters.h"

#include "snow_capture.h"

#include <QDebug>
#include <QElapsedTimer>
#include <QImage>
#include <QMetaObject>

#include <limits>
#include <algorithm>
#include <cmath>
#include <optional>
#include <utility>

namespace {
using snow_shot::presentation::capture::imageFromFrameLease;
using snow_shot::presentation::capture::validFrameInfo;

ScreenshotCaptureBackend backendFromNative(std::uint8_t backend) {
    switch (backend) {
    case SNOW_CAPTURE_BACKEND_DXGI:
        return ScreenshotCaptureBackend::Dxgi;
    case SNOW_CAPTURE_BACKEND_WGC:
        return ScreenshotCaptureBackend::WindowsGraphicsCapture;
    case SNOW_CAPTURE_BACKEND_SCREEN_CAPTURE_KIT:
        return ScreenshotCaptureBackend::ScreenCaptureKit;
    case SNOW_CAPTURE_BACKEND_GDI:
        return ScreenshotCaptureBackend::Gdi;
    default:
        return ScreenshotCaptureBackend::Auto;
    }
}

QString nativeCaptureError(const char* fallback) {
    const char* message = snow_capture_last_error_message();
    return QString::fromUtf8(message != nullptr && *message != '\0' ? message : fallback);
}

std::optional<SnowCaptureLayoutDiagnostics>
layoutDiagnostics(const SnowCaptureDesktopSession* session) {
    SnowCaptureLayoutDiagnostics info{};
    info.version = 1;
    info.struct_size = sizeof(info);
    if (!session || !snow_capture_desktop_session_layout_diagnostics(session, &info))
        return std::nullopt;
    return info;
}

void traceCachedLayout(const SnowCaptureDesktopSession* session,
                       const std::optional<SnowCaptureLayoutDiagnostics>& info,
                       QJsonObject correlation, const char* stage) {
    correlation.insert(QStringLiteral("stage"), QString::fromLatin1(stage));
    if (!info) {
        correlation.insert(QStringLiteral("outcome"), QStringLiteral("unavailable"));
    } else {
        correlation.insert(QStringLiteral("count"), static_cast<qint64>(info->worker_count));
        correlation.insert(QStringLiteral("cached_topology_before"),
                           QString::number(info->cached_topology_before));
        correlation.insert(QStringLiteral("cached_topology_after"),
                           QString::number(info->cached_topology_after));
        correlation.insert(QStringLiteral("current_topology"),
                           QString::number(info->current_topology));
    }
    snow_shot::diagnostics::logEvent(QStringLiteral("snow_shot.capture_trace"),
                                     QStringLiteral("capture.layout_cache"), correlation);
    if (!info)
        return;
    const size_t count = std::min(info->worker_count, size_t{128});
    for (size_t index = 0; index < count; ++index) {
        std::uint32_t displayId = 0;
        if (!snow_capture_desktop_session_cached_display_id(session, index, &displayId))
            continue;
        auto fields = correlation;
        fields.insert(QStringLiteral("display_index"), static_cast<qint64>(index));
        fields.insert(QStringLiteral("display_id"), static_cast<qint64>(displayId));
        snow_shot::diagnostics::logEvent(QStringLiteral("snow_shot.capture_trace"),
                                         QStringLiteral("capture.cache_display"), fields);
    }
}
} // namespace

ScreenshotCaptureWorker::~ScreenshotCaptureWorker() {
    if (m_session != nullptr) {
        snow_capture_desktop_session_destroy(m_session);
        m_session = nullptr;
    }
}

void ScreenshotCaptureWorker::prepare(quint64 requestId,
                                      const QPointer<ScreenshotCaptureCoordinator>& coordinator) {
    const bool ok = prepareSessionIfNeeded();
    postPrepared(requestId, coordinator, ok);
}

void ScreenshotCaptureWorker::refreshLayout(
    quint64 requestId, const QPointer<ScreenshotCaptureCoordinator>& coordinator,
    quint64 captureRequestId) {
    QElapsedTimer timer;
    const QJsonObject correlation{{QStringLiteral("refresh_id"), QString::number(requestId)},
                                  {QStringLiteral("operation"), QString::number(captureRequestId)}};
    const bool sessionReady = ensureSession();
    if (sessionReady)
        traceCachedLayout(m_session, layoutDiagnostics(m_session), correlation, "before_refresh");
    timer.start();
    const bool ok = sessionReady && snow_capture_desktop_session_refresh_layout(m_session) != 0;
    const QString error = ok ? QString{} : nativeCaptureError("Failed to refresh desktop layout");
    const qint64 duration = timer.elapsed();
    if (sessionReady)
        traceCachedLayout(m_session, layoutDiagnostics(m_session), correlation, "after_refresh");
    snow_shot::diagnostics::DiagnosticsService::instance().record(
        ok ? QtInfoMsg : QtWarningMsg, QStringLiteral("snow_shot.capture_trace"),
        QStringLiteral("capture.layout_refresh_finished"), error,
        {{QStringLiteral("refresh_id"), QString::number(requestId)},
         {QStringLiteral("operation"), QString::number(captureRequestId)},
         {QStringLiteral("duration_ms"), duration},
         {QStringLiteral("reason"),
          sessionReady ? QStringLiteral("native_refresh") : QStringLiteral("session_unavailable")},
         {QStringLiteral("outcome"), ok ? QStringLiteral("succeeded") : QStringLiteral("failed")}});
    if (!coordinator.isNull()) {
        QMetaObject::invokeMethod(
            coordinator, snow_shot::runtime::trackRuntimeWork([coordinator, requestId, ok]() {
                if (!coordinator.isNull()) {
                    emit coordinator->layoutRefreshed(requestId, ok);
                }
            }),
            Qt::QueuedConnection);
    }
}

void ScreenshotCaptureWorker::capture(const ScreenshotCaptureRequest& request,
                                      const QPointer<ScreenshotCaptureCoordinator>& coordinator,
                                      SnowCaptureCancellationToken* cancellationToken) {
    SNOW_SHOT_CAPTURE_PERF_MILESTONE("capture.worker_entry");
    ScreenshotCaptureResult captureResult;
    captureResult.requestId = request.requestId;
    captureResult.purpose = request.purpose;
    QElapsedTimer captureTimer;
    captureTimer.start();
    snow_shot::diagnostics::logEvent(
        QStringLiteral("snow_shot.capture_trace"), QStringLiteral("capture.worker_entered"),
        {{QStringLiteral("operation"), QString::number(request.requestId)},
         {QStringLiteral("request_kind"), static_cast<int>(request.purpose)},
         {QStringLiteral("cache_hit"), m_session != nullptr},
         {QStringLiteral("changed"), request.refreshLayout}});
    if (!ensureSession(request.excludedWindowIds)) {
        captureResult.errorMessage = nativeCaptureError("Failed to create desktop capture session");
        postCaptureResult(coordinator, std::move(captureResult));
        return;
    }
    SNOW_SHOT_CAPTURE_PERF_MILESTONE("capture.session_ready");

    std::unique_ptr<SnowCaptureDesktopLayout, decltype(&snow_capture_desktop_layout_destroy)>
        layout(nullptr, snow_capture_desktop_layout_destroy);
    if (request.purpose == ScreenshotCapturePurpose::Initial) {
        SNOW_SHOT_CAPTURE_PERF_SCOPE("capture.layout_snapshot");
        const QJsonObject correlation{
            {QStringLiteral("operation"), QString::number(request.requestId)}};
        const auto before = layoutDiagnostics(m_session);
        traceCachedLayout(m_session, before, correlation, "before_snapshot");
        QElapsedTimer layoutTimer;
        layoutTimer.start();
        layout.reset(
            snow_capture_desktop_session_layout_snapshot(m_session, request.refreshLayout ? 1 : 0));
        const qint64 layoutDuration = layoutTimer.elapsed();
        const QString layoutError =
            layout ? QString{} : nativeCaptureError("Failed to snapshot desktop layout");
        QString source = QStringLiteral("unknown");
        switch (snow_capture_desktop_layout_source(layout.get())) {
        case 0:
            source = QStringLiteral("cache_reuse");
            break;
        case 1:
            source = QStringLiteral("refresh_requested");
            break;
        case 2:
            source = QStringLiteral("empty_cache");
            break;
        case 3:
            source = QStringLiteral("topology_changed");
            break;
        default:
            break;
        }
        traceCachedLayout(m_session, layoutDiagnostics(m_session), correlation, "after_snapshot");
        snow_shot::diagnostics::DiagnosticsService::instance().record(
            layout ? QtInfoMsg : QtWarningMsg, QStringLiteral("snow_shot.capture_trace"),
            QStringLiteral("capture.layout_snapshot"), layoutError,
            {{QStringLiteral("operation"), QString::number(request.requestId)},
             {QStringLiteral("layout_source"), source},
             {QStringLiteral("duration_ms"), layoutDuration},
             {QStringLiteral("outcome"),
              layout ? QStringLiteral("succeeded") : QStringLiteral("failed")}});
        if (!layout) {
            captureResult.errorMessage = layoutError;
            postCaptureResult(coordinator, std::move(captureResult));
            return;
        }
        ScreenshotCaptureLayout prepared;
        prepared.requestId = request.requestId;
        prepared.generation = request.requestId;
        const size_t displayCount = snow_capture_desktop_layout_count(layout.get());
        bool validLayout = displayCount > 0 && displayCount <= 128;
        for (size_t i = 0; validLayout && i < displayCount; ++i) {
            SnowCaptureDisplayDescriptor d{};
            d.version = 1;
            d.struct_size = sizeof(d);
            if (!snow_capture_desktop_layout_display(layout.get(), i, &d) || !std::isfinite(d.x) ||
                !std::isfinite(d.y) || !std::isfinite(d.width) || !std::isfinite(d.height) ||
                !std::isfinite(d.backing_scale) || d.backing_scale <= 0 || d.coordinate_space > 1 ||
                d.width < 1 || d.height < 1 || d.width > std::numeric_limits<int>::max() ||
                d.height > std::numeric_limits<int>::max() ||
                d.x < std::numeric_limits<int>::min() || d.y < std::numeric_limits<int>::min() ||
                d.x + std::max(d.width, static_cast<double>(d.pixel_width)) >
                    std::numeric_limits<int>::max() ||
                d.y + std::max(d.height, static_cast<double>(d.pixel_height)) >
                    std::numeric_limits<int>::max() ||
                d.pixel_width == 0 || d.pixel_height == 0 ||
                d.pixel_width > static_cast<uint32_t>(std::numeric_limits<int>::max()) ||
                d.pixel_height > static_cast<uint32_t>(std::numeric_limits<int>::max())) {
                validLayout = false;
                break;
            }
            CapturedDisplayModel display;
            display.stableId = QString::fromUtf8(d.stable_id);
            display.name = QString::fromUtf8(d.name);
            display.nativeDisplayId = d.display_id;
            display.physicalRect = QRect(qRound(d.x), qRound(d.y), static_cast<int>(d.pixel_width),
                                         static_cast<int>(d.pixel_height));
            display.canvasUsesPoints = d.coordinate_space == 1;
            if (display.canvasUsesPoints) {
                display.capturedLogicalRect =
                    QRect(qRound(d.x), qRound(d.y), qRound(d.width), qRound(d.height));
                display.backingScale = d.backing_scale;
            }
            display.primary = d.is_primary != 0;
            display.active = true;
            snow_shot::diagnostics::logEvent(
                QStringLiteral("snow_shot.capture_trace"), QStringLiteral("capture.native_layout"),
                {{QStringLiteral("operation"), QString::number(request.requestId)},
                 {QStringLiteral("display_index"), static_cast<qint64>(i)},
                 {QStringLiteral("code"), static_cast<qint64>(d.display_id)},
                 {QStringLiteral("display_id"), static_cast<qint64>(d.display_id)},
                 {QStringLiteral("x"), display.physicalRect.x()},
                 {QStringLiteral("y"), display.physicalRect.y()},
                 {QStringLiteral("width"), display.physicalRect.width()},
                 {QStringLiteral("height"), display.physicalRect.height()},
                 {QStringLiteral("scale"), d.backing_scale}});
            prepared.displays.push_back(std::move(display));
        }
        if (!validLayout) {
            captureResult.errorMessage = QStringLiteral("Invalid desktop layout snapshot");
            postCaptureResult(coordinator, std::move(captureResult));
            return;
        }
        SNOW_SHOT_CAPTURE_PERF_MILESTONE("capture.layout_ready");
        QMetaObject::invokeMethod(
            coordinator,
            snow_shot::runtime::trackRuntimeWork([coordinator, prepared = std::move(prepared)]() {
                if (coordinator)
                    emit coordinator->layoutReady(prepared);
            }),
            Qt::QueuedConnection);
    }

    SnowCaptureScreenshotRequest nativeRequest{};
    nativeRequest.version = SNOW_CAPTURE_SCREENSHOT_REQUEST_VERSION;
    nativeRequest.struct_size = sizeof(nativeRequest);
    nativeRequest.flags =
        request.refreshLayout && !layout ? SNOW_CAPTURE_SCREENSHOT_REQUEST_REFRESH_LAYOUT : 0;
    if (request.restoreOriginalScreenColors) {
        nativeRequest.flags |= SNOW_CAPTURE_SCREENSHOT_REQUEST_RESTORE_ORIGINAL_COLORS;
    }
    nativeRequest.cancellation_token = cancellationToken;

    SnowCaptureScreenshotResult* nativeResult = nullptr;
    {
        SNOW_SHOT_CAPTURE_PERF_SCOPE("capture.native_ffi");
        nativeResult = layout ? snow_capture_desktop_session_capture_with_layout(
                                    m_session, &nativeRequest, layout.get())
                              : snow_capture_desktop_session_capture(m_session, &nativeRequest);
    }
    SNOW_SHOT_CAPTURE_PERF_MILESTONE("capture.native_returned");
    // Read the native error before logging or resetting the session can replace it.
    const QString captureError =
        nativeResult == nullptr ? nativeCaptureError("Screenshot capture failed") : QString{};
    snow_shot::diagnostics::logEvent(
        QStringLiteral("snow_shot.capture_trace"), QStringLiteral("capture.native_returned"),
        {{QStringLiteral("operation"), QString::number(request.requestId)},
         {QStringLiteral("duration_ms"), captureTimer.elapsed()},
         {QStringLiteral("outcome"),
          nativeResult ? QStringLiteral("succeeded") : QStringLiteral("failed")}});
    if (nativeResult == nullptr) {
        static_cast<void>(snow_capture_desktop_session_reset_to_prepared(m_session));
        captureResult.errorMessage = captureError;
        postCaptureResult(coordinator, std::move(captureResult));
        return;
    }

    const size_t count = snow_capture_screenshot_result_display_count(nativeResult);
    captureResult.displays.reserve(static_cast<int>(count));
    bool valid = count != 0;
    for (size_t index = 0; index < count; ++index) {
        SnowCaptureFrameInfo info{};
        if (snow_capture_screenshot_result_display_info(nativeResult, index, &info) == 0 ||
            !validFrameInfo(info)) {
            valid = false;
            break;
        }

        SnowCaptureFrameLease* lease =
            snow_capture_screenshot_result_display_retain(nativeResult, index);
        QImage image = imageFromFrameLease(
            lease, info.rgba_bytes, info.rgba_len, info.width, info.height, info.stride_bytes,
            info.pixel_format, snow_shot::presentation::capture::FrameAlphaMode::Opaque);
        if (image.isNull()) {
            valid = false;
            break;
        }

        CapturedDisplayModel display;
        display.stableId = QString::fromUtf8(info.stable_id != nullptr ? info.stable_id : "");
        display.name = QString::fromUtf8(info.name != nullptr ? info.name : "");
        display.physicalRect =
            QRect(info.x, info.y, static_cast<int>(info.width), static_cast<int>(info.height));
#ifdef Q_OS_MACOS
        SnowCaptureFrameGeometry geometry{};
        geometry.version = SNOW_CAPTURE_FRAME_GEOMETRY_VERSION;
        geometry.struct_size = sizeof(geometry);
        if (!snow_capture_screenshot_result_display_geometry(nativeResult, index, &geometry)) {
            valid = false;
            break;
        }
        const auto logicalRect = snow_shot::presentation::capture::logicalFrameRect(geometry);
        if (!logicalRect || geometry.display_id == 0) {
            valid = false;
            break;
        }
        display.capturedLogicalRect = *logicalRect;
        display.physicalRect.moveTopLeft(display.capturedLogicalRect.topLeft());
        display.nativeDisplayId = geometry.display_id;
        display.backingScale = geometry.backing_scale;
        display.canvasUsesPoints = true;
#endif
        display.canvasRect = display.physicalRect;
        display.image = std::move(image);
        if (request.captureCursor && request.cursorSnapshot) {
            SnowCaptureFrameInfo patchInfo{};
            SnowCaptureFrameLease* patchLease = nullptr;
            if (snow_capture_screenshot_result_cursor_patch(
                    nativeResult, request.cursorSnapshot.get(), index, &patchInfo, &patchLease) &&
                patchLease != nullptr) {
                display.cursorPatch = imageFromFrameLease(
                    patchLease, patchInfo.rgba_bytes, patchInfo.rgba_len, patchInfo.width,
                    patchInfo.height, patchInfo.stride_bytes, patchInfo.pixel_format,
                    snow_shot::presentation::capture::FrameAlphaMode::Opaque);
                display.cursorPixelRect =
                    QRect(patchInfo.x, patchInfo.y, static_cast<int>(patchInfo.width),
                          static_cast<int>(patchInfo.height));
                captureResult.cursorAvailable |= !display.cursorPatch.isNull();
            }
        }
        display.active = true;
        display.backend = backendFromNative(info.backend_kind);
        captureResult.displays.push_back(std::move(display));
    }

    if (!valid && captureResult.errorMessage.isEmpty()) {
        captureResult.errorMessage = nativeCaptureError("Screenshot capture returned invalid data");
    }
    if (!valid) {
        captureResult.displays.clear();
    }
    captureResult.succeeded = valid;
    snow_capture_screenshot_result_destroy(nativeResult);
    SNOW_SHOT_CAPTURE_PERF_MILESTONE("capture.frames_wrapped");
    SNOW_SHOT_CAPTURE_PERF_MILESTONE("capture.result_posted");
    postCaptureResult(coordinator, std::move(captureResult));
}

bool ScreenshotCaptureWorker::ensureSession(const QVector<std::uint32_t>& excludedWindowIds) {
    const QString apiMode = snow_shot::storage::ScreenshotSettings().apiMode();
    auto requested = snow_shot::presentation::capture::screenshotApiModeFromValue(
        apiMode.toLatin1().constData());
    if (requested == snow_shot::presentation::capture::ScreenshotApiMode::Auto) {
        requested = snow_shot::presentation::capture::resolveAutoScreenshotApiMode();
    }
    const auto backend =
        snow_shot::presentation::capture::nativeBackendForNormalScreenshot(requested);
    if (m_session != nullptr && m_sessionBackend == backend &&
        m_sessionExcludedWindowIds == excludedWindowIds) {
        return true;
    }

    // Backend policy and exclusions are fixed at creation, including for prewarmed sessions.
    if (m_session != nullptr) {
        snow_capture_desktop_session_destroy(m_session);
        m_session = nullptr;
    }

    SnowCaptureDesktopSessionConfig config{};
    config.capture_retry_count = 1;
    config.capture_backend = backend;
    config.exclusions.windows = excludedWindowIds.constData();
    config.exclusions.window_count = static_cast<size_t>(excludedWindowIds.size());
#if defined(Q_OS_WIN) || defined(_WIN32)
    config.pixel_format = SNOW_CAPTURE_PIXEL_FORMAT_BGRA8;
#endif
    m_session = snow_capture_desktop_session_create(&config);
    if (m_session == nullptr) {
        qWarning("Failed to create desktop capture session: %s", snow_capture_last_error_message());
        return false;
    }
    m_sessionBackend = backend;
    m_sessionExcludedWindowIds = excludedWindowIds;
    snow_shot::diagnostics::logEvent(
        QStringLiteral("snow_shot.capture"), QStringLiteral("capture.backend_ready"),
        {{QStringLiteral("backend"), static_cast<int>(backend)},
         {QStringLiteral("count"), static_cast<qint64>(config.capture_retry_count)}});
    return true;
}

bool ScreenshotCaptureWorker::sessionPrepared() const {
    if (m_session == nullptr) {
        return false;
    }

    SnowCaptureDesktopSessionState state{};
    if (snow_capture_desktop_session_state(m_session, &state) == 0) {
        return false;
    }
    return state.prepared != 0;
}

bool ScreenshotCaptureWorker::prepareSessionIfNeeded() {
    if (!ensureSession()) {
        return false;
    }
    if (sessionPrepared()) {
        return true;
    }
    return snow_capture_desktop_session_prepare(m_session) != 0;
}

void ScreenshotCaptureWorker::postPrepared(
    quint64 requestId, const QPointer<ScreenshotCaptureCoordinator>& coordinator, bool ok) {
    if (coordinator.isNull()) {
        return;
    }

    QMetaObject::invokeMethod(coordinator,
                              snow_shot::runtime::trackRuntimeWork([coordinator, requestId, ok]() {
                                  if (!coordinator.isNull()) {
                                      emit coordinator->prepared(requestId, ok);
                                  }
                              }),
                              Qt::QueuedConnection);
}

void ScreenshotCaptureWorker::postCaptureResult(
    const QPointer<ScreenshotCaptureCoordinator>& coordinator, ScreenshotCaptureResult result) {
    snow_shot::diagnostics::DiagnosticsService::instance().record(
        result.succeeded || coordinator.isNull() ? QtInfoMsg : QtWarningMsg,
        QStringLiteral("snow_shot.capture"), QStringLiteral("capture.finished"),
        result.errorMessage,
        {{QStringLiteral("operation"), QString::number(result.requestId)},
         {QStringLiteral("count"), result.displays.size()},
         {QStringLiteral("outcome"), result.succeeded       ? QStringLiteral("succeeded")
                                     : coordinator.isNull() ? QStringLiteral("cancelled")
                                                            : QStringLiteral("failed")}});
    if (coordinator.isNull()) {
        return;
    }

    QMetaObject::invokeMethod(
        coordinator,
        snow_shot::runtime::trackRuntimeWork([coordinator, result = std::move(result)]() mutable {
            if (!coordinator.isNull()) {
                emit coordinator->captureFinished(std::move(result));
            }
        }),
        Qt::QueuedConnection);
}
