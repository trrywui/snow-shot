#include <QApplication>
#include <QDir>
#include <QImage>
#include <QLabel>
#include <QLayout>
#include <QPainter>
#include <QSet>
#include <QStyleFactory>
#include <QTranslator>
#include <QVariant>

#include <array>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <utility>

#include "icon_theme_adapter.h"
#include "result_docs_page.h"
#include "theme/theme_manager.h"
#include "widgets/button.h"

using adqt::theme::ThemeManager;
using adqt::theme::ThemeScheme;
using adqt::widgets::AdButton;
using adqt::widgets::AdResult;

namespace {

void require(bool condition, const char* message) {
  if (!condition) {
    std::cerr << "FAILED: " << message << '\n';
    std::exit(1);
  }
}

template <typename T>
T* namedChild(ResultDocsPage& page, const char* name) {
  auto* widget = page.findChild<T*>(QString::fromLatin1(name));
  require(widget != nullptr, "Result examples must expose named widgets");
  return widget;
}

void sectionNavigation(ResultDocsPage& page) {
  require(page.sectionAnchors().size() == 12, "Result documentation must expose twelve sections");
  require(page.sectionTitles() == ResultDocsPage::defaultSectionTitles(),
          "lazy Result navigation titles must match the instantiated page");
  QSet<QWidget*> unique;
  for (qsizetype index = 0; index < page.sectionAnchors().size(); ++index) {
    auto* anchor = page.sectionAnchors().at(index);
    require(anchor && page.isAncestorOf(anchor), "Result anchors must belong to their page");
    require(anchor->objectName() == QStringLiteral("resultSection%1").arg(index),
            "Result sections must retain stable identities");
    require(!page.sectionTitles().at(index).isEmpty(), "Result navigation titles must be nonempty");
    unique.insert(anchor);
  }
  require(unique.size() == 12, "Result sections must have unique anchors");
}

void statusExamples(ResultDocsPage& page) {
  const std::array<std::pair<const char*, AdResult::Status>, 7> examples = {{
      {"resultSuccess", AdResult::Status::Success},
      {"resultInfo", AdResult::Status::Info},
      {"resultWarning", AdResult::Status::Warning},
      {"resultError", AdResult::Status::Error},
      {"resultForbidden", AdResult::Status::Forbidden},
      {"resultNotFound", AdResult::Status::NotFound},
      {"resultServerError", AdResult::Status::ServerError},
  }};
  for (const auto& [name, status] : examples) {
    auto* widget = namedChild<AdResult>(page, name);
    require(widget->status() == status, "the Result demo must include every status");
    require(!widget->title().isEmpty() && !widget->subTitle().isEmpty(),
            "status examples must have both a title and an explanation");
  }
  require(namedChild<AdResult>(page, "resultError")->contentWidget() != nullptr,
          "the error example must include a detail panel");
  auto* custom = namedChild<AdResult>(page, "resultCustomWidgets");
  require(custom->iconWidget() && custom->titleWidget() && custom->subTitleWidget() &&
              custom->extraWidget() && custom->contentWidget(),
          "the custom example must demonstrate every native widget slot");
  auto* body = qobject_cast<QLabel*>(custom->contentWidget());
  require(body && body->textInteractionFlags().testFlag(Qt::TextSelectableByKeyboard),
          "custom body content must retain native text selection");
  const auto tokens = namedChild<AdResult>(page, "resultTokens")->componentTokens();
  require(tokens.titleFontSize == 20 && tokens.subtitleFontSize == 13 &&
              tokens.iconFontSize == 48 && tokens.extraMargin == QMargins(0, 16, 0, 0),
          "the token example must demonstrate all four component tokens");
}

class TestTranslator final : public QTranslator {
 public:
  bool isEmpty() const override { return false; }

  QString translate(const char* context, const char* source, const char*, int) const override {
    if (std::strcmp(context, "ResultDocsPage") == 0) {
      return QStringLiteral("translated: %1").arg(QString::fromUtf8(source));
    }
    return {};
  }
};

void interactionsAndLanguageChange(QApplication& app, ResultDocsPage& page) {
  auto* success = namedChild<AdResult>(page, "resultSuccess");
  auto* repeat = namedChild<AdButton>(page, "resultRepeatButton");
  const QString initialSubtitle = success->subTitle();
  repeat->click();
  require(success->subTitle() != initialSubtitle && success->property("operationRepeated").toBool(),
          "the repeat action must update the success explanation");
  auto* custom = namedChild<AdResult>(page, "resultCustomIcon");
  auto* toggle = namedChild<AdButton>(page, "resultDefaultIconButton");
  require(!custom->icon().isNull() && !toggle->isChecked(),
          "the custom icon example must initially show its QIcon");
  toggle->click();
  require(custom->icon().isNull() && toggle->isChecked(),
          "the default icon action must restore the status icon");

  TestTranslator translator;
  app.installTranslator(&translator);
  QApplication::processEvents();
  require(page.sectionTitles().first().startsWith(QStringLiteral("translated: ")),
          "a language change must refresh Result section titles");
  require(page.sectionTitles() == ResultDocsPage::defaultSectionTitles(),
          "translated lazy navigation must match the displayed section titles");
  require(success->subTitle().startsWith(QStringLiteral("translated: ")) &&
              success->subTitle().contains(QStringLiteral("completed again")),
          "a language change must translate and preserve the repeat action state");
  require(toggle->isChecked() && custom->icon().isNull() &&
              toggle->text().startsWith(QStringLiteral("translated: ")),
          "a language change must preserve the icon toggle and translate its current action");
  app.removeTranslator(&translator);
  QApplication::processEvents();
  toggle->click();
  require(!custom->icon().isNull() && !toggle->isChecked(),
          "the custom icon can be restored after a language change");
}

void layOutPage(ResultDocsPage& page, int width) {
  page.resize(width, 600);
  page.show();
  QApplication::processEvents();
  const int height = page.layout()->heightForWidth(width);
  page.resize(width, qMax(height, page.layout()->sizeHint().height()));
  page.layout()->activate();
  QApplication::processEvents();
  require(page.width() == width, "the Result documentation must fit its requested viewport width");
}

void themeAndNarrowLayout(ResultDocsPage& page) {
  auto* semantic = namedChild<AdResult>(page, "resultSemantic");
  ThemeManager::instance().setColorScheme(ThemeScheme::Light);
  const auto lightBackground = semantic->semanticStyles().root.backgroundColor;
  ThemeManager::instance().setColorScheme(ThemeScheme::Dark);
  require(semantic->semanticStyles().root.backgroundColor != lightBackground,
          "semantic example colors must follow a change of theme");
  auto* narrow = namedChild<AdResult>(page, "resultNarrow");
  for (const int width : {760, 360}) {
    layOutPage(page, width);
    auto* custom = namedChild<AdResult>(page, "resultCustomIcon");
    auto* icon = custom->findChild<QWidget*>(QStringLiteral("adqt-result-icon"));
    require(icon, "the custom Result icon must retain its artwork surface");
    QImage image(icon->size(), QImage::Format_RGB32);
    image.fill(page.palette().color(QPalette::Window));
    {
      QPainter painter(&image);
      icon->render(&painter);
    }
    int contrastingPixels = 0;
    for (int y = 0; y < image.height(); ++y) {
      for (int x = 0; x < image.width(); ++x) {
        if (qGray(image.pixel(x, y)) > 100) {
          ++contrastingPixels;
        }
      }
    }
    require(contrastingPixels > 40,
            "the custom Smile icon must remain visible against a dark theme background");
    require(narrow->width() <= 320, "the narrow example must retain its 320-pixel width limit");
    for (auto* widget : page.findChildren<AdResult*>()) {
      const QPoint origin = widget->mapTo(&page, QPoint());
      require(origin.x() >= 0 && origin.x() + widget->width() <= page.width(),
              "Result examples must fit inside wide and narrow documentation layouts");
      for (const char* name : {"adqt-result-title", "adqt-result-subtitle"}) {
        auto* text = widget->findChild<QLabel*>(QString::fromLatin1(name));
        if (text && text->isVisibleTo(&page)) {
          if (text->height() < text->heightForWidth(text->width())) {
            std::cerr << "page width=" << width << " actual=" << page.width() << 'x'
                      << page.height() << " page_hfw=" << page.layout()->heightForWidth(width)
                      << " result=" << widget->objectName().toStdString()
                      << " size=" << widget->width() << 'x' << widget->height()
                      << " result_hfw=" << widget->heightForWidth(widget->width())
                      << " label=" << text->objectName().toStdString() << " size=" << text->width()
                      << 'x' << text->height() << " needed=" << text->heightForWidth(text->width())
                      << '\n';
          }
          require(text->height() >= text->heightForWidth(text->width()),
                  "Result example text must receive its required wrapped height");
        }
      }
    }
  }
  require(narrow->heightForWidth(250) >= narrow->heightForWidth(320),
          "narrower Result text must receive enough vertical space");
  ThemeManager::instance().setColorScheme(ThemeScheme::Light);
  page.hide();
}

void renderPage(ResultDocsPage& page, const QString& directory, ThemeScheme scheme, int width) {
  ThemeManager::instance().setColorScheme(scheme);
  layOutPage(page, width);
  QImage image(page.size(), QImage::Format_ARGB32_Premultiplied);
  image.fill(page.palette().color(QPalette::Window));
  QPainter painter(&image);
  page.render(&painter);
  painter.end();
  const QString name =
      QStringLiteral("result-demo-%1-%2.png")
          .arg(scheme == ThemeScheme::Light ? QStringLiteral("light") : QStringLiteral("dark"))
          .arg(width);
  require(image.save(QDir(directory).filePath(name)), "the Result demo screenshot must save");
}

}  // namespace

int main(int argc, char** argv) {
#ifdef Q_OS_WIN
  if (qEnvironmentVariableIsEmpty("QT_QPA_FONTDIR")) {
    const QString fonts = QDir(qEnvironmentVariable("WINDIR")).filePath(QStringLiteral("Fonts"));
    if (QDir(fonts).exists()) {
      qputenv("QT_QPA_FONTDIR", fonts.toUtf8());
    }
  }
#endif
  QApplication app(argc, argv);
  if (QStyleFactory::keys().contains(QStringLiteral("Fusion"), Qt::CaseInsensitive)) {
    app.setStyle(QStyleFactory::create(QStringLiteral("Fusion")));
  }
  ThemeManager::instance().applyTo(app);
  demo::installIconThemeResolver();
  ResultDocsPage page;
  sectionNavigation(page);
  statusExamples(page);
  interactionsAndLanguageChange(app, page);
  themeAndNarrowLayout(page);

  const QStringList arguments = app.arguments();
  const qsizetype renderIndex = arguments.indexOf(QStringLiteral("--render-dir"));
  if (renderIndex >= 0) {
    require(renderIndex + 1 < arguments.size(), "--render-dir requires a directory path");
    const QString directory = arguments.at(renderIndex + 1);
    require(QDir().mkpath(directory), "the Result demo screenshot directory must be created");
    for (const auto scheme : {ThemeScheme::Light, ThemeScheme::Dark}) {
      for (const int width : {760, 360}) {
        renderPage(page, directory, scheme, width);
      }
    }
  }
  std::cout << "Result demo tests passed\n";
  return 0;
}
