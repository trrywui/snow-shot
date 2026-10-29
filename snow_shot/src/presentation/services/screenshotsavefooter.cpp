#include "screenshotsavefooter.h"

#include <QStackedLayout>
#include <algorithm>

namespace {
class FooterPages final : public QStackedLayout {
  public:
    explicit FooterPages(QWidget* parent) : QStackedLayout(parent) {
        setContentsMargins(0, 0, 0, 0);
    }
    bool hasHeightForWidth() const override {
        return true;
    }
    int heightForWidth(int width) const override {
        int height = 0;
        for (int index = 0; index < count(); ++index) {
            // Query each page's layout directly: a hidden QWidgetItem has an empty
            // size hint, but its children must still participate in reservation.
            const auto* page = widget(index);
            const auto* pageLayout = page->layout();
            height = std::max(height, pageLayout->hasHeightForWidth()
                                          ? pageLayout->totalHeightForWidth(width)
                                          : pageLayout->totalSizeHint().height());
        }
        return height;
    }
    int minimumHeightForWidth(int width) const override {
        return heightForWidth(width);
    }
};
} // namespace

ScreenshotSaveFooter::ScreenshotSaveFooter(QWidget* editing, QWidget* result)
    : m_pages(new FooterPages(this)) {
    setObjectName(QStringLiteral("saveDialogFooter"));
    setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Minimum);
    m_pages->addWidget(editing);
    if (result)
        m_pages->addWidget(result);
}

void ScreenshotSaveFooter::setResultVisible(bool visible) {
    m_pages->setCurrentIndex(visible ? 1 : 0);
}
