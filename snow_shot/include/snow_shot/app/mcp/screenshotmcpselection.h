#pragma once

#include "snow_shot/presentation/screenshotselectionmodel.h"
#include <QJsonObject>
#include <QJsonArray>
#include <algorithm>
#include <cmath>
#include <utility>

namespace snow_shot::app::mcp {
inline QJsonObject selectionStyleState(const ScreenshotSelectionModel& model) {
    const auto color = [](const QColor& value) {
        return QJsonArray{value.red(), value.green(), value.blue(), value.alpha()};
    };
    return {{QStringLiteral("corner_radius"), model.cornerRadius()},
            {QStringLiteral("shadow_width"), model.shadowWidth()},
            {QStringLiteral("shadow_color"), color(model.shadowColor())},
            {QStringLiteral("border_type"), screenshotSelectionBorderTypeId(model.borderType())},
            {QStringLiteral("border_width"), model.borderWidth()},
            {QStringLiteral("border_color"), color(model.borderColor())},
            {QStringLiteral("aspect_ratio_locked"), model.aspectRatioLocked()}};
}

// Both interactive screenshots and background documents patch the same model.
// Parse into a candidate so invalid fields cannot partially modify either host.
inline bool applySelectionStyle(ScreenshotSelectionModel& model, const QJsonObject& params,
                                QString* field, qreal minimumSelectionSize = 1.0) {
    const auto fail = [field](const QString& key) {
        if (field)
            *field = key;
        return false;
    };
    auto candidate = model;
    for (const auto& key : {QStringLiteral("corner_radius"), QStringLiteral("shadow_width"),
                            QStringLiteral("border_width")}) {
        if (!params.contains(key))
            continue;
        const auto value = params.value(key);
        const double number = value.toDouble(-1);
        if (!value.isDouble() || !std::isfinite(number) || number < 0 || number > 8192 ||
            std::floor(number) != number)
            return fail(key);
        if (key == QStringLiteral("corner_radius"))
            static_cast<void>(candidate.setCornerRadius(static_cast<int>(number)));
        else if (key == QStringLiteral("shadow_width"))
            static_cast<void>(candidate.setShadowWidth(static_cast<int>(number)));
        else
            static_cast<void>(candidate.setBorderWidth(static_cast<int>(number)));
    }
    for (const auto& key : {QStringLiteral("shadow_color"), QStringLiteral("border_color")}) {
        if (!params.contains(key))
            continue;
        const auto values = params.value(key).toArray();
        if (values.size() != 4)
            return fail(key);
        for (const auto& value : values) {
            const double number = value.toDouble(-1);
            if (!value.isDouble() || !std::isfinite(number) || number < 0 || number > 255 ||
                std::floor(number) != number)
                return fail(key);
        }
        const QColor color(values[0].toInt(), values[1].toInt(), values[2].toInt(),
                           values[3].toInt());
        if (key == QStringLiteral("shadow_color"))
            candidate.setShadowColor(color);
        else
            candidate.setBorderColor(color);
    }
    if (params.contains(QStringLiteral("border_type"))) {
        const auto type = params.value(QStringLiteral("border_type"));
        if (type != QStringLiteral("shadow") && type != QStringLiteral("border"))
            return fail(QStringLiteral("border_type"));
        static_cast<void>(candidate.setBorderType(type == QStringLiteral("border")
                                                      ? ScreenshotSelectionBorderType::Border
                                                      : ScreenshotSelectionBorderType::Shadow));
    }
    if (params.contains(QStringLiteral("aspect_ratio_locked"))) {
        const auto locked = params.value(QStringLiteral("aspect_ratio_locked"));
        if (!locked.isBool())
            return fail(QStringLiteral("aspect_ratio_locked"));
        static_cast<void>(
            candidate.setAspectRatioLockEnabled(locked.toBool(), minimumSelectionSize));
    }
    model = std::move(candidate);
    return true;
}

// Validates the complete operand before touching the live model. All coordinates are
// half-open canvas coordinates; out-of-canvas values are rejected, never silently rounded in.
inline bool applySelection(ScreenshotSelectionModel& model, const QRectF& canvas,
                           const QJsonObject& params, QString* field) {
    const auto fail = [field](const QString& value) {
        if (field)
            *field = value;
        return false;
    };
    const QString operation =
        params.value(QStringLiteral("operation")).toString(QStringLiteral("replace"));
    if (operation != QStringLiteral("replace") && operation != QStringLiteral("add") &&
        operation != QStringLiteral("subtract"))
        return fail(QStringLiteral("operation"));
    const QString type = params.value(QStringLiteral("type")).toString(QStringLiteral("rectangle"));
    ScreenshotRegionGeometry operand;
    ScreenshotRegionType regionType = ScreenshotRegionType::Rectangle;
    const auto finite = [](const QJsonValue& v) {
        return v.isDouble() && std::isfinite(v.toDouble());
    };
    if (type == QStringLiteral("rectangle")) {
        const auto b = params.value(QStringLiteral("bounds")).toArray();
        if (b.size() != 4 || !std::all_of(b.begin(), b.end(), finite))
            return fail(QStringLiteral("bounds"));
        const QRectF rect(b[0].toDouble(), b[1].toDouble(), b[2].toDouble(), b[3].toDouble());
        if (rect.width() < 1 || rect.height() < 1 || !canvas.contains(rect))
            return fail(QStringLiteral("bounds"));
        operand = ScreenshotRegionGeometry(rect.toAlignedRect());
    } else {
        if (type != QStringLiteral("polygon") && type != QStringLiteral("polyline") &&
            type != QStringLiteral("freehand"))
            return fail(QStringLiteral("type"));
        const auto points = params.value(QStringLiteral("points")).toArray();
        if (points.size() < 3 || points.size() > 8192)
            return fail(QStringLiteral("points"));
        QPainterPath path;
        for (qsizetype i = 0; i < points.size(); ++i) {
            const auto p = points[i].toArray();
            if (p.size() != 2 || !finite(p[0]) || !finite(p[1]) ||
                !canvas.contains(QPointF(p[0].toDouble(), p[1].toDouble())))
                return fail(QStringLiteral("points[%1]").arg(i));
            const QPointF point(p[0].toDouble(), p[1].toDouble());
            if (i == 0)
                path.moveTo(point);
            else
                path.lineTo(point);
        }
        path.closeSubpath();
        regionType = type == QStringLiteral("freehand") ? ScreenshotRegionType::Freehand
                                                        : ScreenshotRegionType::Polyline;
        operand = ScreenshotRegionGeometry::fromPath(path, regionType);
    }
    const auto current = model.selectionRegion();
    if (operation != QStringLiteral("replace") &&
        (current.toJson().value(QStringLiteral("operands")).toArray().size() >= 64 ||
         current.toJson().value(QStringLiteral("rectangles")).toArray().size() >= 64))
        return fail(QStringLiteral("operation"));
    auto result = operation == QStringLiteral("add")        ? current.united(operand)
                  : operation == QStringLiteral("subtract") ? current.subtracted(operand)
                                                            : operand;
    if (result.isEmpty() || result.boundingRect().width() < 1 || result.boundingRect().height() < 1)
        return fail(QStringLiteral("bounds"));
    model.setRegionType(regionType);
    model.setSelectionRegion(result);
    model.clearAspectRatioPresetForReplacement();
    return true;
}
} // namespace snow_shot::app::mcp
