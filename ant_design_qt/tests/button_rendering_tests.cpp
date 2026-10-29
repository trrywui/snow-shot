#include "theme/theme.h"
#include "widgets/button.h"
#include "widgets/modal.h"

#include <QApplication>
#include <QImage>
#include <QPainter>
#include <QRegion>
#include <QTest>

#include <iostream>
#include <stdexcept>

namespace {

using adqt::widgets::AdButton;
using adqt::widgets::AdModal;

void require(bool condition, const char* message) {
  if (!condition) throw std::runtime_error(message);
}

void settle() {
  QCoreApplication::sendPostedEvents();
  QCoreApplication::processEvents();
}

QImage render(QWidget& widget, qreal scale) {
  settle();
  QImage image(qCeil(widget.width() * scale), qCeil(widget.height() * scale),
               QImage::Format_ARGB32_Premultiplied);
  image.setDevicePixelRatio(scale);
  image.fill(Qt::transparent);
  QPainter painter(&image);
  widget.render(&painter);
  return image;
}

void requireContinuousOutline(QWidget& surface, AdButton& button, qreal scale) {
  // Exercise QPushButton's inherited setter, including calls made through Qt's base API.
  auto& nativeButton = static_cast<QPushButton&>(button);
  nativeButton.setDefault(false);
  const QImage plain = render(surface, scale);
  nativeButton.setDefault(true);
  const QImage outlined = render(surface, scale);
  const QPoint origin = button.mapTo(&surface, QPoint());
  const QRect bounds(qFloor(origin.x() * scale), qFloor(origin.y() * scale),
                     qCeil(button.width() * scale), qCeil(button.height() * scale));

  bool left = false;
  bool right = false;
  bool top = false;
  bool bottom = false;
  QVector<QPoint> changed;
  QImage visited(outlined.size(), QImage::Format_Grayscale8);
  visited.fill(0);
  for (int y = 0; y < outlined.height(); ++y) {
    for (int x = 0; x < outlined.width(); ++x) {
      if (plain.pixel(x, y) == outlined.pixel(x, y)) continue;
      visited.scanLine(y)[x] = 1;
      changed.append(QPoint(x, y));
      if (qAbs(y - bounds.center().y()) <= 1) {
        left |= x < bounds.left();
        right |= x > bounds.right();
      }
      if (qAbs(x - bounds.center().x()) <= 1) {
        top |= y < bounds.top();
        bottom |= y > bounds.bottom();
      }
    }
  }
  if (!(left && right && top && bottom)) {
    std::cerr << "outline size=" << button.width() << 'x' << button.height() << " scale=" << scale
              << " shape=" << static_cast<int>(button.shape())
              << " role=" << static_cast<int>(button.accentRole()) << " edges=" << left << right
              << top << bottom << '\n';
  }
  require(left && right && top && bottom,
          "a default button must paint its complete outline beyond all four widget edges");

  QVector<QPoint> connected{changed.first()};
  visited.scanLine(connected.first().y())[connected.first().x()] = 0;
  for (qsizetype index = 0; index < connected.size(); ++index) {
    for (int dy = -1; dy <= 1; ++dy) {
      for (int dx = -1; dx <= 1; ++dx) {
        const QPoint next = connected.at(index) + QPoint(dx, dy);
        if (!visited.rect().contains(next) || visited.constScanLine(next.y())[next.x()] == 0)
          continue;
        visited.scanLine(next.y())[next.x()] = 0;
        connected.append(next);
      }
    }
  }
  require(connected.size() == changed.size(),
          "the default outline must form one connected ring without isolated corner fragments");

  nativeButton.setDefault(false);
  require(render(surface, scale) == plain,
          "clearing the default role must remove its outline without leaving stale pixels");
}

void defaultButtonOutlinesAreContinuous() {
  QWidget owner;
  owner.resize(240, 120);
  owner.setAutoFillBackground(true);
  AdButton button(&owner);
  button.setGeometry(60, 40, 120, 40);
  owner.show();
  owner.activateWindow();
  button.setFocus(Qt::OtherFocusReason);
  for (const auto scheme : {adqt::theme::ThemeScheme::Light, adqt::theme::ThemeScheme::Dark}) {
    adqt::theme::ThemeManager::instance().setColorScheme(scheme);
    for (const auto shape : {AdButton::Shape::Rounded, AdButton::Shape::Pill,
                             AdButton::Shape::Rectangle, AdButton::Shape::Circle}) {
      button.setShape(shape);
      button.resize(shape == AdButton::Shape::Circle ? 40 : 120, 40);
      for (const auto role : {AdButton::AccentRole::Danger, AdButton::AccentRole::Primary,
                              AdButton::AccentRole::Neutral}) {
        button.setAccentRole(role);
        button.setButtonStyle(role == AdButton::AccentRole::Neutral ? AdButton::ButtonStyle::Outline
                                                                    : AdButton::ButtonStyle::Solid);
        for (const qreal scale : {1.0, 1.25, 1.5, 2.0}) {
          requireContinuousOutline(owner, button, scale);
        }
      }
    }
  }
}

void defaultOutlineFollowsInteractionState() {
  QWidget owner;
  owner.resize(240, 120);
  owner.setAutoFillBackground(true);
  AdButton button(&owner);
  button.setGeometry(60, 40, 120, 40);
  button.setButtonStyle(AdButton::ButtonStyle::Solid);
  button.setAccentRole(AdButton::AccentRole::Danger);
  button.setDefault(true);
  owner.show();
  owner.activateWindow();
  button.setFocus(Qt::OtherFocusReason);
  const QImage initial = render(owner, 1.0);
  button.clearFocus();
  button.setFocus(Qt::TabFocusReason);
  require(render(owner, 1.0) != initial,
          "keyboard focus must take precedence over the default outline");
  button.clearFocus();
  button.setFocus(Qt::OtherFocusReason);
  require(render(owner, 1.0) == initial, "leaving keyboard focus must restore the default outline");

  for (const auto scheme : {adqt::theme::ThemeScheme::Light, adqt::theme::ThemeScheme::Dark}) {
    adqt::theme::ThemeManager::instance().setColorScheme(scheme);
    const QImage themed = render(owner, 1.0);
    button.setDefault(false);
    render(owner, 1.0);
    button.setDefault(true);
    require(render(owner, 1.0) == themed, "theme changes must refresh an existing default outline");
  }

  button.setEnabled(false);
  const QImage disabled = render(owner, 1.0);
  button.setDefault(false);
  require(render(owner, 1.0) == disabled, "disabled buttons must remove their default outline");
  button.setDefault(true);
  button.setEnabled(true);
  const QImage enabled = render(owner, 1.0);
  button.setBusy(true);
  require(!button.isDefault(), "busy buttons must suspend their default role");
  render(owner, 1.0);
  button.setBusy(false);
  require(button.isDefault() && render(owner, 1.0) == enabled,
          "leaving the busy state must restore the default role and its outline");

  button.hide();
  const QImage hidden = render(owner, 1.0);
  button.setDefault(false);
  require(render(owner, 1.0) == hidden, "hidden buttons must remove their default outline");
}

void outlineFootprintLeavesNativeContentUncovered() {
  QWidget owner;
  owner.resize(320, 240);
  QWidget nativeContent(&owner);
  nativeContent.setGeometry(20, 20, 100, 80);
  nativeContent.winId();
  AdButton button(&owner);
  button.setGeometry(160, 160, 120, 40);
  button.setDefault(true);
  owner.show();
  render(owner, 1.0);

  QWidget* overlay = nullptr;
  for (QWidget* child : owner.findChildren<QWidget*>(Qt::FindDirectChildrenOnly)) {
    if (child->property("adqt.interaction.overlay").toBool()) overlay = child;
  }
  require(overlay && overlay->isVisible() && !overlay->mask().isEmpty(),
          "an outline overlay must constrain its native window footprint");
  const auto footprint = [&] { return overlay->mask().translated(overlay->pos()); };
  require(!footprint().intersects(nativeContent.geometry()) &&
              footprint().contains(button.geometry().center()),
          "outlines must leave unrelated native content uncovered");
  const QPoint previousCenter = button.geometry().center();
  button.move(160, 120);
  render(owner, 1.0);
  require(!footprint().contains(previousCenter) &&
              footprint().contains(button.geometry().center()) &&
              !footprint().intersects(nativeContent.geometry()),
          "moving an outlined control must move its native footprint without stale coverage");
}

void destroyConfirmationOutlineAndActivation(AdModal::Mode mode) {
  QWidget owner;
  owner.resize(640, 360);
  owner.show();
  AdModal modal(&owner);
  modal.setMode(mode);
  modal.setPreset(AdModal::Preset::Confirm);
  modal.setWindowTitle(QStringLiteral("Destroy pinned window"));
  modal.setText(QStringLiteral("Destroy this pinned window? This action cannot be undone."));
  modal.setAcceptText(QStringLiteral("Destroy"));
  modal.setAcceptAccentRole(AdButton::AccentRole::Danger);
  modal.open();
  settle();
  auto* accept = qobject_cast<AdButton*>(modal.acceptButton());
  require(accept && accept->isDefault(), "the modal must preserve its default accept button");
  QWidget* surface = accept->window();
  accept->setFocus(Qt::OtherFocusReason);
  requireContinuousOutline(*surface, *accept, 1.0);
  requireContinuousOutline(*surface, *accept, 1.5);
  accept->setDefault(true);
  bool accepted = false;
  QObject::connect(&modal, &AdModal::accepted, &owner, [&] { accepted = true; });
  QTest::keyClick(accept, Qt::Key_Return);
  require(accepted && !modal.isOpen(), "Enter must still accept a default modal button");
}

}  // namespace

int main(int argc, char* argv[]) {
  QApplication app(argc, argv);
  auto& theme = adqt::theme::ThemeManager::instance();
  theme.applyTo(app);
  try {
    defaultButtonOutlinesAreContinuous();
    defaultOutlineFollowsInteractionState();
    outlineFootprintLeavesNativeContentUncovered();
    destroyConfirmationOutlineAndActivation(AdModal::Mode::Overlay);
    destroyConfirmationOutlineAndActivation(AdModal::Mode::Window);
    std::cout << "Button rendering tests passed\n";
    return 0;
  } catch (const std::exception& error) {
    std::cerr << error.what() << '\n';
    return 1;
  }
}
