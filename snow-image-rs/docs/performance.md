# Performance and release gates

Measurements use an Apple M4 (10 logical CPUs, 16 GiB RAM), macOS 27.0.1, Rust
1.97.1, the workspace `performance` profile, and the independently pinned native
prefix. The legacy harness uses the existing macOS ARM64 CMake `performance`
build. No Qt application source or build configuration is changed.

The generated corpus covers 1080p, 4K and 8K screenshot-like UI with colored
strokes, transparency, JPEG previews, a 256×131072 scrolling capture, explicit
scRGB-to-screenshot-SDR processing, and four simultaneous GIF sessions. Generation,
input snapshotting and roundtrip verification occur outside timed operations.
The tall capture streams a 256×1800 periodic tile instead of allocating its
entire 128 MiB raster. Each timing has one warmup and seven measured samples.

The new-engine timings include scheduling, input transfer, codec work, output
publication and required worker exit. A process-isolation cost is still a user
latency cost; it is not subtracted from the comparison. PNG uses adaptive Balanced
compression versus libpng level 6, with identical RGBA samples verified after
decoding. JPEG uses quality 95, 4:4:4 and sequential output, with matching byte
counts and roundtrip PSNR. Both preview implementations fit 2048 pixels with
native reduced JPEG decoding followed by linear-light, premultiplied Lanczos3.
Preview RGBA comparisons report PSNR, maximum channel difference and changed
sample counts because the two resize implementations are not byte-identical.

See the [measured legacy comparison](legacy-performance.md) and its linked JSON
reports for current results and the remaining platform gates. Earlier measurements are
retained alongside subsequent reports; the initial bicubic/PNG-Fast comparisons
are historical diagnostics and do not establish matched-settings acceptance.

## Reproduce

Run only this workspace's related benchmark, with the prepared native prefix:

```sh
cargo build --locked --profile performance -p snow-image-worker -p xtask
target/cargo/performance/xtask bench --iterations 7 --corpus target/corpus \
  --preview-reference target/legacy --output benchmarks/macos-arm64-final.json
python3 benchmarks/legacy.py --iterations 7 \
  --output benchmarks/legacy-macos-arm64-final.json
python3 benchmarks/compare_legacy.py benchmarks/legacy-macos-arm64-final.json \
  benchmarks/macos-arm64-final.json --output benchmarks/comparison-macos-arm64.json
```

On an initial run, omit `--preview-reference`, run the legacy harness to create
its reference rasters, then repeat the new-engine run with that option. Use
sequential quiet runs, not parallel benchmarks. The comparison script fails when
any common PNG/JPEG/preview median regresses by more than 10%; it also checks
matching dimensions, JPEG size/quality, preview comparison presence and released
reservations. Additional workloads remain visible even though that specific
common-path gate does not include them. `xtask compare` compares future engine
reports only when their platform and settings strings match exactly.

The lower-level `profile_paths` codec example excludes supervision/publication
and is useful for attribution. It is not a substitute for service measurements.

## Memory and cleanup measurements

`peak_accounted_bytes` is a reservation high-water mark, including conservative
child envelopes. `peak_reserved_disk_bytes` includes admitted disk envelopes,
not actual occupied filesystem blocks. Neither is an allocator trace or RSS.
`sampled_worker_footprint_bytes` samples worker physical footprint; short-lived
allocation peaks can occur between samples. A macOS address-space ceiling and
sampled physical-footprint threshold constrain different metrics. Do not interpret
either the reservation counters or the sampler as a portable hard RAM cap.

The benchmark fails if memory or disk reservations remain after dropping each
workload's owners. Cancellation is measured from cancelling an active streaming
PNG export, after its source has supplied rows, through completion and release
of job reservations. It does not include an artificial producer delay. Separate
fault tests exercise uncooperative workers, failed producers, process crashes,
parent death, early replies, malformed descriptors and incomplete output.
Repeated-workload tests also compare actual OS handle counts after reaping and
prove that retained snapshots survive service/session destruction.

## Remaining release gates

Native execution has been performed only on macOS ARM64. Windows x64, Windows
ARM64 and macOS x64 have Rust cross-compilation checks, which do not establish
native codec, SIMD, lifecycle or performance correctness on those machines.
Run `scripts/validate.py`, the adapter lifecycle/sanitizer checks and the related
release benchmarks on every target before publication.

Short seeded fuzz runs validate the harnesses; sustained campaigns remain a
release task. Disk-full writer injection, quota rejection, read-only backing and
worker-crash cleanup are covered. The opt-in [real APFS exhaustion test](validation.md#real-filesystem-exhaustion-on-macos)
also reproduced a mapped-output false success and verifies the correction: the
worker now rejects the result when filesystem writeback reports `ENOSPC`, even
when dirty cached pixels still match the reference. This evidence is from macOS
ARM64; equivalent Windows filesystem-failure validation remains outstanding.
Unsigned packaging is local staging, not a
signed or notarized production release. See [validation](validation.md) and
[native builds](native.md) for the exact evidence and its limits.
