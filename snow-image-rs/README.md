# Snow Image Rust

An independent image engine for Snow Shot, licensed under GPL-3.0-or-later. The
public library does not link Qt, GPUI, a codec, or an async runtime. It supervises
the separately distributed `snow-image-worker` executable.

This workspace implements screenshot export, imported images, bounded previews,
prepared transforms, and APNG/GIF/WebP playback sessions. The current Qt
applications are unchanged. This is a development implementation: see
[validation](docs/validation.md) and [performance](docs/performance.md) for the
executed checks and outstanding release gates. In particular, Rust cross-checks
are not native platform validation.

## Build

Use Rust 1.97.1 and the [native prerequisites](docs/native.md). Run from this directory:

```sh
cargo xtask fetch-native
cargo xtask build-native
cargo build --locked -p snow-image-worker
cargo test --locked -p snow-image-core
cargo test --locked -p snow-image-protocol
cargo test --locked -p snow-image-transport
cargo test --locked -p snow-image-codecs --features native-backends
cargo test --locked -p snow-image-native-sys
cargo test --locked -p snow-image-rs
SNOW_IMAGE_WORKER="$PWD/target/cargo/debug/snow-image-worker" \
  cargo test --locked -p snow-image-rs --test service -- --ignored --test-threads=1
```

On Windows set `SNOW_IMAGE_WORKER` to the absolute `.exe` path before the final
command. Dependency preparation explicitly downloads and verifies archives;
ordinary native builds never download them. Cargo uses its own lockfile and target
directory. The Rust dependency cache can be populated with `cargo fetch --locked
--target <target>` before an offline build. Target support is macOS 15+ and Windows,
on ARM64 and x64; build native dependencies on the matching target machine.

## Use

```rust,no_run
use std::sync::Arc;
use snow_image_rs::*;

# fn main() -> Result<()> {
let service = ImageService::new(ServiceConfig::new("/absolute/path/snow-image-worker"))?;
let input = service.input_file("capture.png")?;
let decoded = service.decode(input, DecodeOptions::default()).wait()?;
let prepared = service.prepare_transform(Arc::new(decoded), TransformOptions {
    dynamic_range: DynamicRangePolicy::ScreenshotSdr,
    ..Default::default()
}).wait()?;
let artifact = service.encode(prepared.source(), EncodeOptions::default()).wait()?;
artifact.save("export.png", WriteMode::CreateNew)?;
println!("{} bytes; {:?}", artifact.receipt().bytes, artifact.receipt().fidelity);
# Ok(())
# }
```

Every operation returns a `Job<T>` implementing `Future<Output = Result<T>>`.
Use `.await` in a GPUI/background executor or `.wait()` in a command-line thread.
`.cancel()` requests cancellation; dropping an unfinished job also cancels it.
Input snapshot creation performs file I/O, so create inputs and use `.wait()` on
a background thread. `Image` clones share immutable pixels and
metadata. Explicit `try_copy` is fallible. Returned images, artifacts and input
snapshots keep their accounting leases until their final owner is dropped.

Capture producers can supply BGRA, padded strides, U8/U16/F16/F32 and explicit
color descriptions through `ImageDescriptor` and `RasterSource`. A custom row
producer must return promptly and honor cancellation; the service cannot forcibly
interrupt application code. Keep a `PreparedTransform` to reuse the same immutable
pixels across changes to encoding settings. Use `capabilities()` to query the
actual linked worker. Unsupported precision/exactness requests fail explicitly.

Animation sessions share their encoded input. `next_frame()` returns an owned
snapshot and duration; the application controls clocks, looping, visibility and
display caches. Snapshots remain valid after session closure or worker failure.

## Development and distribution

```sh
cargo fmt --all --check
cargo clippy -p snow-image-rs -p snow-image-worker -p snow-image-codecs \
  -p snow-image-native-sys -p snow-image-core -p snow-image-protocol \
  -p snow-image-transport -p xtask \
  --all-targets -- -D warnings
cargo build --profile performance -p snow-image-worker -p xtask
target/cargo/performance/xtask bench --iterations 5 --corpus target/corpus
cargo xtask licenses --repository-check
cargo xtask package
python3 scripts/test_packaged_worker.py
```

`python3 scripts/validate.py` runs the scoped native-machine checks above and
records their results. It never runs the containing repository's test suite.

`package` stages an unsigned worker and dependency notices under `target/dist/`.
It does not sign, notarize, install, or publish. Include the worker beside the
application, use its explicit trusted path, and ship the corresponding source and
notices. The protocol is versioned and private; a public C ABI is outside this
release. Linux distribution, animation encoding, PDF composition, and viewer-only
formats are also outside scope.

See [architecture](docs/architecture.md), [format contracts](docs/formats.md),
[native builds](docs/native.md), and [third-party notices](THIRD_PARTY_NOTICES.md).
