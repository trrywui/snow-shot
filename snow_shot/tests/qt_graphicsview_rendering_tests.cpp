#include <QApplication>
#include <QFontDatabase>
#include <QGraphicsRectItem>
#include <QGraphicsScene>
#include <QGraphicsSimpleTextItem>
#include <QGraphicsView>
#include <QImage>
#include <QPainter>
#include <QRegion>

#include <cstdlib>
#include <iostream>

namespace {

void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(EXIT_FAILURE);
    }
}

class CountedTextItem final : public QGraphicsSimpleTextItem {
  public:
    CountedTextItem() : QGraphicsSimpleTextItem(QStringLiteral("OCR first frame")) {}

    void paint(QPainter* painter, const QStyleOptionGraphicsItem* option,
               QWidget* widget) override {
        ++paintCount;
        QGraphicsSimpleTextItem::paint(painter, option, widget);
    }

    int paintCount = 0;
};

QImage renderViewport(QGraphicsView& view, qreal dpr, const QRegion& region) {
    const QSize size = view.viewport()->size();
    QImage image(QSize(qCeil(size.width() * dpr), qCeil(size.height() * dpr)),
                 QImage::Format_ARGB32_Premultiplied);
    image.setDevicePixelRatio(dpr);
    image.fill(Qt::white);
    QPainter painter(&image);
    view.viewport()->render(&painter, region.boundingRect().topLeft(), region);
    return image;
}

void verifyViewportPainting(qreal dpr, const QPointF& origin, const QTransform& transform) {
    QGraphicsScene scene;
    scene.setSceneRect(QRectF(origin, QSizeF(400, 250)));
    require(scene.itemIndexMethod() == QGraphicsScene::BspTreeIndex,
            "The regression must exercise indexed scene culling.");

    const QColor markerColor(30, 80, 140);
    const QRectF markerRect(origin + QPointF(180, 145), QSizeF(40, 24));
    scene.addRect(markerRect, QPen(Qt::NoPen), QBrush(markerColor));
    auto* text = new CountedTextItem;
    text->setPos(origin + QPointF(140, 95));
    scene.addItem(text);

    QGraphicsView view(&scene);
    view.setFrameShape(QFrame::NoFrame);
    view.setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    view.setVerticalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    view.resize(400, 250);
    view.setTransform(transform);
    view.centerOn(scene.sceneRect().center());
    view.show();
    QApplication::processEvents();

    // Full viewport damage starts at (0, 0). Qt expands its culling bounds by
    // one pixel, so both leading edges become negative before scene mapping.
    // A partial interior repaint alone does not exercise that conversion.
    text->paintCount = 0;
    const QRegion fullRegion(view.viewport()->rect());
    const QImage firstFrame = renderViewport(view, dpr, fullRegion);
    require(text->paintCount > 0, "Full viewport damage must paint text without interaction.");
    const QPointF markerPixel = view.viewportTransform().map(markerRect.center()) * dpr;
    require(firstFrame.pixelColor(qFloor(markerPixel.x()), qFloor(markerPixel.y())) == markerColor,
            "Full viewport damage must retain visible indexed items.");

    text->paintCount = 0;
    const QRegion partialRegion(QRect(90, 60, 220, 140));
    const QImage partialFrame = renderViewport(view, dpr, partialRegion);
    require(text->paintCount > 0, "Interior damage must also paint text.");
    require(partialFrame.pixelColor(qFloor(markerPixel.x()), qFloor(markerPixel.y())) ==
                markerColor,
            "Partial and full damage must agree on visible items.");

    text->setBrush(Qt::red);
    require(renderViewport(view, dpr, fullRegion) != firstFrame,
            "Text changes must appear in the next full repaint.");
    text->setBrush(Qt::black);
    require(renderViewport(view, dpr, fullRegion) == firstFrame,
            "The first frame must match the frame after interaction.");
}

} // namespace

int main(int argc, char** argv) {
    QApplication application(argc, argv);
#ifdef Q_OS_WIN
    require(QFontDatabase::addApplicationFont(QStringLiteral("C:/Windows/Fonts/segoeui.ttf")) >= 0,
            "The offscreen text fixture requires the Windows system font.");
#endif
    for (const qreal dpr : {1.0, 1.25, 2.0}) {
        for (const QPointF origin : {QPointF(), QPointF(-200, -125)}) {
            verifyViewportPainting(dpr, origin, QTransform());
            QTransform transformed;
            transformed.scale(1.2, 1.2);
            transformed.rotate(7);
            verifyViewportPainting(dpr, origin, transformed);
        }
    }
    std::cout << "Graphics View first-frame rendering tests passed.\n";
    return EXIT_SUCCESS;
}
