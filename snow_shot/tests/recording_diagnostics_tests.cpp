#include "snow_shot/diagnostics/diagnostics.h"
#include "snow_shot/diagnostics/ffmpegdiagnostics.h"
#include "snow_shot/diagnostics/microphonediagnostics.h"

#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QJsonDocument>
#include <QTemporaryDir>
#ifdef Q_OS_MACOS
#include "snow_recording_macos.h"
#endif

extern "C" {
#include <libavutil/log.h>
}

#include <cstdio>
#include <stdexcept>
#include <string>
#include <thread>

namespace {
void require(bool condition, const char* message) {
    if (!condition)
        throw std::runtime_error(message);
}
void ffmpegErrorsAndRecordingMetadataReachExportedLogs() {
    using namespace snow_shot::diagnostics;
    QTemporaryDir directory(QDir(QDir::tempPath()).canonicalPath() +
                            QStringLiteral("/snow-recording-diag-XXXXXX"));
    require(directory.isValid(), "diagnostic directory exists");
    DiagnosticsOptions options;
    options.directories = {directory.path()};
    options.enableCrashCapture = false;
    options.installMessageHandler = false;
    options.mirrorToConsole = false;
    auto& service = DiagnosticsService::instance();
    require(service.initialize(options), "diagnostics initialize");
#ifdef Q_OS_MACOS
    // The shipping archive disables legacy-capture; the diagnostic entry point
    // must still be exported for live meters and the current recording backend.
    snow_recording_macos_set_microphone_diagnostic_callback(recordMicrophoneFailure);
#endif
    installFfmpegDiagnostics();
    installFfmpegDiagnostics();
    std::thread worker([] {
        av_log(nullptr, AV_LOG_ERROR, "pts %d < dts %d in stream %d; token=private-value\n", 6144,
               7000, 0);
        av_log(nullptr, AV_LOG_ERROR, "%s\n", std::string(20000, 'x').c_str());
        av_log(nullptr, AV_LOG_INFO, "ordinary codec progress\n");
    });
    worker.join();
    std::thread microphoneWorker([] {
        const std::string message =
            "com.apple.coreaudio.avfaudio (0): microphone install-tap: required condition is "
            "false: "
            "format.sampleRate == hwFormat.sampleRate; device=123 hardware=48000Hz/1ch "
            "output=44100Hz/2ch";
        recordMicrophoneFailure(reinterpret_cast<const uint8_t*>(message.data()), message.size());
    });
    microphoneWorker.join();
    const QJsonObject fields{{QStringLiteral("operation"), QStringLiteral("trim-session")},
                             {QStringLiteral("export_attempt"), 2},
                             {QStringLiteral("stage"), QStringLiteral("exporting")},
                             {QStringLiteral("action"), QStringLiteral("save")},
                             {QStringLiteral("deferred"), true},
                             {QStringLiteral("detached"), false},
                             {QStringLiteral("cache_hit"), false},
                             {QStringLiteral("first_frame"), 15},
                             {QStringLiteral("end_frame"), 45},
                             {QStringLiteral("start_us"), 500000},
                             {QStringLiteral("end_us"), 1500000},
                             {QStringLiteral("speed_percent"), 125},
                             {QStringLiteral("fps"), 30},
                             {QStringLiteral("format"), 0},
                             {QStringLiteral("codec"), 0},
                             {QStringLiteral("hardware"), true},
                             {QStringLiteral("quality"), 80},
                             {QStringLiteral("encoding_preset"), 1},
                             {QStringLiteral("backend_preference"), QStringLiteral("auto")}};
    auto supplied = fields;
    supplied.insert(QStringLiteral("window_title"), QStringLiteral("private content"));
    service.record(QtWarningMsg, QStringLiteral("snow_shot.recording"),
                   QStringLiteral("recording.trim_export_failed"),
                   QStringLiteral("encode error: video packet mux: Invalid argument"), supplied);
    stopFfmpegDiagnostics();
    av_log(nullptr, AV_LOG_ERROR, "error after forwarding stops\n");
    const auto exported = service.exportDay(QDate::currentDate()).get();
    require(exported.success, "diagnostic snapshot exports");
    QFile file(exported.path);
    require(file.open(QIODevice::ReadOnly), "diagnostic snapshot opens");
    int nativeErrors = 0;
    bool foundRecording = false;
    bool foundMicrophone = false;
    const auto bytes = file.readAll();
    for (const auto& line : bytes.split('\n')) {
        const auto record = QJsonDocument::fromJson(line).object();
        const auto event = record.value(QStringLiteral("event")).toString();
        if (event == QStringLiteral("ffmpeg.error")) {
            ++nativeErrors;
            const auto message = record.value(QStringLiteral("message")).toString();
            require(message.size() <= 4095, "native messages stay bounded");
            require(record.value(QStringLiteral("level")) == QStringLiteral("ERROR"),
                    "native failures retain severity");
            if (message.startsWith(QStringLiteral("pts")))
                require(message.contains(QStringLiteral("pts 6144 < dts 7000")) &&
                            !message.contains(QStringLiteral("private-value")),
                        "native timestamp evidence survives secret redaction");
        } else if (event == QStringLiteral("microphone.native_failed")) {
            const auto message = record.value(QStringLiteral("message")).toString();
            require(
                record.value(QStringLiteral("level")) == QStringLiteral("WARN") &&
                    message.contains(QStringLiteral("com.apple.coreaudio.avfaudio")) &&
                    message.contains(QStringLiteral("install-tap")) &&
                    message.contains(QStringLiteral("format.sampleRate == hwFormat.sampleRate")) &&
                    message.contains(QStringLiteral("hardware=48000Hz/1ch")) &&
                    message.contains(QStringLiteral("output=44100Hz/2ch")),
                "native microphone exception and format evidence reach exported logs");
            foundMicrophone = true;
        } else if (event == QStringLiteral("recording.trim_export_failed")) {
            require(record.value(QStringLiteral("fields")).toObject() == fields,
                    "recording metadata survives filtering without window content");
            require(record.value(QStringLiteral("message"))
                        .toString()
                        .contains(QStringLiteral("video packet mux: Invalid argument")),
                    "exported recording event includes its error message");
            foundRecording = true;
        }
    }
    require(nativeErrors == 2 && foundRecording && foundMicrophone,
            "export contains native and application evidence");
    require(!bytes.contains("ordinary codec progress"), "progress does not flood file logs");
    require(!bytes.contains("error after forwarding stops"),
            "forwarding stops before logger teardown");
    file.close();
#ifdef Q_OS_MACOS
    snow_recording_macos_set_microphone_diagnostic_callback(nullptr);
#endif
    service.shutdown();
    require(QFile::setPermissions(exported.path, QFileDevice::ReadOwner | QFileDevice::WriteOwner),
            "release the exported fixture's read-only protection before temporary cleanup");
}
} // namespace

int main(int argc, char** argv) {
    QCoreApplication app(argc, argv);
    try {
        ffmpegErrorsAndRecordingMetadataReachExportedLogs();
    } catch (const std::exception& error) {
        std::fprintf(stderr, "%s\n", error.what());
        return 1;
    }
    return 0;
}
