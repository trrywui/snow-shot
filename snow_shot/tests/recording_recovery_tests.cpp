#include "../src/presentation/recording/recordingrecovery.h"
#include <QCoreApplication>
#include <QProcess>
#include <QTemporaryDir>
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <stdexcept>

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        std::fprintf(stderr, "%s\n", message);
        std::exit(1);
    }
}

void startupRecoveryRaces(const QTemporaryDir& directory) {
    using namespace snow_shot::presentation::recording;
    const QString output = directory.filePath(QStringLiteral("startup-output.mp4"));
    const QString source = directory.filePath(QStringLiteral("finished.snowrec"));
    const QString marker = directory.filePath(QStringLiteral("startup-recovery.json"));
    StartupRecovery outputFirst;
    require(outputFirst.publishOutput(output) && !QFileInfo::exists(marker),
            "normal startup publishes progress without creating a terminal recovery notice");
    require(outputFirst.beginPreservation(marker) && loadRecovery(marker)->outputPath == output,
            "terminal preservation finds a destination published before native startup blocks");

    QFile::remove(marker);
    StartupRecovery terminalFirst;
    require(!terminalFirst.beginPreservation(marker) && !QFileInfo::exists(marker),
            "terminal preservation can precede destination selection without an empty marker");
    require(terminalFirst.publishOutput(output) && loadRecovery(marker)->outputPath == output,
            "startup publishes the recovery pointer when terminal preservation arrived first");
    require(terminalFirst.beginPreservation(marker, {source, output, {}}) &&
                terminalFirst.publishOutput(output) && loadRecovery(marker)->sourcePath == source,
            "late startup progress never erases a finalized source pointer");

    for (int iteration = 0; iteration < 8; ++iteration) {
        auto progress = std::make_shared<StartupRecovery>();
        std::promise<void> release;
        const auto gate = release.get_future().share();
        std::thread startup([progress, output, gate] {
            gate.wait();
            require(progress->publishOutput(output), "concurrent startup progress publishes");
        });
        std::thread terminal([progress, marker, gate] {
            gate.wait();
            static_cast<void>(progress->beginPreservation(marker));
        });
        release.set_value();
        startup.join();
        terminal.join();
        const auto saved = loadRecovery(marker);
        require(saved && saved->outputPath == output,
                "racing destination and terminal publication cannot lose the recovery pointer");
    }

    const QString staged = directory.filePath(QStringLiteral(".snow-deferred-late"));
    const QString direct = directory.filePath(QStringLiteral(".snow-recording-direct-late.mp4"));
    require(QDir().mkpath(staged), "create retained deferred staging fixture");
    QFile directFile(direct);
    require(directFile.open(QIODevice::WriteOnly), "create retained direct staging fixture");
    directFile.write("unfinished native media");
    directFile.close();
    QFile sibling(source);
    require(sibling.open(QIODevice::WriteOnly), "create unrelated finalized sibling");
    sibling.write("unrelated finalized recording");
    sibling.close();
    const auto original = loadRecovery(marker);
    require(original && original->stagingPaths.isEmpty(),
            "fixtures appeared after the startup recovery pointer was saved");
    const auto refreshed = refreshRecoveryStaging(*original);
    require(refreshed.sourcePath.isEmpty() && refreshed.stagingPaths.contains(staged) &&
                refreshed.stagingPaths.contains(direct) &&
                !refreshed.stagingPaths.contains(source) && directFile.exists(),
            "next launch discovers native staging read-only without guessing a sibling snowrec");
    require(saveRecovery(marker, refreshed) &&
                loadRecovery(marker)->stagingPaths == refreshed.stagingPaths,
            "exact retained staging locations survive marker reload");
    require(clearRecovery(marker, *original),
            "refreshing staging metadata does not prevent clearing the recovered recording");
}

int blockedStartupChild(const QString& marker, const QString& output) {
    using namespace snow_shot::presentation::recording;
    auto progress = std::make_shared<StartupRecovery>();
    auto startupPending = std::make_shared<std::promise<void>>();
    auto pending = startupPending->get_future();
    auto start = std::async(std::launch::async, [progress, startupPending, output] {
        require(progress->publishOutput(output), "blocked startup publishes its destination");
        const QString staging =
            QFileInfo(output).dir().filePath(QStringLiteral(".snow-deferred-blocked"));
        require(QDir().mkpath(staging), "blocked startup creates its native staging directory");
        QFile inputs(QDir(staging).filePath(QStringLiteral("inputs.bin")));
        require(inputs.open(QIODevice::WriteOnly), "blocked startup retains its input data");
        inputs.write("unfinished recording inputs");
        inputs.close();
        startupPending->set_value();
        std::promise<void> never;
        never.get_future().wait();
    });
    require(pending.wait_for(std::chrono::seconds(2)) == std::future_status::ready,
            "native startup reaches the blocked boundary");
    auto markerSaved = std::make_shared<std::promise<void>>();
    auto saved = markerSaved->get_future();
    require(!runBoundedPreservation(
                [progress, marker, markerSaved, start = std::move(start)]() mutable {
                    require(progress->beginPreservation(marker),
                            "terminal preservation saves location before joining blocked startup");
                    markerSaved->set_value();
                    start.get();
                },
                0),
            "blocked startup returns from the terminal deadline");
    require(saved.wait_for(std::chrono::seconds(2)) == std::future_status::ready,
            "recovery pointer is durable before terminating the blocked native process");
    std::_Exit(EXIT_SUCCESS);
}

void blockedStartupRetainsUsefulRecovery(const QTemporaryDir& directory) {
    using namespace snow_shot::presentation::recording;
    const QString marker = directory.filePath(QStringLiteral("blocked-recovery.json"));
    const QString output = directory.filePath(QStringLiteral("blocked-output.mp4"));
    QProcess child;
    child.start(QCoreApplication::applicationFilePath(),
                {QStringLiteral("--blocked-startup-child"), marker, output});
    require(child.waitForStarted() && child.waitForFinished(5000) &&
                child.exitStatus() == QProcess::NormalExit && child.exitCode() == 0,
            "blocked native startup subprocess ends without joining or deleting staging");
    const auto record = loadRecovery(marker);
    const QString staging = directory.filePath(QStringLiteral(".snow-deferred-blocked"));
    require(record && record->outputPath == output && record->sourcePath.isEmpty() &&
                record->stagingPaths.contains(staging) &&
                QFileInfo::exists(QDir(staging).filePath(QStringLiteral("inputs.bin"))),
            "next launch can locate exact retained files even if startup never returned a session");
}
} // namespace

int main(int argc, char** argv) {
    QCoreApplication app(argc, argv);
    if (app.arguments().size() == 4 &&
        app.arguments().at(1) == QStringLiteral("--blocked-startup-child"))
        return blockedStartupChild(app.arguments().at(2), app.arguments().at(3));
    using namespace snow_shot::presentation::recording;
    QTemporaryDir directory;
    require(directory.isValid(), "create recovery storage");
    const auto marker = directory.filePath(QStringLiteral("LocalState/recording-recovery.json"));
    const RecoveryRecord recording{directory.filePath(QStringLiteral("retained.snowrec")),
                                   directory.filePath(QStringLiteral("output.mp4")),
                                   {}};
    require(saveRecovery(marker, recording), "publish recovery pointer atomically");
    QFile source(recording.sourcePath);
    require(source.open(QIODevice::WriteOnly), "create retained recording fixture");
    source.write("finalized recording fixture");
    source.close();
    const auto loaded = loadRecovery(marker);
    require(loaded && loaded->sourcePath == recording.sourcePath &&
                loaded->outputPath == recording.outputPath,
            "retained source and output survive recovery reload");
    require(!runBoundedPreservation([] { throw std::runtime_error("finalization failed"); }, 4000),
            "finalization failures are contained");
    auto release = std::make_shared<std::promise<void>>();
    const auto gate = release->get_future().share();
    auto completed = std::make_shared<std::promise<void>>();
    auto done = completed->get_future();
    const RecoveryRecord late{
        recording.sourcePath, directory.filePath(QStringLiteral("late-output.mp4")), {}};
    require(!runBoundedPreservation(
                [gate, completed, marker, late] {
                    gate.wait();
                    require(saveRecovery(marker, late),
                            "late finalization publishes recovery pointer");
                    completed->set_value();
                },
                0) &&
                done.wait_for(std::chrono::seconds(0)) == std::future_status::timeout,
            "deadline returns without joining or cancelling native ownership");
    release->set_value();
    require(done.wait_for(std::chrono::seconds(2)) == std::future_status::ready,
            "preservation work retains ownership and finishes after deadline");
    const auto afterDeadline = loadRecovery(marker);
    require(afterDeadline && afterDeadline->outputPath == late.outputPath && source.exists(),
            "failure and timeout preserve source media and late finalization metadata");
    require(!clearRecovery(marker, recording) && loadRecovery(marker) == afterDeadline,
            "completion of an older recovery cannot erase a newer recording pointer");
    require(clearRecovery(marker, late) && !QFileInfo::exists(marker) && source.exists(),
            "successful recovery clears only its pointer and keeps original source media");

    auto destroyed = std::make_shared<std::atomic<int>>(0);
    struct Owner {
        std::shared_ptr<std::atomic<int>> destroyed;
        ~Owner() {
            ++*destroyed;
        }
    };
    std::function<void()> retainedWorker;
    require(!runBoundedPreservation([owner = std::make_unique<Owner>(destroyed)] {}, 4000,
                                    [&](auto worker) {
                                        retainedWorker = std::move(worker);
                                        throw std::runtime_error("thread creation failed");
                                    }) &&
                destroyed->load() == 0,
            "thread creation failure retains native owners without cancellation or destruction");
    retainedWorker();
    require(destroyed->load() == 1,
            "retained preservation operation owns native resources until it finishes");

    auto releaseNative = std::make_shared<std::promise<void>>();
    auto nativeGate = releaseNative->get_future().share();
    auto native = std::async(std::launch::async, [nativeGate] { nativeGate.wait(); });
    auto retired = std::make_shared<std::promise<void>>();
    auto retiredResult = retired->get_future();
    require(
        !runBoundedPreservation(
            [native = std::move(native), retired]() mutable {
                native.get();
                retired->set_value();
            },
            0) &&
            retiredResult.wait_for(std::chrono::seconds(0)) == std::future_status::timeout,
        "async-origin native futures never join the GUI when the preservation deadline expires");
    releaseNative->set_value();
    require(retiredResult.wait_for(std::chrono::seconds(2)) == std::future_status::ready,
            "timed-out native future remains owned until its worker completes");
    QFile invalid(marker);
    require(invalid.open(QIODevice::WriteOnly | QIODevice::Truncate), "replace recovery fixture");
    invalid.write("{\"schema\":1,\"source\":\"relative.snowrec\"}");
    invalid.close();
    require(!loadRecovery(marker), "invalid recovery pointers never open relative files");
    startupRecoveryRaces(directory);
    blockedStartupRetainsUsefulRecovery(directory);
    return 0;
}
