#pragma once

#include <QColor>
#include <QRectF>

class QPainter;

namespace adqt::widgets::detail {

// Original, resolution-independent artwork with a 250 x 295 logical canvas.
void paintResultIllustration(QPainter* painter, const QRectF& rect, int status,
                             const QColor& primary);

}  // namespace adqt::widgets::detail
