#pragma once

#include <QPointF>
#include <QWidget>
#include <array>

class QToolButton;
class QAbstractButton;

// An input surface only: the camera image continues to use the shared renderer.
class RecordingWebcamControl final : public QWidget {
    Q_OBJECT
  public:
    explicit RecordingWebcamControl(QWidget* parent);
    void present(const QRectF& bounds, const QSize& region, int size, const QPoint& offset,
                 const QPoint& fittedOffset, bool circle);
    void cancelInteraction();
    [[nodiscard]] bool interacting() const {
        return m_interacting;
    }

  signals:
    void layoutPreviewed(int size, const QPoint& offset);
    void layoutCommitted(int size, const QPoint& offset);
    void interactionChanged();
    // Clockwise, counterclockwise, horizontal flip, vertical flip, in toolbar order.
    void orientationRequested(int action);

  protected:
    bool event(QEvent* event) override;
    bool eventFilter(QObject* watched, QEvent* event) override;
    void paintEvent(QPaintEvent*) override;
    void mousePressEvent(QMouseEvent* event) override;
    void mouseMoveEvent(QMouseEvent* event) override;
    void mouseReleaseEvent(QMouseEvent* event) override;

  private:
    void beginInteraction(QMouseEvent* event, bool resize);
    QPointF pointerPosition(const QPointF& globalPosition) const;
    void updateInteraction(const QPointF& point);
    void releaseInteraction();
    void retranslateUi();
    void updateToolbar();
    QToolButton* m_resize;
    QWidget* m_toolbar;
    std::array<QAbstractButton*, 4> m_orientationButtons{};
    QRectF m_bounds;
    QSize m_region;
    QPoint m_offset;
    QPoint m_fittedOffset;
    int m_size = 150;
    qreal m_scale = 1;
    bool m_circle = false;
    bool m_interacting = false;
    bool m_resizing = false;
    bool m_changed = false;
    bool m_escapeRelease = false;
    QPointF m_origin;
    QPoint m_initialOffset;
    QPoint m_initialFittedOffset;
    int m_initialSize = 150;
    int m_initialSide = 150;
    qreal m_initialScale = 1;
    int m_nextSize = 150;
    QPoint m_nextOffset;
};
