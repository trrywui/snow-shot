#include "widgets/detail/timing_hub.h"
#include <QCoreApplication>
#include <QEventLoop>
#include <QPointer>
#include <QTimer>
#include <QThread>
#include <cstdlib>
#include <iostream>
#include <limits>
#include <memory>
#include <vector>

using namespace adqt::widgets::detail;
static void require(bool value, const char* message) {
  if (!value) {
    std::cerr << message << '\n';
    std::exit(1);
  }
}
static void dueTasksRespectOrderingAndCancellation() {
  for (int scenario = 0; scenario < 4; ++scenario) {
    QObject first;
    auto* second = new QObject;
    std::vector<int> order;
    QEventLoop loop;
    scheduleTimingTask(&first, "first", 0, [&]() {
      order.push_back(1);
      if (scenario == 0) {
        delete second;
        second = nullptr;
      }
      if (scenario == 1) {
        cancelTimingTask(second, "second");
      }
      if (scenario == 2) {
        scheduleTimingTask(second, "second", 0, [&]() { order.push_back(4); });
      }
      scheduleTimingTask(&first, "next", 0, [&]() {
        order.push_back(5);
        loop.quit();
      });
    });
    scheduleTimingTask(second, "second", 0, [&]() { order.push_back(2); });
    scheduleTimingTask(&first, "third", 0, [&]() { order.push_back(3); });
    QTimer::singleShot(5000, &loop, &QEventLoop::quit);
    QThread::msleep(10);  // All original deadlines must be due in the same dispatch.
    loop.exec();
    const std::vector<int> expected = scenario == 2   ? std::vector<int>{1, 3, 4, 5}
                                      : scenario == 3 ? std::vector<int>{1, 2, 3, 5}
                                                      : std::vector<int>{1, 3, 5};
    require(order == expected, "due tasks must respect ordering, cancellation and lifetime");
    delete second;
  }
}

int main(int argc, char** argv) {
  std::unique_ptr<QObject> context;
  QPointer<QObject> deadlineCapture;
  QPointer<QObject> frameCapture;
  {
    QCoreApplication app(argc, argv);
    dueTasksRespectOrderingAndCancellation();
    // Process-owned contexts may outlive the application. Its hub must still release pending work
    // and active timer subscriptions before Qt tears down the main thread's event dispatcher.
    context = std::make_unique<QObject>();
    auto deadlineLifetime = std::make_shared<QObject>();
    deadlineCapture = deadlineLifetime.get();
    scheduleTimingTask(context.get(), QStringLiteral("application-shutdown"),
                       std::numeric_limits<int>::max(),
                       [deadlineLifetime]() { Q_UNUSED(deadlineLifetime); });
    auto frameLifetime = std::make_shared<QObject>();
    frameCapture = frameLifetime.get();
    setFrameSubscription(context.get(), QStringLiteral("application-shutdown"), true,
                         [frameLifetime](qint64, qint64) { Q_UNUSED(frameLifetime); });
  }
  require(deadlineCapture.isNull() && frameCapture.isNull(),
          "application teardown must release pending timing callbacks and frame subscriptions");
  // Owners can perform their ordinary cancellation after the application has released the hub.
  cancelTimingTask(context.get(), QStringLiteral("application-shutdown"));
  clearFrameSubscription(context.get(), QStringLiteral("application-shutdown"));
  setFrameSubscription(context.get(), QStringLiteral("application-shutdown"), false, {});
  context.reset();
  {
    QCoreApplication app(argc, argv);
    dueTasksRespectOrderingAndCancellation();
  }
  return 0;
}
