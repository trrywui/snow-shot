// SPDX-License-Identifier: GPL-3.0-or-later
#include "snow_shot/presentation/screenshotselectionshadowrenderer.h"
#include "windows_early_tls.h"

#include <cstdio>

int main() {
    if (!snow_test::early_tls::initializedBeforeCrt()) {
        std::fputs("FAIL: early TLS callback did not run.\n", stderr);
        return 1;
    }
    const auto diagnostics = ScreenshotSelectionShadowRenderer::diagnosticsForCurrentThread();
    if (diagnostics.retainedBytes != 0 || diagnostics.retainedEntries != 0)
        return 1;
    ScreenshotSelectionShadowRenderer::resetCacheForCurrentThread();
    if (ScreenshotSelectionShadowRenderer::diagnosticsForCurrentThread().retainedBytes != 0)
        return 1;
    std::puts("PASS: screenshot shadow cache initializes after CRT startup.");
    return 0;
}
