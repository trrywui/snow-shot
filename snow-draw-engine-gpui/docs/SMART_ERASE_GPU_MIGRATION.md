# Smart Erase GPU migration

The migration keeps reconstruction in the immutable source grid. Display camera,
DPI and opacity never change reconstruction identity. Eligible source layers are
the only observations: annotations, visual-only layers, masked colors and coverage
gaps cannot enter donor patches, guides or pyramid observations.

Stage profiling, adaptive regions, layered GPU donor assembly, multiscale GPU
reconstruction and resident result display/export are implemented on Windows D3D11.
Workers rasterize source-grid masks and retain cheap affine/periodic CPU paths.
Layered texture inputs and exact GPU-backed inputs stay resident through domain,
model and pyramid preparation. Native input has been measured with isolated GPU
reconstruction active, and production-view result latency is measured separately.
Event-driven completion wakes, paired native crossover measurements and bounded
automatic CPU routing are implemented. The qualification below covers Windows
x64 on the local adapter; driver first-use stalls remain an explicit limit.

## Implementation sequence

1. **Implemented:** add opt-in CPU stage profiles and focused, branch-checked workloads. Establish
   a Release baseline before changing donor neighborhoods or algorithms.
2. **Implemented:** replace unconditional 512-pixel donor neighborhoods with adaptive regions and
   bounded escalation. Verify donor support, transparency and reconstruction quality.
3. **Implemented:** run the multiscale solver on GPUI's D3D11 device. Keep guides, conservative
   pyramids, match fields and voting buffers resident. Parallel propagation uses
   explicit read/write fields and deterministic per-pixel random sequences.
4. **Implemented:** retain immutable completed GPU patches and consume them directly in display
   and export. Preserve CPU fallback and frozen export identity. Charge transient,
   published and retiring allocations to the shared device budget.
5. **Implemented:** move eligible layered
   source assembly to the GPU while preserving clipping, placement, coverage,
   alpha and source-pixel sampling rules.
6. **Implemented and locally qualified:** profile CPU preparation, transfers, GPU execution and
   owner-thread submission, native latency under compute load and production-view
   result presentation. Native fence events replace completion timers where supported.
   Exact-geometry CPU/GPU pairs establish the bounded tiny-raster CPU route and
   host overrides. First-process and driver-stall measurements determine the
   allocation policy and its limits. Source-grid masks and affine/periodic models
   remain on CPU where transfer/setup costs exceed their work.

GPU submission and completion polling belong to the GPUI service owner thread.
Jobs submit bounded batches, yield between batches, reject obsolete completion,
and retain submitted resources through cancellation until retirement. No windowed
host creates a second device. Device epochs scope all GPU caches.

## Focused measurements

`SmartEraseService::solve_profiled` resets a caller-owned `SmartEraseProfile` on
every call. It reports result-cache hits separately, the selected solver, source
region, masked/donor pixel counts, estimated solver scratch, pyramid work counts
and non-overlapping completed CPU stage times. It does not change result identity,
pixels or cancellation. Ordinary `solve` calls do not read the clock or allocate
profiling records. Failed calls retain available diagnostics; partial-stage time
is included in total elapsed time but is not attributed to a completed stage.

## Adaptive donor regions

The first region uses a 64–128 source-pixel halo based on the default donor reach.
If it has no observations, or the selected domain plus sixteen pixels of guide
support would cross an artificial region edge, reconstruction requests a larger
region before running periodic/patch synthesis. Halos double up to the original
512-pixel maximum. Natural source edges do not force pointless retries. Every
attempt is cancellation-aware and checked against the existing working-pixel and
scratch limits before its expensive allocations. Only completed patches enter
the result cache.

Regions align outward to the original neighborhood's 32-pixel pyramid grid.
Spatial penalties and random-search windows retain the original neighborhood
extent at each level. Cropping otherwise changes the search algorithm, which
caused a brush regression in an initial experiment. A deterministic brush test
compares the cropped result directly with full-context reconstruction. Additional
tests check donor escalation/cancellation, large flat masks, textured bands,
transparent gaps and independence from masked colors. The optimized path can now
solve a centered 512×512 flat mask in a 2048×2048 source without exceeding the
unchanged working-region limit.

Run the standalone Rust package's Release-derived `canvas-perf` profile in the
repository's `windows-msvc-performance` build environment:

```powershell
cd snow-draw-engine-gpui
.\scripts\run-smart-erase-perf.ps1 -OutputFile ..\artifacts\snow-draw-smart-erase.json
# One related workload, or a different number of independent solve samples:
.\scripts\run-smart-erase-perf.ps1 -Workload texture_medium -Samples 10
```

Each workload warms up once and then constructs a fresh service per solve sample.
Source allocation and fixture construction are excluded. The benchmark asserts the
expected solver branch and reports cache-hit time separately. Workloads cover a
flat small mask, the same mask in a larger source, periodic patterns, two texture
sizes, transparent gaps, a long brush and layered donor exclusion. All workload
failures are written to the report and fail the process. Small sample counts are
exploratory; use more samples when comparing percentiles for a release decision.

The CPU benchmark excludes GPU upload, GPU execution and presentation. The
existing `gpu_workloads` Smart Erase fixture reports patch upload and result
presentation separately; its flat-background timing is not a texture-solver
baseline. GPU migration measurements must report cold eligible-source uploads,
resident-source reuse, solver GPU timestamps and final composition separately.

### Initial local measurements

Windows x64, pinned Rust 1.98.1, optimized `canvas-perf`. Before uses five solve
samples and after uses ten, following one warmup per workload; these exploratory
medians do not establish release percentile guarantees. Reports are generated
under ignored `artifacts/` as `snow-draw-smart-erase-baseline.json` and
`snow-draw-smart-erase-adaptive-aligned.json`.

| Workload | Before median | After median | Before working pixels | After working pixels |
| --- | ---: | ---: | ---: | ---: |
| 32×32 flat mask, 1920×1080 source | 19.60 ms | 0.55 ms | 1,115,136 | 36,864 |
| 128×96 texture mask, 512×384 source | 224.87 ms | 145.69 ms | 196,608 | 102,400 |
| Layered source, 64×48 flat mask | 25.45 ms | 5.57 ms | 393,216 | 49,152 |

All eight initial CPU workloads select their expected branch and complete. Grid alignment
leaves some small fixtures at the same dimensions as before; timing differences
for those fixtures should be treated as run variability, not ROI speedups.

### Multiscale GPU measurements

```powershell
.\scripts\run-smart-erase-perf.ps1 -Backend gpu -Samples 10 -OutputFile ..\artifacts\snow-draw-smart-erase-gpu.json
```

Local Windows x64, Radeon RX 6750 XT, optimized `canvas-perf`, ten measured
GPU solves after one cold solve per workload. The CPU reference is one profiled
solve per workload; the comparison is exploratory, not a paired CPU/GPU percentile
study. Both paths use the same adaptive source ROI and original search extent.

| Workload | CPU reference | GPU core median | GPU core p95 |
| --- | ---: | ---: | ---: |
| Small texture mask | 39.64 ms | 9.36 ms | 13.64 ms |
| Medium texture mask | 174.00 ms | 19.57 ms | 19.81 ms |
| Transparency gaps | 35.62 ms | 8.83 ms | 9.26 ms |
| Long brush | 54.90 ms | 9.07 ms | 9.29 ms |

GPU core duration includes CPU cropping/mask/domain preparation, owner allocation,
uploads, submission and completion. It excludes the production affine/periodic
classifier, the view's 1 ms polling timers and final display composition. Active
GPU time is approximately 5.6–12.8 ms across these cases. Individual owner polls
peaked at 0.24 ms; initial resource allocation/upload is separate and peaked at
3.55 ms. Larger ROIs and native interaction need qualification before claiming a
frame-time guarantee. The tight benchmark loop polls much more often than the view.

These initial captures read back 320 bytes of control metadata and no reconstruction image.
The report separates source/flag upload bytes from constant-buffer updates. Cold
uploads are 393,216–819,200 bytes in these fixtures. This earlier core benchmark
uploads CPU-prepared inputs; the resident pipeline below reuses donor pages and
uses the current 352-byte metadata packet. The runtime's peak memory counter covers the shared device and
is cumulative across workloads. GPU timestamps cover submitted batches, excluding
CPU preparation and event-loop gaps.

### Native interaction during GPU reconstruction

```powershell
.\scripts\run-native-input-perf.ps1 -Load smart-erase
# Matching interaction fixture without reconstruction:
.\scripts\run-native-input-perf.ps1 -Load idle
```

The window benchmark uses the same GPUI device for 10,000 retained shapes and a
512×384 reconstruction grid with a 128×96 mask. This grid intentionally includes
the full fixture; the isolated medium benchmark uses a smaller adaptive ROI.
Posted F24 messages edit the selected shape while compute runs. It asserts that
every callback overlaps an active reconstruction, and checks idle presentation,
resize/minimize restoration and zero display readback. There are eight warmups
and sixty-four measured samples. This isolates compute coexistence: it excludes
worker classification, input preparation and physical scanout, and posts input
after the separately reported initial allocation.

The initial implementation requested a 1 ms timer after every owner poll. On this
Windows run those wakes were about 15 ms apart: median reconstruction latency was
245.90 ms despite 37 ms of active GPU work. A timer-resolution guard did not fix
that delay and is not retained. Ready batches now resume after an event-loop yield;
only GPU completion waits requested a timer in that implementation. The corrected run reports:

| Measurement | Local result |
| --- | ---: |
| Reconstruction median / p95 | 57.81 / 58.86 ms |
| Native queue to Present1 return p95 | 5.84 ms |
| Native callback to Present1 return p95 | 5.51 ms |
| Active GPU median | 35.73 ms |
| Largest measured owner poll / allocation | 0.52 / 3.99 ms |
| Display readback / idle presentations | 0 bytes / 0 |

All seventy-two samples observed reconstruction still active when presentation
was reported. Cold allocation reached 9.83 ms in the corrected run. An earlier
cold run observed a 27.53 ms owner poll, so neither the warmed p95 nor the poll's
encoding budget is a hard upper bound on driver calls. Shader warmup/allocation
reuse and larger inputs remain qualification work. Reports are
`snow-draw-native-smart-erase.json` (initial) and
`snow-draw-native-smart-erase-yield.json` (corrected) under ignored `artifacts/`.
The matching idle run (`snow-draw-native-smart-erase-idle.json`) measured 6.82 ms
queue-to-present p95. These separate runs show no observed input-latency regression
in this fixture; the small difference does not establish a latency improvement.

### Production-view result latency

```powershell
.\scripts\run-native-input-perf.ps1 -Load view-layered
.\scripts\run-native-input-perf.ps1 -Load view-texture
.\scripts\run-native-input-perf.ps1 -Load view-affine
.\scripts\run-native-input-perf.ps1 -Load view-periodic
```

These modes post F24 through the native queue and replace one rectangle filter
using ordinary engine commands. The production `CanvasView` prepares workers,
selects a backend, solves and publishes the result. Each run keeps a 512×384 source,
a 128×96 mask and 10,000 shapes. Every mask has distinct geometry and element
identity; completed results remain in the ordinary bounded cache. Source/scene
fixture construction is excluded. There are eight warmups and sixty-four measured
commands. The harness verifies the current filter's completed patch, a settled
view and a presentation after its native callback; the reported endpoint is the
service's Present1 return timestamp, excluding scanout. Timer-based observation
can lag that timestamp by about 13 ms and is reported separately.

| Source/model | Selected backend | Timer captures median / p95 | Event captures median / p95 |
| --- | --- | ---: | ---: |
| Raster affine | CPU | 52.57 / 60.18 ms | 61.99 / 69.07 ms |
| Raster periodic | CPU | 53.34 / 61.55 ms | 61.88 / 69.07 ms |
| Raster texture | GPU multiscale | 103.49 / 118.52 ms | 96.93 / 128.46 ms |
| Layered texture | GPU assembly/preparation/multiscale | 103.14 / 117.55 ms | 89.85 / 103.83 ms |

All four runs report zero editing image readback and zero idle presentations.
The first layered/raster texture commands took 142.86/115.06 ms; these are first
commands in their processes after earlier GPU measurements, not independent cold
driver-start distributions. Reports are `snow-draw-native-view-{affine,periodic,
texture,layered}.json` and their `-events.json` counterparts under ignored `artifacts/`.

Result latency includes several frame/event-loop turns and is materially higher
than isolated pipeline execution. These numbers measure completion of an erasure,
whereas the preceding native-input benchmark measures responsiveness while compute
runs. The four rows use different solver workloads and cannot establish a paired
CPU/GPU texture crossover. The before/after captures are separate process runs:
layered completion improves locally, while raster p95 and CPU fast-path captures
show run variability/regressions that need further qualification. No universal
full-view improvement or automatic mask-area cutoff is inferred from them.

### Event-driven completion

`GpuSmartErase::completion_wake` and `GpuCanvas::completion_wake` use the same
device-epoch notifier implementation. On supported devices, an
[immediate-context fence signal](https://learn.microsoft.com/en-us/windows/win32/api/d3d11_3/nf-d3d11_3-id3d11devicecontext4-signal)
follows submitted work, and a Windows thread-pool wait wakes the owner task when
its event signals. Callbacks never access an immediate context. Every wake leads
to another ordinary job/renderer readiness check, preserving cancellation,
device validation and publication authority. A 500 ms watchdog permits those
checks to proceed during a stalled device; unsupported fence capabilities retain
timer polling. Ready command batches continue to yield cooperatively.

Dropping a wake first disarms its wait and cancels/joins callbacks, following the
[Windows wait-object cleanup sequence](https://learn.microsoft.com/en-us/archive/msdn-magazine/2011/november/windows-with-c-thread-pool-synchronization).
The callback holds its own state reference and disassociates before invoking the
executor, avoiding a callback waiting for itself. Registered event/fence handles
then retire through the service's existing deferred-release queue, including
early cancellation. Retirement payloads do not retain a service reference.

```powershell
.\scripts\run-native-input-perf.ps1 -Load smart-erase-polling
.\scripts\run-native-input-perf.ps1 -Load smart-erase
```

Local same-build control: 512×384 reconstruction, 128×96 mask, 10,000 shapes,
eight warmups and sixty-four measured samples per process. Both runs use 112
dispatches and two levels. Initial owner allocation precedes input posting, as
in the earlier compute-coexistence benchmark. Only compute-completion scheduling
differs between these modes; both use the current production renderer.

| Measurement | Timer control | Completion events |
| --- | ---: | ---: |
| Reconstruction median / p95 | 44.42 / 60.08 ms | 21.95 / 25.69 ms |
| Native queue to Present1 return p95 | 5.19 ms | 5.22 ms |
| Native callback to Present1 return p95 | 4.85 ms | 4.79 ms |
| Completion event wakes across 64 measured solves | 0 | 128 |
| Largest warm owner poll | 0.34 ms | 1.03 ms |

Both runs have zero editing image readback and idle presentations; all input
callbacks overlap reconstruction. Their separate process captures are exploratory,
with no hard driver-call or release-percentile guarantee. Reports are
`snow-draw-native-smart-erase-polling-control.json` and
`snow-draw-native-smart-erase-events.json`. These measurements concern scheduling;
shader algorithms and work counts are unchanged by the notifier.

## GPU-resident image contract

`SourceImage::from_gpu_texture` retains an immutable, premultiplied GPUI texture
without readback. It preserves source-grid placement and image identity; display
and raster export use that image on the same device/epoch. `raster_pixels()`
returns `None`, while `gpu_texture()` exposes the retained texture. This permits
completed reconstruction patches to keep the existing `SourceImage` API without
inventing CPU pixels or requiring an immediate GPU-to-CPU-to-GPU round trip.

The renderer validates device identity and epoch before binding a source, retains
the source through submitted GPU work, and builds reusable premultiplied LODs on
the GPU. Source placement, fractional image coverage and parent-layer clipping
follow the existing raster-source rules. CPU donor access fails explicitly until
materialization; it never treats an unavailable GPU raster as transparent donors.

`GpuSavePreparation` asynchronously materializes GPU-backed inputs before CPU
encoding, just as it materializes layered sources. Reconstructed leaf images retain
their exact straight RGBA buffers for persistence; saving them reads those buffers
instead of unpremultiplying the display texture. Portable SVG checks reject
GPU-only image leaves so exports select the existing GPU raster fallback. No
editing-time readback is required by these source-image paths.

## Eligible donor assembler

`GpuDonorInput::new` prepares only placement/coverage coordinate tables on a
worker. `GpuDonorAssembler` submits bounded page draws on the existing GPUI owner
thread and publishes a GPU-backed source with both straight RGBA and premultiplied
display storage. No donor image is read back. `CanvasView` uses this path for
layered texture inputs and exact GPU-backed sources. Domain and model selection
consume the assembled source directly on the same context.

The input retains normalized, ordered leaf layers. Visual-only layers are skipped
before GPU validation/upload; eligible CPU leaves honor saved donor-pixel overrides.
CPU-generated 16-bit fractional texel coordinates follow the existing sampler's
evaluation order at large world coordinates. Per-layer bilinear sampling uses
premultiplied alpha, then rounds to straight RGBA before the existing source-over
blend/truncation rules. Scalar arithmetic and correctly rounded division preserve
the CPU's byte results; vector reassociation changed a few truncation boundaries
in an initial experiment. Filtered layered sampling requires D3D11 double-precision
operations and 11.1 extended double instructions; unsupported devices retain the
CPU preparation path. Plain leaf copies use a separate shader without those
optional instructions, including copies from exact GPU-backed leaves.

A 64 MiB LRU retains immutable 256-pixel raw source pages with a one-texel sampling
halo. Only intersecting pages upload, once per source identity/page. Overlapping
ROIs reuse them without copying CPU pixel data again. All allocations, including
coordinate tables, cache pages, outputs and retiring work, use the shared device
budget. Existing exact GPU-backed leaves bind their straight buffers directly;
GPU textures without that exact buffer require explicit materialization and are
rejected by this input path. Results from another device/epoch are rejected before
allocating a new job.

Device-backed tests cover exact leaf bytes (including hidden RGB at zero alpha),
all 65,536 foreground/background alpha combinations, fractional anisotropic
sampling, large coordinates, nested clips, filtered page seams, overlapping ROI
reuse, visual-only donor exclusion, cancellation during/after submission, partial
allocation failure, foreign devices and device retirement.

```powershell
.\scripts\run-smart-erase-perf.ps1 -Backend donors -Samples 10 -OutputFile ..\artifacts\snow-draw-gpu-donor-assembly.json
```

The benchmark adds layered texture and transparency fixtures to the existing
layered flat fixture. Each uses the CPU solver's adaptive 256×192 donor region.
CPU numbers below are one profiled assembly reference; GPU numbers are ten
measured assemblies after one cold run. They measure this stage, not complete
Smart Erase latency. GPU time includes input tables, owner allocation, submission
and completion polling, with a resident raw-page cache and no UI timer/display.

| Workload | CPU assembly reference | GPU assembly median | GPU assembly p95 | Cold GPU assembly |
| --- | ---: | ---: | ---: | ---: |
| Layered flat | 2.15 ms | 0.54 ms | 0.80 ms | 7.30 ms |
| Layered texture | 4.94 ms | 1.03 ms | 1.12 ms | 2.37 ms |
| Layered alpha | 5.05 ms | 0.99 ms | 1.02 ms | 1.47 ms |

Cold uploads were 527,364–791,560 bytes. Every measured warm run uploaded zero
source bytes; coordinate tables were 7,168–14,336 bytes. Owner polls peaked at
0.25 ms in these samples. The flat fixture's first run includes driver shader
warmup, so cold numbers are not a distribution of independent cold starts.

GPU domain construction, constant/periodic model detection and conservative pyramid
preparation follow donor assembly without an intermediate completion wait. One
352-byte packet supplies domain support, model choice, adaptive-margin status and
pyramid counts. The multiscale solver consumes the prepared pyramid without a
second metadata readback or input upload. Truncated ROIs skip pyramid work and
request the next halo before synthesis. Domain flags match the CPU reference,
including alpha gaps, remote donors and odd image extents.

For CPU-accessible layers, a worker samples only the masked bounds and a four-pixel
ring to preserve the existing cheap affine result. Periodic hints route to the
ordinary CPU periodic solver. Inputs without a successful CPU fast model use GPU
preparation; GPU-only sources can select constant/periodic fills without image
readback. These fills avoid multiscale match/vote scratch allocations. GPU
capability, native texture-size and allocation failures retain CPU fallback when
the original source is CPU-accessible.

### Resident pipeline measurements

```powershell
.\scripts\run-smart-erase-perf.ps1 -Backend pipeline -Samples 10 -OutputFile ..\artifacts\snow-draw-gpu-resident-pipeline.json
```

Local Windows x64, Radeon RX 6750 XT, optimized `canvas-perf`. GPU measurements
include coordinate preparation, mask conversion, allocation, uploads, donor
assembly, domain/model/pyramid preparation and reconstruction through completion.
The rectangular mask fixture is constructed before timing; worker fast-model
classification, UI timer waits and final composition are excluded. The CPU
reference is one profiled solve; GPU medians/p95 use ten runs after one cold run.
These are stage-pipeline comparisons, not complete-view or paired release percentiles.

| Workload | CPU solve reference | GPU pipeline median | GPU pipeline p95 |
| --- | ---: | ---: | ---: |
| Small affine | 0.92 ms | 4.85 ms | 5.74 ms |
| Affine in large source | 0.72 ms | 3.02 ms | 3.14 ms |
| Periodic | 2.12 ms | 3.85 ms | 4.22 ms |
| Small texture | 35.06 ms | 11.12 ms | 12.59 ms |
| Medium texture | 134.97 ms | 20.07 ms | 21.18 ms |
| Alpha gaps | 34.11 ms | 10.65 ms | 11.83 ms |
| Layered flat | 3.27 ms | 3.42 ms | 3.66 ms |
| Layered texture | 39.07 ms | 11.63 ms | 12.14 ms |
| Layered alpha | 38.35 ms | 11.66 ms | 11.78 ms |

All warm assemblies uploaded zero source bytes. Each preparation read 352 control
bytes; resident reconstruction uploaded zero input bytes and read zero control
bytes. The largest warm owner poll was 0.21 ms; warm job construction peaked at
0.91 ms. The first cold constant-model run took 129.63 ms, including a 116.59 ms
owner poll consistent with driver shader warmup. This exceeds the dispatch
encoding budget and remains a driver first-use limitation. Later cold
texture runs took 21.94–33.81 ms after shared kernels had warmed. Solver branch and masked
pixel counts are checked against the CPU reference. Affine/periodic measurements
support retaining the cheap CPU paths; they do not establish a texture crossover
threshold. `GpuSourcePrepareProfile`, donor profiles and `GpuSmartEraseProfile`
separate allocation/upload, dispatch, control and GPU timestamp costs.

## Compute integration

The CPU worker prepares immutable mask/coordinate inputs and tries cheap
affine/periodic branches. Raster leaves retain CPU domain preparation; layered
texture and exact GPU-backed inputs use resident preparation. Unresolved
multiscale inputs go to `GpuSmartEraseJob` on the existing GPUI owner thread.
`CanvasView` seeds only completed, uncancelled results; device,
allocation or capability failures use the same source-grid CPU fallback. Both
paths share adaptive ROI escalation and reconstruction identity.

Dedicated `cs_5_0` kernels use structured buffers for float colors, integer flags,
match fields, costs and donor counts. Compute allocations account for their actual
4/16-byte strides in `GpuMemory::shared`. Published patches retain two straight
RGBA buffers and two premultiplied display textures, totaling sixteen bytes per
retained pixel. Cancellation keeps submitted allocations charged until retirement.

Conservative pyramid reduction, Lab/guide statistics, donor compaction, matching
and voting execute on the GPU. Propagation uses explicit input/output fields,
deterministic per-pixel randomness and frozen integer reuse counts. Donor/hole
compaction uses deterministic row prefixes. Original search extent and pyramid
phase are retained. Exact separable Manhattan-distance sweeps, with explicit seed
tie-breaking, match the CPU BFS in device-backed tests. A metadata copy supplies
active levels and sparse target counts; image buffers stay resident between stages.

Only masked RGB changes; alpha and unmasked straight bytes remain exact. Display
textures are generated after final RGB rounding. `SmartErasePatch` exposes the
completed GPU-backed sources through the existing image accessors. Frozen export
and cache reuse validate device identity and epoch; stale entries do not suppress
a replacement solve after device loss.

Owner polls submit at most eight dispatches and check a two-millisecond encoding
budget after each dispatch. A single driver call can exceed that budget.
Matching/voting dispatches contain at most 4,096 targets. Ready batches use a
cooperative event-loop yield. Actual completion waits use native fence events
where supported, with timer polling as the capability fallback.
Completion and staging maps are nonblocking, including transient
`DXGI_ERROR_WAS_STILL_DRAWING` after an event signals. Initial resource allocation
is performed by job construction and does not share the poll's time bound. Affine/periodic
CPU results avoid GPU solver setup. Paired native measurements below qualify a
bounded tiny-raster CPU route, with GPU preference available to the host. Current warm
construction costs do not justify retaining a large solver scratch pool merely
to address the much larger first-dispatch stall. Shared immutable source pages
already reuse uploads. Tiny CPU routing also avoids first-use solver creation.
Uncached driver execution remains outside the encoding-time bound; no speculative
startup warmup or large retained solver pool is enabled.

## Native crossover and execution policy

`CanvasViewOptions::smart_erase_execution` selects `Automatic` (default), `Cpu`
or `GpuPreferred`. This controls new reconstruction work. Completed valid patches
remain reusable, as checked by the view tests. CPU mode requires CPU-readable
eligible donors. GPU preference preserves cheap affine/periodic CPU models and
the existing capability/allocation fallback.

On Windows x64, automatic execution finishes a texture fill on its worker when
both covered-hole bounds are at most sixteen source pixels, both donor-region
dimensions are at most 192 pixels, and every region pixel is covered. It reuses
the CPU-prepared level and produces exactly the ordinary CPU result. Thin strokes,
coverage gaps, expanded regions, layered assembly and GPU-only sources retain GPU
preference. Other architectures retain the previous texture routing until native
crossover measurements support a change. This is a bounded local heuristic;
hosts can choose GPU preference on differently balanced hardware.

```powershell
.\scripts\run-native-input-perf.ps1 -Load view-texture-compare -MaskWidth 8 -MaskHeight 8 `
    -OutputFile ..\artifacts\snow-draw-native-crossover-raster-8x8.json
.\scripts\run-native-input-perf.ps1 -Load view-layered-compare -MaskWidth 128 -MaskHeight 96 `
    -OutputFile ..\artifacts\snow-draw-native-crossover-layered-128x96.json
# Check the default policy without forcing either backend:
.\scripts\run-native-input-perf.ps1 -Load view-texture -MaskWidth 16 -MaskHeight 16 `
    -OutputFile ..\artifacts\snow-draw-native-crossover-automatic-raster-16x16.json
```

Each comparison runs in one window with the same immutable 512×384 source and
10,000 shapes. Every CPU/GPU pair has identical mask geometry and distinct element
IDs. Order alternates between GPU/CPU and CPU/GPU pairs. Cache counts verify a new
result for every command. Sixteen warmups precede 64 measured pairs (64 commands
per backend). A preflight CPU profile verifies the multiscale branch outside
timing; the GPU-preferred path must actually publish a GPU result. Both paths use
the production view, worker preparation and final presentation.

Local Windows 11 x64, Ryzen 9 5950X (12 cores / 24 logical processors exposed),
Radeon RX 6750 XT, driver 32.0.21045.16010, Rust 1.98.1 `canvas-perf`. The reference
repeat reports a 1280×720 logical window at 1.5 DPI scale (1920×1080 physical),
with a 6.944 ms refresh interval. Environment details and the latest benchmark
binary hash are in `snow-draw-native-crossover-environment.json`.

| Source | Mask | CPU median / p95 | GPU median / p95 | Paired GPU minus CPU median |
| --- | --- | ---: | ---: | ---: |
| Raster | 8×8 | 65.72 / 83.59 ms | 72.18 / 91.56 ms | +6.88 ms |
| Raster | 16×16 | 68.01 / 78.31 ms | 70.73 / 80.60 ms | +1.87 ms |
| Raster | 32×32 | 76.37 / 84.84 ms | 71.82 / 81.69 ms | −3.78 ms |
| Raster | 64×48 | 98.46 / 117.42 ms | 82.08 / 101.96 ms | −17.89 ms |
| Raster | 128×96 | 207.69 / 239.08 ms | 90.93 / 105.37 ms | −115.22 ms |
| Raster | 4×64 | 73.52 / 94.30 ms | 74.55 / 83.21 ms | +0.39 ms |
| Raster | 128×2 | 76.35 / 100.02 ms | 76.05 / 88.58 ms | −1.59 ms |
| Layered | 8×8 | 67.26 / 76.78 ms | 71.32 / 81.05 ms | +3.84 ms |
| Layered | 16×16 | 71.43 / 84.83 ms | 70.58 / 88.03 ms | −0.01 ms |
| Layered | 32×32 | 80.71 / 96.96 ms | 71.61 / 81.60 ms | −9.99 ms |
| Layered | 64×48 | 100.61 / 113.91 ms | 76.26 / 86.19 ms | −24.36 ms |
| Layered | 128×96 | 215.71 / 234.47 ms | 94.38 / 107.84 ms | −122.54 ms |

The 16×16 raster repeat measured CPU 67.07 / 78.58 ms and GPU 70.12 / 79.78 ms,
with a +2.29 ms paired median. The small advantage at this boundary and the thin
mask tails support bounding shape, coverage and region work rather than using
mask area alone. Layered inputs retain resident assembly because CPU assembly
cost also depends on the number and placement of eligible layers.

Default-policy runs verify CPU results for 8×8 and 16×16, and a GPU result for
32×32. Their medians/p95 are 61.43/69.99, 67.54/77.12 and 71.46/88.50 ms,
respectively. Every comparison/default run reports zero editing image readback
and zero idle presentations. Reports are `snow-draw-native-crossover-{raster,
layered}-<width>x<height>.json` and `...-automatic-raster-<size>.json` under ignored
`artifacts/`. These are local qualification measurements, not universal thresholds
or release percentile guarantees.

### First-use and allocation policy

The first GPU-preferred view command in each of the twelve comparison processes
took 71.16–125.70 ms, depending on workload. Driver caches were retained and CPU
preflight had run; these are first view solves with fresh application resources,
not independent uncached-driver trials. The default 8×8 CPU run's first command
took 63.29 ms and did not request a GPU reconstruction job. Existing source-page
reuse removes warm uploads; measured warm pipeline construction peaks at 0.91 ms,
so retaining a large solver scratch pool would consume the shared device budget
without addressing the earlier 116.59 ms first-dispatch outlier. The chosen policy
keeps bounded on-demand allocations and routes qualified tiny work to CPU.

Native driver calls can still exceed the encoding budget. Cold shader execution
and initial large allocations therefore have no hard input-latency guarantee.
GPU-only inputs without exact straight-RGBA storage still require explicit
materialization, and unsupported precise layered sampling retains CPU fallback.
Native ARM64 runtime validation remains necessary before production publication.

## Acceptance evidence

Run only related tests. Maintain exact alpha/unmasked bytes, independence from
masked RGB, conservative coverage/pyramid donors, deterministic results and the
existing band/diagonal/texture quality checks. Add device-backed checks for GPU
solver output, source-grid mask edges, tile seams, cache reuse, strict frozen
exports, cancellation, stale revisions, memory pressure and device replacement.
Measure input/presentation latency while reconstruction is active, in addition to
isolated solve duration. Stage availability alone does not prove a speedup.

The focused `cargo test --locked --offline --lib smart_erase -- --test-threads=1`
run passes 53 related tests. The view checks cover raster, layered and GPU-only
backgrounds, CPU preference, completed-result reuse across preference changes,
cancellation and actual shared-budget fallback. Tiny automatic routing is checked
against exact CPU bytes, with rejection cases for thin masks, alpha gaps, enlarged
regions and layered sources. Additional checks
cover CPU fallback beyond native texture width and plain GPU leaf copies without
optional filtered-sampling shaders. The completion filter passes five related
tests, covering event delivery, early completion/drop, owner/epoch rejection,
optional-capability fallback and renderer service reuse. The view filter passes
36 tests with the real-device I/O scheduler enabled for native callbacks. Setup
explicitly retires queued native work: draining logical tasks alone cannot settle
event-driven GPU work. Immediate-publication tests retain their current-frame and
pixel assertions and run no CPU workers during the forced paint. Logical
result, cancellation, stale-publication and rendering assertions remain intact.
Strict Clippy for the library/tests and both
benchmarks passes, as does the ARM64 library compile check. Cross compilation
does not replace native ARM64 runtime qualification.
