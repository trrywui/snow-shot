#ifndef SNOW_SHOT_PRESENTATION_COMPONENTS_HISTORYENTRYCARD_H
#define SNOW_SHOT_PRESENTATION_COMPONENTS_HISTORYENTRYCARD_H

#include "snow_shot/presentation/styles/themecolorscheme.h"

#include <QFrame>

class QBoxLayout;
class QHBoxLayout;
class QLabel;

namespace adqt::widgets {
class AdButton;
class AdCheckbox;
} // namespace adqt::widgets

namespace snow_shot::presentation::components {

// The common entry surface for Screenshot History and Pin to Screen Management.
class HistoryEntryCard : public QFrame {
  public:
    static constexpr int kPreviewWidth = 260;
    static constexpr int kPreviewHeight = 156;

    explicit HistoryEntryCard(QWidget* parent = nullptr);

    [[nodiscard]] adqt::widgets::AdCheckbox* selectionCheckbox() const;
    [[nodiscard]] QLabel* sourceLabel() const;
    [[nodiscard]] QLabel* summaryLabel() const;
    [[nodiscard]] QLabel* metadataLabel() const;
    [[nodiscard]] QWidget* detailsWidget() const;
    [[nodiscard]] static int minimumHeightForWidth(int availableWidth);

    void addAction(adqt::widgets::AdButton* button);
    void setPreviewWidget(QWidget* preview);
    void applyTheme(const styles::ThemeColorScheme& scheme);
    void updateBackgroundStyle();

  protected:
    bool eventFilter(QObject* watched, QEvent* event) override;
    void resizeEvent(QResizeEvent* event) override;
    void paintEvent(QPaintEvent* event) override;

  private:
    void updateResponsiveLayout();

    QBoxLayout* m_layout = nullptr;
    QWidget* m_details = nullptr;
    adqt::widgets::AdCheckbox* m_selectionCheckbox = nullptr;
    QLabel* m_sourceLabel = nullptr;
    QLabel* m_summaryLabel = nullptr;
    QLabel* m_metadataLabel = nullptr;
    QHBoxLayout* m_actions = nullptr;
    QWidget* m_preview = nullptr;
    styles::ThemeColorScheme m_scheme;
};

} // namespace snow_shot::presentation::components

#endif // SNOW_SHOT_PRESENTATION_COMPONENTS_HISTORYENTRYCARD_H
