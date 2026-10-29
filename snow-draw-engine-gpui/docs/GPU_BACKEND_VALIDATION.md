# Retained GPU backend validation

This record covers the Windows D3D11 backend on 2026-10-24. The implementation
uses the GPUI fork at `D:\gpui`, based on
`79fe6a645f68f8c9230578f154bf99852d9f716f`, with default-off `gpu-extensions`.
The package selects Rust 1.98.1 without changing the shared engine workspace's
Rust 1.97.1 toolchain. The machine is a Ryzen 9 5950X with an AMD Radeon RX 6750 XT
(vendor 4098, device 29663). Supported RGBA8 sample counts are 1, 2, 4 and 8.

## Measurement boundaries

All timing benchmarks use `canvas-perf`, which inherits Release with optimization
level 3. `gpu_profile` applies an input mutation, flushes sparse logical deltas,
prepares the affected GPU tiles, and polls a native completion marker. It records
128 edits per opacity/pose group and excludes eight warm-up samples from p95.
GPU timestamp queries are reported separately from CPU preparation. Device initialization and fixture setup
are separate from cold rendering.

The primary workloads contain 10,000 rectangles distributed across 1920x1080 and
3840x2160 surfaces. The 960x640 dense fixture uses 9x6 spacing and is reported
separately. All raster checks use the production GPU graph.

`native_input` posts WM_KEYDOWN to its own native window and measures queue-to-
Present1 return and callback-to-Present1 return independently. It does not measure
physical keyboard latency or scanout. Its native lifecycle checks cover idle,
resize and minimize/restore. It records DPI, refresh rate, dirty pixel counts,
full/partial presentations and the actual adapter.

`gpu_workloads` reports dense text, long-stroke poses, pan/zoom, overlapping
filters, background Smart Erase completion, and an 8192x4096 streaming PNG as
separate groups. A group failure is recorded explicitly and fails the benchmark.
No single local-edit timing represents these heavier operations.

## Deterministic requirements

- Ordinary material/pose edits reuse mesh buffers; source revisions control page
  uploads. Color-font pages and immutable source LOD pages are retained.
- Sparse operation counters bound changed-owner composition and node comparisons.
  The shared engine's 10,000-owner checks also cover text dependencies.
- Overlay changes do not recompose document tiles. Text selection, preedit and
  caret publication remains coherent when document preparation fails.
- Display readback is checked using cumulative service counters. Export staging
  participates in the shared canvas budget and a bounded readback ring.
- Incremental frames are compared with fresh GPU rendering, including sampling
  halos, tile seams, fractional DPI, source pages and pressure-stroke tails.
- Resource accounting includes active, transient and pending-retirement leases.
  Deferred release survives canvas teardown and worker-thread final frame drops.
- The window compositor copies complete dirty regions from its retained target
  into each swapchain buffer before presenting those exact rectangles.

## Focused verification

The related shared document, scene, engine and C ABI checks pass, including
8 document synchronization tests, 8 retained-scene tests, 4 retained-engine tests,
25 ABI patch tests, and the large label-cache regressions. Strict Clippy passes
for those four changed packages. Native GPUI feature-disabled checks and the
fork's required scoped release Clippy workflow pass.

Adapter checks cover device-backed rendering, interaction, GPU exports,
resource accounting, retained source metadata and Smart Erase. Incremental GPU
output is compared with fresh full GPU rendering. Text and vector tests inspect
shared shaping and outlined serialization directly.

Focused adapter runs pass 23 GPU rendering tests, 11 GPU export tests, 11 geometry
tests, 3 retained-scene tests, 12 vector tests, 2 font-cache tests, 71 view tests,
7 document tests and 7 Smart Erase service tests. Integration checks pass all
22 canvas-engine tests, 4 selection-control tests and the GPU smoke test. One
manual font profiler remains ignored by default.

Strict adapter Clippy passes for all targets with `native-capture` enabled.
Rust 1.98.1 ARM64 `--all-targets` compilation passes. The shared core retains its
Rust 1.97.1 toolchain. Formatting and whitespace checks pass in both repositories.
Native ARM64 runtime qualification and Metal/WGPU implementations remain outside
this Windows delivery.

The fork passes 17 focused Windows tests covering native texture ownership,
asynchronous readback and reuse, worker-thread retirement, premultiplied alpha,
scene order, clipping, shadow/filter damage, swapchain rotations, failed
candidates, multiple windows, atlas reuse and resize. Two portable region/order
tests also pass: their original test bodies run unchanged in an isolated harness
against the real public GPUI types, because Cargo refuses an external dependency's
own dev-dependency test target. Its feature-disabled Windows build also passes.
The upstream workspace's uncached unrelated git
dependencies required a small isolated consumer for the feature-disabled check.
The required `script/clippy` workflow used a Cargo adapter that removed only the
inapplicable cross-workspace `--all-features` option and Git Bash's conflicting
linker path; release, all-targets and deny-warnings remained enabled.

Native demo validation passes at 1.5 DPI: creation/selection/resize, pointer
capture and interrupted gestures, undo/redo, pan/zoom, theme changes, custom
crosshairs, UTF-16 preedit and emoji commit, AltGr, and visible selection/caret
behavior. Native captures use deliberate validation readbacks; those are
separate from the zero-readback production display measurements. Physical IME
candidate UI is not automated.

## Recorded performance

The final offscreen report is `artifacts/snow-draw-gpu-after.json`. Each local
edit composes one owner and compares one retained node, renders one 256x256 tile,
and uploads no meshes or source pixels. Uniform uploads are reported separately.
Unchanged submissions render zero tiles and upload zero bytes. Measured display
readback is zero bytes across all cases.

| 10,000-element surface | Edit | Cold render through GPU completion | Warm input through GPU completion p95 | CPU preparation p95 | GPU duration p95 |
| --- | --- | ---: | ---: | ---: | ---: |
| 960x640, dense fixture | Opacity | 90.12 ms | 9.88 ms | 5.43 ms | 2.88 ms |
| 960x640, dense fixture | Pointer movement | 87.56 ms | 10.45 ms | 4.71 ms | 2.84 ms |
| 1920x1080 | Opacity | 93.70 ms | **3.28 ms** | 1.20 ms | 0.84 ms |
| 1920x1080 | Pointer movement | 78.40 ms | **4.55 ms** | 1.37 ms | 0.81 ms |
| 3840x2160 | Opacity | 98.22 ms | **1.59 ms** | 0.35 ms | 0.26 ms |
| 3840x2160 | Pointer movement | 108.76 ms | **2.96 ms** | 0.38 ms | 0.24 ms |

All four requested 1080p/4K opacity/pose cases meet the 8.33 ms target. The denser 960x640
comparison does not; its dirty tile contains more overlapping work. The device initializes in 79.81 ms; logical fixture setup
takes 170.56–193.24 ms separately. Recorded shared-budget peaks range from 6.26
to 67.26 MiB, including retained and pending retirement allocations.
Percentiles in different columns are independent and
should not be added together.

Pose cases use the existing secondary-button quick-move gesture on the tiny
5x3-pixel rectangles. Each edit applies Down, an 8-pixel movement to cross the
engine's drag threshold, the final alternating 1/2-pixel offset, and Up before
one presentation flush. The initial point is 0.5 pixels inside the top-left
corner to avoid overlapping hit tolerances in the dense fixture. Assertions
verify the same owner moves, geometry/material revisions stay unchanged,
and its mesh is not uploaded. These are local movements within one output tile;
pan/zoom and long strokes are measured separately.

The native report is `artifacts/snow-draw-gpu-native-input.json`: RX 6750 XT,
1920x1080 physical pixels, 1.5 DPI, 144 Hz. Posted WM_KEYDOWN to `Present1` return
is **6.87 ms p95**; native callback to `Present1` return is **6.48 ms p95**.
All 64 measured edits present 66,306 pixels (3.20% of the window), with zero
full presents. Startup and editing readback counters are both zero. Idle adds
zero presents, and real resize/minimize/restore checks pass. These numbers do not
include physical keyboard latency or display scanout.

The separate workload report is `artifacts/snow-draw-gpu-workloads.json`:

Font fallback resolution caches the exact character and ordered excluded-font
IDs, including failed lookups, while preserving the selector and glyph positions.

| Workload | Measurement |
| --- | ---: |
| 1,000 mixed-script text elements, cold frame | 174.16 ms |
| 1,000 text elements, warmed local edit | 3.40 ms p95 |
| 8,192-point stroke, rigid pointer drag | 10.00 ms p95 |
| 1,200 elements, combined pan/zoom | 14.01 ms p95 |
| Six overlapping filters, local edit | 1.64 ms p95 |
| Smart Erase CPU solve, 512x384 source | 3.12 ms |
| Smart Erase result through GPU completion | 0.80 ms |
| 8192x4096 streaming PNG, GPU readback and encoding | 382.87 ms |

Long strokes and pan/zoom exceed 8.33 ms and are reported separately from the
specified warmed local-edit acceptance cases. Smart Erase uploads its 10,368-byte
cropped patch. Large export reads back 128 MiB using two staging allocations,
1,022 staging reuses, and at most 256 KiB pending staging in this fixture; it
never allocates a full output RGBA image. Export timing uses nonblocking owner
polls with thread yields and includes encoding, but no UI timer delay.

GPU showcase exports and native captures are available at
`artifacts/snow-draw-gpu-after.png`, `artifacts/snow-draw-gpu-native/` and
`artifacts/snow-draw-gpu-native-text/`.

## Reproduce

```powershell
cd D:\snow-apps-workspace-2\snow-apps\snow-draw-engine-gpui
cargo test --locked --offline --lib gpu_tests -- --test-threads=1
cargo test --locked --offline --lib gpu_export::tests -- --test-threads=1
cargo test --locked --offline --lib gpu_memory::tests -- --test-threads=1
cargo test --locked --offline --lib view:: -- --test-threads=1
cargo bench --locked --offline --profile canvas-perf --bench gpu_profile -- ..\artifacts\snow-draw-gpu-after.json
cargo bench --locked --offline --profile canvas-perf --bench gpu_workloads -- ..\artifacts\snow-draw-gpu-workloads.json
cargo bench --locked --offline --profile canvas-perf --bench native_input -- ..\artifacts\snow-draw-gpu-native-input.json
cargo check --locked --offline --target aarch64-pc-windows-msvc --all-targets
```

Only affected packages and focused fork filters are used. No repository-wide
suite is required or was run for this work.
