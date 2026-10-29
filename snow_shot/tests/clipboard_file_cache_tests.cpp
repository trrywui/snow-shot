#include "snow_shot/storage/clipboardfilecache.h"
#include "snow_shot/app/distribution.h"

#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QProcess>
#include <QTemporaryDir>
#include <QScopeGuard>

#include <cstdlib>
#include <iostream>

namespace {
using snow_shot::storage::ClipboardFileCache;

void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(EXIT_FAILURE);
    }
}

QDateTime fixtureTime() {
    return QDateTime::fromString(QStringLiteral("2026-10-14T12:00:00Z"), Qt::ISODate);
}

QString writeFile(const ClipboardFileCache::ExportDirectory& directory) {
    const QString path = QDir(directory.path()).filePath(QStringLiteral("image.png"));
    QFile file(path);
    require(file.open(QIODevice::WriteOnly) && file.write("image") == 5,
            "clipboard cache fixture must be writable");
    return path;
}

void stagingAndRetainedFilesHaveSeparateLifetimes() {
    QTemporaryDir root;
    require(root.isValid(), "clipboard cache fixture root must be available");
    QString retainedPath;
    {
        ClipboardFileCache cache(root.path());
        QString error = QStringLiteral("stale error");
        auto staged = cache.createDirectory(&error, fixtureTime());
        require(staged && error.isEmpty(), "clipboard staging must clear previous errors");
        const QString discardedPath = writeFile(*staged);
        staged.reset();
        require(!QFileInfo::exists(QFileInfo(discardedPath).path()),
                "failed or cancelled staging must remove its entire directory");

        auto retained = cache.createDirectory(&error, fixtureTime());
        require(bool(retained), "completed clipboard export must have a staging directory");
        retainedPath = writeFile(*retained);
        retained->retain();
        retained.reset();
        require(QFileInfo::exists(retainedPath),
                "completed files must outlive the export directory wrapper");
    }
    require(QFileInfo::exists(retainedPath), "cache destruction must preserve completed files");
}

void expiryPreservesRecentAndActiveExports() {
    QTemporaryDir root;
    require(root.isValid(), "clipboard cache fixture root must be available");
    ClipboardFileCache cache(root.path());
    QString error;
    const QDateTime createdAt = fixtureTime();
    auto first = cache.createDirectory(&error, createdAt);
    auto second = cache.createDirectory(&error, createdAt.addSecs(1));
    auto active = cache.createDirectory(&error, createdAt);
    require(first && second && active, "clipboard exports must have independent directories");
    const QString firstPath = writeFile(*first);
    const QString secondPath = writeFile(*second);
    const QString activePath = writeFile(*active);
    require(firstPath != secondPath, "identical export filenames must never collide");
    first->retain();
    second->retain();
    active->retain();
    first.reset();
    second.reset();

    const QDateTime expiresAt = createdAt.addDays(ClipboardFileCache::RetentionDays);
    cache.removeExpired(expiresAt.addMSecs(-1));
    require(QFileInfo::exists(firstPath) && QFileInfo::exists(secondPath),
            "cache cleanup must preserve the full retention interval");
    cache.removeExpired(expiresAt);
    require(!QFileInfo::exists(QFileInfo(firstPath).path()) && QFileInfo::exists(secondPath) &&
                QFileInfo::exists(activePath),
            "expiry must remove old exports while preserving recent files and active writers");
    active.reset();
    cache.removeExpired(expiresAt.addSecs(1));
    require(!QFileInfo::exists(QFileInfo(secondPath).path()) &&
                !QFileInfo::exists(QFileInfo(activePath).path()),
            "completed writers must become eligible for independent cache expiry");
}

void newExportsCleanExpiredFilesWithoutRemovingOtherDirectories() {
    QTemporaryDir root;
    require(root.isValid(), "clipboard cache fixture root must be available");
    ClipboardFileCache cache(root.path());
    QString error;
    auto old = cache.createDirectory(&error, fixtureTime());
    require(bool(old), "old clipboard fixture must be available");
    const QString oldPath = writeFile(*old);
    old->retain();
    old.reset();
    const QString unrelated = root.filePath(QStringLiteral("other-1000000000000-abcdef"));
    const QString malformed = root.filePath(QStringLiteral("export-invalid-abcdef"));
    require(QDir().mkpath(unrelated) && QDir().mkpath(malformed),
            "unrelated cache fixtures must be available");
    auto next =
        cache.createDirectory(&error, fixtureTime().addDays(ClipboardFileCache::RetentionDays));
    require(next && !QFileInfo::exists(oldPath) && QFileInfo::exists(unrelated) &&
                QFileInfo::exists(malformed),
            "new exports must reclaim expired files without deleting other directories");
}

void abandonedWritersAreReclaimedAfterExpiry() {
    QTemporaryDir root;
    require(root.isValid(), "abandoned clipboard cache fixture must be available");
    QProcess child;
    child.start(QCoreApplication::applicationFilePath(),
                {QStringLiteral("--abandon-directory"), root.path()});
    require(child.waitForStarted(2000) && child.waitForFinished(5000) && child.exitCode() == 0 &&
                child.exitStatus() == QProcess::NormalExit,
            "abandoned clipboard staging process must finish");
    const QString path = QString::fromUtf8(child.readAllStandardOutput()).trimmed();
    require(QDir::fromNativeSeparators(path).startsWith(QDir::fromNativeSeparators(root.path()) +
                                                        u'/') &&
                QFileInfo::exists(path),
            "abandoned process must leave its own staging file");
    ClipboardFileCache cache(root.path());
    cache.removeExpired(fixtureTime().addDays(ClipboardFileCache::RetentionDays));
    require(!QFileInfo::exists(QFileInfo(path).path()),
            "expiry must reclaim abandoned directories and stale process locks");
}

void unavailableCacheReportsFilesystemFailure() {
    QTemporaryDir root;
    require(root.isValid(), "unavailable clipboard cache fixture must be available");
    const QString path = root.filePath(QStringLiteral("file"));
    QFile file(path);
    require(file.open(QIODevice::WriteOnly), "clipboard cache blocker must be writable");
    file.close();
    ClipboardFileCache cache(path);
    QString error;
    require(!cache.createDirectory(&error, fixtureTime()) && !error.isEmpty(),
            "an unavailable clipboard cache must report its filesystem error");
}

void emptyRootDoesNotUseTheWorkingDirectory() {
    QTemporaryDir root;
    require(root.isValid(), "empty clipboard cache fixture must be available");
    const QString previousDirectory = QDir::currentPath();
    const auto restoreDirectory = qScopeGuard([&] { QDir::setCurrent(previousDirectory); });
    require(QDir::setCurrent(root.path()), "empty clipboard cache fixture must become current");
    ClipboardFileCache fixture(root.path());
    QString error;
    auto retained = fixture.createDirectory(&error, fixtureTime());
    require(bool(retained), "unrelated clipboard export must be available");
    const QString path = writeFile(*retained);
    retained->retain();
    retained.reset();
    ClipboardFileCache empty{QString()};
    require(!empty.createDirectory(&error, fixtureTime()) && !error.isEmpty(),
            "an empty clipboard cache root must fail without a relative fallback");
    empty.removeExpired(fixtureTime().addDays(ClipboardFileCache::RetentionDays));
    require(QFileInfo::exists(path), "empty cache expiry must preserve working-directory exports");
}

void defaultRootFollowsTheDistributionPolicy() {
    namespace distribution = snow_shot::app::distribution;
    if (!distribution::isStoreManaged()) {
        require(ClipboardFileCache::defaultRootDirectory() ==
                    QDir::temp().filePath(QStringLiteral("snow-shot-clipboard")),
                "standalone clipboard cache must retain its native temporary root");
        return;
    }
    QTemporaryDir root;
    require(root.isValid(), "Store clipboard fixture must be available");
    const bool hadLocal = qEnvironmentVariableIsSet("LOCALAPPDATA");
    const QByteArray previousLocal = qgetenv("LOCALAPPDATA");
    const auto restoreLocal = qScopeGuard([&] {
        if (hadLocal)
            qputenv("LOCALAPPDATA", previousLocal);
        else
            qunsetenv("LOCALAPPDATA");
    });
    require(qputenv("LOCALAPPDATA", root.path().toUtf8()),
            "Store clipboard LOCALAPPDATA fixture must be set");
    const QString storeRoot = ClipboardFileCache::defaultRootDirectory();
    require(!distribution::writableDataDirectory().isEmpty() &&
                storeRoot == QDir(distribution::writableDataDirectory())
                                 .filePath(QStringLiteral("cache/clipboard")),
            "Store clipboard exports must use package LocalState cache storage");
    ClipboardFileCache native(root.filePath(QStringLiteral("native-clipboard")));
    ClipboardFileCache store(storeRoot);
    QString error;
    auto sharedDirectory = ClipboardFileCache::shared().createDirectory(&error, fixtureTime());
    require(sharedDirectory && sharedDirectory->path().startsWith(storeRoot + u'/'),
            "shared Store clipboard exports must use the package cache root");
    sharedDirectory.reset();
    auto nativeDirectory = native.createDirectory(&error, fixtureTime());
    auto storeDirectory = store.createDirectory(&error, fixtureTime());
    require(nativeDirectory && storeDirectory, "channel clipboard exports must be available");
    const QString nativePath = writeFile(*nativeDirectory);
    const QString storePath = writeFile(*storeDirectory);
    nativeDirectory->retain();
    storeDirectory->retain();
    nativeDirectory.reset();
    storeDirectory.reset();
    store.removeExpired(fixtureTime().addDays(ClipboardFileCache::RetentionDays));
    require(!QFileInfo::exists(storePath) && QFileInfo::exists(nativePath),
            "Store expiry must preserve standalone clipboard exports");
    require(qunsetenv("LOCALAPPDATA"), "Store clipboard LOCALAPPDATA fixture must be unset");
    require(ClipboardFileCache::defaultRootDirectory().isEmpty(),
            "missing Store storage must not fall back to the shared clipboard root");
    require(qputenv("LOCALAPPDATA", "relative-store-data"),
            "relative Store clipboard LOCALAPPDATA fixture must be set");
    require(ClipboardFileCache::defaultRootDirectory().isEmpty(),
            "relative Store storage must not create working-directory clipboard exports");
}
} // namespace

int main(int argc, char** argv) {
    QCoreApplication app(argc, argv);
    if (app.arguments().contains(QStringLiteral("--abandon-directory"))) {
        ClipboardFileCache cache(app.arguments().last());
        QString error;
        auto directory = cache.createDirectory(&error, fixtureTime());
        require(bool(directory), "abandoned clipboard staging directory must be available");
        std::cout << writeFile(*directory).toStdString() << std::endl;
        // Simulate process termination before the writer can release its staging lock.
        std::_Exit(EXIT_SUCCESS);
    }
    defaultRootFollowsTheDistributionPolicy();
    emptyRootDoesNotUseTheWorkingDirectory();
    if (app.arguments().contains(QStringLiteral("--distribution-paths-only")))
        return EXIT_SUCCESS;
    stagingAndRetainedFilesHaveSeparateLifetimes();
    expiryPreservesRecentAndActiveExports();
    newExportsCleanExpiredFilesWithoutRemovingOtherDirectories();
    abandonedWritersAreReclaimedAfterExpiry();
    unavailableCacheReportsFilesystemFailure();
    return EXIT_SUCCESS;
}
