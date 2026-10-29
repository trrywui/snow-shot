#include "snow_canvas_runtime_clients.h"

#include "snow_draw_engine_qt/snow_canvas_runtime.h"

#include <QScopeGuard>

#include <algorithm>
#include <utility>

namespace snow_canvas_runtime {
ClientRegistry::~ClientRegistry() {
    m_lifetime.reset();
    abandonClientsWithoutDetach();
}

void ClientRegistry::registerClient(Client* client) {
    if (client == nullptr ||
        std::any_of(m_clients.begin(), m_clients.end(),
                    [client](const auto& entry) { return entry->client == client; })) {
        return;
    }
    const auto entry = std::make_shared<Registration>(Registration{client});
    m_clients.push_back(entry);
    if (m_replacementDetachQueue != nullptr) {
        m_replacementDetachQueue->push_back(entry);
    }
}

void ClientRegistry::unregisterClient(Client* client) {
    for (auto it = m_clients.begin(); it != m_clients.end();) {
        if ((*it)->client == client) {
            (*it)->client = nullptr;
            it = m_clients.erase(it);
        } else {
            ++it;
        }
    }
}

ClientRegistry::DetachedClients
ClientRegistry::detachForRuntimeReplacement(const std::function<bool()>& isCurrent) {
    const std::weak_ptr<const int> lifetime = m_lifetime;
    DetachedClients pendingClients = m_clients;
    auto* previousQueue = std::exchange(m_replacementDetachQueue, &pendingClients);
    const auto restoreQueue = qScopeGuard([this, lifetime, previousQueue] {
        if (!lifetime.expired()) {
            m_replacementDetachQueue = previousQueue;
        }
    });
    DetachedClients detachedClients;
    detachedClients.reserve(m_clients.size());
    for (qsizetype index = 0; index < pendingClients.size(); ++index) {
        if (lifetime.expired() || !isCurrent()) {
            break;
        }
        // A callback can append registrations and reallocate the queue.
        const auto entry = pendingClients.at(index);
        if (entry->client == nullptr) {
            continue;
        }
        entry->client->detachRuntimeForReplacement();
        if (entry->client != nullptr) {
            detachedClients.push_back(entry);
        }
    }

    return detachedClients;
}

void ClientRegistry::attachAfterRuntimeReplacement(const DetachedClients& clients,
                                                   SnowRuntime runtime,
                                                   const std::function<bool()>& isCurrent) {
    for (const auto& entry : clients) {
        if (!isCurrent()) {
            break;
        }
        if (entry->client != nullptr) {
            entry->client->attachRuntime(runtime);
        }
    }
}

void ClientRegistry::detachAndReleaseClients(SnowCanvasRuntime& owner) {
    const std::weak_ptr<const int> lifetime = m_lifetime;
    // Retirement must reach registrations admitted by callbacks before releasing the engine.
    // Remove and cancel each registration first so nested drains cannot detach it twice.
    while (!m_clients.isEmpty()) {
        const auto entry = m_clients.takeFirst();
        Client* client = std::exchange(entry->client, nullptr);
        if (client != nullptr) {
            client->detachRuntimeOwner(&owner);
            if (lifetime.expired()) {
                return;
            }
        }
    }
}

void ClientRegistry::abandonClientsWithoutDetach() {
    for (const auto& entry : m_clients) {
        entry->client = nullptr;
    }
    m_clients.clear();
}

void ClientRegistry::clearRenderState() {
    const DetachedClients snapshot = m_clients;
    for (const auto& entry : snapshot) {
        if (entry->client != nullptr) {
            entry->client->clearRenderState();
        }
    }
}

void ClientRegistry::resetDocumentRetainedState() {
    const DetachedClients snapshot = m_clients;
    for (const auto& entry : snapshot) {
        if (entry->client != nullptr) {
            entry->client->resetDocumentRetainedState();
        }
    }
}

bool ClientRegistry::resetEditingState() {
    const DetachedClients snapshot = m_clients;
    for (const auto& entry : snapshot) {
        if (entry->client != nullptr && !entry->client->resetEditingState(false)) {
            return false;
        }
    }
    return true;
}

void ClientRegistry::syncChangedViewports(std::vector<std::uint64_t> changedViewportIds) {
    const DetachedClients snapshot = m_clients;
    for (const auto& entry : snapshot) {
        Client* client = entry->client;
        if (client == nullptr) {
            continue;
        }
        if (std::any_of(
                changedViewportIds.begin(), changedViewportIds.end(),
                [client](std::uint64_t id) { return client->observesRuntimeViewport(id); })) {
            client->syncAfterEngineMutation();
        } else {
            client->refreshStateFromEngine(true);
        }
    }
}

} // namespace snow_canvas_runtime

void snow_canvas_runtime::ClientRegistry::smartEraseChanged() {
    const auto clients = m_clients;
    for (const auto& entry : clients)
        if (entry->client)
            entry->client->smartEraseChanged();
}
