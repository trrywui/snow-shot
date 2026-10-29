#pragma once

#include <QObject>
#include <functional>
#include <memory>

class QScreen;
class QWidget;
class SnowCanvasWidget;
class ScreenshotFloatingToolPaletteWindow;
class RecordingEffectsSource;

namespace snow_shot::presentation {
class GlobalCanvasController final : public QObject {
    Q_OBJECT
  public:
    struct Platform {
        std::function<QScreen*()> pointerScreen;
        std::function<bool(QWidget*, bool)> setInputTransparent;
        std::function<std::unique_ptr<RecordingEffectsSource>()> createEffectsSource = {};
    };
    explicit GlobalCanvasController(QObject* parent = nullptr);
    GlobalCanvasController(QObject* parent, Platform platform);
    ~GlobalCanvasController() override;
    void activate();
    void shutdown();
    [[nodiscard]] bool active() const;
    [[nodiscard]] bool clickThrough() const;
    [[nodiscard]] QWidget* window() const;
    [[nodiscard]] SnowCanvasWidget* canvas() const;
    [[nodiscard]] ScreenshotFloatingToolPaletteWindow* toolbar() const;

  signals:
    void activeChanged(bool active);
    void errorOccurred(const QString& message);

  private:
    class Session;
    Platform m_platform;
    std::unique_ptr<Session> m_session;
};
} // namespace snow_shot::presentation
