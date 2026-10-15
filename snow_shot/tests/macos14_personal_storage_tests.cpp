#include "snow_shot/app/edition.h"
#include "snow_shot/platform/macos/loginitemservice.h"
#include "snow_shot/storage/applicationstorage.h"
#include "snow_shot/storage/configurationschema.h"
#include "snow_shot/storage/storageusagetracker.h"

#include <QCoreApplication>
#include <QDir>
#include <QFileInfo>
#include <QTemporaryDir>
#include <cstdio>
#include <cstdlib>

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        std::fprintf(stderr, "%s\n", message);
        std::exit(1);
    }
}
} // namespace

int main(int argc, char** argv) {
    QCoreApplication application(argc, argv);
    namespace storage = snow_shot::storage;
    require(snow_shot::app::edition::isMacOS14Personal, "requires the personal edition");
    require(storage::ConfigurationSchema::defaultValue(QStringLiteral("system/auto_start_at_boot"))
                .toBool(),
            "maintained builds preserve the upstream launch-at-login default");
    const auto disabled =
        storage::ConfigurationSchema::normalize(QStringLiteral("system/auto_start_at_boot"), false);
    require(disabled.valid && !disabled.value.toBool(), "users can disable launch at login");
    for (const auto& key : {QStringLiteral("pin_to_screen/automatic_text_recognition"),
                            QStringLiteral("screenshot/auto_recognize_qr_code"),
                            QStringLiteral("text_recognition/resident_process"),
                            QStringLiteral("text_recognition/model_hot_start")}) {
        require(!storage::ConfigurationSchema::defaultValue(key).toBool(),
                "fresh personal settings must avoid automatic recognition");
        const auto enabled = storage::ConfigurationSchema::normalize(key, true);
        require(enabled.valid && enabled.value.toBool(),
                "users can still enable automatic recognition explicitly");
    }
    QTemporaryDir temporary;
    require(temporary.isValid(), "temporary personal data root exists");
    const QString root = QDir(temporary.path()).filePath(QStringLiteral("user-data"));
    qputenv("SNOW_SHOT_PERSONAL_DATA_DIR", root.toUtf8());
    require(storage::StorageUsageTracker::defaultThumbnailCacheDirectory() ==
                QDir(root).filePath(QStringLiteral("cache/history-thumbnails")),
            "thumbnail cache stays inside personal data");
    require(storage::StorageUsageTracker::defaultRecordingTempDirectory() ==
                QDir(root).filePath(QStringLiteral("cache/recordings")),
            "recording temporary files stay inside personal data");
    storage::StorageInitializationOptions options;
    options.executableDirectory = temporary.path();
    options.appDataDirectory = root;
    const auto selection = storage::ApplicationStorage::resolveDirectory(options);
    require(selection.effectiveDirectory == root &&
                selection.mode == storage::StorageMode::ApplicationData,
            "startup resolves the removable personal data directory");
    const auto login = snow_shot::platform::macos::nativeLoginItemOperations();
    require(!login.initialized(), "new personal data has no login initialization marker");
    require(login.markInitialized() && login.initialized(),
            "login initialization marker round-trips without registering a login item");
    require(QFileInfo::exists(QDir(root).filePath(QStringLiteral("macos-login-item.ini"))),
            "login marker stays in removable personal data");
    std::puts("macOS 14 personal settings and storage tests passed");
}
