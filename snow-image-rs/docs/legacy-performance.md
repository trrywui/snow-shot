# Legacy performance comparison

The latest adjacent macOS ARM64 runs meet the no-more-than-10% median
regression requirement on all 12 common PNG/JPEG/preview paths. The largest
regression is the unchanged 1080p preview at 8.4%; opaque and alpha PNG export
are 31–47% faster. Tall PNG and four-animation playback are 32.4% and 42.2%
faster respectively. These are seven-sample measurements on one machine, not
four-platform release acceptance. No memory limits or fidelity requirements
were relaxed for these runs.

## Reproduce

Build the existing Snow Image benchmark using the repository's macOS ARM64
**performance** preset. Its static library and dependencies must already exist.
Generate the new engine corpus with the `xtask bench --corpus` option, then run:

```sh
cd snow-image-rs
python3 benchmarks/legacy.py --iterations 7 --output benchmarks/legacy-macos-arm64-final.json
```

The script obtains the precise compiler and linker arguments from the existing
Ninja benchmark target, substitutes only the standalone source/object/output
paths, and writes its executable under `target/legacy/`. It does not modify or
reconfigure the Qt applications or the legacy library. The legacy build uses
`-O3 -DNDEBUG -flto=thin`, C++20, ARM64, and macOS 15 minimum deployment. The Rust
report uses the independent workspace's `performance` profile. Each operation
has one warmup followed by the requested number of measurements; the reported
result is the median. The initial reports below used five measurements. Current
runs use seven, PNG level 6 for both opaque and alpha fixtures, and preserve the
earlier reports as historical measurements.
The corpora contain generated UI panels, colored strokes and gradients, with no
user screenshots.

The recorded source data is in
[`legacy-macos-arm64.json`](../benchmarks/legacy-macos-arm64.json) and
[`macos-arm64-initial.json`](../benchmarks/macos-arm64-initial.json). The runnable
harness is [`legacy.cpp`](../benchmarks/legacy.cpp), with its build/recording script
[`legacy.py`](../benchmarks/legacy.py).

The extended harness also covers the generated 256×131072 scrolling raster and
four shared-input GIF sessions. Its tall source prepares a 256×1800 periodic tile
outside timing and serves checked rows from that small tile; it never creates a
complete 128 MiB source. Tall PNG uses level 6 with adaptive filtering, matching
the new engine's Balanced/Adaptive setting, and an untimed streaming decoder
checks every output sample. Animation timing resets four already-open sessions,
then performs eight interleaved advances per session with an independently owned
snapshot after every advance. Both engines consume `target/corpus/four-animations.gif`.
The fixture is opaque, so premultiplied legacy snapshots and straight-alpha new
snapshots contain the same samples. Untimed checks compare every frame sample.
Use `--size extended` to run only these workloads.

The new `ScreenshotSdr` HDR policy has no verified equivalent in the legacy
benchmark. HDR measurements remain separate; substituting a different legacy
tone mapper would not establish a matched-quality comparison.

## Latest adjacent runs

The seven-sample reports are
[`legacy-macos-arm64-final.json`](../benchmarks/legacy-macos-arm64-final.json)
and [`macos-arm64-final.json`](../benchmarks/macos-arm64-final.json). The Rust run
completed immediately before the legacy run, with the other development agents
idle. Both use adaptive PNG level 6/Balanced for opaque, alpha and tall images,
JPEG quality 95 with 4:4:4 sequential output, and linear-light premultiplied
Lanczos3 for resizing. Source generation and immutable input snapshotting are
outside timing. Worker transport, supervision and publication remain inside the
Rust timings.
The machine-readable [comparison](../benchmarks/comparison-macos-arm64.json)
passes the common-path latency, matched-output and released-reservation checks.
The preceding [bounded-read run](../benchmarks/comparison-macos-arm64-bounded-read.json)
missed the threshold on 1080p preview at 10.5%. The final input-handle change
removes an encoded-input copy by transferring the frozen, read-only snapshot;
its resource reservation still covers the mapping and retained storage.

| Operation | Image | Legacy median | Rust median | Change |
| --- | --- | ---: | ---: | ---: |
| PNG | 1920×1080 | 19.084 ms | 13.219 ms | −30.7% |
| PNG | 3840×2160 | 75.764 ms | 48.832 ms | −35.5% |
| PNG | 7680×4320 | 296.589 ms | 199.023 ms | −32.9% |
| PNG alpha | 1920×1080 | 25.540 ms | 14.269 ms | −44.1% |
| PNG alpha | 3840×2160 | 101.369 ms | 55.171 ms | −45.6% |
| PNG alpha | 7680×4320 | 402.661 ms | 214.019 ms | −46.8% |
| JPEG | 1920×1080 | 9.107 ms | 9.629 ms | +5.7% |
| JPEG | 3840×2160 | 36.163 ms | 35.590 ms | −1.6% |
| JPEG | 7680×4320 | 142.605 ms | 137.518 ms | −3.6% |
| Preview | 1920×1080 | 8.015 ms | 8.685 ms | +8.4% |
| Preview | 3840×2160 | 53.076 ms | 54.922 ms | +3.5% |
| Preview | 7680×4320 | 139.549 ms | 137.245 ms | −1.7% |
| Tall PNG | 256×131072 | 426.484 ms | 288.494 ms | −32.4% |
| Four animations, 32 snapshots | 320×180 | 6.227 ms | 3.599 ms | −42.2% |

JPEG byte counts remain identical and the roundtrip PSNR is approximately
42.02 dB for both implementations. PNG remains sample-exact, including
transparent RGB. New opaque PNG sizes are 28,700 / 96,829 / 348,662 bytes against
34,098 / 115,940 / 417,317 legacy bytes. Alpha PNG sizes are
47,062 / 164,786 / 596,745 against 73,241 / 271,246 / 1,029,388. The tall output is
1,205,322 bytes against 1,496,157, and its untimed decoder compares every sample
through a bounded row sink.

The 1080p preview matches every legacy RGBA sample. At 4K, 4,853 of 9,437,184
output samples differ, with maximum channel error 24 and RGBA PSNR 65.86 dB; at
8K, 7,252 samples differ, with maximum error 19 and PSNR 62.41 dB. These record
the remaining implementation differences at matched dimensions, color policy
and filter. They do not imply bit-identical resizing. The animation harness
checks every snapshot sample outside timing and uses the same encoded GIF.

All new-engine workloads return accounted memory and temporary-disk usage to
zero after their retained results are dropped. Peak reservation values include
coarse worker admission envelopes; they are not measured allocator peaks or
actual disk occupancy. Physical footprint is sampled during supervision and may
miss short peaks. The report records cancellation and HDR timings separately,
without claiming an equivalent legacy comparison for either.

## Measured initial result

| Operation | Image | Legacy median | Rust median | Change |
| --- | --- | ---: | ---: | ---: |
| PNG | 1920×1080 | 19.855 ms | 19.787 ms | −0.3% |
| PNG | 3840×2160 | 77.867 ms | 62.772 ms | −19.4% |
| PNG | 7680×4320 | 305.343 ms | 230.757 ms | −24.4% |
| JPEG | 1920×1080 | 9.420 ms | 17.495 ms | +85.7% |
| JPEG | 3840×2160 | 37.650 ms | 58.864 ms | +56.3% |
| JPEG | 7680×4320 | 146.606 ms | 213.262 ms | +45.5% |
| Preview | 1920×1080 | 8.634 ms | 12.645 ms | +46.6% |
| Preview | 3840×2160 | 53.150 ms | 183.010 ms | +244.3% |
| Preview | 7680×4320 | 139.380 ms | 266.080 ms | +90.9% |

The baseline measures the public legacy in-process API. The new implementation
measures the public service through its worker, including transport, temporary
spooling, supervision and artifact publication. Those are intentional user
latency costs and remain included in the acceptance check. Source generation and
input snapshotting happen before timing. Legacy encoding writes to a file, not
a discard sink. Both sources retain the same packed RGBA8 samples.

For PNG, the legacy native `RasterSource` encoding path preserves RGBA and uses
libpng adaptive filtering at compression level 6; Rust uses `png` Balanced and
Adaptive. Compression implementations have different tuning scales; both paths
are sample-exact and their output sizes are reported. Rust's opaque PNGs are
16–17% smaller in this corpus. The legacy `Document` convenience encoder can
automatically index colors or remove opaque alpha; that different representation
is not used as the primary baseline.

JPEG uses quality 95, explicit 4:4:4, sequential output and no imported metadata.
The encoded byte counts match at all three resolutions: 1,570,998; 6,279,534;
25,153,744. Measured roundtrip RGBA PSNR also matches (approximately 42.02 dB).
The report therefore does not attribute the JPEG regression to changed settings
or improved output quality.

Both primary preview paths return 2048×1152 pixels for the 4K/8K inputs, resize
in linear light with premultiplied alpha, and consume the **same** JPEG corpus
file. The legacy path uses Lanczos3; the initial native zimg path uses Catmull–Rom
bicubic. The table therefore matches preview dimensions and color policy, but
does not establish an identical resampling-filter comparison. The legacy harness requests the smallest supported `n/8` IDCT decode at or
above that exact target, then resizes. Its resize thread allowance is eight.
The initial Rust implementation selects power-of-two reduced decoding using a
square target; this leaves unnecessarily large intermediate images. The
implementations' resize arithmetic need not be byte-identical. Pixel-level
cross-implementation preview quality measurements remain necessary before
claiming full matched-quality acceptance. Running the harness also writes untimed
`target/legacy/legacy-preview-WIDTHxHEIGHT.rgba` reference files in packed RGBA8
for that comparison; the primary 4K/8K outputs are 2048×1152 pixels.

The legacy `maximum_extent=2048` shortcut returns only 1920×1080 for these 4K/8K
JPEGs. Its 30.637/112.593 ms measurements are retained as
`preview-reduced-only-*` records, but are excluded from the comparison table
because their output size differs.

## Alpha PNG warning and remaining measurements

Rust `PngCompression::Fast` and libpng compression level 1 are **not equivalent
size settings**. Their speed-only comparison must not be counted as matched
settings acceptance:

| Image | Legacy PNG1 bytes | Rust Fast bytes | Size ratio |
| --- | ---: | ---: | ---: |
| 1920×1080 | 161,868 | 2,440,052 | 15.1× |
| 3840×2160 | 623,174 | 9,758,364 | 15.7× |
| 7680×4320 | 2,332,389 | 39,023,828 | 16.7× |

Both alpha encodes roundtrip every RGBA sample, including transparent RGB. The
size difference is a compression tradeoff, and must be addressed through
appropriate settings rather than reducing fidelity.

The recorded runs cover one machine and generated UI fixtures. They do not
validate macOS x64, Windows x64, or Windows ARM64. Matched legacy measurements for
HDR, cancellation and physical footprint remain outstanding. The latest runs
add matched animation and scrolling-capture workloads; the original reports are
preserved so the regressions and subsequent optimizations remain reviewable.
