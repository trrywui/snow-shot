#include "widgets/result.h"
#include "theme/theme_manager.h"
#include "widgets/button.h"
#include "widgets/detail/timing_hub.h"

#include <QAccessible>
#include <QApplication>
#include <QDir>
#include <QEventLoop>
#include <QHBoxLayout>
#include <QImage>
#include <QLabel>
#include <QList>
#include <QPainter>
#include <QPixmap>
#include <QPointer>
#include <QPushButton>
#include <QTimer>

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdlib>
#include <iostream>
#include <limits>

namespace {

using adqt::theme::ThemeManager;
using adqt::theme::ThemeScheme;
using adqt::widgets::AdButton;
using adqt::widgets::AdResult;

constexpr std::array statuses = {AdResult::Status::Success,    AdResult::Status::Error,
                                 AdResult::Status::Info,       AdResult::Status::Warning,
                                 AdResult::Status::Forbidden,  AdResult::Status::NotFound,
                                 AdResult::Status::ServerError};

void require(bool condition, const char* message) {
  if (!condition) {
    std::cerr << message << '\n';
    std::exit(EXIT_FAILURE);
  }
}

void settle() {
  QCoreApplication::sendPostedEvents(nullptr, QEvent::LayoutRequest);
  QCoreApplication::processEvents();
}

QImage render(QWidget& widget, qreal dpr = 1.0) {
  widget.ensurePolished();
  QImage image(QSize(qCeil(widget.width() * dpr), qCeil(widget.height() * dpr)),
               QImage::Format_ARGB32_Premultiplied);
  image.setDevicePixelRatio(dpr);
  image.fill(Qt::transparent);
  QPainter painter(&image);
  widget.render(&painter);
  return image;
}

QWidget* namedWidget(const AdResult& result, const char* name) {
  QWidget* widget = result.findChild<QWidget*>(QString::fromLatin1(name));
  require(widget != nullptr, "result must retain named semantic child widgets");
  return widget;
}

QLabel* textLabel(const AdResult& result, const char* name) {
  auto* label = qobject_cast<QLabel*>(namedWidget(result, name));
  require(label != nullptr, "the default text slot must use a native QLabel");
  return label;
}

int colorPixels(const QImage& image, QColor expected, int tolerance = 4) {
  int pixels = 0;
  for (int y = 0; y < image.height(); ++y) {
    for (int x = 0; x < image.width(); ++x) {
      const QColor actual = image.pixelColor(x, y);
      if (actual.alpha() > 240 && std::abs(actual.red() - expected.red()) <= tolerance &&
          std::abs(actual.green() - expected.green()) <= tolerance &&
          std::abs(actual.blue() - expected.blue()) <= tolerance) {
        ++pixels;
      }
    }
  }
  return pixels;
}

QIcon solidIcon(QColor color) {
  QPixmap pixmap(32, 32);
  pixmap.fill(color);
  return QIcon(pixmap);
}

void propertyAndNotificationInvariants() {
  AdResult result;
  require(result.status() == AdResult::Status::Info && result.iconVisible() &&
              result.title().isEmpty() && result.subTitle().isEmpty() && result.icon().isNull(),
          "the default result must be informational with an automatic icon and empty text");
  require(!result.iconWidget() && !result.titleWidget() && !result.subTitleWidget() &&
              !result.extraWidget() && !result.contentWidget(),
          "custom slots must start empty");
  int titleSignals = 0;
  int subtitleSignals = 0;
  int statusSignals = 0;
  int iconSignals = 0;
  int visibilitySignals = 0;
  int tokenSignals = 0;
  int styleSignals = 0;
  QObject::connect(&result, &AdResult::titleChanged, [&] { ++titleSignals; });
  QObject::connect(&result, &AdResult::subTitleChanged, [&] { ++subtitleSignals; });
  QObject::connect(&result, &AdResult::statusChanged, [&] { ++statusSignals; });
  QObject::connect(&result, &AdResult::iconChanged, [&] { ++iconSignals; });
  QObject::connect(&result, &AdResult::iconVisibleChanged, [&] { ++visibilitySignals; });
  QObject::connect(&result, &AdResult::componentTokensChanged, [&] { ++tokenSignals; });
  QObject::connect(&result, &AdResult::semanticStylesChanged, [&] { ++styleSignals; });

  result.setTitle(QStringLiteral("Saved successfully"));
  result.setTitle(result.title());
  result.setSubTitle(QStringLiteral("The image is available in your library."));
  result.setSubTitle(result.subTitle());
  require(titleSignals == 1 && subtitleSignals == 1,
          "identical title and subtitle updates must not publish duplicate changes");
  for (const auto status : statuses) {
    result.setStatus(status);
    result.setStatus(status);
    require(result.status() == status, "all seven statuses must round-trip");
  }
  require(statusSignals == static_cast<int>(statuses.size()),
          "each distinct status must publish exactly one change");
  const QIcon custom = solidIcon(Qt::magenta);
  result.setIcon(custom);
  result.setIcon(custom);
  require(result.icon().cacheKey() == custom.cacheKey() && iconSignals == 1,
          "a custom icon must round-trip without duplicate notifications");
  result.setIcon({});
  result.setIcon({});
  require(result.icon().isNull() && iconSignals == 2,
          "a null icon must restore the status icon exactly once");
  result.setIconVisible(false);
  result.setIconVisible(false);
  result.setIconVisible(true);
  require(visibilitySignals == 2, "icon visibility must emit only on a state transition");

  AdResult::ComponentTokens tokens;
  tokens.titleFontSize = 31;
  tokens.subtitleFontSize = 17;
  tokens.iconFontSize = 88;
  tokens.extraMargin = QMargins(7, 21, 9, 3);
  result.setComponentTokens(tokens);
  result.setComponentTokens(tokens);
  require(result.componentTokens().titleFontSize == tokens.titleFontSize &&
              result.componentTokens().subtitleFontSize == tokens.subtitleFontSize &&
              result.componentTokens().iconFontSize == tokens.iconFontSize &&
              result.componentTokens().extraMargin == tokens.extraMargin && tokenSignals == 1,
          "component tokens must round-trip with one notification per effective change");
  result.resetComponentTokens();
  result.resetComponentTokens();
  require(!result.componentTokens().titleFontSize && tokenSignals == 2,
          "resetting already-default component tokens must be a no-op");
  tokens.titleFontSize = std::numeric_limits<qreal>::quiet_NaN();
  tokens.subtitleFontSize = std::numeric_limits<qreal>::infinity();
  tokens.iconFontSize = -3;
  tokens.extraMargin.reset();
  result.setComponentTokens(tokens);
  require(!result.componentTokens().titleFontSize && !result.componentTokens().subtitleFontSize &&
              !result.componentTokens().iconFontSize && tokenSignals == 2,
          "invalid token sizes must normalize to defaults before equality and notifications");
  tokens.titleFontSize = 100000;
  tokens.extraMargin = QMargins(-2, 100000, -7, 3);
  result.setComponentTokens(tokens);
  require(
      result.componentTokens().titleFontSize == std::optional<qreal>(4096) &&
          result.componentTokens().extraMargin == std::optional<QMargins>(QMargins(0, 4096, 0, 3)),
      "oversized token dimensions and negative margins must remain bounded");
  result.resetComponentTokens();

  AdResult::SemanticStyles styles;
  styles.root.backgroundColor = QColor(Qt::black);
  styles.title.textColor = QColor(Qt::red);
  styles.subTitle.textColor = QColor(Qt::green);
  styles.body.backgroundColor = QColor(Qt::blue);
  result.setSemanticStyles(styles);
  result.setSemanticStyles(styles);
  require(result.semanticStyles().title.textColor == styles.title.textColor && styleSignals == 1,
          "semantic styles must round-trip without duplicate notifications");
  result.resetSemanticStyles();
  result.resetSemanticStyles();
  require(!result.semanticStyles().title.textColor && styleSignals == 2,
          "resetting already-default semantic styles must be a no-op");
}

void responsiveTextAndGeometry() {
  AdResult result;
  result.setTitle(QStringLiteral("Your changes have been saved and are ready to share"));
  result.setSubTitle(QStringLiteral(
      "The updated document is available in your library. You can continue editing it or return "
      "to your project and share the result with the rest of your team."));
  auto* title = textLabel(result, "adqt-result-title");
  auto* subtitle = textLabel(result, "adqt-result-subtitle");
  require(title->wordWrap() && subtitle->wordWrap() && result.hasHeightForWidth(),
          "result text must wrap using native height-for-width layout");
  require(result.heightForWidth(280) > result.heightForWidth(760),
          "narrow results must grow vertically to keep the full text visible");
  require(result.minimumSizeHint().width() >= 0 && result.minimumSizeHint().height() >= 0 &&
              result.sizeHint().width() > 0 && result.sizeHint().height() > 0,
          "natural and minimum result sizes must remain valid");
  for (const int width : {280, 480, 760}) {
    result.resize(width, result.heightForWidth(width));
    result.show();
    settle();
    require(title->isVisible() && subtitle->isVisible() &&
                title->mapTo(&result, QPoint()).y() < subtitle->mapTo(&result, QPoint()).y(),
            "the title must precede the subtitle at every supported width");
    for (QWidget* child : {static_cast<QWidget*>(title), static_cast<QWidget*>(subtitle)}) {
      const QRect rect(child->mapTo(&result, QPoint()), child->size());
      if (!result.rect().contains(rect) ||
          child->height() < child->heightForWidth(child->width())) {
        std::cerr << "result width=" << width << " size=" << result.width() << 'x'
                  << result.height() << " hfw=" << result.heightForWidth(width)
                  << " child=" << child->objectName().toStdString() << " rect=" << rect.x() << ','
                  << rect.y() << ',' << rect.width() << ',' << rect.height()
                  << " child_hfw=" << child->heightForWidth(child->width()) << '\n';
      }
      require(
          result.rect().contains(rect) && child->height() >= child->heightForWidth(child->width()),
          "wrapped text must fit within the result without clipping");
    }
    const QImage first = render(result);
    require(first == render(result),
            "repeated result paints must produce identical settled output");
  }
  result.setTitle(QStringLiteral("<b>Saved</b> & ready"));
  result.setSubTitle(QStringLiteral("<a href='example'>Open</a>"));
  require(title->textFormat() == Qt::PlainText && subtitle->textFormat() == Qt::PlainText &&
              title->text() == result.title() && subtitle->text() == result.subTitle(),
          "text properties must display literal strings rather than interpreting markup");
  result.setTitle({});
  result.setSubTitle({});
  settle();
  require(title->isHidden() && subtitle->isHidden(),
          "empty text slots must collapse rather than reserving blank label space");
}

void statusesThemesAndDeviceScales() {
  ThemeManager& manager = ThemeManager::instance();
  const auto original = manager.config();
  AdResult result;
  result.setTitle(QStringLiteral("Operation complete"));
  result.setSubTitle(QStringLiteral("Review the details below."));
  result.resize(500, 430);
  result.show();
  for (const auto scheme : {ThemeScheme::Light, ThemeScheme::Dark}) {
    manager.setPreset(scheme);
    for (const auto status : statuses) {
      result.setStatus(status);
      settle();
      QWidget* icon = namedWidget(result, "adqt-result-icon");
      require(icon->isVisible() && !icon->size().isEmpty(),
              "every result status must render a visible default icon or illustration");
      const auto theme = manager.resolveTheme(&result);
      QColor statusColor;
      switch (status) {
        case AdResult::Status::Success:
          statusColor = theme.colorSuccess;
          break;
        case AdResult::Status::Error:
          statusColor = theme.colorError;
          break;
        case AdResult::Status::Info:
          statusColor = theme.colorInfo;
          break;
        case AdResult::Status::Warning:
          statusColor = theme.colorWarning;
          break;
        default:
          break;
      }
      for (const qreal dpr : {1.0, 1.25, 2.0}) {
        const QImage image = render(*icon, dpr);
        require(!image.isNull() && image.devicePixelRatio() == dpr,
                "status artwork must render at integral and fractional device scales");
        if (statusColor.isValid()) {
          require(colorPixels(image, statusColor) > 20,
                  "standard status icons must use their resolved semantic theme color");
        } else {
          require(icon->width() >= 100 && icon->height() >= 100,
                  "exception statuses must retain their larger illustration presentation");
          require(colorPixels(image, image.pixelColor(0, 0)) < image.width() * image.height() - 100,
                  "exception artwork must produce visible raster content");
        }
      }
    }
  }
  result.setStatus(AdResult::Status::Info);
  result.setIcon(solidIcon(Qt::magenta));
  settle();
  require(colorPixels(render(*namedWidget(result, "adqt-result-icon")), Qt::magenta) > 100,
          "a custom QIcon must replace the automatic artwork without a theme tint");
  result.setIconVisible(false);
  settle();
  require(namedWidget(result, "adqt-result-icon")->isHidden(),
          "icon visibility must suppress the default or custom QIcon");
  result.setIcon({});
  result.setIconVisible(true);
  settle();
  require(colorPixels(render(*namedWidget(result, "adqt-result-icon")),
                      manager.resolveTheme(&result).colorInfo) > 20,
          "clearing the custom QIcon must restore themed status artwork");
  result.setStatus(AdResult::Status::NotFound);
  result.resize(180, result.heightForWidth(180));
  settle();
  QWidget* narrowIcon = namedWidget(result, "adqt-result-icon");
  require(
      result.width() == 180 && narrowIcon->width() <= result.width() &&
          result.rect().contains(QRect(narrowIcon->mapTo(&result, QPoint()), narrowIcon->size())),
      "exception artwork must scale down to fit narrow results without clipping or forcing width");
  manager.setConfig(original);
}

struct Slot {
  QWidget* (AdResult::*get)() const;
  void (AdResult::*set)(QWidget*);
  QWidget* (AdResult::*take)();
  void (AdResult::*changed)(QWidget*);
};

constexpr std::array slotCases = {
    Slot{&AdResult::iconWidget, &AdResult::setIconWidget, &AdResult::takeIconWidget,
         &AdResult::iconWidgetChanged},
    Slot{&AdResult::titleWidget, &AdResult::setTitleWidget, &AdResult::takeTitleWidget,
         &AdResult::titleWidgetChanged},
    Slot{&AdResult::subTitleWidget, &AdResult::setSubTitleWidget, &AdResult::takeSubTitleWidget,
         &AdResult::subTitleWidgetChanged},
    Slot{&AdResult::extraWidget, &AdResult::setExtraWidget, &AdResult::takeExtraWidget,
         &AdResult::extraWidgetChanged},
    Slot{&AdResult::contentWidget, &AdResult::setContentWidget, &AdResult::takeContentWidget,
         &AdResult::contentWidgetChanged},
};

void slotOwnershipAndLifetime() {
  for (const Slot& slot : slotCases) {
    AdResult result;
    result.resize(420, 500);
    result.show();
    int changes = 0;
    QObject::connect(&result, slot.changed, [&] { ++changes; });
    QPointer<QWidget> first = new QLabel(QStringLiteral("First custom slot"));
    (result.*slot.set)(first);
    (result.*slot.set)(first);
    settle();
    require((result.*slot.get)() == first && result.isAncestorOf(first) && first->isVisible() &&
                changes == 1,
            "custom slots must adopt and display widgets once");
    auto* replacement = new QLabel(QStringLiteral("Replacement custom slot"));
    (result.*slot.set)(replacement);
    require((result.*slot.get)() == replacement && result.isAncestorOf(replacement) && changes == 2,
            "slot replacement must immediately commit the replacement and publish once");
    QCoreApplication::sendPostedEvents(nullptr, QEvent::DeferredDelete);
    require(first.isNull(), "replaced owned slot widgets must be released by deferred deletion");
    QWidget* taken = (result.*slot.take)();
    require(taken == replacement && !taken->parentWidget() && taken->isHidden() &&
                !(result.*slot.get)() && changes == 3,
            "taking a slot must detach and hide its widget without destroying it");
    require(!(result.*slot.take)() && changes == 3,
            "taking an empty slot must leave state and notifications unchanged");
    delete taken;
    auto* externallyDeleted = new QWidget;
    (result.*slot.set)(externallyDeleted);
    delete externallyDeleted;
    require(!(result.*slot.get)() && changes == 4,
            "external destruction must clear the slot before deferring its notification");
    settle();
    require(changes == 5, "external destruction must publish the empty slot after native teardown");
    QWidget newOwner;
    auto* reparented = new QWidget;
    (result.*slot.set)(reparented);
    reparented->setParent(&newOwner);
    require(!(result.*slot.get)() && reparented->parentWidget() == &newOwner && changes == 6,
            "external reparenting must release the slot before deferring its notification");
    settle();
    require(changes == 7,
            "external reparenting must publish the empty slot after native reparenting");
  }
  AdResult result;
  auto* moved = new QLabel(QStringLiteral("Move between result slots"));
  result.setTitleWidget(moved);
  result.setSubTitleWidget(moved);
  require(!result.titleWidget() && result.subTitleWidget() == moved && result.isAncestorOf(moved),
          "moving one widget between slots must preserve exactly one owner");
  QCoreApplication::sendPostedEvents(nullptr, QEvent::DeferredDelete);
  require(result.subTitleWidget() == moved,
          "a moved slot widget must never remain queued for deferred deletion");
  auto* owner = new AdResult;
  QPointer<QWidget> owned = new QWidget;
  owner->setContentWidget(owned);
  delete owner;
  require(owned.isNull(), "destroying a result must destroy its current owned slot widgets");
}

void notificationsPermitOwnerDeletion() {
  const auto check = [](auto signal, auto mutate) {
    QPointer<AdResult> result = new AdResult;
    QObject::connect(result, signal, [&] { delete result.data(); });
    mutate(*result);
    require(result.isNull(), "result changed notifications must permit synchronous owner deletion");
  };
  check(&AdResult::statusChanged,
        [](AdResult& result) { result.setStatus(AdResult::Status::Success); });
  check(&AdResult::titleChanged,
        [](AdResult& result) { result.setTitle(QStringLiteral("Complete")); });
  check(&AdResult::subTitleChanged,
        [](AdResult& result) { result.setSubTitle(QStringLiteral("Your file is ready.")); });
  check(&AdResult::iconChanged, [](AdResult& result) { result.setIcon(solidIcon(Qt::magenta)); });
  check(&AdResult::iconVisibleChanged, [](AdResult& result) { result.setIconVisible(false); });
  check(&AdResult::componentTokensChanged, [](AdResult& result) {
    AdResult::ComponentTokens tokens;
    tokens.titleFontSize = 29;
    result.setComponentTokens(tokens);
  });
  check(&AdResult::semanticStylesChanged, [](AdResult& result) {
    AdResult::SemanticStyles styles;
    styles.root.backgroundColor = Qt::red;
    result.setSemanticStyles(styles);
  });
  for (const Slot& slot : slotCases) {
    check(slot.changed, [&](AdResult& result) { (result.*slot.set)(new QWidget); });
    QPointer<AdResult> takeOwner = new AdResult;
    (takeOwner.data()->*slot.set)(new QWidget);
    QObject::connect(takeOwner, slot.changed, [&] { delete takeOwner.data(); });
    QWidget* taken = (takeOwner.data()->*slot.take)();
    require(
        takeOwner.isNull() && taken && !taken->parentWidget(),
        "taking a custom widget must return ownership even when its notification deletes Result");
    delete taken;

    QWidget adoptingParent;
    QPointer<AdResult> reparentOwner = new AdResult;
    auto* reparented = new QWidget;
    (reparentOwner.data()->*slot.set)(reparented);
    QObject::connect(reparentOwner, slot.changed, [&] { delete reparentOwner.data(); });
    reparented->setParent(&adoptingParent);
    require(reparentOwner && !(reparentOwner.data()->*slot.get)(),
            "external reparenting must defer changed callbacks until QWidget::setParent returns");
    settle();
    require(reparentOwner.isNull() && reparented->parentWidget() == &adoptingParent,
            "queued external reparent notifications must safely permit deletion of Result");

    QPointer<AdResult> destroyedOwner = new AdResult;
    auto* externallyDestroyed = new QWidget;
    (destroyedOwner.data()->*slot.set)(externallyDestroyed);
    QObject::connect(destroyedOwner, slot.changed, [&] { delete destroyedOwner.data(); });
    delete externallyDestroyed;
    require(
        destroyedOwner && !(destroyedOwner.data()->*slot.get)(),
        "external destruction must defer changed callbacks until the QWidget destructor returns");
    settle();
    require(destroyedOwner.isNull(),
            "queued external destruction notifications must safely permit deletion of Result");
  }
  QPointer<AdResult> movedOwner = new AdResult;
  auto* moved = new QWidget;
  movedOwner->setTitleWidget(moved);
  QObject::connect(movedOwner, &AdResult::titleWidgetChanged, [&] { delete movedOwner.data(); });
  movedOwner->setSubTitleWidget(moved);
  require(movedOwner.isNull(),
          "deleting the result while moving a widget between slots must stop later notifications");
}

void queuedSlotNotificationsDiscardObsoleteState() {
  for (const Slot& slot : slotCases) {
    for (const bool destroy : {false, true}) {
      AdResult result;
      QWidget adoptingParent;
      auto* original = new QWidget;
      (result.*slot.set)(original);
      QList<QWidget*> published;
      QObject::connect(&result, slot.changed,
                       [&](QWidget* widget) { published.push_back(widget); });
      if (destroy)
        delete original;
      else
        original->setParent(&adoptingParent);
      require(!(result.*slot.get)() && published.isEmpty(),
              "native lifecycle changes must clear the getter before dispatching notifications");
      auto* replacement = new QWidget;
      (result.*slot.set)(replacement);
      require((result.*slot.get)() == replacement && published == QList<QWidget*>{replacement},
              "a replacement must publish its committed slot state synchronously");
      settle();
      require(published == QList<QWidget*>{replacement},
              "a new slot generation must suppress obsolete queued empty notifications");
    }
  }
}

void slotMoveNotificationsObserveReentrantChanges() {
  for (const bool replace : {false, true}) {
    AdResult result;
    QPointer<QWidget> moved = new QWidget;
    result.setTitleWidget(moved);
    QWidget* replacement = replace ? new QWidget : nullptr;
    QWidget* taken = nullptr;
    QList<QWidget*> published;
    QObject::connect(&result, &AdResult::subTitleWidgetChanged,
                     [&](QWidget* widget) { published.push_back(widget); });
    QObject::connect(&result, &AdResult::titleWidgetChanged, [&](QWidget*) {
      if (replace)
        result.setSubTitleWidget(replacement);
      else
        taken = result.takeSubTitleWidget();
    });
    result.setSubTitleWidget(moved);
    require(
        !result.titleWidget() && result.subTitleWidget() == replacement && published.size() == 1 &&
            published.front() == replacement,
        "a move notification must never publish stale destination state after reentrant changes");
    if (replace) {
      QCoreApplication::sendPostedEvents(nullptr, QEvent::DeferredDelete);
      require(moved.isNull(), "a replaced moved widget must retain normal deferred deletion");
    } else {
      require(taken == moved && taken->isHidden() && !taken->parentWidget(),
              "a reentrantly taken moved widget must retain its transferred ownership");
      delete taken;
    }
  }
}

void customSlotsPreserveFallbacksAndNativeControls() {
  AdResult result;
  result.setTitle(QStringLiteral("Original title"));
  result.setSubTitle(QStringLiteral("Original subtitle"));
  auto* titleLabel = textLabel(result, "adqt-result-title");
  auto* subtitleLabel = textLabel(result, "adqt-result-subtitle");
  result.setTitleWidget(new QLabel(QStringLiteral("Custom title")));
  result.setSubTitleWidget(new QLabel(QStringLiteral("Custom subtitle")));
  auto* icon = new QLabel(QStringLiteral("Custom icon"));
  icon->setFixedSize(84, 60);
  result.setIconWidget(icon);
  auto* action = new QPushButton(QStringLiteral("Continue"));
  result.setExtraWidget(action);
  int clicks = 0;
  QObject::connect(action, &QPushButton::clicked, [&] { ++clicks; });
  result.resize(500, result.heightForWidth(500));
  result.show();
  settle();
  require(titleLabel->isHidden() && subtitleLabel->isHidden() &&
              namedWidget(result, "adqt-result-icon")->isHidden() && icon->isVisible(),
          "custom presentation widgets must replace the matching default slot content");
  result.setTitle(QStringLiteral("Updated fallback title"));
  result.setSubTitle(QStringLiteral("Updated fallback subtitle"));
  delete result.takeTitleWidget();
  delete result.takeSubTitleWidget();
  settle();
  require(titleLabel->isVisible() && subtitleLabel->isVisible() &&
              titleLabel->text() == result.title() && subtitleLabel->text() == result.subTitle(),
          "taking custom text widgets must restore the latest property text");
  result.setIconVisible(false);
  require(!icon->isVisible(), "icon visibility must also suppress the custom icon widget");
  result.setIconVisible(true);
  require(icon->isVisible(), "restoring icon visibility must restore its custom widget");
  delete result.takeIconWidget();
  settle();
  require(namedWidget(result, "adqt-result-icon")->isVisible(),
          "taking the custom icon widget must restore automatic status artwork");
  action->click();
  require(
      clicks == 1 && action->focusPolicy() != Qt::NoFocus && result.focusPolicy() == Qt::NoFocus,
      "native action widgets must retain interaction and keyboard focus independently of Result");
  result.setEnabled(false);
  require(!action->isEnabled(), "custom actions must inherit the result's disabled state");
}

void explicitPaletteAndFonts() {
  AdResult result;
  result.setTitle(QStringLiteral("Export ready"));
  result.setSubTitle(QStringLiteral("Choose how to continue."));
  auto* title = textLabel(result, "adqt-result-title");
  auto* subtitle = textLabel(result, "adqt-result-subtitle");
  QPalette palette = result.palette();
  palette.setColor(QPalette::WindowText, QColor(201, 17, 31));
  palette.setColor(QPalette::Text, QColor(23, 147, 79));
  result.setPalette(palette);
  QFont font = result.font();
  font.setPixelSize(21);
  font.setWeight(QFont::DemiBold);
  result.setFont(font);
  require(title->palette().color(QPalette::WindowText) == palette.color(QPalette::WindowText) &&
              subtitle->palette().color(QPalette::WindowText) == palette.color(QPalette::Text) &&
              title->font().pixelSize() == 21 && subtitle->font().pixelSize() == 21 &&
              title->font().weight() == QFont::DemiBold,
          "explicit native palette and font settings must reach both default text slots");
  AdResult::ComponentTokens tokens;
  tokens.titleFontSize = 34;
  result.setComponentTokens(tokens);
  require(title->font().pixelSize() == 34 && subtitle->font().pixelSize() == 21,
          "a title size token must override only that slot's explicit base font size");
  AdResult::SemanticStyles styles;
  styles.title.textColor = QColor(43, 71, 223);
  result.setSemanticStyles(styles);
  require(title->palette().color(QPalette::WindowText) == *styles.title.textColor &&
              subtitle->palette().color(QPalette::WindowText) == palette.color(QPalette::Text),
          "semantic text styles must override the native palette only for their assigned slot");
  result.resetSemanticStyles();
  require(title->palette().color(QPalette::WindowText) == palette.color(QPalette::WindowText),
          "resetting semantic text styles must restore the explicit native palette");
}

void tokensSemanticStylesAndScopes() {
  ThemeManager& manager = ThemeManager::instance();
  const auto original = manager.config();
  QWidget scope;
  AdResult result(&scope);
  result.setTitle(QStringLiteral("Ready"));
  result.setSubTitle(QStringLiteral("Review the details."));
  result.setContentWidget(new QLabel(QStringLiteral("Body content")));
  result.setExtraWidget(new QPushButton(QStringLiteral("Continue")));
  scope.resize(540, 560);
  result.resize(540, 560);
  scope.show();
  settle();
  auto* title = textLabel(result, "adqt-result-title");
  auto* subtitle = textLabel(result, "adqt-result-subtitle");
  AdResult::ComponentTokens tokens;
  tokens.titleFontSize = 32;
  tokens.subtitleFontSize = 19;
  tokens.iconFontSize = 92;
  result.setComponentTokens(tokens);
  settle();
  require(title->font().pixelSize() == 32 && subtitle->font().pixelSize() == 19 &&
              namedWidget(result, "adqt-result-icon")->height() >= 92,
          "font and icon size tokens must update native typography and layout");
  AdResult::SemanticStyles weightOnlyStyles;
  QFont weightOnlyFont;
  weightOnlyFont.setWeight(QFont::Bold);
  weightOnlyStyles.title.font = weightOnlyFont;
  result.setSemanticStyles(weightOnlyStyles);
  require(title->font().pixelSize() == 32 && title->font().weight() == QFont::Bold &&
              subtitle->font().pixelSize() == 19,
          "a partial semantic font must preserve the slot's unspecified token typography");
  result.resetSemanticStyles();
  AdResult::SemanticStyles styles;
  styles.root.backgroundColor = QColor(7, 13, 19);
  styles.title.textColor = QColor(229, 31, 47);
  styles.subTitle.textColor = QColor(11, 173, 83);
  styles.body.backgroundColor = QColor(37, 53, 71);
  QFont titleFont = title->font();
  titleFont.setPixelSize(27);
  titleFont.setWeight(QFont::Bold);
  styles.title.font = titleFont;
  result.setSemanticStyles(styles);
  settle();
  require(title->palette().color(QPalette::WindowText) == *styles.title.textColor &&
              subtitle->palette().color(QPalette::WindowText) == *styles.subTitle.textColor &&
              title->font().pixelSize() == 27 && title->font().weight() == QFont::Bold,
          "semantic text colors and fonts must override component defaults");
  const QImage styled = render(result);
  require(colorPixels(styled, *styles.root.backgroundColor) > 1000 &&
              colorPixels(styled, *styles.body.backgroundColor) > 100,
          "semantic root and body background colors must paint in their respective regions");
  manager.setPreset(ThemeScheme::Dark);
  settle();
  require(title->palette().color(QPalette::WindowText) == *styles.title.textColor &&
              title->font().pixelSize() == 27,
          "explicit semantic colors and fonts must survive live theme changes");
  result.resetSemanticStyles();
  result.resetComponentTokens();
  adqt::theme::ThemeOverride overrideValue;
  overrideValue.info = QColor(137, 31, 219);
  overrideValue.appFont = QFont(QStringLiteral("Arial"));
  manager.setScopeOverride(&scope, overrideValue);
  settle();
  require(
      colorPixels(render(*namedWidget(result, "adqt-result-icon")),
                  manager.resolveTheme(&result).colorInfo) > 20 &&
          title->palette().color(QPalette::WindowText) == manager.resolveTheme(&result).colorText,
      "ancestor scopes must update the result's icon and text palette immediately");
  manager.clearScopeOverride(&scope);
  manager.setConfig(original);
}

void accessibilityAndLayoutDirection() {
  AdResult result;
  result.setTitle(QStringLiteral("Export complete"));
  result.setSubTitle(QStringLiteral("The document is ready to download."));
  auto* accessible = QAccessible::queryAccessibleInterface(&result);
  require(accessible && accessible->text(QAccessible::Name) == result.title() &&
              accessible->text(QAccessible::Description) == result.subTitle(),
          "accessibility must expose the title and subtitle without requiring duplicate labels");
  result.setTitle(QStringLiteral("Export updated"));
  require(accessible->text(QAccessible::Name) == result.title(),
          "the accessible title must follow property changes");
  auto* customTitle = new QLabel(QStringLiteral("Custom heading"));
  auto* customSubtitle = new QLabel(QStringLiteral("Custom description"));
  result.setTitleWidget(customTitle);
  result.setSubTitleWidget(customSubtitle);
  require(accessible->text(QAccessible::Name) == customTitle->text() &&
              accessible->text(QAccessible::Description) == customSubtitle->text(),
          "custom native text widgets must supply the result's accessible fallback text");
  result.setTitleWidget(nullptr);
  result.setSubTitleWidget(nullptr);
  result.setAccessibleName(QStringLiteral("Document export status"));
  result.setAccessibleDescription(QStringLiteral("Available for download"));
  result.setTitle(QStringLiteral("Export complete again"));
  result.setSubTitle(QStringLiteral("The new document is ready."));
  require(
      accessible->text(QAccessible::Name) == QStringLiteral("Document export status") &&
          accessible->text(QAccessible::Description) == QStringLiteral("Available for download"),
      "explicit accessible names and descriptions must survive result text updates");
  auto* extra = new QWidget;
  auto* layout = new QHBoxLayout(extra);
  layout->setContentsMargins(0, 0, 0, 0);
  auto* first = new QPushButton(QStringLiteral("Back"));
  auto* second = new QPushButton(QStringLiteral("Continue"));
  layout->addWidget(first);
  layout->addWidget(second);
  result.setExtraWidget(extra);
  result.resize(540, result.heightForWidth(540));
  result.show();
  settle();
  const int ltrHeight = result.heightForWidth(540);
  require(first->geometry().left() < second->geometry().left(),
          "extra actions must follow their native left-to-right layout");
  result.setLayoutDirection(Qt::RightToLeft);
  settle();
  require(first->geometry().left() > second->geometry().left() &&
              result.heightForWidth(540) == ltrHeight,
          "RTL must propagate to native action layouts without changing centered result height");
  const QWidget* icon = namedWidget(result, "adqt-result-icon");
  const qreal center = icon->mapTo(&result, QPoint()).x() + icon->width() / 2.0;
  require(std::abs(center - result.width() / 2.0) <= 1.0,
          "the status icon must remain horizontally centered in RTL");
}

class ActivityCounter final : public QObject {
 public:
  int paints = 0;
  int timers = 0;
  int layoutRequests = 0;

 protected:
  bool eventFilter(QObject*, QEvent* event) override {
    if (event->type() == QEvent::Paint) ++paints;
    if (event->type() == QEvent::Timer) ++timers;
    if (event->type() == QEvent::LayoutRequest) ++layoutRequests;
    return false;
  }
};

void settledResultsStayIdle() {
  AdResult result;
  result.setTitle(QStringLiteral("All done"));
  result.setSubTitle(QStringLiteral("The result can remain visible indefinitely."));
  result.resize(500, result.heightForWidth(500));
  result.show();
  settle();
  settle();
  ActivityCounter counter;
  result.installEventFilter(&counter);
  for (QWidget* child : result.findChildren<QWidget*>()) child->installEventFilter(&counter);
  QObject probe;
  QEventLoop loop;
  QTimer guard;
  guard.setSingleShot(true);
  QObject::connect(&guard, &QTimer::timeout, &loop, &QEventLoop::quit);
  int frames = 0;
  adqt::widgets::detail::setFrameSubscription(&probe, QStringLiteral("ResultIdleTestProbe"), true,
                                              [&](qint64, qint64) {
                                                if (++frames == 3) loop.quit();
                                              });
  guard.start(5000);
  loop.exec();
  adqt::widgets::detail::clearFrameSubscription(&probe, QStringLiteral("ResultIdleTestProbe"));
  require(frames == 3, "the timing probe must observe the requested independent frames");
  require(counter.paints == 0 && counter.timers == 0 && counter.layoutRequests == 0,
          "a settled visible result must schedule no paints, timers, or recurring layout work");
}

void renderContactSheet(const QString& directory, ThemeScheme scheme, qreal dpr) {
  ThemeManager::instance().setPreset(scheme);
  constexpr int cellWidth = 420;
  constexpr int cellHeight = 610;
  QImage sheet(QSize(qCeil(cellWidth * 4 * dpr), qCeil(cellHeight * 2 * dpr)),
               QImage::Format_ARGB32_Premultiplied);
  sheet.setDevicePixelRatio(dpr);
  sheet.fill(ThemeManager::instance().resolveTheme().colorBgContainer);
  QPainter painter(&sheet);
  const std::array titles = {QStringLiteral("Successfully submitted"),
                             QStringLiteral("Submission failed"),
                             QStringLiteral("Your operation has been executed"),
                             QStringLiteral("There are some problems"),
                             QStringLiteral("403"),
                             QStringLiteral("404"),
                             QStringLiteral("500")};
  for (std::size_t index = 0; index < statuses.size(); ++index) {
    AdResult result;
    result.setStatus(statuses[index]);
    result.setTitle(titles[index]);
    result.setSubTitle(index < 4 ? QStringLiteral("Review the details, then choose your next step.")
                                 : QStringLiteral("Sorry, something went wrong."));
    auto* action = new AdButton(QStringLiteral("Back home"));
    action->setButtonStyle(AdButton::ButtonStyle::Solid);
    action->setAccentRole(AdButton::AccentRole::Primary);
    result.setExtraWidget(action);
    result.resize(cellWidth, cellHeight);
    result.show();
    settle();
    painter.drawImage(
        QPointF(static_cast<int>(index % 4) * cellWidth, static_cast<int>(index / 4) * cellHeight),
        render(result, dpr));
  }
  painter.end();
  const QString fileName =
      QStringLiteral("result-%1-%2x.png")
          .arg(scheme == ThemeScheme::Light ? QStringLiteral("light") : QStringLiteral("dark"))
          .arg(dpr);
  require(sheet.save(QDir(directory).filePath(fileName)), "result contact sheet must save");
}

}  // namespace

int main(int argc, char** argv) {
#ifdef Q_OS_WIN
  if (qEnvironmentVariableIsEmpty("QT_QPA_FONTDIR")) {
    const QString fonts = QDir(qEnvironmentVariable("WINDIR")).filePath(QStringLiteral("Fonts"));
    if (QDir(fonts).exists()) qputenv("QT_QPA_FONTDIR", fonts.toUtf8());
  }
#endif
  QApplication app(argc, argv);
  ThemeManager::instance().applyTo(app);
  propertyAndNotificationInvariants();
  responsiveTextAndGeometry();
  statusesThemesAndDeviceScales();
  slotOwnershipAndLifetime();
  notificationsPermitOwnerDeletion();
  queuedSlotNotificationsDiscardObsoleteState();
  slotMoveNotificationsObserveReentrantChanges();
  customSlotsPreserveFallbacksAndNativeControls();
  explicitPaletteAndFonts();
  tokensSemanticStylesAndScopes();
  accessibilityAndLayoutDirection();
  settledResultsStayIdle();
  const QStringList arguments = app.arguments();
  const qsizetype renderIndex = arguments.indexOf(QStringLiteral("--render-dir"));
  if (renderIndex >= 0) {
    require(renderIndex + 1 < arguments.size(), "--render-dir requires a directory path");
    const QString directory = arguments[renderIndex + 1];
    require(QDir().mkpath(directory), "the result contact sheet directory must be created");
    for (ThemeScheme scheme : {ThemeScheme::Light, ThemeScheme::Dark}) {
      for (qreal dpr : {1.0, 1.25, 2.0}) renderContactSheet(directory, scheme, dpr);
    }
  }
  std::cout << "Result tests passed\n";
  return EXIT_SUCCESS;
}
