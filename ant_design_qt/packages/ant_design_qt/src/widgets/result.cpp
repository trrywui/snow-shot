#include "result.h"

#include "antd_icons.h"
#include "result_illustration_p.h"
#include "theme/theme.h"

#include <QAccessible>
#include <QAccessibleWidget>
#include <QEvent>
#include <QFontInfo>
#include <QHBoxLayout>
#include <QLabel>
#include <QPainter>
#include <QPointer>
#include <QTextLayout>
#include <QVBoxLayout>

#include <algorithm>
#include <array>
#include <cmath>

namespace adqt::widgets {
namespace {

enum Slot { Icon, Title, SubTitle, Extra, Body, SlotCount };

// Keep the native QLabel accessible interface while laying out plain text with
// the design system's line height. The cached QTextLayout also handles bidi,
// explicit newlines and unbroken URLs without imposing a minimum width.
class ResultLabel final : public QLabel {
 public:
  explicit ResultLabel(QWidget* parent) : QLabel(parent) {
    setTextFormat(Qt::PlainText);
    setWordWrap(true);
    setAlignment(Qt::AlignHCenter | Qt::AlignTop);
    QSizePolicy policy(QSizePolicy::Expanding, QSizePolicy::Preferred);
    policy.setHeightForWidth(true);
    setSizePolicy(policy);
  }
  void configure(const QString& text, const QFont& font, qreal lineHeight) {
    if (text == QLabel::text() && font == this->font() && lineHeight == lineHeight_) return;
    setText(text);
    setFont(font);
    lineHeight_ = lineHeight;
    cachedWidth_ = -1;
    updateGeometry();
    update();
  }
  void setContent(const QString& content) {
    if (content == text()) return;
    setText(content);
    cachedWidth_ = -1;
    updateGeometry();
    update();
  }
  QSize sizeHint() const override {
    int width = 1;
    for (const QString& line : text().split(QLatin1Char('\n')))
      width = std::max(width, fontMetrics().horizontalAdvance(line));
    width = std::min(width, 640);
    return QSize(width, heightForWidth(width));
  }
  QSize minimumSizeHint() const override { return QSize(0, qCeil(lineHeight_)); }
  bool hasHeightForWidth() const override { return true; }
  int heightForWidth(int width) const override {
    ensureLayout(width);
    return cachedHeight_;
  }

 protected:
  void paintEvent(QPaintEvent*) override {
    ensureLayout(width());
    QPainter painter(this);
    painter.setPen(
        palette().color(isEnabled() ? QPalette::Active : QPalette::Disabled, QPalette::WindowText));
    textLayout_.draw(&painter, QPointF());
  }
  void changeEvent(QEvent* event) override {
    QLabel::changeEvent(event);
    if (event->type() == QEvent::FontChange || event->type() == QEvent::LayoutDirectionChange) {
      cachedWidth_ = -1;
      updateGeometry();
      update();
    }
  }

 private:
  void ensureLayout(int width) const {
    width = std::max(1, width);
    if (cachedWidth_ == width) return;
    cachedWidth_ = width;
    QString content = text();
    content.replace(QStringLiteral("\r\n"), QStringLiteral("\n"));
    content.replace(QLatin1Char('\n'), QChar::LineSeparator);
    textLayout_.setText(content);
    textLayout_.setFont(font());
    QTextOption option(Qt::AlignHCenter);
    option.setWrapMode(QTextOption::WrapAtWordBoundaryOrAnywhere);
    option.setTextDirection(layoutDirection());
    textLayout_.setTextOption(option);
    textLayout_.setCacheEnabled(true);
    qreal y = 0;
    textLayout_.beginLayout();
    while (true) {
      QTextLine line = textLayout_.createLine();
      if (!line.isValid()) break;
      line.setLineWidth(width);
      const qreal height = std::max(lineHeight_, line.height());
      line.setPosition(QPointF(0, y + (height - line.height()) / 2));
      y += height;
    }
    textLayout_.endLayout();
    cachedHeight_ = qCeil(y);
  }
  qreal lineHeight_ = 22;
  mutable QTextLayout textLayout_;
  mutable int cachedWidth_ = -1;
  mutable int cachedHeight_ = 0;
};

class ResultSurface : public QWidget {
 public:
  using QWidget::QWidget;
  QColor background;
  int backgroundTop = 0;

 protected:
  void paintEvent(QPaintEvent*) override {
    if (background.isValid() && background.alpha() > 0) {
      QPainter painter(this);
      painter.fillRect(rect().adjusted(0, backgroundTop, 0, 0), background);
    }
  }
};

class ResultIcon final : public QWidget {
 public:
  explicit ResultIcon(QWidget* parent) : QWidget(parent) {
    QSizePolicy policy(QSizePolicy::Preferred, QSizePolicy::Preferred);
    policy.setHeightForWidth(true);
    setSizePolicy(policy);
    setMaximumSize(72, 72);
  }
  AdResult::Status status = AdResult::Status::Info;
  QIcon custom;
  QColor color;
  QColor primary;
  QSize naturalSize = QSize(72, 72);
  QSize sizeHint() const override { return naturalSize; }
  QSize minimumSizeHint() const override { return QSize(0, 0); }
  bool hasHeightForWidth() const override { return true; }
  int heightForWidth(int width) const override {
    return qRound(std::min(width, naturalSize.width()) * static_cast<qreal>(naturalSize.height()) /
                  naturalSize.width());
  }

 protected:
  void paintEvent(QPaintEvent*) override {
    QPainter painter(this);
    painter.setRenderHint(QPainter::Antialiasing);
    const QSizeF fitted = QSizeF(naturalSize).scaled(size(), Qt::KeepAspectRatio);
    const QRectF imageRect(
        QPointF((width() - fitted.width()) / 2, (height() - fitted.height()) / 2), fitted);
    if (!custom.isNull()) {
      custom.paint(&painter, imageRect.toAlignedRect(), Qt::AlignCenter,
                   isEnabled() ? QIcon::Normal : QIcon::Disabled);
      return;
    }
    int code = 0;
    switch (status) {
      case AdResult::Status::Forbidden:
        code = 403;
        break;
      case AdResult::Status::NotFound:
        code = 404;
        break;
      case AdResult::Status::ServerError:
        code = 500;
        break;
      default:
        break;
    }
    if (code) {
      detail::paintResultIllustration(&painter, imageRect, code, primary);
      return;
    }
    const auto colors = adqt::icons::IconColors::primary(color);
    adqt::icons::IconRef ref;
    switch (status) {
      case AdResult::Status::Success:
        ref = adqt::icons::antd::filled::CheckCircle(colors);
        break;
      case AdResult::Status::Error:
        ref = adqt::icons::antd::filled::CloseCircle(colors);
        break;
      case AdResult::Status::Warning:
        ref = adqt::icons::antd::filled::Warning(colors);
        break;
      default:
        ref = adqt::icons::antd::filled::ExclamationCircle(colors);
        break;
    }
    adqt::icons::paintIcon(&painter, ref, imageRect);
  }
};

QString widgetText(QWidget* widget) {
  if (!widget) return {};
  if (!widget->accessibleName().isEmpty()) return widget->accessibleName();
  if (QAccessibleInterface* interface = QAccessible::queryAccessibleInterface(widget))
    return interface->text(QAccessible::Name);
  return {};
}

class AccessibleResult final : public QAccessibleWidget {
 public:
  explicit AccessibleResult(AdResult* widget) : QAccessibleWidget(widget, QAccessible::Grouping) {}
  QString text(QAccessible::Text type) const override {
    auto* result = static_cast<AdResult*>(widget());
    if (type == QAccessible::Name && result->accessibleName().isEmpty())
      return result->titleWidget() ? widgetText(result->titleWidget()) : result->title();
    if (type == QAccessible::Description && result->accessibleDescription().isEmpty())
      return result->subTitleWidget() ? widgetText(result->subTitleWidget()) : result->subTitle();
    return QAccessibleWidget::text(type);
  }
};

QAccessibleInterface* resultAccessibleFactory(const QString&, QObject* object) {
  if (auto* result = qobject_cast<AdResult*>(object)) return new AccessibleResult(result);
  return nullptr;
}

qreal positive(const std::optional<qreal>& value, qreal fallback) {
  return value && std::isfinite(*value) && *value > 0 ? std::clamp(*value, 1.0, 4096.0) : fallback;
}

QFont mergedFont(const QFont& overrideFont, const QFont& base) {
  QFont merged = overrideFont.resolve(base);
  // QFont::resolve retains only the override's mask. Keep token attributes
  // explicit when QWidget subsequently resolves against its parent font.
  merged.setResolveMask(overrideFont.resolveMask() | base.resolveMask());
  return merged;
}

void accessibleChanged(QWidget* widget, QAccessible::Event type) {
  QAccessibleEvent event(widget, type);
  QAccessible::updateAccessibility(&event);
}

}  // namespace

struct AdResult::Private {
  struct Hosted {
    ResultSurface* host = nullptr;
    QBoxLayout* layout = nullptr;
    QPointer<QWidget> custom;
    QMetaObject::Connection destroyed;
    quint64 revision = 0;
  };
  Status status = Status::Info;
  QString title;
  QString subTitle;
  QIcon icon;
  bool iconVisible = true;
  bool refreshing = false;
  ComponentTokens tokens;
  SemanticStyles semantic;
  QColor background;
  int iconSize = 72;
  QVBoxLayout* layout = nullptr;
  std::array<Hosted, SlotCount> hosted;
  ResultLabel* titleLabel = nullptr;
  ResultLabel* subTitleLabel = nullptr;
  ResultIcon* artwork = nullptr;
};

AdResult::AdResult(QWidget* parent) : QWidget(parent), d_(std::make_unique<Private>()) {
  static const bool installed = [] {
    QAccessible::installFactory(resultAccessibleFactory);
    return true;
  }();
  Q_UNUSED(installed)
  setFocusPolicy(Qt::NoFocus);
  QSizePolicy policy(QSizePolicy::Expanding, QSizePolicy::Preferred);
  policy.setHeightForWidth(true);
  setSizePolicy(policy);
  d_->layout = new QVBoxLayout(this);
  d_->layout->setSpacing(0);
  const std::array<QString, SlotCount> names = {QStringLiteral("icon"), QStringLiteral("title"),
                                                QStringLiteral("subtitle"), QStringLiteral("extra"),
                                                QStringLiteral("body")};
  for (std::size_t index = 0; index < SlotCount; ++index) {
    auto& slot = d_->hosted[index];
    slot.host = new ResultSurface(this);
    slot.host->setObjectName(QStringLiteral("adqt-result-%1-host").arg(names[index]));
    if (index == Icon || index == Extra) {
      slot.layout = new QHBoxLayout(slot.host);
      slot.layout->setAlignment(Qt::AlignHCenter);
    } else {
      slot.layout = new QVBoxLayout(slot.host);
    }
    slot.layout->setSpacing(0);
    slot.layout->setContentsMargins(0, 0, 0, 0);
    d_->layout->addWidget(slot.host);
  }
  // A trailing stretch keeps the result top-aligned without QLayout's aligned
  // rectangle clamping wrapped height to the unwrapped size hint.
  d_->layout->addStretch(1);
  d_->hosted[Body].host->setObjectName(QStringLiteral("adqt-result-body"));
  d_->titleLabel = new ResultLabel(d_->hosted[Title].host);
  d_->titleLabel->setObjectName(QStringLiteral("adqt-result-title"));
  d_->hosted[Title].layout->addWidget(d_->titleLabel);
  d_->subTitleLabel = new ResultLabel(d_->hosted[SubTitle].host);
  d_->subTitleLabel->setObjectName(QStringLiteral("adqt-result-subtitle"));
  d_->hosted[SubTitle].layout->addWidget(d_->subTitleLabel);
  d_->artwork = new ResultIcon(d_->hosted[Icon].host);
  d_->artwork->setObjectName(QStringLiteral("adqt-result-icon"));
  d_->artwork->setAttribute(Qt::WA_TransparentForMouseEvents);
  d_->hosted[Icon].layout->addWidget(d_->artwork);
  connect(&theme::ThemeManager::instance(), &theme::ThemeManager::themeChanged, this,
          &AdResult::refreshAppearance);
  refreshAppearance();
}

AdResult::~AdResult() {
  disconnect(&theme::ThemeManager::instance(), nullptr, this, nullptr);
  // QWidget destroys children after the private state, so disconnect callbacks
  // before the owned custom widgets begin destruction.
  for (auto& slot : d_->hosted) {
    disconnect(slot.destroyed);
    if (slot.custom) slot.custom->removeEventFilter(this);
  }
}

AdResult::Status AdResult::status() const { return d_->status; }
void AdResult::setStatus(Status value) {
  if (value < Status::Success || value > Status::ServerError || d_->status == value) return;
  d_->status = value;
  refreshAppearance();
  emit statusChanged(value);
}
QString AdResult::title() const { return d_->title; }
void AdResult::setTitle(const QString& value) {
  if (d_->title == value) return;
  d_->title = value;
  d_->titleLabel->setContent(value);
  refreshContent();
  accessibleChanged(this, QAccessible::NameChanged);
  emit titleChanged(value);
}
QString AdResult::subTitle() const { return d_->subTitle; }
void AdResult::setSubTitle(const QString& value) {
  if (d_->subTitle == value) return;
  d_->subTitle = value;
  d_->subTitleLabel->setContent(value);
  refreshContent();
  accessibleChanged(this, QAccessible::DescriptionChanged);
  emit subTitleChanged(value);
}
QIcon AdResult::icon() const { return d_->icon; }
void AdResult::setIcon(const QIcon& value) {
  if (d_->icon.cacheKey() == value.cacheKey()) return;
  d_->icon = value;
  refreshContent();
  emit iconChanged(value);
}
bool AdResult::iconVisible() const { return d_->iconVisible; }
void AdResult::setIconVisible(bool value) {
  if (d_->iconVisible == value) return;
  d_->iconVisible = value;
  refreshContent();
  emit iconVisibleChanged(value);
}

QWidget* AdResult::iconWidget() const { return d_->hosted[Icon].custom; }
void AdResult::setIconWidget(QWidget* widget) { setSlot(Icon, widget); }
QWidget* AdResult::takeIconWidget() { return takeSlot(Icon); }
QWidget* AdResult::titleWidget() const { return d_->hosted[Title].custom; }
void AdResult::setTitleWidget(QWidget* widget) { setSlot(Title, widget); }
QWidget* AdResult::takeTitleWidget() { return takeSlot(Title); }
QWidget* AdResult::subTitleWidget() const { return d_->hosted[SubTitle].custom; }
void AdResult::setSubTitleWidget(QWidget* widget) { setSlot(SubTitle, widget); }
QWidget* AdResult::takeSubTitleWidget() { return takeSlot(SubTitle); }
QWidget* AdResult::extraWidget() const { return d_->hosted[Extra].custom; }
void AdResult::setExtraWidget(QWidget* widget) { setSlot(Extra, widget); }
QWidget* AdResult::takeExtraWidget() { return takeSlot(Extra); }
QWidget* AdResult::contentWidget() const { return d_->hosted[Body].custom; }
void AdResult::setContentWidget(QWidget* widget) { setSlot(Body, widget); }
QWidget* AdResult::takeContentWidget() { return takeSlot(Body); }

void AdResult::notifySlot(std::size_t index, QWidget* widget) {
  switch (index) {
    case Icon:
      emit iconWidgetChanged(widget);
      break;
    case Title:
      accessibleChanged(this, QAccessible::NameChanged);
      emit titleWidgetChanged(widget);
      break;
    case SubTitle:
      accessibleChanged(this, QAccessible::DescriptionChanged);
      emit subTitleWidgetChanged(widget);
      break;
    case Extra:
      emit extraWidgetChanged(widget);
      break;
    case Body:
      emit contentWidgetChanged(widget);
      break;
    default:
      break;
  }
}

void AdResult::queueSlotCleared(std::size_t index, quint64 revision) {
  // Native destruction and reparenting still use the old widget hierarchy
  // after sending their event/signal. User callbacks may delete the Result,
  // so dispatch only after that operation has completed, and discard changes
  // superseded by a later adoption, take, or destruction.
  QMetaObject::invokeMethod(
      this,
      [this, index, revision] {
        if (d_->hosted[index].revision == revision && !d_->hosted[index].custom)
          notifySlot(index, nullptr);
      },
      Qt::QueuedConnection);
}

void AdResult::setSlot(std::size_t index, QWidget* widget) {
  auto& slot = d_->hosted[index];
  if (slot.custom == widget) return;
  std::size_t movedFrom = SlotCount;
  if (widget) {
    // Reject cycles and our own implementation widgets. Descendants of a
    // custom slot can be promoted after detaching, before deleting its parent.
    if (widget == this || widget->isAncestorOf(this)) return;
    for (const auto& existing : d_->hosted)
      if (widget == existing.host) return;
    if (widget == d_->artwork || widget == d_->titleLabel || widget == d_->subTitleLabel) return;
    for (std::size_t other = 0; other < SlotCount; ++other)
      if (d_->hosted[other].custom == widget) {
        releaseSlot(other);
        movedFrom = other;
      }
    widget->hide();
    widget->setParent(slot.host);
  }
  if (QWidget* previous = releaseSlot(index)) {
    previous->setParent(this);
    previous->deleteLater();
  }
  slot.custom = widget;
  ++slot.revision;
  if (widget) {
    slot.layout->addWidget(widget);
    widget->installEventFilter(this);
    slot.destroyed = connect(widget, &QObject::destroyed, this, [this, index] {
      d_->hosted[index].custom.clear();
      d_->hosted[index].destroyed = {};
      const quint64 revision = ++d_->hosted[index].revision;
      refreshContent();
      queueSlotCleared(index, revision);
    });
  }
  refreshContent();
  if (widget) widget->show();
  if (movedFrom < SlotCount) {
    QPointer<AdResult> guard(this);
    notifySlot(movedFrom, nullptr);
    if (!guard) return;
    if (d_->hosted[index].custom != widget) return;
  }
  notifySlot(index, widget);
}

QWidget* AdResult::releaseSlot(std::size_t index) {
  auto& slot = d_->hosted[index];
  QWidget* widget = slot.custom;
  if (!widget) return nullptr;
  disconnect(slot.destroyed);
  slot.destroyed = {};
  slot.custom.clear();
  ++slot.revision;
  widget->removeEventFilter(this);
  slot.layout->removeWidget(widget);
  widget->hide();
  widget->setParent(nullptr);
  return widget;
}

QWidget* AdResult::takeSlot(std::size_t index) {
  QWidget* widget = releaseSlot(index);
  if (!widget) return nullptr;
  refreshContent();
  notifySlot(index, nullptr);
  return widget;
}

AdResult::ComponentTokens AdResult::componentTokens() const { return d_->tokens; }
void AdResult::setComponentTokens(const ComponentTokens& value) {
  ComponentTokens normalized = value;
  for (auto* size :
       {&normalized.titleFontSize, &normalized.subtitleFontSize, &normalized.iconFontSize})
    if (*size) {
      if (!std::isfinite(**size) || **size <= 0)
        size->reset();
      else
        **size = std::clamp(**size, 1.0, 4096.0);
    }
  if (normalized.extraMargin) {
    const QMargins m = *normalized.extraMargin;
    normalized.extraMargin =
        QMargins(std::clamp(m.left(), 0, 4096), std::clamp(m.top(), 0, 4096),
                 std::clamp(m.right(), 0, 4096), std::clamp(m.bottom(), 0, 4096));
  }
  if (d_->tokens == normalized) return;
  d_->tokens = normalized;
  refreshAppearance();
  emit componentTokensChanged();
}
void AdResult::resetComponentTokens() { setComponentTokens({}); }
AdResult::SemanticStyles AdResult::semanticStyles() const { return d_->semantic; }
void AdResult::setSemanticStyles(const SemanticStyles& value) {
  if (d_->semantic == value) return;
  d_->semantic = value;
  refreshAppearance();
  emit semanticStylesChanged();
}
void AdResult::resetSemanticStyles() { setSemanticStyles({}); }

void AdResult::refreshAppearance() {
  if (!d_ || d_->refreshing) return;
  d_->refreshing = true;
  const auto resolved = theme::ThemeManager::instance().resolve(this);
  const auto& v = resolved.values;
  const auto& semantic = d_->semantic;
  const bool paletteOverride = testAttribute(Qt::WA_SetPalette);
  const auto group = isEnabled() ? QPalette::Active : QPalette::Disabled;
  QColor titleColor = paletteOverride ? palette().color(group, QPalette::WindowText) : v.colorText;
  QColor subtitleColor =
      paletteOverride ? palette().color(group, QPalette::Text) : v.colorTextSecondary;
  if (semantic.root.textColor) titleColor = subtitleColor = *semantic.root.textColor;
  QFont base = semantic.root.font ? mergedFont(*semantic.root.font, font()) : font();
  QFont titleFont = base;
  QFont subtitleFont = base;
  const bool explicitFont =
      (testAttribute(Qt::WA_SetFont) && (font().resolveMask() & QFont::SizeResolved)) ||
      (semantic.root.font && (semantic.root.font->resolveMask() & QFont::SizeResolved));
  if (d_->tokens.titleFontSize || !explicitFont)
    titleFont.setPixelSize(qRound(positive(d_->tokens.titleFontSize, v.fontSizeHeading3)));
  if (d_->tokens.subtitleFontSize || !explicitFont)
    subtitleFont.setPixelSize(qRound(positive(d_->tokens.subtitleFontSize, v.fontSize)));
  if (semantic.title.font) titleFont = mergedFont(*semantic.title.font, titleFont);
  if (semantic.subTitle.font) subtitleFont = mergedFont(*semantic.subTitle.font, subtitleFont);
  const qreal titleSize = QFontInfo(titleFont).pixelSize();
  const qreal subtitleSize = QFontInfo(subtitleFont).pixelSize();
  d_->titleLabel->configure(d_->title, titleFont, titleSize * v.lineHeightHeading3);
  d_->subTitleLabel->configure(d_->subTitle, subtitleFont, subtitleSize * v.lineHeight);
  const std::array<SemanticSlotStyle, SlotCount> styles = {
      semantic.icon, semantic.title, semantic.subTitle, semantic.extra, semantic.body};
  for (std::size_t index = 0; index < SlotCount; ++index) {
    auto& slot = d_->hosted[index];
    slot.host->background = styles[index].backgroundColor.value_or(QColor());
    if (index == Body && !styles[index].backgroundColor) slot.host->background = v.colorFillAlter;
    QColor color = index == SubTitle ? subtitleColor : titleColor;
    color = styles[index].textColor.value_or(color);
    QPalette hostPalette = palette();
    hostPalette.setColor(QPalette::WindowText, color);
    hostPalette.setColor(QPalette::Text, color);
    slot.host->setPalette(hostPalette);
    slot.host->setFont(styles[index].font ? mergedFont(*styles[index].font, base) : base);
    slot.host->update();
  }
  // Apply label fonts after host inheritance has settled.
  d_->titleLabel->setFont(titleFont);
  d_->subTitleLabel->setFont(subtitleFont);
  d_->background = semantic.root.backgroundColor.value_or(QColor());
  d_->layout->setContentsMargins(qRound(v.sizeXL), qRound(v.sizeLG * 2), qRound(v.sizeXL),
                                 qRound(v.sizeLG * 2));
  d_->hosted[Icon].layout->setContentsMargins(0, 0, 0, qRound(v.sizeLG));
  d_->hosted[Title].layout->setContentsMargins(0, qRound(v.sizeXS), 0, qRound(v.sizeXS));
  d_->hosted[Extra].layout->setContentsMargins(
      d_->tokens.extraMargin.value_or(QMargins(0, qRound(v.sizeLG), 0, 0)));
  // Body padding and its separation from the action row belong to distinct
  // layout levels so its background never paints into the preceding gap.
  d_->hosted[Body].host->backgroundTop = qRound(v.sizeLG);
  d_->hosted[Body].layout->setContentsMargins(qRound(v.size * 2.5), qRound(v.sizeLG * 2),
                                              qRound(v.size * 2.5), qRound(v.sizeLG));
  d_->layout->setStretch(Body, 0);
  d_->artwork->status = d_->status;
  d_->artwork->primary = v.colorPrimary;
  QColor iconColor = v.colorInfo;
  if (d_->status == Status::Success)
    iconColor = v.colorSuccess;
  else if (d_->status == Status::Error)
    iconColor = v.colorError;
  else if (d_->status == Status::Warning)
    iconColor = v.colorWarning;
  d_->artwork->color = semantic.icon.textColor.value_or(iconColor);
  d_->iconSize = qRound(positive(d_->tokens.iconFontSize, v.fontSizeHeading3 * 3));
  d_->refreshing = false;
  refreshContent();
  d_->artwork->update();
  update();
}

void AdResult::refreshContent() {
  if (!d_) return;
  const bool artworkChanged =
      d_->artwork->custom.cacheKey() != d_->icon.cacheKey() || d_->artwork->status != d_->status;
  d_->artwork->custom = d_->icon;
  d_->artwork->status = d_->status;
  const bool exception =
      d_->icon.isNull() && (d_->status == Status::Forbidden || d_->status == Status::NotFound ||
                            d_->status == Status::ServerError);
  const QSize naturalSize = exception ? QSize(250, 295) : QSize(d_->iconSize, d_->iconSize);
  if (d_->artwork->naturalSize != naturalSize) {
    d_->artwork->naturalSize = naturalSize;
    d_->artwork->setMaximumSize(naturalSize);
    d_->artwork->updateGeometry();
  }
  d_->artwork->setVisible(d_->iconVisible && !d_->hosted[Icon].custom);
  d_->titleLabel->setVisible(!d_->hosted[Title].custom && !d_->title.isEmpty());
  d_->subTitleLabel->setVisible(!d_->hosted[SubTitle].custom && !d_->subTitle.isEmpty());
  d_->hosted[Icon].host->setVisible(d_->iconVisible);
  d_->hosted[Title].host->setVisible(d_->hosted[Title].custom || !d_->title.isEmpty());
  d_->hosted[SubTitle].host->setVisible(d_->hosted[SubTitle].custom || !d_->subTitle.isEmpty());
  d_->hosted[Extra].host->setVisible(d_->hosted[Extra].custom);
  d_->hosted[Body].host->setVisible(d_->hosted[Body].custom);
  d_->layout->setSpacing(0);
  d_->layout->invalidate();
  if (artworkChanged) d_->artwork->update();
  updateGeometry();
}

QSize AdResult::sizeHint() const {
  const QSize natural = d_->layout->sizeHint();
  const int width = std::max(360, natural.width());
  return QSize(width, heightForWidth(width));
}
QSize AdResult::minimumSizeHint() const { return d_->layout->minimumSize(); }
bool AdResult::hasHeightForWidth() const { return true; }
int AdResult::heightForWidth(int width) const {
  return std::max(d_->layout->minimumSize().height(),
                  d_->layout->totalHeightForWidth(std::max(1, width)));
}
bool AdResult::event(QEvent* event) {
  const bool result = QWidget::event(event);
  if (event->type() == QEvent::ParentChange) refreshAppearance();
  return result;
}
bool AdResult::eventFilter(QObject* watched, QEvent* event) {
  if (event->type() == QEvent::ParentChange) {
    for (std::size_t index = 0; index < SlotCount; ++index) {
      auto& slot = d_->hosted[index];
      if (slot.custom == watched && slot.custom->parentWidget() != slot.host) {
        disconnect(slot.destroyed);
        slot.destroyed = {};
        slot.custom->removeEventFilter(this);
        slot.layout->removeWidget(slot.custom);
        slot.custom.clear();
        const quint64 revision = ++slot.revision;
        refreshContent();
        queueSlotCleared(index, revision);
        break;
      }
    }
  }
  return QWidget::eventFilter(watched, event);
}
void AdResult::changeEvent(QEvent* event) {
  QWidget::changeEvent(event);
  if (event->type() == QEvent::FontChange || event->type() == QEvent::PaletteChange ||
      event->type() == QEvent::StyleChange || event->type() == QEvent::EnabledChange ||
      event->type() == QEvent::LayoutDirectionChange || event->type() == QEvent::LanguageChange)
    refreshAppearance();
}
void AdResult::paintEvent(QPaintEvent*) {
  if (d_->background.isValid() && d_->background.alpha() > 0) {
    QPainter painter(this);
    painter.fillRect(rect(), d_->background);
  }
}

}  // namespace adqt::widgets
