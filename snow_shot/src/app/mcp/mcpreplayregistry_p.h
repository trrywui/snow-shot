#pragma once

#include "snow_shot/app/mcp/screenshotmcpserver.h"
#include <QCryptographicHash>
#include <QElapsedTimer>
#include <QHash>
#include <QJsonDocument>
#include <QPointer>
#include <QQueue>
#include <memory>
#include <utility>

namespace snow_shot::app::mcp {

// A replay belongs to an operation; each caller owns a separate subscription.
// Keep both identities until delivery, including while callbacks reenter the service.
class McpReplayRegistry final {
  public:
    struct Options {
        qsizetype maximumResponseBytes = 65536;
        bool retainFailures = true;
        qint64 lifetimeMs = 600000;
    };
    struct Operation {
        ScreenshotMcpServer::Completion complete;
        std::function<bool()> current;
        explicit operator bool() const {
            return static_cast<bool>(complete);
        }
    };

    explicit McpReplayRegistry(QObject& owner, Options options)
        : m_state(std::make_shared<State>(&owner, options)) {}

    Operation begin(const ScreenshotMcpRequest& request,
                    ScreenshotMcpServer::Completion completion) {
        const auto state = m_state;
        const auto key = requestKey(request.connectionId, request.requestId);
        if (state->stopped || state->active.size() >= 64 || state->active.contains(key)) {
            completion(failure(request, state->stopped ? QStringLiteral("unavailable")
                                                       : QStringLiteral("queue_full")));
            return {};
        }
        auto caller = std::make_shared<Caller>();
        caller->request = request;
        caller->completion = std::move(completion);
        state->active.insert(key, caller);
        const std::weak_ptr<State> weak = state;
        const std::weak_ptr<Caller> weakCaller = caller;
        const auto current = [weak, key, weakCaller] {
            const auto live = weak.lock();
            const auto active = weakCaller.lock();
            return live && live->owner && active && live->active.value(key) == active;
        };
        if (request.idempotencyKey.isEmpty())
            return {[weak, weakCaller](auto response) {
                        if (const auto active = weakCaller.lock())
                            deliver(weak, active, std::move(response));
                    },
                    current};

        prune(*state);
        const auto replayKey = requestKey(request.connectionId, request.idempotencyKey);
        const auto fingerprint = QCryptographicHash::hash(
            QJsonDocument(QJsonObject{{QStringLiteral("method"), request.method},
                                      {QStringLiteral("params"), request.params},
                                      {QStringLiteral("revision"),
                                       request.expectedRevision ? QJsonValue(static_cast<qint64>(
                                                                      *request.expectedRevision))
                                                                : QJsonValue{}}})
                .toJson(QJsonDocument::Compact),
            QCryptographicHash::Sha256);
        if (const auto found = state->replay.value(replayKey)) {
            if (found->fingerprint != fingerprint)
                deliver(weak, caller, failure(request, QStringLiteral("idempotency_conflict")));
            else if (found->response) {
                caller->alias = true;
                deliver(weak, caller, *found->response);
            } else if (found->waiters.size() < 8) {
                caller->alias = true;
                caller->replayKey = replayKey;
                found->waiters.append(caller);
            } else
                deliver(weak, caller, failure(request, QStringLiteral("queue_full")));
            return {};
        }
        if (state->replay.size() - state->completed.size() >= 32) {
            deliver(weak, caller, failure(request, QStringLiteral("queue_full")));
            return {};
        }
        auto entry = std::make_shared<Entry>();
        entry->fingerprint = fingerprint;
        state->replay.insert(replayKey, entry);
        const std::weak_ptr<Entry> weakEntry = entry;
        return {[weak, replayKey, weakEntry, weakCaller](auto response) {
                    const auto live = weak.lock();
                    const auto entry = weakEntry.lock();
                    const auto caller = weakCaller.lock();
                    if (!live || !live->owner || !entry || !caller ||
                        live->replay.value(replayKey) != entry || entry->response)
                        return;
                    auto waiters = std::move(entry->waiters);
                    const auto bytes =
                        response.attachment.size() +
                        QJsonDocument(response.result).toJson(QJsonDocument::Compact).size() +
                        QJsonDocument(response.errorDetails).toJson(QJsonDocument::Compact).size() +
                        response.errorMessage.toUtf8().size() + 1024;
                    if ((response.ok || live->options.retainFailures) &&
                        response.attachment.isEmpty() &&
                        bytes <= live->options.maximumResponseBytes) {
                        entry->response = response;
                        entry->response->afterSend = {};
                        entry->bytes = bytes;
                        entry->completedAt = live->clock.elapsed();
                        live->bytes += bytes;
                        live->completed.enqueue(replayKey);
                        prune(*live);
                    } else
                        live->replay.remove(replayKey);
                    deliver(weak, caller, response);
                    for (const auto& waiter : waiters)
                        deliver(weak, waiter, response);
                },
                current};
    }

    bool cancelAlias(quint64 owner, const QString& requestId) {
        const auto state = m_state;
        const auto caller = state->active.value(requestKey(owner, requestId));
        if (!caller || !caller->alias)
            return false;
        if (const auto entry = state->replay.value(caller->replayKey))
            entry->waiters.removeAll(caller);
        deliver(state, caller, failure(caller->request, QStringLiteral("canceled")));
        return true;
    }

    void disconnected(quint64 owner) {
        const auto live = m_state;
        auto& state = *live;
        QList<std::shared_ptr<Caller>> retired;
        const auto prefix = QString::number(owner) + u':';
        for (auto it = state.active.begin(); it != state.active.end();)
            if (it.key().startsWith(prefix)) {
                retired.append(*it);
                it = state.active.erase(it);
            } else
                ++it;
        for (auto it = state.replay.begin(); it != state.replay.end();)
            if (it.key().startsWith(prefix)) {
                state.bytes -= (*it)->bytes;
                it = state.replay.erase(it);
            } else
                ++it;
        state.completed.removeIf([&](const auto& key) { return key.startsWith(prefix); });
    }

    void shutdown() {
        const auto state = m_state;
        state->stopped = true;
        const auto retired = std::exchange(state->active, {});
        state->replay.clear();
        state->completed.clear();
        state->bytes = 0;
    }

  private:
    struct Caller {
        ScreenshotMcpRequest request;
        ScreenshotMcpServer::Completion completion;
        QString replayKey;
        bool alias = false;
    };
    struct Entry {
        QByteArray fingerprint;
        std::optional<ScreenshotMcpResponse> response;
        QList<std::shared_ptr<Caller>> waiters;
        qsizetype bytes = 0;
        qint64 completedAt = 0;
    };
    struct State {
        State(QObject* value, Options settings) : owner(value), options(settings) {
            clock.start();
        }
        QPointer<QObject> owner;
        Options options;
        bool stopped = false;
        QHash<QString, std::shared_ptr<Caller>> active;
        QHash<QString, std::shared_ptr<Entry>> replay;
        QQueue<QString> completed;
        qsizetype bytes = 0;
        QElapsedTimer clock;
    };
    static QString requestKey(quint64 owner, const QString& id) {
        return QString::number(owner) + u':' + id;
    }
    static ScreenshotMcpResponse failure(const ScreenshotMcpRequest& request, const QString& code) {
        ScreenshotMcpResponse response;
        response.requestId = request.requestId;
        response.errorCode = code;
        response.errorMessage = code;
        return response;
    }
    static void deliver(const std::weak_ptr<State>& weak, const std::shared_ptr<Caller>& caller,
                        ScreenshotMcpResponse response) {
        const auto state = weak.lock();
        const auto key = requestKey(caller->request.connectionId, caller->request.requestId);
        if (!state || !state->owner || state->active.value(key) != caller)
            return;
        state->active.remove(key);
        auto completion = std::move(caller->completion);
        response.requestId = caller->request.requestId;
        if (caller->alias)
            response.afterSend = {};
        completion(std::move(response));
    }
    static void prune(State& state) {
        while (!state.completed.isEmpty()) {
            const auto oldest = state.replay.value(state.completed.head());
            const bool expired =
                state.options.lifetimeMs > 0 &&
                state.clock.elapsed() - oldest->completedAt > state.options.lifetimeMs;
            if (!expired && state.completed.size() <= 128 && state.bytes <= 8 * 1024 * 1024)
                break;
            state.bytes -= state.replay.take(state.completed.dequeue())->bytes;
        }
    }
    std::shared_ptr<State> m_state;
};

} // namespace snow_shot::app::mcp
