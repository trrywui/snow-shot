# Format and fidelity contracts

| Format | Inspect/decode | Still encode | Lazy animation | Input samples for encode |
| --- | --- | --- | --- | --- |
| PNG | Yes | Yes, row streaming | APNG | U8, U16 |
| JPEG | libjpeg-turbo; reduced decode for preview | Row streaming | No | U8 |
| BMP | `image` decoder | Checked row writer | No | U8 |
| ICO | Validated selected entry, `image` decoder | No | No | — |
| GIF | Yes | No | Yes | — |
| WebP | libwebp | Yes; materialized | Yes | U8 |
| JXL | libjxl | Yes; chunked | No | U8, U16, F16, F32, subject to exactness/color options |
| AVIF | libavif with dav1d | libavif with libaom; materialized | No | U8/U16 at supported 8/10/12-bit output precision |

The worker's `capabilities()` is authoritative for a particular build. A build
without native backends supports the Rust formats and explicitly rejects native
operations; it is useful for ownership tests, not the production format matrix.
Codec and resource dimension limits can be lower than the logical raster ceiling.
Unsupported color/precision combinations fail before silently quantizing pixels.

Pixel storage is interleaved in its declared channel order. Strides are byte
counts and may include padding; row-source reads return packed rows. U16, IEEE
F16 and IEEE F32 use native byte order. U16 spans 0..65535 even for sources with
10/12 significant bits. Channel order, alpha convention, source color encoding
and orientation are explicit; callers must not infer them from byte length.

`EncodeOptions` carries format-specific settings. An `EncodeReceipt` reports the
actual dimensions, bit depth, format, byte count, applied options, and fidelity.
`Exact` refers to accepted samples, including RGB under alpha zero. The tests
decode exact outputs and compare samples; a backend's lossless flag is insufficient.
Lossy color conversion, precision loss and alpha flattening must not be disguised
as exact sample preservation. Pixel fidelity does not claim preservation of EXIF/XMP;
the metadata policy is stated separately below. A prior explicitly requested
transform has its own output samples, so an exact encoding of those samples does
not claim to recover the pre-transform source. `AlphaMode::Opaque` explicitly
marks physically present alpha bytes as unused; exporters normalize those bytes
to full opacity. Straight-alpha inputs retain meaningful alpha and transparent RGB.

PNG/BMP retain accepted integer samples. JPEG has explicit subsampling and rejects
unsupported alpha/precision. WebP exact mode retains transparent RGB. JXL distance
and exactness are independent validated settings. AVIF exact requests are limited
to `AvifOptions::exact = true`, both quality settings at 100, and samples exactly
representable at the selected depth. PNG receipts report physical 8/16-bit depth;
its sBIT metadata separately preserves the significant source precision. Test these contracts when
changing native versions or conversion flags.

Decoded metadata includes orientation, source encoding and bounded ICC/EXIF/XMP
where supported by the adapter. PNG gAMA/cHRM and calibrated BMP endpoints/gamma
remain explicit color descriptions. Complete descriptions use actual Little CMS
profiles for conversion; incomplete descriptions round-trip without invented
primaries or transfer functions. `preserve_metadata = false` does not throw away an
ICC profile while returning pixels that still require it. Export through
`ScreenshotSdr` to convert imported pixels and remove imported descriptive
metadata. Applying orientation or resizing strips EXIF/XMP because arbitrary
packets can contain stale orientation, dimensions, crops and thumbnails; the
engine does not rewrite them. Orientation alone retains the ICC profile. A color
conversion instead uses the output's declared color encoding and does not attach
the old source profile. A geometry-preserving `PreserveDynamicRange` identity
transform retains its source metadata. All current encoders omit EXIF/XMP,
regardless of decode or transform policy; metadata-preserving export is not an
exposed option. Use `PreserveDynamicRange` explicitly for HDR exports. To encode scRGB
as AVIF HDR, request `hdr_output: Some(HdrOutput::Bt2020Pq10)` or `Bt2020Pq12`
in the prepared transform, then match the AVIF depth. This is an explicit
quantization step into normalized U16 PQ samples. Out-of-gamut values and values
above 10,000 nits are rejected. JXL can also preserve floating-point scRGB samples.
`hdr_peak_nits` controls the screenshot SDR highlight mapping; it does not change
the 80-nit absolute scRGB convention. HLG processing uses an explicit reference white and
1.2 system gamma, with HLG 0.75 mapped to reference white. Imported PNG/native
HLG uses a documented 203-nit reference-white convention. Current JXL/AVIF
adapters do not serialize a custom HLG reference white, so export rejects
non-203-nit HLG instead of changing that meaning on roundtrip; PNG encoding
does not currently accept HLG. Transform to a supported explicit encoding when
a different reference white must be preserved.

Animation sessions retain immutable input and metadata; returned frames are
independently owned composited snapshots. APNG hidden posters do not become
playback frames. GIF/APNG/WebP composition honors timing, loops, frame rectangles,
blend and disposal. Malformed animation is an error, not a fallback to a still
image. Reset and recovery replay are bounded; frame clocks and UI caches belong
to the application.

The deterministic reference corpus is generated in package tests and
`xtask bench --corpus target/corpus`. It includes padded rows, hidden transparent
RGB, U16, JPEG reduced previews, ICO directory selection, APNG poster/disposal,
GIF/WebP sessions, ICC vectors, scRGB/PQ/HLG and generated tall rasters. The checked-in
ICC fixture has its generation source alongside it. No private user images are
included.
