// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include "../../test-support/canvas_quick_selection_test_support.h"
#include "snow_shot/storage/settingsadapters.h"
#include <QScopeGuard>

namespace canvas_automatic_selection_test {
using canvas_quick_selection_test::mouse;
using canvas_quick_selection_test::require;

template <typename CreateHost> void exercise(CreateHost createHost) {
    const snow_shot::storage::DrawingSettings settings;
    const bool original = settings.automaticallySelectNewlyCreatedElements();
    const auto tools = settings.quickSelectionDisabledTools();
    const auto exclusions = settings.automaticSelectionExcludedTools();
    const auto restore = qScopeGuard([&] {
        require(settings.setAutomaticallySelectNewlyCreatedElements(original) &&
                    settings.setQuickSelectionDisabledTools(tools) &&
                    settings.setAutomaticSelectionExcludedTools(exclusions),
                "restore drawing preferences");
    });
    require(settings.setAutomaticallySelectNewlyCreatedElements(true) &&
                settings.setQuickSelectionDisabledTools({QStringLiteral("shape")}) &&
                settings.setAutomaticSelectionExcludedTools({QStringLiteral("shape")}),
            "set automatic selection before constructing the host");
    createHost([&](SnowCanvasWidget& canvas) {
        canvas.resize(400, 300);
        canvas.setInteractionEnabled(true);
        const auto draw = [&](bool selected) {
            require(canvas.resetEditingState() && canvas.setCanvasTool(SnowCanvasTool::Shape),
                    "activate host shape creation");
            mouse(canvas, QEvent::MouseButtonPress, {60, 60}, Qt::LeftButton, Qt::LeftButton);
            mouse(canvas, QEvent::MouseMove, {160, 120}, Qt::NoButton, Qt::LeftButton);
            mouse(canvas, QEvent::MouseButtonRelease, {160, 120}, Qt::LeftButton, Qt::NoButton);
            require(canvas.canvasStyleToolbarState().selectedElementCount == (selected ? 1u : 0u),
                    "host applies automatic selection");
            require(canvas.canvasTool() == SnowCanvasTool::Shape, "host retains the drawing tool");
            require(canvas.undo(), "remove the created test shape");
        };
        draw(false);
        require(settings.setAutomaticSelectionExcludedTools({}), "clear exclusions live");
        draw(true);
        require(settings.setAutomaticallySelectNewlyCreatedElements(false), "disable live");
        draw(false);
        require(settings.setAutomaticallySelectNewlyCreatedElements(true), "enable live");
        draw(true);
        require(settings.setAutomaticSelectionExcludedTools({QStringLiteral("shape")}),
                "exclude shape live");
        draw(false);
    });
}
} // namespace canvas_automatic_selection_test
