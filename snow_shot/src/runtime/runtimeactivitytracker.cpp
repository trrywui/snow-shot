#include "snow_shot/runtime/runtimeactivitytracker.h"

namespace snow_shot::runtime {
RuntimeActivityTracker& RuntimeActivityTracker::shared() {
    // One out-of-line definition keeps all static-library callers on the same
    // tracker, including macOS builds that mix native and LLVM bitcode objects.
    // Detached native cleanup may complete during static teardown, so admission
    // state intentionally lasts until process exit.
    static auto* tracker = new RuntimeActivityTracker;
    return *tracker;
}
} // namespace snow_shot::runtime
