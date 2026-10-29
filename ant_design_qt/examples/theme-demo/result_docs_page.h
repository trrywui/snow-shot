#pragma once

#include <QStringList>
#include <QVector>
#include <QWidget>

#include <functional>

#include "widgets/result.h"

class QLabel;
class QVBoxLayout;

class ResultDocsPage final : public QWidget {
 public:
  explicit ResultDocsPage(QWidget* parent = nullptr);

  static const QStringList& defaultSectionTitles();
  const QVector<QWidget*>& sectionAnchors() const;
  const QStringList& sectionTitles() const;

 protected:
  void changeEvent(QEvent* event) override;

 private:
  void addSection(QVBoxLayout* root, int index, QWidget* content);
  QLabel* label(const char* source, bool hint = false);
  adqt::widgets::AdResult* result(adqt::widgets::AdResult::Status status, const char* title,
                                  const char* subTitle, const char* objectName);
  QWidget* actions(const char* primary, const char* secondary = nullptr, bool vertical = false);
  QWidget* buildSuccessDemo();
  QWidget* buildErrorDemo();
  QWidget* buildCustomIconDemo();
  QWidget* buildCustomWidgetsDemo();
  QWidget* buildTokenDemo();
  QWidget* buildSemanticDemo();
  QWidget* buildNarrowDemo();

  QVector<QWidget*> anchors_;
  QStringList titles_;
  QVector<std::function<void()>> translations_;
};
