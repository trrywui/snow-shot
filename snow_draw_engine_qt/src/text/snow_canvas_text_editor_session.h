#pragma once

#include <QPoint>
#include <QPointF>
#include <QRectF>
#include <QRegion>
#include <QString>
#include <QTextCursor>
#include <QVariant>

#include "snow_canvas_display_cache.h"
#include "snow_canvas_text_draft.h"
#include "snow_canvas_text_measurement.h"
#include "snow_draw_engine.h"

#include <optional>

class QFont;
class QInputMethodEvent;
class QKeyEvent;
class QPainter;

class SnowCanvasTextEditorSession final {
  public:
    struct FinishedEdit {
        QString text;
        QPointF canvasCenter;
        SnowElementId elementId{};
        SnowElementId arrowId{};
        SnowElementId bubbleId{};
        double minimumGraphemeWidth = 1.0;
        double textInkReserve = 0.0;
        bool hasExistingElement = false;
        SnowTextLayoutSize measuredLayout{1.0, 1.0, 0.0, 0.0};
        SnowTextStyle style{};
        bool autoResize = false;
        bool styleChanged = false;
        bool distanceValue = false;
        bool distanceValueChanged = false;
        std::optional<double> parsedDistanceValue;

        bool shouldCommit(bool hasViewport) const;
    };

    enum class EventCommand {
        None,
        Commit,
        DeleteElement,
        Cancel,
    };

    struct KeyResult {
        EventCommand command = EventCommand::None;
        bool handled = false;
        bool changed = false;
    };

    bool begin(const SnowTextElementInfo& info, const SnowSceneDisplayItem* existingSceneItem,
               const QFont& baseFont, const SnowTextStyle* newTextStyle = nullptr,
               const QString* completeText = nullptr);
    void beginDistanceValue(const QFont& baseFont);
    bool isDistanceValue() const {
        return !m_distanceSuffix.isEmpty();
    }
    QString distanceValuePresentationText() const {
        return m_draft.displayText();
    }
    std::optional<double> parsedDistanceValue() const;
    FinishedEdit finish(const QFont& baseFont);
    void cancel();
    void releaseRetainedState();
    void invalidateFontMetrics(const QFont& baseFont);
    std::optional<QRectF> textEditingBounds() const;
    // Returns whether changed bounds reflowed an eligible active draft.
    bool setTextEditingBounds(const std::optional<QRectF>& bounds, const QFont& baseFont);

    SnowElementId arrowId() const {
        return m_arrowId;
    }
    SnowElementId bubbleId() const {
        return m_bubbleId;
    }
    double minimumGraphemeWidth() const {
        return m_minimumGraphemeWidth;
    }
    double textInkReserve() const {
        return m_textInkReserve;
    }
    bool isActive() const;
    bool containsViewPosition(const SceneDisplayInfo& sceneInfo, const QPointF& viewPosition,
                              const QFont& baseFont) const;

    const SnowSceneDisplayItem* previewItem() const;
    QString presentationText() const;
    bool activeDraftHasExistingElement() const;
    bool activeDraftAutoResize() const;
    SnowTextStyle currentTextStyle() const;
    bool applyActiveDraftPresentation(const SnowTextElementInfo& info, const SnowTextStyle& style);
    bool syncActiveDraftPresentation(SnowRuntime runtime, SnowViewport viewport,
                                     const QFont& baseFont, const SceneDisplayInfo& sceneInfo,
                                     QRegion* outUpdateRegion = nullptr);
    QRegion previewRegion(const SceneDisplayInfo& sceneInfo, int paddingPx = 2) const;
    QRegion editingRegion(const QFont& baseFont, const SceneDisplayInfo& sceneInfo) const;
    QRegion caretRegion(const QFont& baseFont, const SceneDisplayInfo& sceneInfo) const;
    void updatePreviewFromState(const QFont& baseFont);
    QRegion updatePreviewFromState(const QFont& baseFont, const SceneDisplayInfo& sceneInfo);
    QRegion applyTextStyle(const SnowTextStyle& style, const QFont& baseFont,
                           const SceneDisplayInfo& sceneInfo);
    void updateGeometry(const QFont& baseFont, const SceneDisplayInfo& sceneInfo);
    bool setCursorFromViewPosition(const QFont& baseFont, const SceneDisplayInfo& sceneInfo,
                                   const QPointF& viewPosition, bool keepSelection = false);
    KeyResult handleKeyPress(QKeyEvent* event, const QFont& baseFont,
                             const SceneDisplayInfo& sceneInfo, bool deleteTextAfterCursor = false);
    bool handleInputMethodEvent(QInputMethodEvent* event, const QFont& baseFont);
    QVariant inputMethodQuery(Qt::InputMethodQuery query, const QFont& baseFont,
                              const SceneDisplayInfo& sceneInfo) const;
    void renderEditorOverlay(QPainter& painter, const QFont& baseFont,
                             const SceneDisplayInfo& sceneInfo, bool caretVisible = true) const;

  private:
    SnowCanvasSceneItem editableItem(const QFont& baseFont) const;
    void resetState();
    void updatePreviewLayout(const QFont& baseFont, bool forceLayout);
    void updatePreviewAnchor();
    bool canConstrainAutomaticWidth() const;
    bool moveCursor(QTextCursor::MoveOperation operation, QTextCursor::MoveMode mode,
                    const QFont& baseFont, const SceneDisplayInfo& sceneInfo);

    QPointF m_canvasAnchor;
    SnowElementId m_elementId{};
    SnowElementId m_arrowId{};
    SnowElementId m_bubbleId{};
    SnowTextElementInfo m_bubbleInfo{};
    double m_minimumGraphemeWidth = 1.0;
    double m_textInkReserve = 0.0;
    snow_canvas_text_measurement::BubbleTextLayoutCache m_bubbleLayouts;
    double m_arrowWidth = 0.0;
    bool m_hasExistingElement = false;
    SnowCanvasSceneItem m_previewItem;
    bool m_hasPreview = false;
    bool m_previewAutoResize = false;
    bool m_styleChanged = false;
    SnowCanvasTextDraft m_draft;
    QString m_distanceSuffix;
    QString m_initialDistanceValue;
    std::optional<QRectF> m_textEditingBounds;
};
