# Recording performance experiments

## Capture pipeline regression checks

Use only the filters that cover the changed pipeline. From `snow-crates/`, with
the platform FFmpeg environment initialized:

```text
cargo test -p snow-recording-runtime --lib deferred::tests
cargo test -p snow-recording-runtime --lib capture_queue::tests
cargo test -p snow-recording-runtime --lib direct::scheduling_tests
cargo test -p snow-recording-runtime --lib direct::encoding::tests
cargo test -p snow-recording-runtime --lib capture_failure::tests
cargo test -p snow-recording-runtime --lib gpu_startup::tests
cargo test -p snow-recording-export --lib streaming::recovery::tests
cargo test -p snow-recording-export --lib failure_preservation_keeps_only_opted_in_plain_staging_after_drop
cargo test -p snow-recording-export --lib offline_output_rejects_live_recovery
```

On macOS, the additional offscreen checks are:

```text
cargo test -p snow-recording-runtime --lib macos
cargo test -p snow-macos --lib desktop::tests
cargo test -p snow-macos --lib any_display_can_wake
cargo test -p snow-macos --lib identical_metal_copy
```

On native Windows, run the WARP lease test and the explicitly ignored hardware
tests on each relevant GPU vendor. These use generated pixels without desktop
capture or injected user input:

```text
cargo test -p snow-capture --lib region_copies_preserve_pixels_and_bounded_immutable_leases
cargo test -p snow-capture --lib gpu_backpressure_does_not_consume_the_final_desktop_update
cargo test -p snow-recording-runtime --lib hardware_clean_source_ -- --ignored
cargo test -p snow-recording-runtime --lib hardware_shared_probe_covers_direct_and_recording_source -- --ignored
cargo test -p snow-recording-runtime --lib hardware_desktop_cache_ -- --ignored
cargo test -p snow-recording-runtime --lib hardware_sparse_effects_and_webcam_match_cpu_draw_order_and_expiration -- --ignored
cargo test -p snow-recording-export --lib hardware_static_image_survives_failure_without_a_successor -- --ignored
```

These checks establish correctness within their fixtures. Use the platform
**performance** preset and the paired workload protocol below for performance
claims, covering direct/deferred, native/downscaled/odd-size regions, static and
moving content, effects/webcam, pause/resume, and GPU fallback. Windows type
checking from another OS does not substitute for native driver validation.

For deferred startup fallback, inspect `source.capture_report().startup` for the
adapter, attempted encoders, failure stage and reason. Runtime recovery remains
in `recovery_count` and `recovery_reason`; a startup fallback does not imply a
mid-recording recovery. Direct recording reports expose the same startup
diagnostics through their existing adapter/attempt/stage/reason fields.

## Windows throughput protocol

Use the `windows-msvc-performance` environment and Release builds. Benchmark
controls are Rust-only and do not alter the C FFI or application settings.
The workload covers the leftmost monitor and feeds synthetic observations to
recording effects; it does not inject OS mouse or keyboard input.

## Build and preserve an executable

From the repository root:

```powershell
# Detailed stage measurements.
./scripts/run-realtime-recording-perf.ps1 -BuildOnly

# Compile out stage probes for throughput confirmation.
./scripts/run-realtime-recording-perf.ps1 -BuildOnly -Metrics ''
```

Copy each executable to a unique location under `build/recording-throughput/`
before rebuilding. Preserve its SHA-256, source archive, Cargo.lock, feature
configuration, compiler versions and installed FFmpeg identity. The current
experiment archives are in `build/recording-throughput/sources/`, with frozen
executables in `build/recording-throughput/binaries/`. Do not overwrite a binary
that an earlier result identifies by hash.

Baseline and candidate must use the same harness, Release configuration and
timing features. Screen one mechanism at a time. Optional benchmark experiments
remain disabled by default; required recording clarity filtering stays enabled.
Switches select optional experiments.
Omit `-EncodeThreads` / `--encode-threads` to preserve the application policy.
Explicit zero requests the exporter's automatic worker count; 1, 2 or 4 requests that count.
The exporter resolves its automatic count from physical cores before opening
FFmpeg; it does not pass a zero thread count through to FFmpeg.
This distinction is corrected after frozen v15: older executables interpreted
zero as application policy, including its existing software 30-fps cap.
For resize, explicit zero or one requests the serial path, while omission
preserves the application's resize policy. `-AutomaticPolicies` enables
the validated application defaults in benchmark builds (which otherwise keep
automatic optimization selection disabled to support isolated comparisons).
For example:

```powershell
./scripts/run-realtime-recording-perf.ps1 `
  -Executable build/recording-throughput/binaries/prototype-v19-instrumented.exe `
  -Scenario application-default -Fps 60 -Clarity 1080p -PreferHardware `
  -OutputDirectory build/recording-throughput/example-screen
```

Run only one realtime benchmark at a time. Keep other builds, tests and desktop
activity out of the recording window. The window excludes itself from desktop
Peek; this is a workload-fixture policy. It does not modify capture behavior.
Workload publication cadence is independently measured at successful GDI
publication. It is not a measurement of physical display scanout.

## Alternating confirmation pairs

Schema 9 adds `encode_threads_override` to run metadata (`None` for omitted,
`Some(0)` for explicit automatic worker selection). The existing numeric
`encode_threads` metadata remains for compatibility. `effective_resize_threads`
reports the largest worker count selected for a composed frame; an automatic
pool whose backend/geometry rule never matches reports one. Pool size and the
eligibility rule are fixed before startup and do not retune during recording.

Write an options JSON with `common_args`, `baseline_args`, and `candidate_args`.
For synchronous versus asynchronous software H.264 at 60 fps:

```json
{
  "common_args": ["--scenario", "all-effects", "--fps", "60",
                  "--clarity", "1080p", "--workload", "continuous"],
  "baseline_args": [],
  "candidate_args": ["--async-encoder"]
}
```

```powershell
python scripts/run-recording-pairs.py `
  --baseline build/recording-throughput/binaries/prototype-v7-no-timing.exe `
  --candidate build/recording-throughput/binaries/prototype-v7-no-timing.exe `
  --config build/recording-throughput/confirm-async-sw60-config.json `
  --output build/recording-throughput/confirm-async-sw60-v7-no-timing
```

The runner fixes ten alternating pairs, five seconds of warmup and 30 seconds
of measurement for each recording. It records binary hashes and arguments and
stops on invalid workload/input/identifier measurements or competing work.
Build observations after final encoder drain are preserved separately; a
conservative one-second polling margin remains protected. Older executables
without recording boundaries protect their entire process lifetime.

Use the same command with `--resume` after addressing an interruption. It
requires the same hashes and configuration, preserves complete valid pairs,
and archives both members of an incomplete pair before repeating it. Do not
delete an invalid marker or replace a recording because its fps is low.

`comparison.json` contains paired recording-level absolute differences and
Student-t 95% confidence intervals over log ratios. Individual frames and
time windows are not independent trials. `confirmation_eligible` requires ten
valid pairs; it does not assert correctness, quality, latency, or acceptance.
Retain a throughput change only with supported fresh-fps improvement, followed
by confirmation with timing probes disabled. A stage-only improvement requires
supported lower processing time at already-satisfied cadence and no fresh-fps
regression. The asynchronous path must also satisfy the one-output-interval
capture-to-first-packet latency allowance.

## Evidence files and endurance inspection

For the final cumulative comparison, use the preserved original pipeline and
v19 executable with the matching feature configuration:

```powershell
python scripts/run-recording-pairs.py `
  --baseline build/recording-throughput/binaries/original-current-v18-instrumented.exe `
  --candidate build/recording-throughput/binaries/prototype-v19-instrumented.exe `
  --config build/recording-throughput/cumulative-sw60-final.json `
  --output build/recording-throughput/reproduce-cumulative-sw60
```

Use `cumulative-hw60-final.json` for hardware preference. For the timing-disabled
confirmation, replace both executable suffixes with `-no-timing.exe` and use a
new output directory. These configs use all-effects, continuous content, 60 fps,
1080p output, with `--automatic-policies` only on the final candidate. Frozen
source ZIPs and manifests are under `sources/`; original-source preparation and
adaptation records are under `cumulative-original-v18-harness/`.

Prepared final coverage and endurance commands, run separately from builds and
offline analysis:

```powershell
python -u build/recording-throughput/run-coverage-resumable.py `
  build/recording-throughput/final-matrix-v19-jobs.json
python -u build/recording-throughput/run-audio-endurance.py `
  prototype-v19-instrumented.exe endurance-v19
```

The coverage manifest contains 120 short recordings, including original/final
comparisons, 30/60 fps, both encoder preferences, four effect scenarios, three
content patterns, and additional geometries. These are coverage screens, not
ten-pair confirmation evidence. The endurance helper plays a known PCM tone
through the current default output without changing device or volume settings,
checks nonzero loopback audio first, and stops playback in `finally`.

Each recording preserves media, decoded source identifiers, workload
publication times, process memory samples, summary metrics and failure reasons.
Instrumented builds additionally preserve stage summaries, raw stage samples,
source/PTS traces, encoder admission/submission/packet events, and per-PTS
capture-to-first-packet latency. Handoff latency retains its earlier meaning
and must not be substituted for packet latency.

The current harness also writes `*-composition.csv`, joining composition
start/end to output PTS. Its signed nanosecond offsets use the same first
encoder-admission origin as `*-packets.csv`; negative offsets preserve
composition that began before that admission. Join these events to
`*-sources.csv` by output PTS. Older frozen binaries without this additive file
still preserve the earlier source, stage, packet and latency observations.

Schema 8 distinguishes `conversion_backend`, `configured_conversion_threads`
and `effective_conversion_threads`. The last field reads the initialized
FFmpeg context's thread setting and stays empty for an unresolved automatic
setting. In older schema 7 files, `effective_conversion_threads=0` was the
legacy-backend selector, not evidence of zero conversion workers.

Schema 10 adds `restoration_only` for the experiment selected by
`--restoration-only`. It restores recycled-buffer overlay coverage only when
the pristine background is unchanged, with full refresh and ordinary drawing
on changed backgrounds. It does not enable source-damage updates. Static
Auto-capture runs can transition from DXGI to WGC; `capture_backend` is the
last observed backend, and `effective_resize_threads` is the largest count
used during the recording. Do not interpret those two summary values as
evidence that one backend or worker count was used for every frame.

After timed runs finish, inspect one recording directory:

```powershell
python scripts/analyze-recording-run.py `
  build/recording-throughput/confirm-async-sw60/baseline-1 `
  --executable build/recording-throughput/binaries/prototype-v19-instrumented.exe `
  --window-seconds 30
```

The script saves raw media/audio inspection output and
`recording-diagnostics.json`. It reports cadence by media-time window, packet
latency when available, memory trends excluding initialization/teardown,
replacements, dropped audio, A/V endpoints and decoded audio signal/continuity.
Static content should not be judged against a moving-content fresh-fps target.
An audio track with packets but zero signal is insufficient evidence of
nonzero audio continuity. Audio inspection uses Windows Media Foundation's
installed AAC decoder because the deployed FFmpeg has no AAC decoder; this
does not affect the recording pipeline. Provide a known continuous system-audio source for
the five-minute endurance recordings.

Run the focused Python checks without starting a recording:

```powershell
python -m unittest scripts/test_compare_recording_results.py `
  scripts/test_run_recording_pairs.py scripts/test_analyze_recording_run.py
```

Current experiment status and interruptions are recorded in
`build/recording-throughput/STATE.md`. Prototypes and screening results do not
by themselves authorize new production defaults.

## Recording clarity

Recording backgrounds use pixel-area coverage on each shrinking axis. CPU
composition uses `RecordingResizePlan`; native GPU composition filters RGB before
NV12 conversion. Cropped and rotated native GPU placement uses exact texture loads.
Same-size CPU pixels are copied exactly, and enlargement retains the existing
selection policy. Video export selects FFmpeg AREA for reductions in which neither
axis enlarges, including RGB48 HDR intermediates. Cursor masks keep their existing
nearest-pixel semantics. Filtering preserves thin-stroke coverage and avoids
ringing; it cannot recover source detail beyond the output resolution.

The realtime benchmark's `--half-resize` switch remains for older command
compatibility. Its metadata records the requested flag, while the filtered 2:1
specialization is always enabled. Toggling this flag is not an active-kernel
comparison. The standalone resize benchmark compares the complete nearest and
filtered implementations instead.

In the configured `windows-msvc-performance` environment, with the repository
FFmpeg DLLs on PATH, build from `snow-crates`:

```powershell
cargo build -p snow-recording-export --release --target x86_64-pc-windows-msvc `
  --target-dir ../build/windows-msvc-performance/cargo `
  --example recording_resize_benchmark
```

Then run from the repository root, separately from builds and other benchmarks:

```powershell
./build/windows-msvc-performance/cargo/x86_64-pc-windows-msvc/release/examples/recording_resize_benchmark.exe `
  --quality --output build/recording-clarity/quality
./build/windows-msvc-performance/cargo/x86_64-pc-windows-msvc/release/examples/recording_resize_benchmark.exe `
  --output build/recording-clarity/performance
```

The performance benchmark defaults to five rounds of 500 ms per variant, rotating
and reversing order. It covers exact 2:1, portrait, fractional, stronger reduction,
odd geometry and native output. `--pairs` and `--milliseconds` allow shorter
screening. `area_parallel2` and `area_parallel4` name the requested bounded pools;
native identity copies do not use those workers.

Two focused Release stage comparisons can be run from `snow-crates`:

```powershell
cargo test -p snow-recording-export --lib --release --target x86_64-pc-windows-msvc `
  --target-dir ../build/windows-msvc-performance/cargo `
  resize::recording::tests::fractional_area_kernel_performance_comparison `
  -- --ignored --nocapture --exact
cargo test -p snow-recording-runtime --lib --release --target x86_64-pc-windows-msvc `
  --target-dir ../build/windows-msvc-performance/cargo `
  direct::gpu::tests::hardware_area_resize_stage_benchmark `
  -- --ignored --nocapture --exact
```

The quality CSV compares synthetic RGB fixtures with an independent continuous
pixel-square coverage reference; it does not measure encoded perceptual quality.
Decoded export regressions and offscreen GPU readback checks validate later
pipeline stages separately. Kernel timings and completed GPU timestamps measure
stage cost, not delivered recording cadence; GPU results apply to the measured
adapter and driver. Use the preserved-binary alternating-pair recording workflow
above before claiming throughput changes, preserving source and binary hashes
and feature settings.

## Deferred conversion and empty GPU overlays

Deferred Windows 3840x2160/60-fps MP4 H.264 exports with automatic threads,
VeryFast, and x264-first selection cap the software encoder at four workers.
The existing 30-fps cap remains eight. Eligible SDR exports use four swscale
workers when the resolved software cap is at least four; explicit thread
settings and other resolutions, codecs, presets, and platforms keep their
existing policy. Hardware export uses the same CPU color conversion because
offline replay currently supplies CPU frames. HDR keeps its P010 scaler.

`FrameConverter` lends owned raster storage to FFmpeg through a read-only,
reference-counted AVBuffer. Its release callback owns an Arc, including during
error cleanup. The raster is recycled only after swscale releases every foreign
reference. This removes the experimental converter's full input-frame copy
while preserving BT.709 conversion, initialized SIMD padding, and frame reuse.
Live CPU recording keeps its existing default converter; the benchmark's
`--conversion-threads` switch can still select the owned converter explicitly.

The final export comparison used frozen baseline/candidate executables, five
alternating pairs per case (50 exports), and the same FFmpeg DLLs. These were
Windows x64 Release runs on 2026-10-19, on a Ryzen 9 5950X with 12 enabled physical
cores / 24 logical processors, 64 GiB RAM, and a Radeon RX 6750 XT. Each fixture
is three seconds of 3840x2160/60-fps SDR H.264, quality 80, VeryFast, with no
audio or webcam. Automatic software encoding resolved to twelve workers before
and four afterward. Times below are medians; memory is median peak process
private bytes sampled every 20 ms, excluding dedicated GPU memory.

| Source / effects / encoder | Previous time | New time | Time reduction | Previous private memory | New private memory |
| --- | ---: | ---: | ---: | ---: | ---: |
| Moving / combined / x264 | 6.677 s | 4.493 s | 32.7% | 1904.7 MiB | 1133.6 MiB |
| Static / progress bar / x264 | 4.924 s | 3.009 s | 38.9% | 1884.5 MiB | 1138.7 MiB |
| Static / playback time / x264 | 0.245 s | 0.224 s | 8.6% | 1136.3 MiB | 567.8 MiB |
| Static / none / x264 | 0.105 s | 0.096 s | 8.0% | 1026.4 MiB | 457.9 MiB |
| Moving / combined / AMF | 7.235 s | 5.899 s | 18.5% | 248.6 MiB | 252.6 MiB |

Combined effects include a progress bar, mouse trail, click rings, and keyboard
overlay. The AMF gain costs about 4.1 MiB of extra private memory. For static
none/time cases, the large private-memory reduction is primarily encoder
commitment: peak working sets remain approximately 177/393 MiB. Moving x264 peak
working set falls from 1518.5 to 972.7 MiB. Static frame reuse remains enabled.
The conversion tests produce identical pixels across supported layouts and
identical H.264 output with fixed encoder threads; changing x264's thread count
can change its bitstream at the same quality setting.

Every accepted export passed duration/dimension checks, and no competing build,
test, or benchmark was detected during its pair. Interrupted pairs are excluded
in full and retained separately. Raw results, commands, process-memory samples'
peaks, output files, and executable hashes are under
`build/recording-pipeline-optimization/export-confirm-20261019-221219/`;
`build/recording-pipeline-optimization/confirmation-summary.json` also records
paired timing estimates. These measurements cover this host and fixture set;
they do not establish ARM64/macOS results or live recording throughput.

To reproduce a deferred export in the configured Release performance environment,
build the benchmark from `snow-crates`, prepare fixtures outside measurement,
and run each comparison in a fresh process:

```powershell
cargo build -p snow-recording-export --locked --release --features bench-timing `
  --target x86_64-pc-windows-msvc --target-dir ../build/windows-msvc-performance/cargo `
  --example postprocessing_export_benchmark
# From the repository root; prepare requires a new directory.
./build/windows-msvc-performance/cargo/x86_64-pc-windows-msvc/release/examples/postprocessing_export_benchmark.exe `
  prepare build/export-fixture-4k-moving 3840 2160 moving
./build/windows-msvc-performance/cargo/x86_64-pc-windows-msvc/release/examples/postprocessing_export_benchmark.exe `
  render build/export-fixture-4k-moving build/export-moving.mp4 combined 0 0
```

The next two render arguments select hardware preference (0/1) and encoder
threads (0 means automatic); the optional final argument is playback speed in
percent (default 100). Use `static` when preparing the static fixture,
and `none`, `bar`, or `time` for the corresponding effect case. Preserve both
executables before comparing changes, alternate their order, and keep builds
and other benchmarks separate from measurement.

The D3D11 compositor skips mouse and keyboard passes whose tile snapshots are
empty. It allocates the full-size overlay texture on first visible use, retains
the last uploaded tile map, and clears obsolete tiles when a visible layer next
uses the texture. Mouse effects remain below the cursor and keyboard effects
remain above it. Empty recordings avoid one RGBA texture: 7.91 MiB of logical
pixel storage at 1080p or 31.64 MiB at 4K, before driver allocation alignment.

To measure completed GPU work in the configured Release performance environment,
run this focused test from `snow-crates`, with no concurrent builds or benchmarks:

```powershell
cargo test -p snow-recording-runtime --lib --release --target x86_64-pc-windows-msvc `
  --target-dir ../build/windows-msvc-performance/cargo `
  direct::gpu::tests::hardware_empty_overlay_stage_benchmark `
  -- --ignored --nocapture --exact
```

It compares the previous transparent passes with empty-layer bypass in ten
alternating pairs of 120 frames after warmup, at 1080p and 4K. The legacy control
exists only in test builds. RGB output equality and lazy allocation are checked
before timing. D3D11 timestamp queries and an event fence report completed GPU
and wall time, including desktop composition and NV12 conversion, excluding
capture and encoding. These are stage measurements, not a recording-FPS claim.

On 2026-10-19, the Windows x64 Release benchmark on a Radeon RX 6750 XT
(driver 32.0.21045.5002) produced these medians across the ten batch pairs:

| Output | Previous GPU stage | Empty-layer bypass | Reduction |
| --- | ---: | ---: | ---: |
| 1920x1080 | 0.1202 ms/frame | 0.0743 ms/frame | 38.2% |
| 3840x2160 | 0.3976 ms/frame | 0.1972 ms/frame | 50.4% |

The run passed RGB equality and allocation checks, with no competing build,
test, or benchmark detected. Separate offscreen tests passed for effect
expiration/reappearance, webcam/cursor/keyboard ordering, and decoded H.264
thin-stroke preservation. Raw results and executable identity are retained under
`build/recording-pipeline-optimization/gpu-confirm-20261019-222010/`.

### Deferred replay reuse and tone mapping (macOS, 2026-10-20)

The replay loop now distinguishes source changes, visible input changes and
animation time. Repeated stationary pointer observations preserve settled trail
pixels, while exits, re-entry, continuity changes, future observations and expiry
still invalidate composition. Proven identical output repeats the encoder's
pending image without a canvas copy or equality scan. This also applies to
retimed video and animated exports: fixed video slots and the GIF/WebP hold
limits remain authoritative. HDR-to-SDR replay caches the tone-mapped source
until its decoded picture changes. This costs one RGBA canvas (31.6 MiB at 4K)
only when tone mapping is requested; HDR-preserving replay allocates no SDR cache.

On an Apple M4 (10 cores), using `snow-shot-macos-arm64-performance`, software
H.264 with four threads, unchanged quality/preset, and three-second 3840×2160
60 FPS sources, the median of three fresh-process runs was:

| Source / effects / speed | Baseline | Updated | Reduction |
| --- | ---: | ---: | ---: |
| Stationary / trail / 100% | 343.2 ms | 49.7 ms | 85.5% |
| Moving / none / 50% | 3249.1 ms | 2919.9 ms | 10.1% |
| Held HDR → SDR / progress bar / 100% | 19512.4 ms | 3191.2 ms | 83.6% |
| Stationary / none / 50% | 2025.9 ms | 1966.8 ms | 2.9% |
| Moving / combined / 100% | 2523.5 ms | 2513.3 ms | 0.4% |
| Stationary / progress bar / 100% | 1967.8 ms | 1930.5 ms | 1.9% |

Small differences in the last three rows should be treated as noise, not a
general encoder speedup. All 18 output pairs were byte-identical, with matching
dimensions and exact duration. The HDR pairs alternated baseline/updated;
the SDR runs collected three baseline runs followed by three updated runs.
No builds or tests overlapped measurement. These fixtures exclude capture,
audio, webcam, UI completion and source packing; they establish no Windows,
hardware-encoder or live-capture throughput claim.

The baseline is `ddbd22d6c28297bd722e4cae470176251490c083` plus the benchmark's
stationary/trail/speed inputs. Raw timings, executable hashes, outputs and
`summary.json` are under `build/deferred-optimization/`. Prepare `stationary`
for the no-op pointer case and `hdr` for held HDR-to-SDR tone mapping, then use
`trail` or `bar` when rendering. For example, from the repository root after
building the performance benchmark target:

```sh
export DYLD_LIBRARY_PATH="$PWD/.tools/macos/installed/dynamic/arm64-osx-snow-shot/lib"
bench=build/snow-shot-macos-arm64-performance/cargo/aarch64-apple-darwin/release/examples/postprocessing_export_benchmark
"$bench" prepare build/export-hdr-4k 3840 2160 hdr
"$bench" render build/export-hdr-4k build/export-hdr-bar.mp4 bar 0 4 100
```

Four final-build confirmation pairs (stationary trail, moving at 50%, moving
combined, and held HDR with a bar) also matched byte for byte and retained the
large improvements. Their alternating order, timings and final executable hash
are recorded in `build/deferred-optimization/final-confirmation.json`.

PCM replay separately uses a bounded 64 KiB reader per recorded track, and
retimed/preview PCM reads avoid seeking when the next request is contiguous.
Tests compare samples across buffer boundaries, rewinds, track routing and
zero-padded endpoints. Render-job start and cleanup completion now wake the GUI
through future notifications; only native progress remains on its 100 ms timer.
A regression disables normal polling to verify that start adoption, task disposal
and source release still complete, alongside cancellation and shutdown checks.
Validation passed 85 effects tests, 42 focused export tests, the post-processing
and render-layout CTests, Clippy with warnings denied, and Rust/C++ formatting.
The post-processing test waits for macOS's asynchronous initial preview sizing
before asserting the unchanged progress-bar pixel condition.

Encoder quality/preset defaults and the source format are unchanged. GPU decode,
decode/encode worker overlap, input checkpoints and direct packet copying need
separate evidence: device-dependent memory/transfer costs, bounded cancellation,
persisted replay state/integrity, and source-versus-output quality semantics
respectively. They are not prerequisites for these pixel-identical improvements.
