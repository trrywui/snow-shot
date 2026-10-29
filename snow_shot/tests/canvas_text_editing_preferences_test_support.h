// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include "snow_shot/storage/settingsadapters.h"
#include "snow_draw_engine_qt/snow_canvas_widget.h"

#include <QScopeGuard>
#include <cstdlib>
#include <iostream>

namespace canvas_text_editing_preferences_test {

inline void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(EXIT_FAILURE);
    }
}

template <typename CreateHost> void exercise(CreateHost createHost) {
    const snow_shot::storage::DrawingSettings settings;
    const bool original = settings.deleteTextAfterCursor();
    const auto restore = qScopeGuard([&] {
        require(settings.setDeleteTextAfterCursor(original), "restore forward Delete preference");
    });
    require(settings.setDeleteTextAfterCursor(true), "enable forward Delete before host creation");
    createHost([&](SnowCanvasWidget& canvas) {
        require(canvas.deleteTextAfterCursor(),
                "new host loads the saved forward Delete preference");
        require(settings.setDeleteTextAfterCursor(false) && !canvas.deleteTextAfterCursor(),
                "open host observes forward Delete being disabled");
        require(settings.setDeleteTextAfterCursor(true) && canvas.deleteTextAfterCursor(),
                "open host observes forward Delete being enabled");
        require(canvas.resetEditingState() && canvas.deleteTextAfterCursor(),
                "resetting editing state preserves the application preference");
    });
    require(settings.setDeleteTextAfterCursor(false),
            "preference changes remain safe after the host and its connections are destroyed");
}

} // namespace canvas_text_editing_preferences_test
