// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef SNOW_SHOT_PRESENTATION_CANVASTEXTEDITINGPREFERENCES_H
#define SNOW_SHOT_PRESENTATION_CANVASTEXTEDITINGPREFERENCES_H

#include "snow_shot/storage/applicationstorage.h"
#include "snow_shot/storage/settingsadapters.h"
#include "snow_draw_engine_qt/snow_canvas_widget.h"

namespace snow_shot::presentation {

inline void bindCanvasTextEditingPreferences(SnowCanvasWidget& canvas) {
    auto& applicationStorage = storage::ApplicationStorage::instance();
    if (!applicationStorage.isInitialized()) {
        return;
    }
    canvas.setDeleteTextAfterCursor(storage::DrawingSettings().deleteTextAfterCursor());
    QObject::connect(&applicationStorage.configuration(),
                     &storage::ConfigurationStore::valueChanged, &canvas,
                     [&canvas](const QString& key, const QJsonValue& value) {
                         if (key == QStringLiteral("drawing/delete_text_after_cursor")) {
                             canvas.setDeleteTextAfterCursor(value.toBool());
                         }
                     });
}

} // namespace snow_shot::presentation

#endif // SNOW_SHOT_PRESENTATION_CANVASTEXTEDITINGPREFERENCES_H
