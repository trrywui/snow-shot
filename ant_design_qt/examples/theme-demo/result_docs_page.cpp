#include "result_docs_page.h"

#include <QCoreApplication>
#include <QEvent>
#include <QFrame>
#include <QHBoxLayout>
#include <QLabel>
#include <QVariant>
#include <QVBoxLayout>

#include <array>

#include "antd_icons.h"
#include "demo_theme_utils.h"
#include "widgets/button.h"

using adqt::widgets::AdButton;
using adqt::widgets::AdResult;
namespace outlined_icons = adqt::icons::antd::outlined;

namespace {

struct SectionText {
  const char* title;
  const char* description;
};

const std::array<SectionText, 12> kSections = {{
    {QT_TRANSLATE_NOOP("ResultDocsPage", "Success"),
     QT_TRANSLATE_NOOP("ResultDocsPage", "Confirm a completed operation and offer a next step.")},
    {QT_TRANSLATE_NOOP("ResultDocsPage", "Information"),
     QT_TRANSLATE_NOOP("ResultDocsPage", "Explain the current state before the user continues.")},
    {QT_TRANSLATE_NOOP("ResultDocsPage", "Warning"),
     QT_TRANSLATE_NOOP("ResultDocsPage",
                       "Call attention to a result that needs the user's review.")},
    {QT_TRANSLATE_NOOP("ResultDocsPage", "Error with details"),
     QT_TRANSLATE_NOOP("ResultDocsPage",
                       "Provide a concise explanation, recovery actions, and additional details.")},
    {QT_TRANSLATE_NOOP("ResultDocsPage", "403"),
     QT_TRANSLATE_NOOP("ResultDocsPage", "Explain that the requested page requires permission.")},
    {QT_TRANSLATE_NOOP("ResultDocsPage", "404"),
     QT_TRANSLATE_NOOP("ResultDocsPage", "Help the user recover when a page cannot be found.")},
    {QT_TRANSLATE_NOOP("ResultDocsPage", "500"),
     QT_TRANSLATE_NOOP("ResultDocsPage", "Offer a recovery path when the server is unavailable.")},
    {QT_TRANSLATE_NOOP("ResultDocsPage", "Custom icon"),
     QT_TRANSLATE_NOOP("ResultDocsPage",
                       "Use a QIcon for a custom symbol. A null icon restores the status icon.")},
    {QT_TRANSLATE_NOOP("ResultDocsPage", "Custom widgets"),
     QT_TRANSLATE_NOOP(
         "ResultDocsPage",
         "Supply native Qt widgets for the icon, title, subtitle, actions, or body.")},
    {QT_TRANSLATE_NOOP("ResultDocsPage", "Component tokens"),
     QT_TRANSLATE_NOOP("ResultDocsPage",
                       "Adjust title, subtitle, and icon sizes and the space around the actions.")},
    {QT_TRANSLATE_NOOP("ResultDocsPage", "Semantic styling"),
     QT_TRANSLATE_NOOP(
         "ResultDocsPage",
         "Style the result's root, icon, title, subtitle, actions, and body independently.")},
    {QT_TRANSLATE_NOOP("ResultDocsPage", "Narrow layout"),
     QT_TRANSLATE_NOOP(
         "ResultDocsPage",
         "Text wraps to the available width. A vertical action group fits a narrow panel.")},
}};

QString translated(const char* source) {
  return QCoreApplication::translate("ResultDocsPage", source);
}

QWidget* column() {
  auto* widget = new QWidget();
  auto* layout = new QVBoxLayout(widget);
  layout->setContentsMargins(0, 0, 0, 0);
  layout->setSpacing(12);
  return widget;
}

}  // namespace

ResultDocsPage::ResultDocsPage(QWidget* parent) : QWidget(parent) {
  setObjectName(QStringLiteral("resultDocsPage"));
  auto* root = new QVBoxLayout(this);
  root->setContentsMargins(16, 16, 16, 24);
  root->setSpacing(16);

  auto* title = label(QT_TRANSLATE_NOOP("ResultDocsPage", "Result"));
  QFont titleFont = title->font();
  titleFont.setPointSize(titleFont.pointSize() + 8);
  titleFont.setBold(true);
  title->setFont(titleFont);
  root->addWidget(title);
  root->addWidget(label(QT_TRANSLATE_NOOP(
      "ResultDocsPage",
      "Show the outcome of an operation with a clear explanation and next steps.")));

  addSection(root, 0, buildSuccessDemo());
  addSection(root, 1,
             result(AdResult::Status::Info,
                    QT_TRANSLATE_NOOP("ResultDocsPage", "Your report is ready to review"),
                    QT_TRANSLATE_NOOP("ResultDocsPage",
                                      "Review the details before sharing it with your team."),
                    "resultInfo"));
  addSection(
      root, 2,
      result(AdResult::Status::Warning,
             QT_TRANSLATE_NOOP("ResultDocsPage", "Some items need your attention"),
             QT_TRANSLATE_NOOP("ResultDocsPage", "Review the highlighted items before continuing."),
             "resultWarning"));
  addSection(root, 3, buildErrorDemo());
  auto* forbidden = result(
      AdResult::Status::Forbidden, QT_TRANSLATE_NOOP("ResultDocsPage", "403"),
      QT_TRANSLATE_NOOP("ResultDocsPage", "Sorry, you are not authorized to access this page."),
      "resultForbidden");
  forbidden->setExtraWidget(actions(QT_TRANSLATE_NOOP("ResultDocsPage", "Back home")));
  addSection(root, 4, forbidden);
  auto* notFound =
      result(AdResult::Status::NotFound, QT_TRANSLATE_NOOP("ResultDocsPage", "404"),
             QT_TRANSLATE_NOOP("ResultDocsPage", "Sorry, the page you visited does not exist."),
             "resultNotFound");
  notFound->setExtraWidget(actions(QT_TRANSLATE_NOOP("ResultDocsPage", "Back home")));
  addSection(root, 5, notFound);
  auto* serverError = result(
      AdResult::Status::ServerError, QT_TRANSLATE_NOOP("ResultDocsPage", "500"),
      QT_TRANSLATE_NOOP("ResultDocsPage", "Sorry, something went wrong. Please try again later."),
      "resultServerError");
  serverError->setExtraWidget(actions(QT_TRANSLATE_NOOP("ResultDocsPage", "Back home")));
  addSection(root, 6, serverError);
  addSection(root, 7, buildCustomIconDemo());
  addSection(root, 8, buildCustomWidgetsDemo());
  addSection(root, 9, buildTokenDemo());
  addSection(root, 10, buildSemanticDemo());
  addSection(root, 11, buildNarrowDemo());
  root->addStretch();
}

const QStringList& ResultDocsPage::defaultSectionTitles() {
  static QStringList titles;
  titles.clear();
  titles.reserve(static_cast<qsizetype>(kSections.size()));
  for (const auto& section : kSections) {
    titles.append(translated(section.title));
  }
  return titles;
}

const QVector<QWidget*>& ResultDocsPage::sectionAnchors() const { return anchors_; }

const QStringList& ResultDocsPage::sectionTitles() const { return titles_; }

void ResultDocsPage::changeEvent(QEvent* event) {
  QWidget::changeEvent(event);
  if (event->type() == QEvent::LanguageChange) {
    for (const auto& refresh : translations_) {
      refresh();
    }
    titles_ = defaultSectionTitles();
  }
}

void ResultDocsPage::addSection(QVBoxLayout* root, int index, QWidget* content) {
  const auto& section = kSections.at(static_cast<std::size_t>(index));
  auto* panel = new QFrame();
  panel->setObjectName(QStringLiteral("resultSection%1").arg(index));
  panel->setFrameShape(QFrame::StyledPanel);
  auto* layout = new QVBoxLayout(panel);
  layout->setContentsMargins(14, 14, 14, 14);
  layout->setSpacing(10);
  auto* title = label(section.title);
  QFont titleFont = title->font();
  titleFont.setBold(true);
  titleFont.setPointSize(titleFont.pointSize() + 1);
  title->setFont(titleFont);
  layout->addWidget(title);
  layout->addWidget(label(section.description, true));
  layout->addWidget(content);
  root->addWidget(panel);
  anchors_.append(panel);
  titles_.append(translated(section.title));
}

QLabel* ResultDocsPage::label(const char* source, bool hint) {
  auto* widget = hint ? demo::makeHintLabel(translated(source)) : new QLabel(translated(source));
  widget->setWordWrap(true);
  translations_.append([widget, source]() { widget->setText(translated(source)); });
  return widget;
}

AdResult* ResultDocsPage::result(AdResult::Status status, const char* title, const char* subTitle,
                                 const char* objectName) {
  auto* widget = new AdResult();
  widget->setObjectName(QString::fromLatin1(objectName));
  widget->setStatus(status);
  const auto refresh = [widget, title, subTitle]() {
    widget->setTitle(translated(title));
    widget->setSubTitle(translated(subTitle));
  };
  refresh();
  translations_.append(refresh);
  return widget;
}

QWidget* ResultDocsPage::actions(const char* primary, const char* secondary, bool vertical) {
  auto* widget = new QWidget();
  QBoxLayout* layout = vertical ? static_cast<QBoxLayout*>(new QVBoxLayout(widget))
                                : static_cast<QBoxLayout*>(new QHBoxLayout(widget));
  layout->setContentsMargins(0, 0, 0, 0);
  layout->setSpacing(8);
  for (const char* source : {primary, secondary}) {
    if (!source) {
      continue;
    }
    auto* button = new AdButton(translated(source));
    if (source == primary) {
      button->setButtonStyle(AdButton::ButtonStyle::Solid);
      button->setAccentRole(AdButton::AccentRole::Primary);
    }
    translations_.append([button, source]() { button->setText(translated(source)); });
    layout->addWidget(button);
  }
  return widget;
}

QWidget* ResultDocsPage::buildSuccessDemo() {
  auto* widget = result(
      AdResult::Status::Success,
      QT_TRANSLATE_NOOP("ResultDocsPage", "Your changes have been saved"),
      QT_TRANSLATE_NOOP("ResultDocsPage", "Everything is up to date. You can continue working."),
      "resultSuccess");
  QWidget* extra = actions(QT_TRANSLATE_NOOP("ResultDocsPage", "Run again"));
  widget->setExtraWidget(extra);
  auto* repeat = extra->findChild<AdButton*>();
  repeat->setObjectName(QStringLiteral("resultRepeatButton"));
  const auto refresh = [widget]() {
    if (widget->property("operationRepeated").toBool()) {
      widget->setSubTitle(translated(
          QT_TRANSLATE_NOOP("ResultDocsPage", "The operation was completed again successfully.")));
    }
  };
  translations_.append(refresh);
  connect(repeat, &QAbstractButton::clicked, widget, [widget, refresh]() {
    widget->setProperty("operationRepeated", true);
    refresh();
  });
  return widget;
}

QWidget* ResultDocsPage::buildErrorDemo() {
  auto* widget = result(
      AdResult::Status::Error, QT_TRANSLATE_NOOP("ResultDocsPage", "Submission failed"),
      QT_TRANSLATE_NOOP("ResultDocsPage", "Please check the following information and try again."),
      "resultError");
  widget->setExtraWidget(actions(QT_TRANSLATE_NOOP("ResultDocsPage", "Try again"),
                                 QT_TRANSLATE_NOOP("ResultDocsPage", "Cancel")));
  QWidget* details = column();
  auto* layout = static_cast<QVBoxLayout*>(details->layout());
  auto* heading =
      label(QT_TRANSLATE_NOOP("ResultDocsPage", "The submitted information has these problems:"));
  QFont headingFont = heading->font();
  headingFont.setBold(true);
  heading->setFont(headingFont);
  layout->addWidget(heading);
  layout->addWidget(
      label(QT_TRANSLATE_NOOP("ResultDocsPage", "The account name is already in use.")));
  layout->addWidget(
      label(QT_TRANSLATE_NOOP("ResultDocsPage", "The selected plan is no longer available.")));
  widget->setContentWidget(details);
  return widget;
}

QWidget* ResultDocsPage::buildCustomIconDemo() {
  auto* widget =
      result(AdResult::Status::Info, QT_TRANSLATE_NOOP("ResultDocsPage", "Welcome back"),
             QT_TRANSLATE_NOOP("ResultDocsPage",
                               "Choose a custom symbol or restore the default information icon."),
             "resultCustomIcon");
  const QIcon customIcon = adqt::icons::makeIcon(outlined_icons::Smile());
  widget->setIcon(customIcon);
  auto* extra = new QWidget();
  auto* layout = new QHBoxLayout(extra);
  layout->setContentsMargins(0, 0, 0, 0);
  auto* toggle = new AdButton();
  toggle->setObjectName(QStringLiteral("resultDefaultIconButton"));
  toggle->setCheckable(true);
  layout->addWidget(toggle);
  const auto refresh = [toggle]() {
    toggle->setText(translated(toggle->isChecked()
                                   ? QT_TRANSLATE_NOOP("ResultDocsPage", "Use custom icon")
                                   : QT_TRANSLATE_NOOP("ResultDocsPage", "Use default icon")));
  };
  refresh();
  translations_.append(refresh);
  connect(toggle, &QAbstractButton::toggled, widget,
          [widget, customIcon, refresh](bool useDefault) {
            widget->setIcon(useDefault ? QIcon() : customIcon);
            refresh();
          });
  widget->setExtraWidget(extra);
  return widget;
}

QWidget* ResultDocsPage::buildCustomWidgetsDemo() {
  auto* widget = new AdResult();
  widget->setObjectName(QStringLiteral("resultCustomWidgets"));
  widget->setStatus(AdResult::Status::Success);
  auto* badge = label(QT_TRANSLATE_NOOP("ResultDocsPage", "Ready"));
  badge->setAlignment(Qt::AlignCenter);
  badge->setMargin(12);
  QFont badgeFont = badge->font();
  badgeFont.setPixelSize(24);
  badgeFont.setBold(true);
  badge->setFont(badgeFont);
  demo::bindThemeRefresh(badge, [badge]() {
    const auto& colors = demo::resolve(badge).theme.palette;
    QPalette palette = badge->palette();
    palette.setColor(QPalette::WindowText, colors.colorSuccess);
    badge->setPalette(palette);
  });
  widget->setIconWidget(badge);
  auto* title = label(QT_TRANSLATE_NOOP("ResultDocsPage", "Everything is ready"));
  title->setAlignment(Qt::AlignCenter);
  QFont titleFont = title->font();
  titleFont.setPixelSize(24);
  title->setFont(titleFont);
  widget->setTitleWidget(title);
  auto* subTitle =
      label(QT_TRANSLATE_NOOP(
                "ResultDocsPage",
                "Native widgets preserve keyboard navigation, selection, and accessibility."),
            true);
  subTitle->setAlignment(Qt::AlignCenter);
  widget->setSubTitleWidget(subTitle);
  auto* body = label(QT_TRANSLATE_NOOP(
      "ResultDocsPage",
      "This selectable body text is an ordinary QLabel supplied by the application."));
  body->setTextInteractionFlags(Qt::TextSelectableByMouse | Qt::TextSelectableByKeyboard);
  widget->setContentWidget(body);
  widget->setExtraWidget(actions(QT_TRANSLATE_NOOP("ResultDocsPage", "Continue")));
  return widget;
}

QWidget* ResultDocsPage::buildTokenDemo() {
  auto* widget = result(
      AdResult::Status::Success, QT_TRANSLATE_NOOP("ResultDocsPage", "A compact confirmation"),
      QT_TRANSLATE_NOOP("ResultDocsPage",
                        "Smaller type and a 48-pixel icon keep this result compact."),
      "resultTokens");
  AdResult::ComponentTokens tokens;
  tokens.titleFontSize = 20;
  tokens.subtitleFontSize = 13;
  tokens.iconFontSize = 48;
  tokens.extraMargin = QMargins(0, 16, 0, 0);
  widget->setComponentTokens(tokens);
  widget->setExtraWidget(actions(QT_TRANSLATE_NOOP("ResultDocsPage", "Continue")));
  return widget;
}

QWidget* ResultDocsPage::buildSemanticDemo() {
  auto* widget = result(
      AdResult::Status::Info, QT_TRANSLATE_NOOP("ResultDocsPage", "Styled for your workspace"),
      QT_TRANSLATE_NOOP("ResultDocsPage",
                        "Semantic styles adapt this result to the current theme."),
      "resultSemantic");
  widget->setExtraWidget(actions(QT_TRANSLATE_NOOP("ResultDocsPage", "Continue")));
  widget->setContentWidget(label(
      QT_TRANSLATE_NOOP("ResultDocsPage", "The body and title use separate semantic styles.")));
  demo::bindThemeRefresh(widget, [widget]() {
    const auto& colors = demo::resolve(widget).theme.palette;
    AdResult::SemanticStyles styles;
    styles.root.backgroundColor = colors.colorInfoBg;
    styles.title.textColor = colors.colorPrimary;
    styles.icon.textColor = colors.colorPrimary;
    styles.body.backgroundColor = colors.colorBgContainer;
    widget->setSemanticStyles(styles);
  });
  return widget;
}

QWidget* ResultDocsPage::buildNarrowDemo() {
  auto* box = new QWidget();
  auto* layout = new QHBoxLayout(box);
  layout->setContentsMargins(0, 0, 0, 0);
  layout->setSpacing(0);
  auto* widget =
      result(AdResult::Status::Warning,
             QT_TRANSLATE_NOOP("ResultDocsPage", "A few details still need your review"),
             QT_TRANSLATE_NOOP(
                 "ResultDocsPage",
                 "This result is limited to 320 logical pixels so longer text wraps naturally."),
             "resultNarrow");
  widget->setMaximumWidth(320);
  widget->setExtraWidget(actions(QT_TRANSLATE_NOOP("ResultDocsPage", "Review details"),
                                 QT_TRANSLATE_NOOP("ResultDocsPage", "Do this later"), true));
  // A horizontal row measures height using the constrained child width before
  // the surrounding column assigns the panel's height.
  layout->addStretch();
  layout->addWidget(widget);
  layout->addStretch();
  return box;
}
