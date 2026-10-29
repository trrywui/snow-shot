#ifndef SNOW_SHOT_DIAGNOSTICS_FFMPEGDIAGNOSTICS_H
#define SNOW_SHOT_DIAGNOSTICS_FFMPEGDIAGNOSTICS_H

namespace snow_shot::diagnostics {
// Install once at application startup, before media workers are created.
void installFfmpegDiagnostics();
// Stop forwarding before the application logger shuts down. Native console logging continues.
void stopFfmpegDiagnostics();
} // namespace snow_shot::diagnostics

#endif
