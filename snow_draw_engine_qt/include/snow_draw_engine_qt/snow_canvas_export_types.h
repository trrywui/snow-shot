#pragma once

#include <QImage>
#include <QRectF>

struct CanvasExportSource {
    QImage image;
    QRectF canvasRect;
    // Empty coverage exposes the entire source without allocating a cropped raster.
    QRectF coverage = {};
};

// Integer source pixels relative to the complete export frame, translated without scaling.
struct CanvasExportTile {
    QRect sourcePixels;
    QPoint destination;
};
