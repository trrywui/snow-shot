#ifndef SNOW_SHOT_PRESENTATION_COMPONENTS_STANDALONETRANSLATIONWINDOW_H
#define SNOW_SHOT_PRESENTATION_COMPONENTS_STANDALONETRANSLATIONWINDOW_H

#include <QObject>
#include <QPointer>
#include <QRect>

#include <optional>

class QScreen;
class QWidget;
class SnowShotApiClient;
class TranslationPageWidget;
namespace adqt::widgets {
class AdModal;
}

namespace snow_shot::presentation {
class StandaloneTranslationWindow final : public QObject {
    Q_OBJECT
  public:
    explicit StandaloneTranslationWindow(SnowShotApiClient* client, QObject* parent = nullptr);
    ~StandaloneTranslationWindow() override;
    void showTranslation(const QString& text, QScreen* screen);
    void close();

  protected:
    bool eventFilter(QObject* watched, QEvent* event) override;

  private:
    SnowShotApiClient* m_client;
    adqt::widgets::AdModal* m_modal;
    QPointer<TranslationPageWidget> m_page;
    std::optional<QRect> m_lastWindowGeometry;
};
} // namespace snow_shot::presentation
#endif
