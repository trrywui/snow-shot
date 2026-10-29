#include "widgets/result.h"
#include "theme/theme_manager.h"

#include <QApplication>
#include <QDir>
#include <QElapsedTimer>
#include <QImage>
#include <QPainter>

#include <array>
#include <iomanip>
#include <iostream>
#include <memory>
#include <vector>

namespace {

using adqt::widgets::AdResult;

constexpr int widgetCount = 24;
constexpr int paintIterations = 20;
constexpr int textIterations = 80;
constexpr std::array statuses = {AdResult::Status::Success,    AdResult::Status::Error,
                                 AdResult::Status::Info,       AdResult::Status::Warning,
                                 AdResult::Status::Forbidden,  AdResult::Status::NotFound,
                                 AdResult::Status::ServerError};

const char* statusName(AdResult::Status status) {
  switch (status) {
    case AdResult::Status::Success:
      return "success";
    case AdResult::Status::Error:
      return "error";
    case AdResult::Status::Info:
      return "info";
    case AdResult::Status::Warning:
      return "warning";
    case AdResult::Status::Forbidden:
      return "forbidden";
    case AdResult::Status::NotFound:
      return "not-found";
    case AdResult::Status::ServerError:
      return "server-error";
  }
  return "unknown";
}

const std::array titles = {QStringLiteral("Your changes have been saved"),
                           QStringLiteral("Your document is ready to share")};
const std::array subtitles = {
    QStringLiteral("The updated document is available in your library. You can continue editing "
                   "or return to the project to share your work with the rest of your team."),
    QStringLiteral("All changes are up to date. Choose your next step when you are ready.")};

void configure(AdResult& result, AdResult::Status status) {
  result.setStatus(status);
  result.setTitle(titles.front());
  result.setSubTitle(subtitles.front());
  result.resize(440, 560);
  result.ensurePolished();
}

quint64 rasterSample(const QImage& buffer) {
  quint64 checksum = 0;
  for (int row = 0; row < buffer.height(); row += 17) {
    const auto* pixels = reinterpret_cast<const QRgb*>(buffer.constScanLine(row));
    for (int column = 0; column < buffer.width(); column += 19) {
      checksum = checksum * 31 + pixels[column];
    }
  }
  return checksum;
}

void report(AdResult::Status status, const char* operation, qreal dpr, int iterations,
            qint64 elapsedNs, quint64 checksum) {
  std::cout << statusName(status) << ',' << operation << ',' << dpr << ',' << widgetCount << ','
            << iterations << ','
            << static_cast<qreal>(elapsedNs) / (1000.0 * widgetCount * iterations) << ','
            << checksum << '\n';
}

void run(AdResult::Status status, qreal dpr) {
  // Populate font and artwork caches before measuring the reusable widget path.
  AdResult warmup;
  configure(warmup, status);
  QImage buffer(QSize(qCeil(warmup.width() * dpr), qCeil(warmup.height() * dpr)),
                QImage::Format_ARGB32_Premultiplied);
  buffer.setDevicePixelRatio(dpr);
  buffer.fill(Qt::transparent);
  {
    QPainter painter(&buffer);
    warmup.render(&painter);
  }

  QWidget parent;
  std::vector<std::unique_ptr<AdResult>> widgets;
  widgets.reserve(widgetCount);
  QElapsedTimer timer;
  timer.start();
  for (int index = 0; index < widgetCount; ++index) {
    auto result = std::make_unique<AdResult>(&parent);
    configure(*result, status);
    widgets.push_back(std::move(result));
  }
  const qint64 constructionNs = timer.nsecsElapsed();
  report(status, "construct-setup", dpr, 1, constructionNs,
         static_cast<quint64>(widgets.front()->sizeHint().height()));

  quint64 heights = 0;
  timer.restart();
  for (int iteration = 0; iteration < textIterations; ++iteration) {
    for (int index = 0; index < widgetCount; ++index) {
      AdResult& result = *widgets[static_cast<std::size_t>(index)];
      const auto alternate = static_cast<std::size_t>((iteration + index) % 2);
      result.setTitle(titles[alternate]);
      result.setSubTitle(subtitles[alternate]);
      heights += static_cast<quint64>(result.heightForWidth(280 + (index % 3) * 120));
    }
  }
  const qint64 layoutNs = timer.nsecsElapsed();
  report(status, "text-layout", dpr, textIterations, layoutNs, heights);

  // One painter and image serve the full batch so the timings measure widget
  // painting without image allocation, encoding, or filesystem work.
  QPainter painter(&buffer);
  for (auto& result : widgets) result->render(&painter);
  for (const bool update : {false, true}) {
    timer.restart();
    for (int iteration = 0; iteration < paintIterations; ++iteration) {
      for (int index = 0; index < widgetCount; ++index) {
        AdResult& result = *widgets[static_cast<std::size_t>(index)];
        if (update) {
          const auto alternate = static_cast<std::size_t>((iteration + index) % 2);
          result.setTitle(titles[alternate]);
          result.setSubTitle(subtitles[alternate]);
        }
        result.render(&painter);
      }
    }
    const qint64 paintNs = timer.nsecsElapsed();
    report(status, update ? "update-paint" : "steady-paint", dpr, paintIterations, paintNs,
           rasterSample(buffer));
  }
  painter.end();
}

}  // namespace

int main(int argc, char** argv) {
#ifndef ADQT_RESULT_BENCHMARK_RELEASE
  Q_UNUSED(argc)
  Q_UNUSED(argv)
  std::cerr << "Result performance measurements require the Release configuration\n";
  return 1;
#else
#ifdef Q_OS_WIN
  if (qEnvironmentVariableIsEmpty("QT_QPA_FONTDIR")) {
    const QString fonts = QDir(qEnvironmentVariable("WINDIR")).filePath(QStringLiteral("Fonts"));
    if (QDir(fonts).exists()) qputenv("QT_QPA_FONTDIR", fonts.toUtf8());
  }
#endif
  QApplication app(argc, argv);
  adqt::theme::ThemeManager::instance().applyTo(app);
  std::cout << std::fixed << std::setprecision(3)
            << "status,operation,dpr,widgets,iterations,us_per_widget,sample_checksum\n";
  for (qreal dpr : {1.25, 2.0}) {
    for (AdResult::Status status : statuses) run(status, dpr);
  }
  return 0;
#endif
}
