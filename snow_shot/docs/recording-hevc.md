# HEVC recording

Choose **H.265 (Hardware)** in Recording settings to record MP4 with a
hardware HEVC encoder. The existing **H.265** option continues to use libx265;
existing preferences and the default H.264 hardware selection are unchanged.
GIF, APNG and WebP retain their own encoders.

HEVC can reduce file size, but playback requires an HEVC-capable player. Newly
encoded HEVC MP4 files use the `hvc1` sample entry and global parameter sets,
including after recording recovery, for compatibility with Apple players.

## Encoder selection and performance

| Platform / adapter | HEVC encoder | Live input |
| --- | --- | --- |
| Windows NVIDIA | `hevc_nvenc` | D3D11 NV12 on the capture adapter |
| Windows AMD | `hevc_amf` | D3D11 NV12 on the capture adapter |
| Windows Intel | `hevc_qsv` | D3D11 surfaces mapped to QSV |
| Windows Media Foundation, including ARM64 | `hevc_mf` | CPU pixels; hardware MFT explicitly required |
| macOS | `hevc_videotoolbox` | CoreVideo surfaces; software VideoToolbox fallback disabled |
| Software fallback | `libx265` | CPU pixels |

Registration in FFmpeg does not prove the adapter can encode HEVC. Windows
negotiates capture, composition and encoding together and verifies actual output
packets. The native path uses the capture adapter's encoder, avoiding a transfer
to another GPU. For HEVC, unsupported adapters or failed negotiation use the CPU
path, which tries registered hardware encoders before libx265. The requested
codec is preserved throughout fallback. `HardwareOnly` callers receive an error
instead of a software encoder.

Live hardware encoding disables B-frames and lookahead and retains a bounded
surface pool. Intel HEVC surfaces have 32-row alignment while their visible crop
retains the requested dimensions. Software fallback does not inherit a hardware
encoder's effective thread count. Quality settings remain effective on all
backends; software presets apply to libx264/libx265, while hardware uses the
existing purpose-specific profiles.

Offline hardware exports can buffer/reorder for compression. They retry a
simpler hardware profile when optional features are unsupported, before trying
another backend or software. HEVC QSV uses ICQ without the H.264-only
`look_ahead` option. HDR exports skip hardware candidates without 10-bit input
support. macOS's existing HDR10 path remains HEVC Main10; selecting HEVC in the
ordinary recording settings does not itself enable HDR capture.

Windows device-loss recovery stores independent video segments and continuous
audio. It normalizes the segments with libx265 into one HEVC MP4, keeping the
active timeline, final still-frame hold and audio tracks. Successful recordings
are remuxed without re-encoding. Recovery can require more stop-time work than
ordinary finalization.

The C ABI adds `SNOW_CAPTURE_ENCODER_PREFERENCE_HARDWARE = 1`; the older
`SNOW_CAPTURE_ENCODER_PREFERENCE_H264_HARDWARE` remains an alias. Rust's
`prefer_hardware_h264` serialized field retains its name and applies to the
selected codec. Automation accepts `encoder: "h265_hw"` without changing the
saved preference.

## Focused validation

Use the repository FFmpeg installation and its runtime library path. Run Cargo
commands from `snow-crates/`:

```sh
cargo test --locked -p snow-d3d11
cargo test --locked -p snow-recording-c --lib direct_
cargo test --locked -p snow-recording-export --lib codec::tests
cargo test --locked -p snow-recording-export --lib streaming::
```

Native hardware checks are opt-in and fail if the requested hardware is absent;
they must not be reported as hardware passes after software fallback:

```sh
cargo test --locked -p snow-recording-export --lib hevc_hardware_live_and_offline_sparse_round_trip -- --ignored
# Windows: NV12 surface submission, non-32-aligned visible height and device loss.
cargo test --locked -p snow-recording-export --lib hevc_hardware_static_image_survives_failure_without_a_successor -- --ignored
# macOS: native CoreVideo SDR and HDR10 input, decode and hvc1 verification.
cargo test --locked -p snow-recording-export --test macos_native round_trip -- --ignored
# macOS: deterministic software HDR fallback with padded P010 input.
cargo test --locked -p snow-recording-export --test macos_native software_hdr_padded_p010_round_trip_preserves_depth_color_and_timeline
```

Build the recording controller/settings test targets and run the filtered CTest
entries `snow-shot-recording-settings-dialog-tests`, `snow-shot-mcp-recording-tests`
and `snow-shot-settings-select-tests`. The storage executable's
`--shortcut-settings-only` mode also validates all schema enum values, including
the persisted HEVC hardware selection. Run `translation_catalogs.py check` after
translation changes.

## Reproducible performance checks

Build `snow-shot-recording-encoder-performance-benchmark` with the platform's
**performance** preset. Its `recording_encoder_benchmark` executable accepts:

```text
output.mp4 seconds threads hardware(0/1) codec(h264/h265) quality scene(ui/gradient) profile(live/offline)
```

Use identical fixed frame counts and scenes, multiple interleaved samples, and
check `hardware_used`, `encoder`, decoded frames, PSNR, file size, submit latency
and finalization time. This experiment includes CPU RGBA upload/conversion; it
does not measure native screen capture or demonstrate zero-copy throughput.
Equal quality-slider values are not a claim of equal visual quality across
encoders. Do not infer a compression advantage from file size alone.

For the Windows native capture/composition path, use the Release performance
runner with `-Codec h265 -PreferHardware`. `run-gpu-recording-perf.ps1 -Codec h265`
covers static, sparse and continuous scenes, effects, audio, 1080p and 4K. Reports
retain the requested codec and actual encoder, fallback status and pixel-copy
counters. Qualify NVIDIA, AMD, Intel and Windows ARM64 separately, including
pause/resume, short/static recordings, device loss and playback on the intended
players. macOS measurements cannot qualify Windows vendor drivers.

### Local measurements, 2026-10-19

Apple M4, macOS 27.0.1, repository FFmpeg 9.0, macOS ARM64 performance preset (Release).
The fixed-frame experiment used 1920×1080, 150 frames / five seconds, quality 80,
automatic threads, live hardware profiles and the Very Fast software preset.
Each case had a discarded warmup and three interleaved measured runs. The table
shows medians. All 150 frames decoded in every run; hardware cases reported the
actual VideoToolbox encoder without fallback.

| Scene | Encoder | Throughput (fps) | Submit p95 (ms) | Finish (ms) | Bytes | RGB PSNR (dB) |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| UI | H.264 VideoToolbox | 239.8 | 2.92 | 23.8 | 66,383 | 38.45 |
| UI | HEVC VideoToolbox | 221.7 | 3.56 | 57.0 | 534,767 | 38.37 |
| UI | libx265 | 164.3 | 6.85 | 97.9 | 108,066 | 38.31 |
| Gradient | H.264 VideoToolbox | 226.4 | 4.20 | 52.5 | 11,749,738 | 24.83 |
| Gradient | HEVC VideoToolbox | 202.7 | 4.77 | 54.5 | 10,198,737 | 24.76 |
| Gradient | libx265 | 53.5 | 77.52 | 576.8 | 5,820,630 | 24.85 |

Throughput includes synthetic frame generation and submission, excluding encoder
startup, final draining and the subsequent decode/quality check. VideoToolbox
startup medians were 108–117 ms; libx265 startup was about 5 ms. These are local
CPU-upload experiments, not real-time desktop FPS or a general speed guarantee.
The HEVC hardware profile has ample encoding headroom in these scenes and lowers
submission stalls relative to libx265, but produces larger files than libx265.
It also produces larger files than H.264 hardware on the simple UI fixture.
Quality was not reduced to hide that tradeoff; PSNR is a limited synthetic metric,
not a substitute for checking small text and real desktop recordings.

The complete Snow Shot application built successfully with the macOS ARM64
performance preset. Local validation also passed the streaming/recovery tests (48), codec policy
tests (6), direct C ABI tests (15), adapter mapping, runtime fallback policy,
settings/automation/storage checks, translation validation, Clippy with warnings
denied, and native VideoToolbox SDR/HDR10 plus live/offline sparse-frame checks.
Software HDR fallback also preserves 10-bit color, timestamps and duration across
live output, offline output and clean recording sources. Software output retains
the selected preset's frame reordering; clean sources remain non-reordering.
Windows vendor execution, native D3D11 recovery and the Windows capture benchmark
matrix remain platform qualification steps; they were not executed on this Mac.
