# Isolated codec implementation

This private crate runs inside `snow-image-worker`. Applications use `snow-image-rs`.
Its functions require an explicit resource budget and cancellation token; failures leave
publication and worker cleanup to the supervisor.

PNG, JPEG and BMP exports read rows once in increasing order. JPEG XL may request rows
again and in a different order; sequential sources are first sealed into private temporary
storage. WebP and AVIF explicitly materialize their input under a reservation. Integer
sample storage is native endian internally, including normalized full-range U16 with a
separate significant-bit count. PNG performs its endian conversion at the row boundary.

RGBA/BGRA samples declared `AlphaMode::Opaque` have ignored alpha bytes. Exporters
normalize those bytes to full opacity. Straight-alpha PNG, WebP exact, JPEG XL exact and
AVIF exact paths retain RGB values behind transparent pixels. AVIF exact mode requires
both quality settings at 100 and samples exactly representable at the selected bit depth.
JPEG rejects actual transparency. PNG receipts report the stored 8/16-bit depth while its
sBIT tag independently retains the source's significant precision.

The worker can decode eligible JPEGs directly into its private writable output mapping.
The inspected plan borrows immutable encoded bytes, validates the exact RGBA layout, and
falls back whenever preview resizing, orientation, or color conversion is required.
The mapping is dropped before any success response; the supervisor still waits for the
writer process to exit before publishing a read-only image.

Preview processing uses linear light and premultiplied alpha with Lanczos3. Reductions
larger than 16× are staged, and zimg's queried scratch and row rings are reserved before
processing. Ordinary SDR resizing converts input rows directly into those rings and
quantizes output regions into the final owned pixels, avoiding full floating-point rasters.
Its horizontal tiles publish bounded regions, so output callbacks never assume
that one callback covers a whole row. Sequential sources are snapshotted before a graph
that may revisit rows. The exact sRGB byte quantizer uses mathematical half-code thresholds;
its table lookup is hoisted outside pixel loops and is verified against a double-precision
IEC reference at every boundary.

Color descriptions remain attached to the samples they describe. Complete PNG gAMA/cHRM
and BMP calibrated descriptions synthesize an ICC profile through Little CMS. Incomplete
calibrated descriptions remain representable and round-trip through their original format,
but cannot be transformed without the missing information. PNG parsing rejects an ICC
chunk that the underlying decoder silently discarded. Source XMP/EXIF are available during
decode when geometry is unchanged. Applying orientation or resizing strips EXIF/XMP
rather than rewriting arbitrary geometry-bearing packets; orientation preserves ICC,
while color conversion replaces the source profile with its declared output encoding.
All current encoders omit EXIF/XMP; there is no metadata-preserving export option.
HDR output quantization requires an explicit
`HdrOutput::Bt2020Pq10` or `Bt2020Pq12` request with `PreserveDynamicRange`.

APNG, GIF and WebP sessions decode lazily. GIF finite repeat counts are normalized to total
playbacks; zero means infinite. Snapshots own their storage independently of the session.

Run only this crate's relevant checks:

```sh
cargo test -p snow-image-codecs --features native-backends
cargo test -p snow-image-codecs --no-default-features
cargo clippy -p snow-image-codecs --features native-backends --all-targets -- -D warnings
```

The native dependencies must already be prepared. For direct codec timing, excluding
worker transport and publication, use the generated benchmark corpus:

```sh
cargo run --profile performance -p snow-image-codecs --features native-backends \
  --example profile_paths -- target/corpus target/legacy
```

The optional second path compares pixels with the legacy preview export. Full service
performance and acceptance measurements belong to `xtask bench`; direct codec timings
cannot establish the service's regression gate.
