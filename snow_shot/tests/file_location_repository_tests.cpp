#include "snow_shot/storage/capturehistoryrepository.h"
#include "snow_shot/storage/pinnedwindowrepository.h"
#include "snowimageqtcodec.h"
#include "pinned_animation_fixtures.h"

#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QTemporaryDir>
#include <QUuid>
#include <atomic>
#include <cstdlib>
#include <iostream>

namespace {
namespace storage = snow_shot::storage;

void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(1);
    }
}

QString newId() {
    return QUuid::createUuid().toString(QUuid::WithoutBraces);
}

QByteArray bytes(const QString& path) {
    QFile file(path);
    require(file.open(QIODevice::ReadOnly), "read fixture manifest");
    return file.readAll();
}

QImage image() {
    QImage result(16, 12, QImage::Format_ARGB32);
    result.fill(Qt::green);
    return result;
}

void historyLocations() {
    QTemporaryDir directory;
    QTemporaryDir moved;
    std::atomic<int> payloadReads = 0;
    storage::CaptureHistoryRepositoryOptions options;
    options.operationObserved = [&](storage::CaptureHistoryOperation operation) {
        if (operation == storage::CaptureHistoryOperation::PayloadRead)
            ++payloadReads;
    };
    auto repository = storage::makeCaptureHistoryRepository(directory.path(), options);
    QVector<storage::CaptureHistoryRecord> records;
    for (int kind = 0; kind < 3; ++kind) {
        storage::CaptureHistoryDraft draft;
        draft.id = newId();
        draft.createdUtc = QDateTime::currentDateTimeUtc();
        draft.canvasBounds = image().rect();
        draft.selection.rectangle = draft.canvasBounds;
        draft.selection.shadowColor = QColor(Qt::black);
        draft.canvasHistory =
            QByteArrayLiteral("{\"schemaVersion\":1,\"document\":{},\"history\":{}}");
        draft.displays.append({QStringLiteral("one"), QStringLiteral("One"), image()});
        if (kind == 0)
            draft.resultImage = image();
        if (kind == 2)
            draft.displays.append({QStringLiteral("two"), QStringLiteral("Two"), image()});
        const auto published = repository->publish(draft).get();
        require(published.storage.success, "publish location fixture");
        records.append(published.record);
    }
    repository->drain();
    const QString manifest = directory.filePath(QStringLiteral("capture_history/index.json"));
    const QByteArray before = bytes(manifest);
    const auto resultAssets = repository->displayAssets(records[0]);
    const auto displayAssets = repository->displayAssets(records[1]);
    require(resultAssets && resultAssets->result && displayAssets, "resolve fixture assets");
    const QString result = resultAssets->result->localFileUrl.toLocalFile();
    require(repository->revealPath(records[0].id) == result, "history reveals saved result");
    require(repository->revealPath(records[1].id) ==
                displayAssets->displays.front().localFileUrl.toLocalFile(),
            "resultless single-display history reveals its display");
    const QString multiDirectory =
        directory.filePath(QStringLiteral("capture_history/records/%1").arg(records[2].id));
    require(repository->revealPath(records[2].id) == multiDirectory,
            "multi-display history opens its entry directory");
    require(QFile::remove(result), "remove primary history file");
    require(repository->revealPath(records[0].id) == QFileInfo(result).absolutePath(),
            "a missing result falls back to its directory");
    require(!repository->revealPath(QStringLiteral("../records")) &&
                !repository->revealPath(newId()),
            "unknown history IDs never reveal directories");
    require(payloadReads == 0 && bytes(manifest) == before && repository->records().size() == 3,
            "reveal does not decode, rewrite, or evict history with missing content");
    repository->suspendWrites(true);
    require(!repository->revealPath(records[1].id), "suspended storage cannot be revealed");
    const QString oldRoot = directory.filePath(QStringLiteral("capture_history"));
    const QString newRoot = moved.filePath(QStringLiteral("capture_history"));
    require(QDir().rename(oldRoot, newRoot), "move fixture history storage");
    require(repository->relocate(moved.path()).success, "reload relocated history metadata");
    repository->suspendWrites(false);
    const auto relocated = repository->revealPath(records[1].id);
    require(relocated && relocated->startsWith(newRoot + u'/'),
            "reveal uses the current storage root after relocation");
    require(repository->remove(records[1].id).get().success &&
                !repository->revealPath(records[1].id),
            "deleted history cannot be revealed");
    require(QDir(newRoot + QStringLiteral("/records/") + records[2].id).removeRecursively(),
            "remove fixture entry directory");
    require(!repository->revealPath(records[2].id), "missing entry directory fails safely");
}

void pinnedLocations() {
    QTemporaryDir directory;
    QTemporaryDir moved;
    std::atomic<int> payloadOperations = 0;
    storage::PinnedWindowRepository repository(
        directory.path(), true, 30000, [&](storage::PinnedWindowOperation operation) {
            if (operation == storage::PinnedWindowOperation::PayloadRead ||
                operation == storage::PinnedWindowOperation::PayloadWrite)
                ++payloadOperations;
        });
    QVector<storage::PinnedWindowRecord> records;
    for (int kind = 0; kind < 5; ++kind) {
        storage::PinnedWindowRecord record;
        record.id = newId();
        record.image = image();
        record.initialWindowSize = record.image.size();
        record.nativeGeometry = record.image.rect();
        record.canvasSourceRect = record.image.rect();
        record.contentCanvasRect = record.image.rect();
        record.surfaceCanvasRect = record.image.rect();
        if (kind == 1) {
            record.sourceKind = storage::PinnedWindowSourceKind::ClipboardImageFile;
            record.originalFileName = QString::fromUtf8("imported 图像, #1.png");
            record.originalFilePath = directory.filePath(record.originalFileName);
            require(record.image.save(record.originalFilePath), "save imported image fixture");
        } else if (kind == 2) {
            record.animationSource = snow_shot::image_codec::decodeAnimationSource(
                pinned_animation_fixtures::images().front().bytes);
            require(record.animationSource && record.animationSource->isAnimated(),
                    "decode animated fixture");
            record.image = record.animationSource->firstFrame;
        } else if (kind >= 3) {
            record.sourceKind = storage::PinnedWindowSourceKind::ClipboardText;
            record.image = {};
            record.originalHtml = QStringLiteral("<b>saved text</b>");
            if (kind == 3)
                record.originalText = QStringLiteral("saved text");
        }
        require(repository.upsert(record).success, "create pin location fixture");
        records.append(record);
    }
    require(repository.flush().success, "persist fixture content");
    const QString root = directory.filePath(QStringLiteral("pinned_windows_v2"));
    const QString manifest = root + QStringLiteral("/index.json");
    const QByteArray before = bytes(manifest);
    payloadOperations = 0;
    const QStringList names{QStringLiteral("source.png"), records[1].originalFileName,
                            QStringLiteral("source_animation.gif"), QStringLiteral("original.txt"),
                            QStringLiteral("original.html")};
    for (int index = 0; index < records.size(); ++index) {
        const QString expected =
            root + QStringLiteral("/pins/%1/%2").arg(records[index].id, names[index]);
        require(repository.revealPath(records[index].id) == expected,
                "pins reveal the appropriate managed source payload");
    }
    require(repository.revealPath(records[1].id) != records[1].originalFilePath,
            "imported pins reveal their managed copy");
    const QString textPath = *repository.revealPath(records[3].id);
    require(QFile::remove(textPath), "remove stored text fixture");
    require(repository.revealPath(records[3].id) ==
                QFileInfo(textPath).dir().filePath(QStringLiteral("original.html")),
            "text falls back to stored HTML");
    const QString animationPath = *repository.revealPath(records[2].id);
    require(QFile::remove(animationPath), "remove stored animation fixture");
    require(repository.revealPath(records[2].id) == QFileInfo(animationPath).absolutePath(),
            "missing animation opens its folder instead of revealing its poster");
    require(!repository.revealPath(newId()) && !repository.revealPath(QStringLiteral("../pins")),
            "unknown pin IDs cannot reveal files");
    require(payloadOperations == 0 && bytes(manifest) == before,
            "revealing pin locations never reads payloads or writes metadata");
    const QString pending = newId();
    repository.reserveCreation(pending);
    require(!repository.revealPath(pending), "pending reservations have no saved content");
    repository.cancelCreation(pending);
    repository.suspendWrites(true);
    require(!repository.revealPath(records[0].id), "suspended pins cannot be revealed");
    const QString newRoot = moved.filePath(QStringLiteral("pinned_windows_v2"));
    require(QDir().rename(root, newRoot), "move fixture pin storage");
    storage::PinnedWindowRepository prepared(moved.path());
    prepared.suspendWrites(true);
    repository.exchangeStorage(prepared);
    repository.suspendWrites(false);
    const auto relocated = repository.revealPath(records[0].id);
    require(relocated && relocated->startsWith(newRoot + u'/'),
            "pin location follows exchanged storage");
    require(repository.remove(records[0].id).success && !repository.revealPath(records[0].id),
            "deleted pins cannot be revealed");
    const QString missingDirectory = newRoot + QStringLiteral("/pins/") + records[1].id;
    require(QDir(missingDirectory).removeRecursively(), "remove fixture pin directory");
    require(!repository.revealPath(records[1].id), "missing pin directory fails safely");
}
} // namespace

int main(int argc, char** argv) {
    QCoreApplication application(argc, argv);
    historyLocations();
    pinnedLocations();
    std::cout << "File-location repository tests passed\n";
}
