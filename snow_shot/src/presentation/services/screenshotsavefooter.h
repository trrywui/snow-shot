#pragma once

#include <QWidget>

class QStackedLayout;

// Reserves the maximum height of both pages at the available width, including
// the hidden page. Page switches therefore cannot move the dialog or preview.
class ScreenshotSaveFooter final : public QWidget {
  public:
    ScreenshotSaveFooter(QWidget* editing, QWidget* result);
    void setResultVisible(bool visible);

  private:
    QStackedLayout* m_pages;
};
