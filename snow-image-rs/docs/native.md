# Native backends

The native adapter is private to worker processes. Application code uses `snow-image-rs` and its resource-owning images and artifacts. The adapter does not expose foreign allocations or native handles to applications.

Static-worker publication transfers a private file handle at startup. The worker duplicates that handle, validates a regular file, and redirects stderr to the null device before calling codecs. On Windows it redirects both the OS standard handle and the shared dynamic-CRT descriptor 2, with explicit ownership transfer for the null handle. Native diagnostics therefore do not share the publication writer. The host publishes mappings only after worker exit and reaping; animation snapshots continue through copied host-owned buffers. Child-process tests verify that Rust and native descriptor writes cannot alter the publication file.

On macOS, a private Unix datagram socket transfers the regular backing file and a separate, immediately unlinked POSIX shared-memory object before worker startup. Eligible direct JPEG decode/preview pixels use the shared-memory object, sized exactly once after reserving its page-rounded allocation. The worker unmaps its writable view before replying; the host validates the kernel object type and allocation length and exposes a read-only view only after process-tree quiescence. Retained images keep the memory reservation. This avoids filesystem writeback for those pixels; encoded artifacts and other file-backed results still require checked synchronization before successful publication. Shared memory remains pageable VM and does not establish a portable hard RAM cap.

The private `snow-image-transport` crate receives a fixed versioned handoff with exactly two descriptors, sets CLOEXEC, validates both object roles, and bounds startup waiting. Darwin allocates SCM_RIGHTS descriptors before truncating ancillary bytes and does not close descriptors omitted by truncation; see the [XNU descriptor conversion](https://github.com/apple-oss-distributions/xnu/blob/xnu-11215.81.4/bsd/kern/uipc_usrreq.c) and [control-message copyout](https://github.com/apple-oss-distributions/xnu/blob/xnu-11215.81.4/bsd/kern/uipc_syscalls.c). The receiver therefore accommodates the bounded kernel control maximum before rejecting excess handles, instead of using a two-descriptor receive buffer. Malformed payloads and excess handles release every delivered descriptor. Its tests cover the observed 64-handle truncation regression, 254-handle messages, repeated handle counts, and retained pixels after writer exit:

```sh
cargo test -p snow-image-transport
```

The retained channel accepts a distinct one-descriptor input message before a
static inspect/decode/preview command. The sender closes its sole private writer
after checked writeback and opens a read-only capability before unlinking the
snapshot. Both endpoints reject writable, event-only, linked or nonregular input
handles; the worker also checks the exact declared length before a read-only map.
The page-rounded input reservation lives with the mapping, and the host's original
disk lease lives through process reaping. Mapping never changes the shared seek
position. A read-only descriptor alone cannot prove the absence of other writable
aliases; closing those aliases is part of the private host snapshot contract.

All 13 transport tests passed with Rust AddressSanitizer on macOS ARM64,
including startup/input message order, invalid input capabilities, child-process
mapping lifetime and repeated descriptor cleanup. The latest isolated log is
`target/native/transport-asan/input-handoff-test.log`. Reproduce with the pinned
nightly toolchain; this checks the Rust transport and its FFI calls, without
linking native codec backends:

```sh
RUSTFLAGS=-Zsanitizer=address CARGO_TARGET_DIR=target/native/transport-asan \
  cargo +nightly-2026-09-08 test -p snow-image-transport --target aarch64-apple-darwin
```

Publication also requires observed process-tree quiescence: zero active processes in the Windows Job Object, or no live members of the assigned Unix process group. macOS can transiently report `EPERM` for a group of dying/zombie processes; the supervisor then queries libproc group membership and BSD process states, accepting only an empty group or fully exited zombies. Sending a termination signal alone is insufficient. Windows limits the Job Object to one process; native codec concurrency uses threads. On Unix the supported worker and selected native stack have no child-spawning paths and do not move descendants to another session/process group. Process-group containment is a lifecycle mechanism, not a hostile-code sandbox: an arbitrary replacement worker executable that deliberately escapes its group is outside this trust boundary.

Unix workers close unexpected inherited descriptors at startup, before opening worker-owned files or starting threads. This prevents an unrelated worker's inherited pipe writer from delaying EOF after that worker exits. Rust 1.97.1 on Windows serializes its own `Command::spawn` standard-handle duplication and process creation with a global lock; it does not provide an explicit handle allowlist for ordinary spawning. Its raw process-attribute API is still unstable. Concurrent Rust spawns therefore do not inherit one another's temporary standard-handle duplicates, but independent native process launchers and preexisting inheritable native handles are outside that guarantee. A Windows launcher with an explicit `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` and native fault tests remains necessary before claiming broader handle-inheritance isolation.

## Reproducible preparation

`native/sources.json` pins every archive URL, version, and SHA-256, including transitive native dependencies. The archives were downloaded and their hashes verified. Dependencies are extracted only under `target/native/sources`; builds and static installations have separate target-specific directories. Cargo validates the installed manifest digest and target identity. Cargo builds never download native sources or discover an unrelated system codec installation.

Prerequisites are Python 3.9+, CMake 3.20+ (CMake 4.2 is supported), Ninja, a matching native C/C++ toolchain, pkg-config/pkgconf, and Meson 1.10.2. Keep Perl available for upstream assembly helper workflows and NASM for native x64 builds. Install Meson explicitly, if needed:

```sh
python3 -m pip install --target target/native/tools meson==1.10.2
python3 native/build.py fetch
python3 native/build.py verify
python3 native/build.py build --jobs 8
cargo test -p snow-image-native-sys --test native_roundtrip
```

The workspace `xtask fetch-native` and `xtask build-native` commands invoke the same Python entry point. `fetch` is the only entry point that retrieves native source archives. `build` rechecks archive hashes before using them. Ordinary Cargo registry dependencies remain independently pinned in Cargo.toml/Cargo.lock.

Native build targets are `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-pc-windows-msvc`, and `x86_64-pc-windows-msvc`. `--target` must match the native host; the script rejects cross builds rather than treating a successfully linked cross binary as release validation. Build on each of the four native machines before publishing. macOS targets have deployment minimum 15.0. Linux is not a release target.

On macOS, use Apple Clang and Command Line Tools/Xcode. On Windows, run from the VS developer environment matching the architecture, install clang-cl, and use the matching Windows SDK. The CMake and Meson builds use the dynamic MSVC CRT (`/MD`), matching Rust's default MSVC CRT. Cargo rejects `+crt-static` for this prefix. Do not mix libraries built with incompatible CRT settings. Native x64 builds require NASM for JPEG, WebP/AV1 assembly where used; ARM64 builds use the corresponding native SIMD/assembly paths. Meson, NASM, Ninja, and pkg-config must be on PATH. The build uses no runtime codec plugins, executables, GUI helpers, JNI, or dynamic library discovery.

JPEG preparation explicitly sets `WITH_CRT_DLL=ON`: libjpeg-turbo otherwise overrides CMake's runtime selection with `/MT`. Any Windows prefix prepared before that option was added must be rebuilt before use. The configuration regression checks the actual generated CMake and Meson commands without compiling dependencies; native Windows linking and execution remain separate release gates.

`cargo xtask package` builds the worker with an explicit supported host target and an absolute target directory, so ambient `CARGO_BUILD_TARGET` or `CARGO_TARGET_DIR` settings cannot redirect the build while an old executable is staged. The complete bundle contains the worker, project license/notices, Cargo lockfile, pinned native-source manifest, collected upstream notices, and hashes for the worker and both dependency manifests. Bundled AOM FAST, vector, and x64 assembly notices keep their source-relative paths to avoid overwriting identically named `LICENSE` files. Staging occurs in a fresh sibling directory; collection failures preserve the previous package, and a failed final rename rolls back the previous directory. This provides complete-bundle publication with ordinary filesystem-error recovery, not power-loss durability or signing.

The release profile leaves host build tools and procedural macros unstripped while preserving the worker's release optimization, LTO, and stripping settings. On the tested Rust 1.97.1/macOS ARM64 toolchain, Cargo's default host `strip=debuginfo` produced a procedural-macro dylib whose LINKEDIT string pool the dynamic loader rejected as misaligned; Rust reported this as E0463 for `thiserror_impl`. Fresh target directories reproduced the failure, and disabling stripping only for host build dependencies resolved it. The override also applies to ordinary release builds; dependency versions and native codec settings are unchanged.

Run the offline build/packaging regressions with `python3 scripts/test_build_support.py`. They use mocked build commands and tiny real filesystem fixtures, covering the Windows CRT options, explicit package target/output paths, distinct bundled notice files, notice failure, and publication rollback.

`SNOW_IMAGE_NATIVE_PREFIX` can select another prepared prefix. It must contain the matching source-manifest digest and target identity. `--no-default-features` is a deliberate Rust-only bootstrap configuration: native capability reporting is empty and native operations return `BackendUnavailable`.

## Libraries and choices

| Role | Library | Build configuration |
| --- | --- | --- |
| JPEG | libjpeg-turbo 3.2.0 | Static libjpeg API; direct RGBA input, row source and bounded destination |
| WebP | libwebp 1.6.0 | Static decoder/encoder, demux, mux and SharpYUV |
| JPEG XL | libjxl 0.12.0 | Chunked input, output processor, bounded custom allocator, private thread runner |
| AVIF decode | libavif 1.4.2 + dav1d 1.5.3 | Explicit dav1d selection; no runtime codec search |
| AVIF encode | libavif 1.4.2 + libaom 3.14.1 | Explicit AOM encoder; AOM decoder disabled |
| ICC | Little CMS 2.17 | One static copy shared by the adapter and libjxl CMS |
| Resize | zimg 3.0.6 | Lanczos3 in linear premultiplied RGBA, queried scratch and ring rows |
| JXL dependencies | Highway 1.2.0 + Brotli 1.1.0 | Static, no optional tools/tests/plugins |

The initial design proposed using the `lcms2` Rust crate. This implementation calls the mature Little CMS C API through the same narrow checked native shim as the codecs. This avoids introducing a second LCMS build/discovery path alongside libjxl's CMS dependency. The Rust surface still uses slices, owned vectors, checked lengths, and `Result`; Little CMS context/profile/transform handles are managed by C++ RAII. Rendering intent is relative colorimetric with black-point compensation and copied alpha. Float, U16, and U8 transforms are available. No generic ICC approximation substitutes for an actual transform.

Complete calibrated RGB descriptions can be converted into bounded ICC profiles using their white point, primary xy coordinates, and per-channel decode exponents. The implementation builds and owns actual Little CMS tone curves and profiles; it rejects invalid chromaticities/exponents and a serialized profile that exceeds the caller's cap. PNG encoding gamma must be inverted before it is supplied as a decode exponent. Incomplete descriptions require explicit caller policy instead of invented primaries or transfer functions.

The small zimg and Brotli CMake recipes select sources from the pinned releases without modifying those sources. zimg ARM64 enables its upstream NEON files. x64 enables SSE/SSE2/AVX/F16C/AVX2 files with per-file ISA flags and upstream runtime CPU dispatch; AVX-512 files are intentionally excluded. There is no process-wide AVX2 requirement. clang-cl uses `/clang:` ISA flags. Brotli's upstream 1.1.0 recipe builds a CLI unconditionally, so its private recipe builds only the three static libraries.

The resize bridge converts RGBA and planar rows in one pass with baseline ARM64 NEON or x64 SSE transposes and scalar tails. Resident images bypass transfer-row copies. Scalar sample-equivalence tests include short rows, vector boundaries, and tiled widths; x64 execution still requires native validation on an x64 host.

## Ownership, precision, and limits

- JPEG `setjmp`/`longjmp` stays wholly inside `jpeg.c`. Its heap-owned state survives unwinding, all allocations are released on error, and callbacks return error status rather than unwind. Truncated JPEG streams that cause decoder warnings are rejected. JPEG encoding rejects nonopaque alpha instead of discarding it.
- Rust output callbacks use fallible reservations and cumulative pixel/metadata limits. No foreign pixel pointer escapes a call. JPEG can write synchronously into an exclusively borrowed caller-owned destination; the callback checks the exact output length and never frees that destination. WebP animation frames are copied before advancing/resetting/closing the decoder, and retained snapshots own their bytes.
- Byte vectors guarantee only byte alignment, including with custom Rust allocators. Before native typed access, the wrapper rejects misaligned AVIF U16 input and refuses decoded AVIF/JXL output allocations that lack 16-bit alignment. libavif uses `uint16_t` accesses and libjxl can write typed half-float samples; the other JXL output conversions use byte stores, and its input shim copies into native aligned chunks. Little CMS receives typed Rust slices. These checks introduce no hidden image copies; an unsuitable output allocation fails within the existing resource limit. JPEG U8, including its caller-owned mapped destination, has no additional alignment requirement.
- `max_memory_bytes` bounds libjxl allocations separately from `max_output_bytes`. Its allocator includes per-allocation headers and serializes its counter. The chunked row source serializes callbacks, requires a `Send` source, and can reread rows in arbitrary order. zimg reports native scratch plus aligned planar ring rows and transfer rows before processing. The caller reserves them before invoking the adapter. zimg may tile horizontally and revisit input rows; its sink receives explicit row, column, and pixel spans. Sequential-only inputs must be backed by random-access storage before resizing. No full-output accumulation is hidden in the callback bridge.
- Native libraries without complete allocator hooks still allocate internal surfaces and scratch. In particular AVIF materializes YUV/RGB planes and a final encoded buffer; WebP materializes encoder/animation canvases, and ICC container muxing temporarily retains encoded output. These paths are subject to worker supervision and caller admission. A mapped input is not a promise of constant-memory encoding.
- JXL uses compatible buffered codestream output within its allocator cap. It does not silently switch to the newer out-of-order `jxlp` mode when a job exceeds its budget. Sequential output callbacks avoid an additional Rust output buffer when writing to an artifact. Float sources use original-profile Modular encoding, which retains negative and over-range scRGB samples; the tested original-profile VarDCT path in libjxl 0.12 does not retain their highlight scale. Streamed float rows are checked for nonfinite samples and invalid alpha before entering the codec.
- Integer U16 pixels span 0..65535. `significant_bits` records source precision, so decoded AVIF 10/12-bit images are normalized to full-range U16. Exact requests validate that samples are representable at the declared precision. Unsupported precision reduction, floating-point exactness claims, and implicit alpha flattening fail explicitly.
- PQ/HLG and primaries are kept separately from sample storage. Explicit scRGB is recorded in JXL's standard container as an optional versioned `sHDR` box: four-byte big-endian version 1, then IEEE-754 reference-white nits as four big-endian bytes. Unknown-box-aware third-party decoders ignore this application metadata while retaining the standard color profile. The reader validates the complete box bounds, rejects duplicate/invalid tags, and accepts the tag only for floating-point linear sRGB. Ordinary float images do not acquire an HDR tag. JXL's `intensity_target` is not misused as a reference-white field.
- JPEG APP1 Exif and standard XMP, WebP Exif/XMP/ICC, and AVIF Exif/XMP/ICC are returned under cumulative metadata caps. Extended multipart JPEG XMP and JXL Exif/XMP boxes are not currently extracted. Encoding follows the screenshot policy and excludes source Exif/XMP. JPEG/WebP orientations are parsed without applying them; image processing performs the requested orientation transform once.

The safe Rust wrapper validates dimensions, sample sizes, finite settings, callback lifetimes, alpha requirements, and returned layouts. It catches callback panics before they can cross C ABI boundaries. These checks and worker isolation complement native sanitizer/fuzz testing; they do not establish a general guarantee that third-party codec code is memory-safe.

## Targeted native validation

The Rust adapter tests cover actual codec roundtrips, transparent RGB, U16/JXL and 10-bit/AVIF precision, real ICC conversion, JPEG orientation/metadata/truncation and fractional native scaling, scRGB tagging, row streaming, wide/tall horizontal resize tiles, buffer refusal, and callbacks returning errors.

The separate `unaligned_allocator` integration test uses a conforming custom allocator that deliberately returns odd addresses for byte allocations. It verifies refusal before AVIF/JXL typed access and confirms that JPEG U8 owned and borrowed destinations remain supported:

```sh
cargo test -p snow-image-native-sys --test unaligned_allocator
```

The standalone lifecycle harness repeats all four codecs, failures, WebP snapshots/reset, scRGB highlights and negative samples, ICC, and zimg scratch limits 32 times. Run only that harness:

```sh
python3 native/build.py check-adapter --sanitizers address,undefined
```

That command instruments the adapter and harness while linking the ordinary pinned libraries. To also instrument every C/C++ dependency, build a separate prefix and rerun:

```sh
python3 native/build.py build --sanitizers address,undefined --jobs 8
python3 native/build.py check-adapter --sanitizers address,undefined --instrumented-dependencies
```

The sanitizer prefix is separate from production builds. Handwritten assembly cannot be instrumented by C/C++ sanitizers. Run the native adapter tests and lifecycle harness on each target; this macOS ARM64 development run is not evidence of validation on Windows or macOS x64.

Apple's ASan runtime does not provide LeakSanitizer. The macOS leak checker can independently inspect the ordinary lifecycle executable:

```sh
python3 native/build.py check-adapter
leaks --atExit -- target/native/aarch64-apple-darwin/adapter-plain/snow_native_lifecycle
```

The development run passed the lifecycle harness with all dependencies instrumented by ASan/UBSan, and the ordinary macOS leak checker reported zero leaked allocations after the repeated lifecycle workload. This is measured coverage of that workload, not proof that every native allocation path is leak-free.

`native/shim/fuzz.cpp` is a bounded libFuzzer target for native inspection, decoding, and WebP animation transitions. Build it with CMake `SNOW_BUILD_FUZZER=ON`, `SNOW_SANITIZERS=address,undefined`, and a prepared sanitizer prefix. If Clang does not ship a libFuzzer runtime, `SNOW_LIBFUZZER_SOURCE` can point to the prepared `libfuzzer` directory from the workspace's pinned `libfuzzer-sys` 0.4.13 dependency. CMake does not fetch that dependency. The adapter and harness receive coverage instrumentation, while the prepared dependency prefix supplies ASan/UBSan instrumentation. The lifecycle harness accepts an optional directory argument to generate valid JPEG, WebP, JXL, and AVIF seed files. Keep generated seeds, crash artifacts, and logs under `target/`.

A 5,000-run native smoke test completed with the instrumented dependency prefix and a 512 MiB RSS limit on macOS ARM64. This is a short harness validation, not a substitute for sustained fuzzing.
