#pragma once

#include "snow_shot/diagnostics/diagnostics.h"
#include <cstddef>
#include <cstdint>

namespace snow_shot::diagnostics {
// Called from the shared Rust microphone backend on its capture worker. Record
// before optional-source error handling reduces the failure to Unavailable.
inline void recordMicrophoneFailure(const uint8_t* message, size_t length) {
    DiagnosticsService::instance().record(
        QtWarningMsg, QStringLiteral("snow_shot.recording"),
        QStringLiteral("microphone.native_failed"),
        QString::fromUtf8(reinterpret_cast<const char*>(message), static_cast<qsizetype>(length)));
}
} // namespace snow_shot::diagnostics
