#ifndef SNOW_SHOT_IMAGE_SCREENSHOTSELECTIONBORDERTYPE_H
#define SNOW_SHOT_IMAGE_SCREENSHOTSELECTIONBORDERTYPE_H

#include <QString>

enum class ScreenshotSelectionBorderType { Shadow, Border };

inline QString screenshotSelectionBorderTypeId(ScreenshotSelectionBorderType type) {
    return type == ScreenshotSelectionBorderType::Border ? QStringLiteral("border")
                                                         : QStringLiteral("shadow");
}

#endif // SNOW_SHOT_IMAGE_SCREENSHOTSELECTIONBORDERTYPE_H
