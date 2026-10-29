#pragma once

#include <QColor>
#include <QFont>
#include <QIcon>
#include <QMargins>
#include <QWidget>

#include <cstddef>
#include <memory>
#include <optional>

namespace adqt::widgets {

// A passive operation result. Custom widgets retain their own interaction and
// accessibility; the Result itself never takes focus or announces on show.
class AdResult final : public QWidget {
  Q_OBJECT
  Q_PROPERTY(Status status READ status WRITE setStatus NOTIFY statusChanged)
  Q_PROPERTY(QString title READ title WRITE setTitle NOTIFY titleChanged)
  Q_PROPERTY(QString subTitle READ subTitle WRITE setSubTitle NOTIFY subTitleChanged)
  Q_PROPERTY(QIcon icon READ icon WRITE setIcon NOTIFY iconChanged)
  Q_PROPERTY(bool iconVisible READ iconVisible WRITE setIconVisible NOTIFY iconVisibleChanged)
  Q_PROPERTY(QWidget* iconWidget READ iconWidget WRITE setIconWidget NOTIFY iconWidgetChanged)
  Q_PROPERTY(QWidget* titleWidget READ titleWidget WRITE setTitleWidget NOTIFY titleWidgetChanged)
  Q_PROPERTY(QWidget* subTitleWidget READ subTitleWidget WRITE setSubTitleWidget NOTIFY
                 subTitleWidgetChanged)
  Q_PROPERTY(QWidget* extraWidget READ extraWidget WRITE setExtraWidget NOTIFY extraWidgetChanged)
  Q_PROPERTY(
      QWidget* contentWidget READ contentWidget WRITE setContentWidget NOTIFY contentWidgetChanged)

 public:
  enum class Status { Success, Error, Info, Warning, Forbidden, NotFound, ServerError };
  Q_ENUM(Status)

  struct ComponentTokens {
    std::optional<qreal> titleFontSize;
    std::optional<qreal> subtitleFontSize;
    std::optional<qreal> iconFontSize;
    std::optional<QMargins> extraMargin;
    bool operator==(const ComponentTokens&) const = default;
  };
  struct SemanticSlotStyle {
    std::optional<QColor> backgroundColor;
    std::optional<QColor> textColor;
    std::optional<QFont> font;
    bool operator==(const SemanticSlotStyle&) const = default;
  };
  struct SemanticStyles {
    SemanticSlotStyle root;
    SemanticSlotStyle icon;
    SemanticSlotStyle title;
    SemanticSlotStyle subTitle;
    SemanticSlotStyle extra;
    SemanticSlotStyle body;
    bool operator==(const SemanticStyles&) const = default;
  };

  explicit AdResult(QWidget* parent = nullptr);
  ~AdResult() override;

  Status status() const;
  void setStatus(Status value);
  // Plain text, with word wrapping and explicit line breaks. Use the widget
  // slots for rich text, links, or interactive headings.
  QString title() const;
  void setTitle(const QString& value);
  QString subTitle() const;
  void setSubTitle(const QString& value);
  QIcon icon() const;
  // A null icon restores the status artwork. A custom icon also overrides
  // exception artwork; iconVisible hides the entire icon area, including its gap.
  void setIcon(const QIcon& value);
  bool iconVisible() const;
  void setIconVisible(bool value);

  // Non-null widgets transfer ownership to the Result. Replaced widgets are
  // deleted later. A widget can occupy only one slot; moving it clears the old
  // slot. The take functions hide and detach widgets, returning ownership.
  // Null title/subtitle widgets restore the corresponding plain text.
  // External destruction/reparenting clears the slot immediately; its change
  // notification is queued until Qt finishes the native lifecycle operation.
  QWidget* iconWidget() const;
  void setIconWidget(QWidget* widget);
  QWidget* takeIconWidget();
  QWidget* titleWidget() const;
  void setTitleWidget(QWidget* widget);
  QWidget* takeTitleWidget();
  QWidget* subTitleWidget() const;
  void setSubTitleWidget(QWidget* widget);
  QWidget* takeSubTitleWidget();
  // Supply an action row (for example a QHBoxLayout with 8 px spacing).
  QWidget* extraWidget() const;
  void setExtraWidget(QWidget* widget);
  QWidget* takeExtraWidget();
  QWidget* contentWidget() const;
  void setContentWidget(QWidget* widget);
  QWidget* takeContentWidget();

  ComponentTokens componentTokens() const;
  void setComponentTokens(const ComponentTokens& value);
  void resetComponentTokens();
  SemanticStyles semanticStyles() const;
  void setSemanticStyles(const SemanticStyles& value);
  void resetSemanticStyles();

  QSize sizeHint() const override;
  QSize minimumSizeHint() const override;
  bool hasHeightForWidth() const override;
  int heightForWidth(int width) const override;

 signals:
  void statusChanged(Status value);
  void titleChanged(const QString& value);
  void subTitleChanged(const QString& value);
  void iconChanged(const QIcon& value);
  void iconVisibleChanged(bool value);
  void iconWidgetChanged(QWidget* value);
  void titleWidgetChanged(QWidget* value);
  void subTitleWidgetChanged(QWidget* value);
  void extraWidgetChanged(QWidget* value);
  void contentWidgetChanged(QWidget* value);
  void componentTokensChanged();
  void semanticStylesChanged();

 protected:
  bool event(QEvent* event) override;
  bool eventFilter(QObject* watched, QEvent* event) override;
  void changeEvent(QEvent* event) override;
  void paintEvent(QPaintEvent* event) override;

 private:
  struct Private;
  std::unique_ptr<Private> d_;
  void refreshAppearance();
  void refreshContent();
  void setSlot(std::size_t index, QWidget* widget);
  QWidget* releaseSlot(std::size_t index);
  QWidget* takeSlot(std::size_t index);
  void notifySlot(std::size_t index, QWidget* widget);
  void queueSlotCleared(std::size_t index, quint64 revision);
};

}  // namespace adqt::widgets
