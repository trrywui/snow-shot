#include "snow_draw_engine_qt/snow_canvas_image.h"
#include "../../test-support/virtualmemory.h"
#include "snow_shot/presentation/screenshotresultcompositor.h"
#include "snow_shot/presentation/screenshotselectionshadowrenderer.h"

#include <QApplication>
#include <QImage>
#include <QPainter>
#include <QPainterPath>
#include <QTransform>

#include <cstdlib>
#include <algorithm>
#include <iostream>
#include <limits>
#include <stdexcept>
#include <utility>

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        throw std::runtime_error(message);
    }
}

QImage solidContent(const QSize& size = QSize(80, 48)) {
    QImage image(size, QImage::Format_RGBA8888);
    image.fill(QColor(30, 100, 210, 255));
    image.setDevicePixelRatio(2.0);
    return image;
}

void persistedStylePreservesQt611Format() {
    // Captured using the Qt 6.11.1 default stream format. Keep fixtures independent of the
    // running Qt version so an upgrade cannot silently change restored pin appearance.
    const QByteArray legacy = QByteArray::fromHex("0000000c0000000601dcdc14141e1e28280000");
    const auto restored = decodeScreenshotResultStyle(legacy);
    require(restored && restored->cornerRadius == 12 && restored->shadowWidth == 6 &&
                restored->shadowColor == QColor(20, 30, 40, 220) && !restored->region &&
                restored->regionScale == 1.0,
            "Qt 6.11 style records must restore their original effects");

    const QByteArray extended = QByteArray::fromHex(
        "0000000c0000000601dcdc14141e1e2828000053535247013ff4000000000000ffffffff");
    const auto current = decodeScreenshotResultStyle(extended);
    require(current && current->regionScale == 1.25 && !current->region,
            "Qt 6.11 extended style records must restore their geometry scale");
    require(encodeScreenshotResultStyle(*current) == extended,
            "writing a restored style must preserve the existing Qt 6.11 bytes");
    require(!decodeScreenshotResultStyle(extended.left(extended.size() - 1)),
            "truncated persisted geometry must remain rejected after a Qt upgrade");
    for (const auto [radius, width] :
         {std::pair{-1, 6}, std::pair{257, 6}, std::pair{12, -1}, std::pair{12, 65}}) {
        const ScreenshotResultStyle invalid{radius, width, Qt::black};
        require(!decodeScreenshotResultStyle(encodeScreenshotResultStyle(invalid)),
                "persisted styles enforce canvas-unit limits before rendering");
        QByteArray invalidLegacy;
        QDataStream stream(&invalidLegacy, QIODevice::WriteOnly);
        stream.setVersion(QDataStream::Qt_6_11);
        stream << invalid.cornerRadius << invalid.shadowWidth << invalid.shadowColor;
        require(!decodeScreenshotResultStyle(invalidLegacy),
                "legacy persisted styles enforce the same canvas-unit limits");
    }
}

void borderCompositionAndStylePersistence() {
    QImage content(QSize(80, 48), QImage::Format_ARGB32_Premultiplied);
    content.fill(QColor(30, 100, 210, 110));
    ScreenshotResultStyle style;
    style.shadowWidth = 13;
    style.borderType = ScreenshotSelectionBorderType::Border;
    style.borderWidth = 4;
    style.borderColor = QColor(Qt::white);
    const QImage output = ScreenshotResultCompositor::compose(content, style);
    require(output.size() == QSize(88, 56), "border dimensions use active width only");
    require(output.pixelColor(0, 20) == QColor(Qt::white) &&
                output.pixelColor(3, 20) == QColor(Qt::white),
            "border is opaque and has no shadow falloff");
    require(output.copy(4, 4, 80, 48) == content,
            "border must not fill translucent selected pixels");
    const auto restored = decodeScreenshotResultStyle(encodeScreenshotResultStyle(style));
    require(restored && restored->borderType == style.borderType && restored->borderWidth == 4 &&
                restored->borderColor == style.borderColor && restored->shadowWidth == 13,
            "result style retains inactive shadow and active border");
    require(!decodeScreenshotResultStyle(encodeScreenshotResultStyle(style).chopped(1)),
            "truncated border style is rejected");
    auto invalid = style;
    invalid.borderWidth = 65;
    require(!decodeScreenshotResultStyle(encodeScreenshotResultStyle(invalid)),
            "invalid saved border width is rejected");
    style.borderWidth = 0;
    require(ScreenshotResultCompositor::compose(content, style) == content,
            "zero border does not activate the stored shadow");
    style.borderWidth = 4;
    style.cornerRadius = 10;
    const QImage rounded = ScreenshotResultCompositor::compose(content, style);
    require(rounded.pixelColor(0, 0).alpha() == 0 && rounded.pixelColor(44, 1) == QColor(Qt::white),
            "border follows rounded corners");
    const auto layout = ScreenshotResultCompositor::layoutForContent(content.size(), style, 1.25);
    require(layout.effectInsets == QMargins(5, 5, 5, 5), "border width follows fractional DPR");
    QImage live(output.size(), QImage::Format_ARGB32_Premultiplied);
    live.fill(Qt::transparent);
    style.cornerRadius = 0;
    {
        QPainter painter(&live);
        painter.drawImage(QPoint(4, 4), content);
        ScreenshotResultCompositor::finishLiveSurface(painter, live.rect(), QRectF(4, 4, 80, 48),
                                                      style, 1);
    }
    require(live == output, "live border and exported border must match");
    QRegion region(QRect(0, 0, 80, 48));
    region -= QRect(20, 10, 40, 28);
    style.region = region;
    const auto ring = ScreenshotResultCompositor::compose(content, style);
    require(ring.pixelColor(25, 20) == QColor(Qt::white) && ring.pixelColor(44, 28).alpha() == 0,
            "border follows inner hole edges without filling the hole");
    QImage background(output.size(), QImage::Format_ARGB32_Premultiplied);
    background.fill(Qt::green);
    style.backgroundOpacityPercent = 50;
    const auto backed = ScreenshotResultCompositor::compose(content, style, 1, 1, background);
    // QPainter quantizes 50% opacity to 127 or 128 depending on the raster path.
    require(qAbs(backed.pixelColor(44, 28).alpha() - 128) <= 1 &&
                backed.pixelColor(25, 20) == QColor(Qt::white) &&
                backed.pixelColor(10, 20) == content.pixelColor(6, 16),
            "background opacity fills holes beneath borders without altering selected pixels");
    style.backgroundOpacityPercent = 0;
    QPainterPath ellipse;
    ellipse.addEllipse(QRectF(0, 0, 80, 48));
    style.region = ScreenshotRegionGeometry::fromPath(ellipse, ScreenshotRegionType::Curve);
    const auto curved = ScreenshotResultCompositor::compose(content, style);
    require(curved.pixelColor(44, 1).alpha() > 240 &&
                curved.pixelColor(44, 28) == content.pixelColor(40, 24),
            "custom curves have an exterior border and preserve their interior");
}

void scaledEffectsKeepTheirOutputPixelWidths() {
    ScreenshotResultStyle overflowing;
    overflowing.borderType = ScreenshotSelectionBorderType::Border;
    overflowing.borderWidth = std::numeric_limits<int>::max();
    require(!ScreenshotResultCompositor::layoutForContent(QSize(80, 48), overflowing).isValid(),
            "unrepresentable output extents fail before padding arithmetic overflows");
    require(!ScreenshotResultCompositor::layoutForContent(QSize(80, 48), {},
                                                          std::numeric_limits<qreal>::infinity())
                 .isValid(),
            "non-finite output scaling cannot produce a raster layout");
    for (const auto type :
         {ScreenshotSelectionBorderType::Shadow, ScreenshotSelectionBorderType::Border}) {
        for (const int width : {6, 48, 64}) {
            for (const qreal scale : {0.5, 1.0, 1.25, 2.0, 3.0}) {
                for (const bool shaped : {false, true}) {
                    ScreenshotResultStyle style{16, width, Qt::black};
                    style.borderType = type;
                    style.borderWidth = width;
                    style.borderColor = Qt::red;
                    if (shaped)
                        style.region = QRegion(0, 0, 120, 80).subtracted(QRect(40, 20, 40, 40));
                    const QSize size(qRound(120 * scale), qRound(80 * scale));
                    auto content = solidContent(size);
                    content.setDevicePixelRatio(1);
                    const auto outputStyle = ScreenshotResultCompositor::scaledStyle(style, scale);
                    const auto output = ScreenshotResultCompositor::compose(content, outputStyle);
                    const int padding = qRound(width * scale);
                    require(output.size() == size + QSize(2 * padding, 2 * padding),
                            "scaled effect padding must not be capped by canvas-unit UI limits");
                    auto referenceStyle = style;
                    referenceStyle.regionScale = scale;
                    // DPR represents physical scaling only at 1x and above.
                    if (scale >= 1)
                        require(
                            output ==
                                ScreenshotResultCompositor::compose(content, referenceStyle, scale),
                            "scaled-style export and DPR composition must have identical pixels");
                }
            }
        }
    }
    ScreenshotResultStyle rounded{200, 0, Qt::black};
    auto content = solidContent(QSize(1200, 1000));
    content.setDevicePixelRatio(1);
    require(ScreenshotResultCompositor::compose(
                content, ScreenshotResultCompositor::scaledStyle(rounded, 2)) ==
                ScreenshotResultCompositor::compose(content, rounded, 2),
            "scaled corner radii must not be capped by the canvas-unit radius limit");
}

void polygonBordersFitTheirDeclaredInsets() {
    for (qreal scale : {1.0, 1.25, 2.0}) {
        for (bool concave : {false, true}) {
            QPainterPath polygon;
            polygon.moveTo(50, 0);
            polygon.lineTo(100, 100);
            if (concave)
                polygon.lineTo(50, 70);
            polygon.lineTo(0, 100);
            polygon.closeSubpath();
            const int width = qRound(6 * scale);
            const int side = qRound(100 * scale);
            const int margin = 40;
            const QRect contentBounds(margin, margin, side, side);
            const QRect declaredBounds = contentBounds.adjusted(-width, -width, width, width);
            QTransform transform;
            transform.translate(margin, margin);
            transform.scale(scale, scale);
            QImage reference(side + 2 * margin, side + 2 * margin,
                             QImage::Format_ARGB32_Premultiplied);
            reference.fill(Qt::transparent);
            {
                QPainter painter(&reference);
                ScreenshotSelectionShadowRenderer::renderBorder(painter, transform.map(polygon),
                                                                width, Qt::red);
            }
            for (int y = 0; y < reference.height(); ++y)
                for (int x = 0; x < reference.width(); ++x)
                    require(declaredBounds.contains(x, y) ||
                                reference.pixelColor(x, y).alpha() == 0,
                            "polygon border must fit the same fixed insets as preview and export");

            ScreenshotResultStyle style;
            style.borderType = ScreenshotSelectionBorderType::Border;
            style.borderWidth = 6;
            style.borderColor = Qt::red;
            style.region = ScreenshotRegionGeometry::fromPath(polygon, ScreenshotRegionType::Curve);
            style.regionScale = scale;
            QImage content(side, side, QImage::Format_ARGB32_Premultiplied);
            content.fill(Qt::blue);
            const auto output = ScreenshotResultCompositor::compose(content, style, scale);
            require(output.size() == declaredBounds.size(), "polygon export uses declared insets");
            QImage live(output.size(), output.format());
            live.fill(Qt::transparent);
            {
                QPainter painter(&live);
                painter.drawImage(QPoint(width, width), content);
                auto physicalStyle = style;
                physicalStyle.borderWidth = width;
                ScreenshotResultCompositor::finishLiveSurface(
                    painter, live.rect(), QRectF(width, width, side, side), physicalStyle, 1);
            }
            int maximumDifference = 0;
            for (int y = 0; y < live.height(); ++y)
                for (int x = 0; x < live.width(); ++x) {
                    const auto a = live.pixel(x, y), b = output.pixel(x, y);
                    maximumDifference = std::max(
                        {maximumDifference, qAbs(qAlpha(a) - qAlpha(b)), qAbs(qRed(a) - qRed(b)),
                         qAbs(qGreen(a) - qGreen(b)), qAbs(qBlue(a) - qBlue(b))});
                }
            // Export multiplies by an Alpha8 mask; live painting subtracts the
            // inverse path. Qt quantizes those equivalent coverages one level apart.
            require(maximumDifference <= 1,
                    "live and exported polygon borders retain the same coverage and color");
        }
    }
}

void squareResultPreservesPhysicalPixels() {
    const QImage result = ScreenshotResultCompositor::compose(solidContent(), {});
    require(result.size() == QSize(80, 48), "square output dimensions changed");
    require(result.format() == QImage::Format_ARGB32_Premultiplied, "result is not premultiplied");
    require(result.devicePixelRatio() == 1.0, "result DPR is not normalized");
    require(result.pixelColor(0, 0) == QColor(30, 100, 210, 255),
            "square output changed a source pixel");
}

void roundedAndShadowedResultHasRealTransparency() {
    const QImage roundedOnly = ScreenshotResultCompositor::compose(
        solidContent(), ScreenshotResultStyle{16, 0, QColor(20, 30, 40, 220)});
    require(roundedOnly.pixelColor(0, 0).alpha() == 0,
            "rounded-only content corner is not transparent");

    const ScreenshotResultStyle style{16, 12, QColor(20, 30, 40, 220)};
    const QImage result = ScreenshotResultCompositor::compose(solidContent(), style);
    require(result.size() == QSize(104, 72), "effect insets were not added to output");
    require(result.pixelColor(0, 0).alpha() == 0, "outer corner is not transparent");
    require(result.pixelColor(12, 12).alpha() < 255,
            "rounded content leaked opaque pixels into the shadow");
    require(result.pixelColor(12 + 40, 12 + 24).alpha() == 255,
            "content center became transparent");
    const int shadowAlpha = result.pixelColor(11, 12 + 24).alpha();
    require(shadowAlpha > 0 && shadowAlpha < 255, "shadow edge is not semitransparent");
}

void exportAndLiveShadowShareSoftFalloff() {
    for (const int radius : {0, 16}) {
        for (const int width : {8, 16, 32}) {
            const QColor color(40, 80, 120, 190);
            QImage content = solidContent();
            content.setDevicePixelRatio(1.0);
            const QImage result =
                ScreenshotSelectionShadowRenderer::composeExport(content, radius, width, color);
            QImage live(result.size(), QImage::Format_ARGB32_Premultiplied);
            live.fill(Qt::transparent);
            {
                QPainter painter(&live);
                ScreenshotSelectionShadowRenderer::renderResultShadow(
                    painter, QRectF(QPointF(width, width), QSizeF(content.size())), radius, width,
                    color, 1.0);
            }
            const int centerX = width + content.width() / 2;
            for (int y = 0; y < width; ++y)
                require(result.pixelColor(centerX, y) == live.pixelColor(centerX, y),
                        "exported and live shadow profiles disagree");
            const int edge = result.pixelColor(centerX, width - 1).alpha();
            require(result.pixelColor(centerX, width - 1 - (width + 5) / 6).alpha() <
                        qRound(color.alpha() * 0.18),
                    "exported shadow retains an overly wide dense band");
            require(result.pixelColor(centerX, width - 1 - width / 4).alpha() * 2 < edge &&
                        result.pixelColor(centerX, width - 1 - width / 2).alpha() * 5 < edge,
                    "exported shadow has a broad opaque shelf");
            require(result.pixelColor(centerX, 0).alpha() == 0,
                    "exported shadow has a visible outer cutoff");
            require(result.pixelColor(centerX, width + content.height() / 2) ==
                        content.pixelColor(content.width() / 2, content.height() / 2),
                    "soft shadow changed the screenshot content");
        }
    }
}

void roundedAlphaMaskMatchesArgbReference() {
    for (const QSize size : {QSize(41, 29), QSize(1025, 513)}) {
        QImage source(size, QImage::Format_ARGB32_Premultiplied);
        for (int y = 0; y < size.height(); ++y) {
            for (int x = 0; x < size.width(); ++x) {
                source.setPixelColor(x, y,
                                     QColor((x * 17 + y) % 256, (x + y * 13) % 256,
                                            (x * 3 + y * 5) % 256, (x + y * 7) % 256));
            }
        }
        const QImage original = source.copy();
        for (const qreal dpr : {1.0, 1.25, 1.75, 2.0}) {
            for (const int radius : {1, 13, 2000}) {
                const ScreenshotResultStyle style{radius, 0, QColor()};
                const auto layout = ScreenshotResultCompositor::layoutForContent(size, style, dpr);
                QImage expected(layout.outputRect.size(), QImage::Format_ARGB32_Premultiplied);
                expected.fill(Qt::transparent);
                QImage mask(expected.size(), QImage::Format_ARGB32_Premultiplied);
                mask.fill(Qt::transparent);
                QPainterPath path;
                const qreal physicalRadius = std::min<qreal>(
                    ScreenshotResultCompositor::normalizedStyle(style).cornerRadius *
                        layout.devicePixelRatio,
                    std::min(layout.contentRect.width(), layout.contentRect.height()) / 2.0);
                path.addRoundedRect(layout.contentRect, physicalRadius, physicalRadius,
                                    Qt::AbsoluteSize);
                {
                    QPainter painter(&mask);
                    painter.setRenderHint(QPainter::Antialiasing, true);
                    painter.setPen(Qt::NoPen);
                    painter.setBrush(Qt::white);
                    painter.drawPath(path);
                }
                {
                    QPainter painter(&expected);
                    painter.setRenderHint(QPainter::Antialiasing, true);
                    painter.drawImage(layout.contentRect, source);
                    painter.setCompositionMode(QPainter::CompositionMode_DestinationIn);
                    painter.drawImage(QPoint(), mask);
                }
                const QImage actual = ScreenshotResultCompositor::compose(source, style, dpr);
                require(actual == expected,
                        "Alpha8 rounded masks must exactly preserve ARGB mask pixels and alpha");
                require(source == original, "rounded composition must preserve its source");
            }
        }
    }
}

void compoundCompositionStartsTransparentForSmallAndLargeImages() {
    for (const QSize size : {QSize(80, 48), QSize(1025, 513)}) {
        QImage source(size, QImage::Format_ARGB32_Premultiplied);
        source.fill(QColor(30, 100, 210, 128));
        const QRect inset = source.rect().adjusted(4, 6, -8, -10);
        QPainterPath ellipse;
        ellipse.addEllipse(inset);
        for (const ScreenshotRegionGeometry& region :
             {ScreenshotRegionGeometry(QRegion(inset).subtracted(QRect(10, 10, 8, 8))),
              ScreenshotRegionGeometry::fromPath(ellipse, ScreenshotRegionType::Curve)}) {
            ScreenshotResultStyle style;
            style.region = region;
            QImage expected(size, QImage::Format_ARGB32_Premultiplied);
            expected.fill(Qt::transparent);
            QImage mask(size, QImage::Format_Alpha8);
            mask.fill(0);
            {
                QPainter painter(&mask);
                painter.setRenderHint(QPainter::Antialiasing, true);
                painter.setPen(Qt::NoPen);
                painter.setBrush(Qt::white);
                painter.drawPath(region.custom() ? region.path() : screenshotRegionPath(region));
            }
            {
                QPainter painter(&expected);
                painter.drawImage(QPoint(), source);
                painter.setCompositionMode(QPainter::CompositionMode_DestinationIn);
                painter.drawImage(QPoint(), mask);
            }
            for (int iteration = 0; iteration < 3; ++iteration) {
                if (iteration == 0)
                    ScreenshotSelectionShadowRenderer::resetCacheForCurrentThread();
                require(ScreenshotResultCompositor::compose(source, style) == expected,
                        "compound composition must start transparent and preserve source alpha");
            }
        }
    }
    ScreenshotSelectionShadowRenderer::resetCacheForCurrentThread();
}

void previewAssetsAreReleasedAfterCapture() {
    ScreenshotSelectionShadowRenderer::resetCacheForCurrentThread();
    const QImage exported = ScreenshotResultCompositor::compose(
        solidContent(), ScreenshotResultStyle{16, 12, QColor(20, 30, 40, 220)});
    require(!exported.isNull(), "the shadowed export must render");
    const auto afterExport = ScreenshotSelectionShadowRenderer::diagnosticsForCurrentThread();
    require(afterExport.retainedEntries == 0 && afterExport.regionCacheRetainedBytes == 0,
            "one-off export composition must not retain preview assets");

    QImage preview(QSize(104, 72), QImage::Format_ARGB32_Premultiplied);
    preview.fill(Qt::transparent);
    {
        QPainter painter(&preview);
        ScreenshotSelectionShadowRenderer::renderPreview(painter, QRectF(12, 12, 80, 48), 16, 12,
                                                         QColor(20, 30, 40, 220), 1.0);
    }
    const auto retained = ScreenshotSelectionShadowRenderer::diagnosticsForCurrentThread();
    require(retained.retainedEntries == 1 && retained.retainedBytes > 0,
            "an active preview must retain its shadow asset");

    ScreenshotSelectionShadowRenderer::resetCacheForCurrentThread();
    const auto released = ScreenshotSelectionShadowRenderer::diagnosticsForCurrentThread();
    require(released.retainedEntries == 0 && released.retainedBytes == 0 &&
                released.regionCacheRetainedBytes == 0,
            "capture cleanup must release selection preview caches");

    {
        QPainter painter(&preview);
        ScreenshotSelectionShadowRenderer::renderPreview(painter, QRectF(12, 12, 80, 48), 16, 12,
                                                         QColor(20, 30, 40, 220), 1.0);
    }
    const auto rebuilt = ScreenshotSelectionShadowRenderer::diagnosticsForCurrentThread();
    require(rebuilt.retainedEntries == 1 && rebuilt.retainedBytes > 0,
            "the next capture must be able to rebuild the shadow preview asset");
    ScreenshotSelectionShadowRenderer::resetCacheForCurrentThread();
}

void layoutScalesOnlyEffectsForFractionalDpr() {
    const ScreenshotResultStyle style{10, 8, QColor()};
    for (const qreal dpr : {1.0, 1.25, 1.5, 2.0}) {
        const ScreenshotResultLayout layout =
            ScreenshotResultCompositor::layoutForContent(QSize(100, 50), style, dpr);
        const int effect = qRound(8.0 * dpr);
        require(layout.isValid(), "fractional-DPR layout is invalid");
        require(layout.contentRect == QRect(effect, effect, 100, 50),
                "fractional-DPR content rect is wrong");
        require(layout.outputRect.size() == QSize(100 + effect * 2, 50 + effect * 2),
                "fractional-DPR output bounds are wrong");
    }
}

void noEffectResultSharesNormalizedStorage() {
    QImage source(QSize(32, 24), QImage::Format_ARGB32_Premultiplied);
    source.fill(QColor(30, 100, 210, 255));
    const QImage result = ScreenshotResultCompositor::compose(source, {});
    require(result.constBits() == source.constBits(), "no-effect result detached source pixels");
    require(result.size() == source.size(), "no-effect result changed dimensions");
}

void outputOpacityScalesTheCompleteComposition() {
    QImage translucent(QSize(32, 24), QImage::Format_ARGB32_Premultiplied);
    translucent.fill(QColor(30, 100, 210, 120));
    const QImage translucentResult = ScreenshotResultCompositor::compose(translucent, {}, 1.0, 0.5);
    require(qAbs(translucentResult.pixelColor(10, 10).alpha() - 60) <= 1,
            "output opacity did not scale existing source alpha");

    const ScreenshotResultStyle style{10, 8, QColor(20, 30, 40, 220)};
    const QImage opaque = ScreenshotResultCompositor::compose(solidContent(), style);
    const QImage faded = ScreenshotResultCompositor::compose(solidContent(), style, 1.0, 0.5);
    require(faded.size() == opaque.size() &&
                qAbs(faded.pixelColor(8 + 40, 8 + 24).alpha() - 128) <= 1,
            "output opacity did not scale composed content");
    const int opaqueShadowAlpha = opaque.pixelColor(7, 8 + 24).alpha();
    const int fadedShadowAlpha = faded.pixelColor(7, 8 + 24).alpha();
    require(opaqueShadowAlpha > 0 &&
                qAbs(fadedShadowAlpha - qRound(opaqueShadowAlpha * 128.0 / 255.0)) <= 1,
            "output opacity did not scale the composed shadow exactly once");

    const QImage transparent = ScreenshotResultCompositor::compose(solidContent(), {}, 1.0, -1.0);
    const QImage clampedOpaque = ScreenshotResultCompositor::compose(solidContent(), {}, 1.0, 2.0);
    const QImage invalidOpaque = ScreenshotResultCompositor::compose(
        solidContent(), {}, 1.0, std::numeric_limits<qreal>::quiet_NaN());
    require(transparent.pixelColor(10, 10).alpha() == 0,
            "negative output opacity was not clamped to transparent");
    require(clampedOpaque.pixelColor(10, 10).alpha() == 255 &&
                invalidOpaque.pixelColor(10, 10).alpha() == 255,
            "oversized or invalid output opacity did not use a safe opaque value");
}

void liveSurfaceClipsExistingCanvasPixelsBeforeAddingShadow() {
    QImage surface(QSize(70, 60), QImage::Format_ARGB32_Premultiplied);
    surface.fill(Qt::transparent);
    {
        QPainter painter(&surface);
        painter.fillRect(QRect(6, 6, 58, 48), QColor(220, 20, 20, 255));
        ScreenshotResultCompositor::finishLiveSurface(
            painter, surface.rect(), QRectF(12, 12, 46, 36),
            ScreenshotResultStyle{10, 6, QColor(0, 0, 0, 220)}, 1.0);
    }
    require(surface.pixelColor(6, 6).alpha() == 0,
            "canvas pixels leaked outside the result and shadow bounds");
    require(surface.pixelColor(12, 12).alpha() < 255,
            "canvas pixels leaked through the rounded corner");
    require(surface.pixelColor(35, 30).alpha() == 255, "live result center was clipped");
    const int shadowAlpha = surface.pixelColor(11, 30).alpha();
    require(shadowAlpha > 0 && shadowAlpha < 255,
            "live surface did not add a semitransparent shadow behind content");
}

void liveSurfacePreservesContentBeyondRoundedWidgetBounds() {
    // A native client is sized in physical pixels. Its smallest covering Qt
    // widget can end at a fractional device pixel before the content ends.
    // In particular, 381 DIPs at 1.75x cover a 667-pixel client after rounding.
    for (const qreal dpr : {1.25, 1.5, 1.75, 2.0}) {
        for (const QSize size : {QSize(869, 937), QSize(1000, 667), QSize(667, 1000)}) {
            QImage surface(size, QImage::Format_ARGB32_Premultiplied);
            surface.setDevicePixelRatio(dpr);
            surface.fill(QColor(30, 100, 210, 255));
            const QImage expected = surface.copy();
            const QRectF content(QPointF(), QSizeF(size) / dpr);
            const QRectF widget(0, 0, qRound(content.width()), qRound(content.height()));
            {
                QPainter painter(&surface);
                ScreenshotResultCompositor::finishLiveSurface(painter, widget, content, {}, dpr,
                                                              1.0 / dpr);
            }
            require(surface == expected,
                    "rounded widget bounds must not erase fractional strips of content");
        }
    }
}

void liveSurfaceSubtractsIntersectingContent() {
    const QRect viewport(8, 8, 32, 24);
    for (const QRect content :
         {QRect(4, 4, 44, 36), QRect(12, 12, 20, 12), QRect(4, 12, 24, 28), QRect(24, 4, 24, 24)}) {
        QImage surface(QSize(52, 44), QImage::Format_ARGB32_Premultiplied);
        const QColor color(30, 100, 210, 128);
        surface.fill(color);
        QImage expected = surface.copy();
        for (int y = 0; y < surface.height(); ++y) {
            for (int x = 0; x < surface.width(); ++x) {
                if (viewport.contains(x, y) && !content.contains(x, y))
                    expected.setPixel(x, y, 0);
            }
        }
        {
            QPainter painter(&surface);
            ScreenshotResultCompositor::finishLiveSurface(painter, viewport, content, {}, 1.0);
        }
        require(surface == expected,
                "live clipping must subtract content instead of XORing intersecting rectangles");
    }
}

void liveSurfacePreservesTranslatedFractionalContent() {
    QImage surface(QSize(868, 936), QImage::Format_ARGB32_Premultiplied);
    surface.setDevicePixelRatio(1.25);
    surface.fill(QColor(30, 100, 210, 255));
    const QImage expected = surface.copy();
    const QTransform transform(0.79999999999999993, 0, 0, 0.79999999999999993, 312.80000000000001,
                               -29.599999999999966);
    const QRectF content = transform.mapRect(QRectF(-391, 37, 868, 936));
    {
        QPainter painter(&surface);
        ScreenshotResultCompositor::finishLiveSurface(painter, QRectF(0, 0, 694, 749), content, {},
                                                      1.25, transform.m11());
    }
    require(surface == expected, "camera roundoff must not erase translated content");
}

void liveMaskPreservesSelectionFillRules() {
    for (const auto rule : {Qt::OddEvenFill, Qt::WindingFill}) {
        for (const bool curved : {false, true}) {
            for (const bool hole : {false, true}) {
                QPainterPath path;
                QPainterPath inner;
                if (curved) {
                    path.addEllipse(QRectF(10, 10, 580, 420));
                    inner.addEllipse(QRectF(150, 100, 300, 220));
                } else {
                    path.addRect(QRectF(10, 10, 580, 420));
                    inner.addRect(QRectF(150, 100, 300, 220));
                }
                path.addPath(hole ? inner.toReversed() : inner);
                path.setFillRule(rule);
                const auto region =
                    ScreenshotRegionGeometry::fromPath(path, ScreenshotRegionType::Curve);
                const auto restored = ScreenshotRegionGeometry::fromJson(region.toJson());
                require(restored && restored->path() == path,
                        "persisted regions retain their fill rule");
                for (const qreal dpr : {1.0, 1.25, 1.75, 2.0}) {
                    for (const QPointF phase : {QPointF(), QPointF(0.3, -0.2)}) {
                        const QSize size(qRound(620 * dpr), qRound(460 * dpr));
                        const QColor color(30, 100, 210, 128);
                        QImage expected(size, QImage::Format_ARGB32_Premultiplied);
                        expected.setDevicePixelRatio(dpr);
                        expected.fill(Qt::transparent);
                        {
                            QPainter painter(&expected);
                            painter.setRenderHint(QPainter::Antialiasing);
                            painter.translate(phase);
                            painter.fillPath(path, color);
                        }
                        QImage actual(size, expected.format());
                        actual.setDevicePixelRatio(dpr);
                        actual.fill(color);
                        ScreenshotSelectionShadowRenderer::resetDiagnosticsForCurrentThread();
                        {
                            QPainter painter(&actual);
                            painter.translate(phase);
                            ScreenshotResultCompositor::maskLiveSurface(
                                painter, QRectF(-10, -10, 640, 480), restored->path());
                        }
                        const auto diagnostics =
                            ScreenshotSelectionShadowRenderer::diagnosticsForCurrentThread();
                        require(diagnostics.regionScratchPeakBytes <= 256 * 256 * 4 &&
                                    diagnostics.selectionSizedTransientAllocations == 0,
                                "live mask scratch storage must stay bounded independently of "
                                "viewport size");
                        if (rule == Qt::OddEvenFill)
                            require(diagnostics.regionMaskBuilds == 0,
                                    "parity masks must retain their allocation-free path");
                        int difference = 0;
                        for (int y = 0; y < size.height(); ++y)
                            for (int x = 0; x < size.width(); ++x) {
                                const auto a = actual.pixel(x, y), b = expected.pixel(x, y);
                                difference =
                                    std::max({difference, qAbs(qAlpha(a) - qAlpha(b)),
                                              qAbs(qRed(a) - qRed(b)), qAbs(qGreen(a) - qGreen(b)),
                                              qAbs(qBlue(a) - qBlue(b))});
                            }
                        require(difference <= 1, "live masks must preserve winding, holes, curves "
                                                 "and fractional coverage");
                    }
                }
            }
        }
    }
}

void exteriorPaintingPreservesClippingAndPainterState() {
    for (const qreal dpr : {1.0, 1.25, 2.0}) {
        for (const int shape : {0, 1, 2, 3}) {
            QPainterPath path;
            path.setFillRule(Qt::WindingFill);
            if (shape != 0) {
                path.addEllipse(QRectF(-40, -20, 600, 430));
                path.addRect(QRectF(130, 110, 220, 160));
                if (shape >= 2) {
                    // Intersecting contours and both winding signs use the same coverage contract.
                    path.addEllipse(QRectF(180, 180, 400, 240));
                    if (shape == 3) {
                        path = path.toReversed();
                        // Qt's reversed path defaults to parity fill; this fixture
                        // intentionally exercises negative winding counts.
                        path.setFillRule(Qt::WindingFill);
                    }
                }
            }
            QTransform transform;
            transform.translate(50.3, 12.7);
            transform.rotate(7);
            transform.scale(0.9, 1.05);
            const QSize size(qRound(640 * dpr), qRound(480 * dpr));
            const QColor color(43, 103, 173, 137);
            QImage exterior(size, QImage::Format_ARGB32_Premultiplied);
            exterior.setDevicePixelRatio(dpr);
            exterior.fill(color);
            {
                QPainter painter(&exterior);
                painter.setTransform(transform);
                painter.setRenderHint(QPainter::Antialiasing);
                painter.setCompositionMode(QPainter::CompositionMode_DestinationOut);
                painter.fillPath(path, Qt::black);
            }
            QImage expected(size, exterior.format());
            expected.setDevicePixelRatio(dpr);
            expected.fill(QColor(170, 80, 50, 120));
            QImage actual = expected.copy();
            const QRectF viewport(15, 25, 520, 380);
            const QRegion clip = QRegion(0, 0, 270, 480) + QRect(310, 100, 250, 320);
            for (const bool reference : {true, false}) {
                QPainter painter(reference ? &expected : &actual);
                painter.setTransform(transform);
                painter.setClipRegion(clip);
                painter.setOpacity(0.65);
                painter.setCompositionMode(QPainter::CompositionMode_SourceOver);
                if (reference) {
                    painter.setClipRect(viewport, Qt::IntersectClip);
                    painter.resetTransform();
                    painter.drawImage(QPointF(), exterior);
                } else {
                    const auto hints = painter.renderHints();
                    ScreenshotResultCompositor::fillOutsideSelection(painter, viewport, path,
                                                                     color);
                    require(painter.worldTransform() == transform && painter.clipRegion() == clip &&
                                painter.opacity() == 0.65 && painter.renderHints() == hints &&
                                painter.compositionMode() == QPainter::CompositionMode_SourceOver,
                            "exterior painting must restore all caller painter state");
                }
            }
            require(
                actual == expected,
                "tiled exterior coverage must match full rasterization under transforms and clips");
        }
    }
}

void windingLiveResultMatchesExport() {
    QPainterPath path;
    path.setFillRule(Qt::WindingFill);
    path.addRect(QRectF(0, 0, 80, 80));
    path.addRect(QRectF(20, 20, 40, 40));
    ScreenshotResultStyle style;
    style.borderType = ScreenshotSelectionBorderType::Border;
    style.borderWidth = 4;
    style.region = ScreenshotRegionGeometry::fromPath(path, ScreenshotRegionType::Curve);
    const auto restored = decodeScreenshotResultStyle(encodeScreenshotResultStyle(style));
    require(restored && restored->region->path().fillRule() == Qt::WindingFill,
            "result style round trips preserve winding selection coverage");
    QImage content(80, 80, QImage::Format_ARGB32_Premultiplied);
    content.fill(Qt::blue);
    const auto exported = ScreenshotResultCompositor::compose(content, *restored);
    QImage live(exported.size(), exported.format());
    live.fill(Qt::transparent);
    {
        QPainter painter(&live);
        painter.drawImage(QPoint(4, 4), content);
        ScreenshotResultCompositor::finishLiveSurface(painter, live.rect(), QRectF(4, 4, 80, 80),
                                                      *restored, 1);
    }
    require(live == exported && live.pixelColor(44, 44) == QColor(Qt::blue),
            "persisted winding selections must have identical bordered live and exported pixels");
}

void sharedMetadataAndOpacityKeepMappedOwnership() {
    QImage source = snowCanvasAllocateImage(QSize(1025, 513), QImage::Format_ARGB32_Premultiplied);
    require(!source.isNull(), "large composition fixture must allocate");
    source.fill(QColor(200, 100, 50));
    source.setDevicePixelRatio(2.0);
    QImage result = ScreenshotResultCompositor::compose(source, {}, 1.0, 0.5);
    require(!result.isNull() && source.devicePixelRatio() == 2.0 &&
                source.pixelColor(0, 0).alpha() == 255 && result.pixelColor(0, 0).alpha() == 128,
            "metadata normalization and opacity must preserve the shared source");
    const auto* middle = result.constBits() + result.sizeInBytes() / 2;
    require(snow::test_support::virtualMemoryMapped(middle),
            "compositor mutations must retain managed output storage");
    result = {};
    require(!snow::test_support::virtualMemoryMapped(middle),
            "compositor output must release its final pixel pages");
}

} // namespace

int main(int argc, char** argv) {
    QApplication application(argc, argv);
    try {
        persistedStylePreservesQt611Format();
        borderCompositionAndStylePersistence();
        scaledEffectsKeepTheirOutputPixelWidths();
        polygonBordersFitTheirDeclaredInsets();
        squareResultPreservesPhysicalPixels();
        sharedMetadataAndOpacityKeepMappedOwnership();
        roundedAndShadowedResultHasRealTransparency();
        exportAndLiveShadowShareSoftFalloff();
        roundedAlphaMaskMatchesArgbReference();
        compoundCompositionStartsTransparentForSmallAndLargeImages();
        previewAssetsAreReleasedAfterCapture();
        layoutScalesOnlyEffectsForFractionalDpr();
        noEffectResultSharesNormalizedStorage();
        outputOpacityScalesTheCompleteComposition();
        liveSurfaceClipsExistingCanvasPixelsBeforeAddingShadow();
        liveSurfacePreservesContentBeyondRoundedWidgetBounds();
        liveSurfaceSubtractsIntersectingContent();
        liveSurfacePreservesTranslatedFractionalContent();
        liveMaskPreservesSelectionFillRules();
        exteriorPaintingPreservesClippingAndPainterState();
        windingLiveResultMatchesExport();
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return EXIT_FAILURE;
    }
    return EXIT_SUCCESS;
}
