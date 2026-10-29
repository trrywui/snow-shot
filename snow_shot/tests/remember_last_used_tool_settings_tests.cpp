#include "snow_shot/presentation/globalshortcutmanager.h"
#include "snow_shot/presentation/settings/settingsbackend.h"
#include "snow_shot/storage/applicationstorage.h"
#include "snow_shot/storage/configurationschema.h"
#include "snow_shot/storage/settingsadapters.h"

#include <QApplication>
#include <QJsonArray>
#include <QTemporaryDir>

#include <cstdlib>
#include <iostream>

namespace {
namespace settings = snow_shot::presentation::settings;
namespace storage = snow_shot::storage;

void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(1);
    }
}

void automaticSelectionDefaultsPersistsAndResets(const QString& configurationPath) {
    snow_shot::presentation::GlobalShortcutManager shortcuts;
    settings::BuiltInSettingsBackend backend(shortcuts);
    constexpr auto binding =
        settings::SettingsSwitchBinding::DrawingAutomaticallySelectNewlyCreatedElements;
    const QString key = QStringLiteral("drawing/automatically_select_newly_created_elements");
    require(backend.switchEnabled(binding) && !backend.switchValue(binding) &&
                !storage::DrawingSettings().automaticallySelectNewlyCreatedElements() &&
                !storage::ConfigurationSchema::defaultValue(key).toBool(),
            "automatic selection must default to disabled");
    require(backend.applySwitchValue(binding, true) && backend.switchValue(binding) &&
                storage::DrawingSettings().automaticallySelectNewlyCreatedElements(),
            "the backend must apply and read the automatic selection preference");
    require(storage::ApplicationStorage::instance().configuration().flushNow().success,
            "the automatic selection preference must be flushable");
    storage::ConfigurationStore reloaded(configurationPath, true, true, 60000);
    require(reloaded.value(key).toBool(),
            "the enabled automatic selection preference must survive reload");
    require(backend.resetSection(settings::SettingsSectionReset::DrawingQuickSelection) &&
                !backend.switchValue(binding) &&
                !storage::DrawingSettings().automaticallySelectNewlyCreatedElements(),
            "resetting annotation settings must disable automatic selection");
}

void automaticSelectionExclusionsPersistAndReset(const QString& configurationPath) {
    snow_shot::presentation::GlobalShortcutManager shortcuts;
    settings::BuiltInSettingsBackend backend(shortcuts);
    constexpr auto binding =
        settings::SettingsMultiSelectBinding::DrawingAutomaticSelectionExcludedTools;
    const QString key = QStringLiteral("drawing/automatic_selection_excluded_tools");
    const QVariantList defaults{QStringLiteral("free-draw"), QStringLiteral("pen-filter"),
                                QStringLiteral("text"), QStringLiteral("speech-bubble")};
    require(backend.multiSelectValue(binding) == defaults,
            "Pen, Pen filter, Text, and Speech Bubble must be excluded by default");
    require(
        backend.applyMultiSelectValue(binding, {QStringLiteral("shape"), QStringLiteral("text")}) &&
            storage::DrawingSettings().automaticSelectionExcludedTools() ==
                QStringList{QStringLiteral("shape"), QStringLiteral("text")},
        "the dropdown must update its independent tool list");
    require(storage::ApplicationStorage::instance().configuration().flushNow().success,
            "automatic selection exclusions must be flushable");
    storage::ConfigurationStore reloaded(configurationPath, true, true, 60000);
    require(reloaded.value(key).toArray() ==
                QJsonArray{QStringLiteral("shape"), QStringLiteral("text")},
            "automatic selection exclusions must survive reload");
    require(backend.applyMultiSelectValue(binding, {}) &&
                backend.multiSelectValue(binding).isEmpty(),
            "the dropdown must allow clearing every exclusion");
    require(backend.resetSection(settings::SettingsSectionReset::DrawingQuickSelection) &&
                backend.multiSelectValue(binding) == defaults,
            "Drawing reset must restore the default exclusions");
}

void deleteTextAfterCursorDefaultsPersistsAndResets(const QString& configurationPath) {
    snow_shot::presentation::GlobalShortcutManager shortcuts;
    settings::BuiltInSettingsBackend backend(shortcuts);
    constexpr auto binding = settings::SettingsSwitchBinding::DrawingDeleteTextAfterCursor;
    const QString key = QStringLiteral("drawing/delete_text_after_cursor");
    require(backend.switchEnabled(binding) && backend.switchValue(binding) &&
                storage::DrawingSettings().deleteTextAfterCursor() &&
                storage::ConfigurationSchema::defaultValue(key).toBool(),
            "Delete text after the cursor must default to enabled");
    require(backend.applySwitchValue(binding, false) && !backend.switchValue(binding) &&
                !storage::DrawingSettings().deleteTextAfterCursor(),
            "the backend must apply and read the forward Delete preference");
    require(storage::ApplicationStorage::instance().configuration().flushNow().success,
            "the forward Delete preference must be flushable");
    storage::ConfigurationStore reloaded(configurationPath, true, true, 60000);
    require(!reloaded.value(key).toBool(),
            "the disabled forward Delete preference must survive reload");
    require(backend.resetSection(settings::SettingsSectionReset::DrawingQuickSelection) &&
                backend.switchValue(binding) && storage::DrawingSettings().deleteTextAfterCursor(),
            "resetting annotation settings must restore forward Delete");
}

void rememberLastUsedToolDefaultsPersistsAndResets(const QString& configurationPath) {
    snow_shot::presentation::GlobalShortcutManager shortcuts;
    settings::BuiltInSettingsBackend backend(shortcuts);
    constexpr auto binding = settings::SettingsSwitchBinding::DrawingRememberLastUsedTool;
    require(backend.switchEnabled(binding) && !backend.switchValue(binding) &&
                !storage::DrawingSettings().rememberLastUsedTool() &&
                !storage::ConfigurationSchema::defaultValue(
                     QStringLiteral("drawing/remember_last_used_tool"))
                     .toBool(),
            "remember last used tool must default to disabled");
    require(backend.applySwitchValue(binding, true) && backend.switchValue(binding) &&
                storage::DrawingSettings().rememberLastUsedTool(),
            "the settings backend must apply and read the remembered tool preference");
    require(storage::ApplicationStorage::instance().configuration().flushNow().success,
            "the remembered tool preference must be flushable");
    storage::ConfigurationStore reloaded(configurationPath, true, true, 60000);
    require(reloaded.value(QStringLiteral("drawing/remember_last_used_tool")).toBool(),
            "the remembered tool preference must survive a configuration reload");
    require(storage::ScreenshotToolbarSettings().lastDrawingTool().isEmpty(),
            "no drawing tool is remembered before any tool activation");
    const auto* lastDrawingToolEntry =
        storage::ConfigurationSchema::entry(QStringLiteral("screenshot_toolbar/last_drawing_tool"));
    require(lastDrawingToolEntry != nullptr &&
                lastDrawingToolEntry->allowedStringValues.contains(QString()) &&
                lastDrawingToolEntry->allowedStringValues.contains(QStringLiteral("shape")) &&
                lastDrawingToolEntry->allowedStringValues.contains(QStringLiteral("watermark")) &&
                storage::ConfigurationSchema::normalize(
                    QStringLiteral("screenshot_toolbar/last_drawing_tool"), QString())
                    .valid &&
                storage::ConfigurationSchema::normalize(
                    QStringLiteral("screenshot_toolbar/last_drawing_tool"), QStringLiteral("shape"))
                    .valid &&
                !storage::ConfigurationSchema::normalize(
                     QStringLiteral("screenshot_toolbar/last_drawing_tool"),
                     QStringLiteral("unknown-tool"))
                     .valid &&
                !storage::ScreenshotToolbarSettings().setLastDrawingTool(
                    QStringLiteral("unknown-tool")) &&
                storage::ScreenshotToolbarSettings().lastDrawingTool().isEmpty(),
            "the remembered drawing tool must accept toolbar item ids and reject unknown ids");
    require(storage::ScreenshotToolbarSettings().setLastDrawingTool(QStringLiteral("shape")) &&
                storage::ScreenshotToolbarSettings().lastDrawingTool() == QStringLiteral("shape"),
            "the remembered drawing tool must round-trip through the toolbar settings");
    require(backend.resetSection(settings::SettingsSectionReset::DrawingQuickSelection) &&
                !backend.switchValue(binding) && !storage::DrawingSettings().rememberLastUsedTool(),
            "resetting the Drawing function settings must restore the default disabled switch");
}

void firstToolbarGroupButtonDefaultsPersistsAndResets(const QString& configurationPath) {
    snow_shot::presentation::GlobalShortcutManager shortcuts;
    settings::BuiltInSettingsBackend backend(shortcuts);
    constexpr auto binding =
        settings::SettingsSwitchBinding::DrawingAlwaysShowFirstToolbarGroupButton;
    const QString key = QStringLiteral("drawing/always_show_first_toolbar_group_button");
    require(backend.switchEnabled(binding) && !backend.switchValue(binding) &&
                !storage::DrawingSettings().alwaysShowFirstToolbarGroupButton() &&
                !storage::ConfigurationSchema::defaultValue(key).toBool(),
            "the first toolbar group button preference must default to disabled");
    require(backend.applySwitchValue(binding, true) && backend.switchValue(binding) &&
                storage::DrawingSettings().alwaysShowFirstToolbarGroupButton(),
            "the backend must apply and read the first toolbar group button preference");
    require(storage::ApplicationStorage::instance().configuration().flushNow().success,
            "the first toolbar group button preference must be flushable");
    storage::ConfigurationStore reloaded(configurationPath, true, true, 60000);
    require(reloaded.value(key).toBool(),
            "the first toolbar group button preference must survive a configuration reload");
    require(backend.resetSection(settings::SettingsSectionReset::DrawingQuickSelection) &&
                !backend.switchValue(binding) &&
                !storage::DrawingSettings().alwaysShowFirstToolbarGroupButton(),
            "resetting annotation settings must disable the first toolbar group button preference");
}
} // namespace

int main(int argc, char** argv) {
    QApplication application(argc, argv);
    QTemporaryDir temporary;
    require(temporary.isValid(), "failed to create isolated settings directory");
    auto& applicationStorage = storage::ApplicationStorage::instance();
    static_cast<void>(
        applicationStorage.initialize({temporary.filePath(QStringLiteral("bin")),
                                       temporary.filePath(QStringLiteral("data")), 60000}));
    automaticSelectionDefaultsPersistsAndResets(
        temporary.filePath(QStringLiteral("data/config.json")));
    automaticSelectionExclusionsPersistAndReset(
        temporary.filePath(QStringLiteral("data/config.json")));
    deleteTextAfterCursorDefaultsPersistsAndResets(
        temporary.filePath(QStringLiteral("data/config.json")));
    rememberLastUsedToolDefaultsPersistsAndResets(
        temporary.filePath(QStringLiteral("data/config.json")));
    firstToolbarGroupButtonDefaultsPersistsAndResets(
        temporary.filePath(QStringLiteral("data/config.json")));
    applicationStorage.shutdown();
    return 0;
}
