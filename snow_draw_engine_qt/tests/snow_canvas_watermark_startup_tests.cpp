// SPDX-License-Identifier: Apache-2.0
#include "snow_canvas_watermark_renderer.h"
#include "windows_early_tls.h"

#include <cstdio>

int main() {
    if (!snow_test::early_tls::initializedBeforeCrt()) {
        std::fputs("FAIL: early TLS callback did not run.\n", stderr);
        return 1;
    }
    if (snow_canvas_renderer::watermarkPlacementWorkspaceBytesForCurrentThread() != 0)
        return 1;
    snow_canvas_renderer::resetWatermarkRenderCacheForCurrentThread();
    if (snow_canvas_renderer::watermarkPlacementWorkspaceBytesForCurrentThread() != 0)
        return 1;
    std::puts("PASS: watermark workspace initializes after CRT startup.");
    return 0;
}
