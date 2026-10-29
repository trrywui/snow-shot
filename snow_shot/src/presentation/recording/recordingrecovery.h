#pragma once

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonDocument>
#include <QJsonArray>
#include <QJsonObject>
#include <QSaveFile>
#include <QString>
#include <QStringList>
#include <algorithm>
#include <chrono>
#include <functional>
#include <future>
#include <memory>
#include <mutex>
#include <optional>
#include <thread>
#include <utility>

namespace snow_shot::presentation::recording {
struct RecoveryRecord {
    QString sourcePath;
    QString outputPath;
    QStringList stagingPaths;
    bool operator==(const RecoveryRecord&) const = default;
};

inline QString recoveryDirectory(const RecoveryRecord& record) {
    const QString path = record.outputPath.isEmpty() ? record.sourcePath : record.outputPath;
    return path.isEmpty() ? QString() : QFileInfo(path).absolutePath();
}

inline RecoveryRecord refreshRecoveryStaging(RecoveryRecord record) {
    QStringList paths;
    for (const auto& path : record.stagingPaths) {
        if (QFileInfo::exists(path))
            paths.append(path);
    }
    const QString directory = recoveryDirectory(record);
    if (!directory.isEmpty()) {
        const QDir parent(directory);
        const auto entries = parent.entryInfoList(
            {QStringLiteral(".snow-deferred-*"), QStringLiteral(".snow-recording-direct-*")},
            QDir::AllEntries | QDir::NoDotAndDotDot | QDir::Hidden | QDir::System |
                QDir::NoSymLinks,
            QDir::Name);
        for (const auto& entry : entries) {
            if ((entry.fileName().startsWith(QStringLiteral(".snow-deferred-")) && entry.isDir()) ||
                (entry.fileName().startsWith(QStringLiteral(".snow-recording-direct-")) &&
                 (entry.isFile() || entry.suffix() == QStringLiteral("recovery"))))
                paths.append(entry.absoluteFilePath());
        }
    }
    paths.removeDuplicates();
    paths.sort();
    record.stagingPaths = paths.mid(0, 64);
    return record;
}

inline bool saveRecovery(const QString& marker, const RecoveryRecord& record) {
    if (marker.isEmpty() || (record.sourcePath.isEmpty() && record.outputPath.isEmpty()) ||
        !QDir().mkpath(QFileInfo(marker).absolutePath()))
        return false;
    const QJsonObject object{
        {QStringLiteral("schema"), 1},
        {QStringLiteral("source"), record.sourcePath},
        {QStringLiteral("output"), record.outputPath},
        {QStringLiteral("directory"), recoveryDirectory(record)},
        {QStringLiteral("staging"), QJsonArray::fromStringList(record.stagingPaths)}};
    const QByteArray bytes = QJsonDocument(object).toJson(QJsonDocument::Compact);
    QSaveFile file(marker);
    return file.open(QIODevice::WriteOnly) && file.write(bytes) == bytes.size() && file.commit();
}

inline std::optional<RecoveryRecord> loadRecovery(const QString& marker) {
    QFile file(marker);
    if (!file.open(QIODevice::ReadOnly) || file.size() > 32768)
        return std::nullopt;
    const auto document = QJsonDocument::fromJson(file.readAll());
    const auto object = document.object();
    RecoveryRecord result{object.value(QStringLiteral("source")).toString(),
                          object.value(QStringLiteral("output")).toString(),
                          {}};
    if (!document.isObject() || object.value(QStringLiteral("schema")).toInt() != 1 ||
        (result.sourcePath.isEmpty() && result.outputPath.isEmpty()) ||
        (!result.sourcePath.isEmpty() && !QDir::isAbsolutePath(result.sourcePath)) ||
        (!result.outputPath.isEmpty() && !QDir::isAbsolutePath(result.outputPath)))
        return std::nullopt;
    const auto staging = object.value(QStringLiteral("staging")).toArray();
    if (staging.size() > 64)
        return std::nullopt;
    for (const auto& value : staging) {
        const QString path = value.toString();
        if (!QDir::isAbsolutePath(path))
            return std::nullopt;
        result.stagingPaths.append(path);
    }
    return result;
}

inline bool clearRecovery(const QString& marker, const RecoveryRecord& recovered) {
    const auto current = loadRecovery(marker);
    return current && current->sourcePath == recovered.sourcePath &&
           current->outputPath == recovered.outputPath && QFile::remove(marker);
}

// Startup and terminal preservation meet on worker threads. Whichever publishes last writes
// the intended destination before native startup/finalization can block; the GUI never waits
// for this mutex or performs recovery I/O itself.
class StartupRecovery final {
  public:
    bool publishOutput(const QString& outputPath) {
        std::lock_guard lock(m_mutex);
        m_record.outputPath = outputPath;
        return m_marker.isEmpty() || writeLocked();
    }
    bool beginPreservation(const QString& marker, const RecoveryRecord& record = {}) {
        std::lock_guard lock(m_mutex);
        m_marker = marker;
        if (!record.sourcePath.isEmpty())
            m_record.sourcePath = record.sourcePath;
        if (!record.outputPath.isEmpty())
            m_record.outputPath = record.outputPath;
        m_record.stagingPaths.append(record.stagingPaths);
        return writeLocked();
    }

  private:
    bool writeLocked() {
        m_record = refreshRecoveryStaging(std::move(m_record));
        return saveRecovery(m_marker, m_record);
    }
    std::mutex m_mutex;
    QString m_marker;
    RecoveryRecord m_record;
};

// The detached worker owns its operation after the deadline. Its future never joins a GUI thread.
template <typename Operation, typename Launch>
bool runBoundedPreservation(Operation operation, int timeoutMs, Launch launch) {
    auto completion = std::make_shared<std::promise<bool>>();
    auto result = completion->get_future();
    // If thread creation fails, retain native owners for process termination. Destroying the
    // operation here could join a start/finalization future or delete recoverable staging.
    auto* retained = new Operation(std::move(operation));
    try {
        launch([retained, completion] {
            std::unique_ptr<Operation> owned(retained);
            try {
                (*owned)();
                completion->set_value(true);
            } catch (...) {
                completion->set_value(false);
            }
        });
    } catch (...) {
        return false;
    }
    return result.wait_for(std::chrono::milliseconds(std::clamp(timeoutMs, 0, 4000))) ==
               std::future_status::ready &&
           result.get();
}

template <typename Operation> bool runBoundedPreservation(Operation operation, int timeoutMs) {
    return runBoundedPreservation(std::move(operation), timeoutMs,
                                  [](auto worker) { std::thread(std::move(worker)).detach(); });
}
} // namespace snow_shot::presentation::recording
