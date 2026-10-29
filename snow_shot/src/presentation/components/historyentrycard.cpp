#include "snow_shot/presentation/components/historyentrycard.h"

#include "snow_shot/presentation/components/historypagecommon.h"
#include "snow_shot/presentation/styles/mainwindowcomponenttoken.h"

#include "widgets/button.h"
#include "widgets/checkbox.h"
#include "widgets/detail/button_rendering.h"

#include <QBoxLayout>
#include <QEvent>
#include <QLabel>
#include <QPainter>

#include <algorithm>

namespace snow_shot::presentation::components {

namespace {
constexpr int kHorizontalPadding = 18;
constexpr int kVerticalPadding = 16;
} // namespace

HistoryEntryCard::HistoryEntryCard(QWidget* parent) : QFrame(parent) {
    setFrameShape(QFrame::NoFrame);
    setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Minimum);
    m_layout = new QBoxLayout(QBoxLayout::LeftToRight, this);
    m_layout->setContentsMargins(kHorizontalPadding, kVerticalPadding, kHorizontalPadding,
                                 kVerticalPadding);
    m_layout->setSpacing(18);

    m_details = new QWidget(this);
    m_details->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Preferred);
    auto* detailsLayout = new QVBoxLayout(m_details);
    detailsLayout->setContentsMargins(0, 0, 0, 0);
    detailsLayout->setSpacing(9);
    auto* topRow = new QHBoxLayout;
    topRow->setContentsMargins(0, 0, 0, 0);
    topRow->setSpacing(8);
    m_selectionCheckbox = new adqt::widgets::AdCheckbox(m_details);
    topRow->addWidget(m_selectionCheckbox, 0, Qt::AlignVCenter);
    topRow->addStretch(1);
    detailsLayout->addLayout(topRow);
    m_sourceLabel = new QLabel(m_details);
    detailsLayout->addWidget(m_sourceLabel, 0, Qt::AlignLeft);
    m_summaryLabel = new QLabel(m_details);
    detailsLayout->addWidget(m_summaryLabel);
    m_metadataLabel = new QLabel(m_details);
    detailsLayout->addWidget(m_metadataLabel);
    detailsLayout->addStretch(1);

    m_actions = new QHBoxLayout;
    m_actions->setContentsMargins(0, 0, 0, 0);
    m_actions->setSpacing(6);
    m_actions->addStretch(1);
    detailsLayout->addLayout(m_actions);
    m_layout->addWidget(m_details, 1);
    if (parent != nullptr) {
        parent->installEventFilter(this);
    }
}

adqt::widgets::AdCheckbox* HistoryEntryCard::selectionCheckbox() const {
    return m_selectionCheckbox;
}

QLabel* HistoryEntryCard::sourceLabel() const {
    return m_sourceLabel;
}

QLabel* HistoryEntryCard::summaryLabel() const {
    return m_summaryLabel;
}

QLabel* HistoryEntryCard::metadataLabel() const {
    return m_metadataLabel;
}

QWidget* HistoryEntryCard::detailsWidget() const {
    return m_details;
}

int HistoryEntryCard::minimumHeightForWidth(int availableWidth) {
    return kPreviewHeight +
           (availableWidth >= history_page::kWideEntryBreakpoint ? 2 * kVerticalPadding : 190);
}

void HistoryEntryCard::addAction(adqt::widgets::AdButton* button) {
    m_actions->insertWidget(m_actions->count() - 1, button);
}

void HistoryEntryCard::setPreviewWidget(QWidget* preview) {
    m_preview = preview;
    m_layout->addWidget(preview, 0, Qt::AlignCenter);
    updateResponsiveLayout();
}

void HistoryEntryCard::applyTheme(const styles::ThemeColorScheme& scheme) {
    m_scheme = scheme;
    QPalette primary = m_selectionCheckbox->palette();
    primary.setColor(QPalette::WindowText, scheme.map.colorText);
    m_selectionCheckbox->setPalette(primary);
    QFont dateFont = m_selectionCheckbox->font();
    dateFont.setPixelSize(scheme.metricAlias.fontSizeLG);
    dateFont.setWeight(QFont::DemiBold);
    m_selectionCheckbox->setFont(dateFont);
    for (QLabel* label : {m_sourceLabel, m_summaryLabel, m_metadataLabel}) {
        history_page::applyEntryDescriptionFont(label, scheme);
    }
    QPalette secondary = m_summaryLabel->palette();
    secondary.setColor(QPalette::WindowText, scheme.map.colorTextSecondary);
    m_summaryLabel->setPalette(secondary);
    QPalette tertiary = m_metadataLabel->palette();
    tertiary.setColor(QPalette::WindowText, scheme.map.colorTextTertiary);
    m_metadataLabel->setPalette(tertiary);
    updateBackgroundStyle();
}

void HistoryEntryCard::updateBackgroundStyle() {
    m_sourceLabel->setStyleSheet(
        QStringLiteral("QLabel { color: %1; background: %2; border: 1px solid %3; "
                       "border-radius: 4px; padding: 2px 7px; }")
            .arg(m_scheme.map.colorPrimaryText.name(),
                 styles::mainWindowBackgroundColor(m_sourceLabel, m_scheme.map.colorPrimaryBg)
                     .name(QColor::HexArgb),
                 m_scheme.map.colorPrimaryBorder.name()));
    update();
}

bool HistoryEntryCard::eventFilter(QObject* watched, QEvent* event) {
    if (watched == parentWidget() && event->type() == QEvent::Resize) {
        updateResponsiveLayout();
    }
    return QFrame::eventFilter(watched, event);
}

void HistoryEntryCard::resizeEvent(QResizeEvent* event) {
    QFrame::resizeEvent(event);
    updateResponsiveLayout();
}

void HistoryEntryCard::updateResponsiveLayout() {
    const int availableWidth = parentWidget() != nullptr ? parentWidget()->width() : width();
    const bool wide = availableWidth >= history_page::kWideEntryBreakpoint;
    m_layout->setDirection(wide ? QBoxLayout::LeftToRight : QBoxLayout::TopToBottom);
    setMinimumHeight(minimumHeightForWidth(availableWidth));
    if (m_preview != nullptr) {
        m_preview->setFixedSize(wide ? kPreviewWidth
                                     : std::max(220, availableWidth - 2 * kHorizontalPadding),
                                kPreviewHeight);
    }
}

void HistoryEntryCard::paintEvent(QPaintEvent* event) {
    Q_UNUSED(event)
    const qreal borderWidth = std::max<qreal>(1.0, m_scheme.metricAlias.lineWidth);
    const QRectF shapeRect =
        adqt::widgets::detail::joinedButtonBorderRect(rect(), borderWidth, false, false);
    if (shapeRect.isEmpty()) {
        return;
    }
    const qreal radius = m_scheme.metricAlias.borderRadius;
    const QPainterPath shapePath =
        adqt::widgets::detail::roundedButtonPath(shapeRect, radius, radius, radius, radius);
    QPainter painter(this);
    painter.setRenderHint(QPainter::Antialiasing, true);
    painter.fillPath(shapePath,
                     styles::mainWindowBackgroundColor(this, m_scheme.map.colorBgContainer));
    painter.setPen(adqt::widgets::detail::makeButtonBorderPen(m_scheme.map.colorBorderSecondary,
                                                              borderWidth, Qt::SolidLine));
    painter.setBrush(Qt::NoBrush);
    painter.drawPath(shapePath);
}

} // namespace snow_shot::presentation::components
