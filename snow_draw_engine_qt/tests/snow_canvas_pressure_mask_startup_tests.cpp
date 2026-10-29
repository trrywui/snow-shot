// SPDX-License-Identifier: Apache-2.0
#include "snow_canvas_pressure_mask_atlas.h"
#include "snow_canvas_display_item.h"
#include "windows_early_tls.h"

#include <Windows.h>

#include <cstdio>

// The startup fixture never renders. Keep unrelated renderer TLS out of this
// executable, and fail immediately if a scene query is accidentally reached.
const std::vector<SnowCanvasSceneItem::PathChunk>& SnowCanvasSceneItem::pressurePathChunks() const {
    ExitProcess(82);
}

void SnowCanvasSceneItem::queryPressurePathChunks(const QRectF&,
                                                  std::vector<std::uint32_t>*) const {
    ExitProcess(82);
}

QRectF SnowCanvasSceneItem::pressureCanvasBounds() const {
    ExitProcess(82);
}

int main() {
    if (!snow_test::early_tls::initializedBeforeCrt()) {
        std::fputs("FAIL: early TLS callback did not run.\n", stderr);
        return 1;
    }
    const auto diagnostics = snow_canvas_pressure_mask::diagnosticsForCurrentThread();
    if (diagnostics.retainedBytes != 0 || diagnostics.entries != 0)
        return 1;
    snow_canvas_pressure_mask::setCacheByteBudgetForCurrentThread(1024);
    snow_canvas_pressure_mask::clearCacheForCurrentThread();
    std::puts("PASS: pressure masks initialize on first use after CRT startup.");
    return 0;
}
