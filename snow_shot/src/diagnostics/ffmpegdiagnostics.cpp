#include "snow_shot/diagnostics/ffmpegdiagnostics.h"
#include "snow_shot/diagnostics/diagnostics.h"

extern "C" {
#include <libavutil/log.h>
}

#include <array>
#include <atomic>
#include <cstdarg>
#include <mutex>

namespace snow_shot::diagnostics {
namespace {
std::atomic_bool forwarding{false};
void ffmpegLog(void* context, int level, const char* format, va_list arguments) noexcept {
    // Keep FFmpeg's console behavior, using a separate va_list for each consumer.
    va_list consoleArguments;
    va_copy(consoleArguments, arguments);
    av_log_default_callback(context, level, format, consoleArguments);
    va_end(consoleArguments);
    if (level > AV_LOG_ERROR || level > av_log_get_level() || !forwarding.load())
        return;
    thread_local bool recording = false;
    if (recording)
        return;
    recording = true;
    try {
        std::array<char, 4096> line{};
        int prefix = 1;
        va_list fileArguments;
        va_copy(fileArguments, arguments);
        av_log_format_line2(context, level, format, fileArguments, line.data(),
                            static_cast<int>(line.size()), &prefix);
        va_end(fileArguments);
        DiagnosticsService::instance().record(QtCriticalMsg, QStringLiteral("snow_shot.ffmpeg"),
                                              QStringLiteral("ffmpeg.error"),
                                              QString::fromUtf8(line.data()).trimmed());
    } catch (...) {
        // A logging failure must never unwind through a native media callback.
    }
    recording = false;
}
} // namespace

void installFfmpegDiagnostics() {
    static std::once_flag installed;
    std::call_once(installed, [] { av_log_set_callback(ffmpegLog); });
    forwarding.store(true);
}

void stopFfmpegDiagnostics() {
    forwarding.store(false);
}
} // namespace snow_shot::diagnostics
