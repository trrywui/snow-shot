#ifndef SNOW_SHOT_APP_STOREUPDATELIFECYCLE_H
#define SNOW_SHOT_APP_STOREUPDATELIFECYCLE_H

#include <QAbstractNativeEventFilter>
#include <functional>
#include <future>

namespace snow_shot::app {

// Coordinates Windows Restart Manager with the same work owners used by normal app shutdown.
class StoreUpdateLifecycle final : public QAbstractNativeEventFilter {
  public:
    struct Hooks {
        std::function<bool()> busy;
        std::function<void(bool)> suspend;
        std::function<bool()> checkpoint;
        std::function<void(int)> preserveRecording;
        std::function<void()> suspendForTermination;
        std::function<std::shared_future<void>()> terminalCheckpoint;
    };

    explicit StoreUpdateLifecycle(Hooks hooks);
    ~StoreUpdateLifecycle() override;
    void install();

    [[nodiscard]] bool queryEndSession();
    void endSession(bool ending);
    [[nodiscard]] bool pending() const;
    bool nativeEventFilter(const QByteArray& eventType, void* message, qintptr* result) override;

  private:
    Hooks m_hooks;
    bool m_installed = false;
    bool m_pending = false;
    bool m_ended = false;
};

} // namespace snow_shot::app

#endif // SNOW_SHOT_APP_STOREUPDATELIFECYCLE_H
