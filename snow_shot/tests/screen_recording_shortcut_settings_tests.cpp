#include "snow_shot/presentation/globalshortcutmanager.h"
#include "snow_shot/presentation/settings/settingsbackend.h"
#include "snow_shot/presentation/settings/settingsruntimesession.h"
#include "snow_shot/storage/applicationstorage.h"
#include "snow_shot/storage/settingsadapters.h"

#include <QApplication>
#include <QDir>
#include <QTemporaryDir>
#include <QJsonArray>

#include <limits>

#include <cstdlib>
#include <iostream>

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(EXIT_FAILURE);
    }
}

void recordingAreaHistoryStorage() {
    namespace storage = snow_shot::storage;
    auto& configuration = storage::ApplicationStorage::instance().configuration();
    const auto key = QStringLiteral("screen_recording/area_history");
    const storage::RecordingAreaHistorySettings history;
    require(history.entries().isEmpty(), "recording area history defaults to empty");
    const QRect repeated(-200, 30, 301, 201);
    require(history.append(repeated) && history.append(repeated) && history.entries().size() == 2 &&
                history.entries().at(0) == history.entries().at(1),
            "each recording start keeps its selected rectangle, including duplicates");
    require(!history.append(QRect()) && history.entries().size() == 2,
            "invalid rectangles cannot append history");
    for (int i = 0; i < 102; ++i)
        require(history.append(QRect(i, 20, 300, 200)), "append recording area");
    require(history.entries().size() == 100 && history.entries().first().rectangle.x() == 2 &&
                history.entries().last().rectangle.x() == 101,
            "only the newest 100 starts survive, in insertion order");
    auto valid = storage::recordingAreaToJson({repeated, storage::recordingAreaCoordinateSpace()});
    auto fractional = valid;
    fractional.insert(QStringLiteral("x"), 1.5);
    auto overflow = valid;
    overflow.insert(QStringLiteral("x"), std::numeric_limits<int>::max());
    auto empty = valid;
    empty.insert(QStringLiteral("height"), 0);
    auto wrongSpace = valid;
    wrongSpace.insert(QStringLiteral("coordinate_space"), QStringLiteral("unknown"));
    auto otherPlatform = valid;
    otherPlatform.insert(QStringLiteral("coordinate_space"),
                         storage::recordingAreaCoordinateSpace() == QStringLiteral("desktop_points")
                             ? QStringLiteral("physical_pixels")
                             : QStringLiteral("desktop_points"));
    require(
        configuration.setValue(key, QJsonArray{valid, fractional, overflow, empty, wrongSpace,
                                               QJsonValue(), otherPlatform, valid}) &&
            history.entries().size() == 3 && history.entries().first().rectangle == repeated &&
            history.entries().last().rectangle == repeated,
        "malformed entries are ignored without losing valid duplicate or other-platform entries");
    require(!configuration.setValue(key, QJsonObject()) && history.entries().size() == 3,
            "invalid history containers are rejected without replacing history");
    const auto snapshot = configuration.snapshot();
    auto legacy = snapshot;
    legacy.remove(QStringLiteral("screen_recording_shortcuts/previous_recording_area"));
    legacy.remove(QStringLiteral("screen_recording_shortcuts/next_recording_area"));
    legacy.insert(QStringLiteral("screen_recording_shortcuts/export"),
                  snow_shot::shortcuts::shortcutBindingsToJson({QStringLiteral("R")}));
    require(configuration.applySnapshot(legacy) &&
                storage::ScreenRecordingShortcutSettings().shortcuts(QStringLiteral("export")) ==
                    snow_shot::shortcuts::ShortcutBindingList{QStringLiteral("R")} &&
                storage::ScreenRecordingShortcutSettings()
                    .shortcuts(QStringLiteral("previous_recording_area"))
                    .isEmpty() &&
                storage::ScreenRecordingShortcutSettings().shortcuts(
                    QStringLiteral("next_recording_area")) ==
                    snow_shot::shortcuts::ShortcutBindingList{QStringLiteral("Shift+R")},
            "new shortcut defaults preserve existing custom bindings during upgrade");
    require(configuration.applySnapshot(snapshot), "restore default shortcut fixture");
}
} // namespace

int main(int argc, char** argv) {
    QApplication application(argc, argv);
    QTemporaryDir temporary;
    require(temporary.isValid(), "create isolated recording shortcut storage");
    const QString executable = QDir(temporary.path()).filePath(QStringLiteral("bin"));
    require(QDir().mkpath(executable), "create recording shortcut executable directory");
    namespace storage = snow_shot::storage;
    namespace settings = snow_shot::presentation::settings;
    auto& applicationStorage = storage::ApplicationStorage::instance();
    require(applicationStorage.initialize({executable, temporary.path(), 60000}).success,
            "initialize recording shortcut storage");
    const storage::ScreenRecordingShortcutSettings recording;
    const auto defaults = recording.allShortcuts();
    require(defaults.value(QStringLiteral("previous_recording_area")) ==
                    snow_shot::shortcuts::ShortcutBindingList{QStringLiteral("R")} &&
                defaults.value(QStringLiteral("next_recording_area")) ==
                    snow_shot::shortcuts::ShortcutBindingList{QStringLiteral("Shift+R")},
            "recording area shortcuts default to R and Shift+R");
    recordingAreaHistoryStorage();
    const auto savedAreas = storage::RecordingAreaHistorySettings().entries();
    const QString exportId = QStringLiteral("export");
    const snow_shot::shortcuts::ShortcutBindingList custom = {QStringLiteral("F12"),
                                                              QStringLiteral("Ctrl+F12")};
    {
        snow_shot::presentation::GlobalShortcutManager globalShortcuts;
        settings::BuiltInSettingsBackend backend(globalShortcuts);
        settings::SettingsRuntimeSession session(settings::builtInSettingsRegistry(), backend);
        const auto scope = settings::SettingsLocalShortcutScope::ScreenRecording;
        for (auto it = defaults.cbegin(); it != defaults.cend(); ++it) {
            require(
                session.localShortcuts(scope, it.key()) == it.value() &&
                    backend.validateLocalShortcut(scope, it.key(), it.value().first()).supported,
                "settings must expose and accept all defaults, including Ctrl+C and Esc");
        }
        require(session.applyLocalShortcuts(scope, exportId, custom) &&
                    recording.shortcuts(exportId) == custom &&
                    session.localShortcuts(scope, exportId) == custom,
                "settings edits must update storage and runtime state");
        require(
            !backend.validateLocalShortcut(scope, exportId, QStringLiteral("Shift+R")).supported,
            "Shift+R participates in recording shortcut conflict detection");
        require(session.applyLocalShortcuts(scope, QStringLiteral("next_recording_area"),
                                            {QStringLiteral("F8")}) &&
                    session.applyLocalShortcuts(scope, QStringLiteral("previous_recording_area"),
                                                {QStringLiteral("Shift"), QStringLiteral("F9")}) &&
                    recording.shortcuts(QStringLiteral("previous_recording_area")).size() == 2,
                "both area actions support rebinding, including modifier-only bindings");
        require(!backend.validateLocalShortcut(scope, exportId, QStringLiteral("Shift")).supported,
                "custom standalone Shift participates in recording shortcut conflict detection");
        const auto duplicate =
            backend.validateLocalShortcut(scope, exportId, QStringLiteral("Ctrl+S"));
        require(!duplicate.supported &&
                    duplicate.failureReason ==
                        snow_shot::presentation::GlobalShortcutFailureReason::AlreadyInUse &&
                    !session.applyLocalShortcuts(scope, exportId, {QStringLiteral("Ctrl+S")}) &&
                    recording.shortcuts(exportId) == custom,
                "duplicate recording keys must be rejected without changing saved settings");
        require(
            !backend.validateLocalShortcut(scope, exportId, QStringLiteral("Ctrl+K, Ctrl+C"))
                    .supported &&
                !backend
                     .validateLocalShortcut(scope, QStringLiteral("unknown"), QStringLiteral("F12"))
                     .supported &&
                !recording.setShortcuts(QStringLiteral("unknown"), custom),
            "unknown actions and multi-step sequences must be rejected");
        auto conflicting = recording.allShortcuts();
        conflicting.insert(QStringLiteral("end_recording"), custom);
        require(!recording.setAllShortcutsAtomic(conflicting) &&
                    recording.shortcuts(exportId) == custom &&
                    recording.shortcuts(QStringLiteral("end_recording")) ==
                        defaults.value(QStringLiteral("end_recording")),
                "adapter must reject duplicate keys atomically");
        conflicting.remove(exportId);
        require(!recording.setAllShortcutsAtomic(conflicting),
                "atomic updates require every action");
        require(session.applyLocalShortcuts(scope, QStringLiteral("end_recording"), {}) &&
                    recording.shortcuts(QStringLiteral("end_recording")).isEmpty(),
                "settings must support disabling shortcuts");
        const auto screenshotBefore = storage::ScreenshotShortcutSettings().allShortcuts();
        const auto drawingBefore = storage::DrawingShortcutSettings().allShortcuts();
        require(session.reset(settings::SettingsSectionReset::ScreenRecordingShortcuts) &&
                    recording.allShortcuts() == defaults &&
                    session.localShortcuts(scope, exportId) == defaults.value(exportId) &&
                    storage::ScreenshotShortcutSettings().allShortcuts() == screenshotBefore &&
                    storage::DrawingShortcutSettings().allShortcuts() == drawingBefore,
                "recording reset must restore defaults without changing other shortcut categories");
        require(session.applyLocalShortcuts(scope, exportId, custom) &&
                    session.applyLocalShortcuts(scope, QStringLiteral("end_recording"), {}),
                "prepare custom and empty bindings for persistence verification");
    }
    require(applicationStorage.flushNow().success, "save recording shortcut configuration");
    applicationStorage.shutdown();
    require(applicationStorage.initialize({executable, temporary.path(), 60000}).success &&
                recording.shortcuts(exportId) == custom &&
                recording.shortcuts(QStringLiteral("end_recording")).isEmpty() &&
                recording.shortcuts(QStringLiteral("toggle_recording")) ==
                    defaults.value(QStringLiteral("toggle_recording")) &&
                recording.shortcuts(QStringLiteral("next_recording_area")) ==
                    defaults.value(QStringLiteral("next_recording_area")) &&
                storage::RecordingAreaHistorySettings().entries() == savedAreas,
            "custom, disabled, and default bindings must survive restart");
    applicationStorage.shutdown();
    return 0;
}
