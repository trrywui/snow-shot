# Validation and failure testing

Run commands from `snow-image-rs/`. Keep validation scoped to the changed package or test target; do not run the repository's unfiltered test suite.

## Deterministic checks

```sh
cargo test -p snow-image-core
cargo test -p snow-image-core --test allocation_failure
cargo test -p snow-image-worker
cargo test -p snow-image-rs --lib
cargo test -p snow-image-rs --test faults
cargo build -p snow-image-worker
SNOW_IMAGE_WORKER="$PWD/target/cargo/debug/snow-image-worker" cargo test -p snow-image-rs --test service -- --ignored --test-threads=1
SNOW_IMAGE_WORKER="$PWD/target/cargo/debug/snow-image-worker" cargo test -p snow-image-rs --test faults -- --ignored --test-threads=1
```

`faults` uses small disposable Python workers for process crashes, inherited output pipes, cancellation, oversized control frames, truncated pixel replies, and a failing raster producer. Backing-publication tests cover responses sent before writer exit, short and oversized files, partial output followed by a crash, and a forked backing writer that closes its response pipe. Semantic reply checks reject impossible or mismatched precision, broken exactness receipts, oversized previews, invalid animation sessions, changed animation canvases and premature end-of-animation replies. These tests are Unix-only and require `python3` on PATH. Native fault cases verify parent-pipe EOF cleanup, actual parent death while another process keeps stdin open, partial streaming-input failure, rejection before a full raster is read, snapshot survival after worker death, replay recovery, and the explicit reset requirement after 64 frames. `service` exercises real worker processes and static formats, prepared transforms, previews, cancellation, immutable input snapshots, HDR preparation and animated pins. A non-square APNG with EXIF rotation checks the agreement between session dimensions, owned snapshots and reset behavior with orientation enabled and disabled. Its repeated-workload check runs in an isolated test process and compares both reservation totals and actual OS handle counts after workers and retained snapshots are dropped.

`RasterSource` callbacks run in the host input sender. Custom producers must return promptly and override `read_rows_cancellable` to check its token during expensive work; stopping a child cannot safely terminate arbitrary blocked Rust callback code. The default checks cancellation before and after each row batch. The supplied memory/file sources meet this contract.

Lifecycle regressions keep the service and animation handles alive after failures. Failed animation opens, including admission failures before any codec request is sent, and crashed or cancelled advance/reset commands must release the failed worker's memory and disk envelopes before completing the job. The encoded input and independently owned snapshots may remain retained for recovery. A separate cooperative-producer test verifies that worker failure cancels the input-transfer token promptly while preserving the original worker error. The cooperative-producer fixture waits for both producer and worker readiness before triggering its crash; its one-second failure-response assertion excludes interpreter startup. A deliberately delayed startup reproduced the former timing false positive while actual failure cleanup took under five milliseconds. Both mechanisms were reproduced failing before their fixes; a retained idle actor previously held about 341 MiB of memory allowance and 5.33 GiB of temporary-disk allowance, and an input callback did not receive cancellation after its worker exited.

Persistent animation workers are checked at command completion and every 50 ms while idle. Fault tests trigger a page-touched allocation beyond the configured footprint threshold, or a process exit, after a snapshot has already been returned. Both cases must retire and reap the connection without another API request, release its worker envelopes, retain the host-owned snapshot, and recover through bounded reopening/replay. Before the idle check, both fixtures retained a live connection with 357,913,941 bytes of scratch allowance and 5,726,623,078 bytes of temporary-disk allowance. The tests now observe retirement, successful replay/reset, and a zero final reservation baseline.

Each static supervisor lane retains a reader and writer executor across disposable worker processes, with capacity-one task and completion channels. Animation connections retain their own pair across commands. Idle executors hold no image buffers or resource reservations; each completed static connection still reaps its process and releases its owners before the job completes. A successful reply cannot complete a job until its input task has also finished. Premature animation acknowledgements remain subject to the operation deadline. Cooperative cancellation writes use the same input executor, so a full pipe cannot block the supervisor's grace-period checks. Both deadline regressions failed before correction with a bounded one-second fake-worker fallback and pass after correction. Executor tests cover panic conversion, continued reuse, one-task admission and cancellation on drop; integration tests confirm that a panicking raster producer does not strand the service and that the same input thread is reused across three disposable workers through failure, panic and failure.

After a completed static response and input transfer, macOS uses a bounded kqueue process-exit notification to wake the supervisor. The event is only a scheduling hint: `Child::try_wait` still owns reaping/status, and publication still requires process-tree quiescence. Notification waits are capped by the remaining operation and cancellation deadlines. Tests cover timeout while the child remains alive, exit waking before the timeout without reaping, already-reaped registration, close-on-exec and repeated handle release. Other platforms retain the existing polling cadence.

On macOS, concurrent Rust `Command::spawn` calls can race the standard library's separate pipe/socket creation and close-on-exec calls. A native cancellation run exposed a sibling worker retaining the cancelled worker's response writer. The new shared-output handoff reproduced the same race for a socket pair created before the spawn lock. Capability creation through worker spawning is now serialized across service instances, and worker startup closes unintended inherited descriptors before duplicating its intended standard handles. A concurrent fake-worker test inspects inherited descriptors, while the real worker startup test exercises closure directly. These are ownership checks in addition to process-group termination.

The current transport uses bounded pipe copies, private anonymous file spools and macOS shared-memory objects. PNG/JPEG/BMP input runs through three reusable 256 KiB buffers with owned transport reservations; the codec consumes sequential rows while input arrives, without first writing the entire raster to disk. Transform and other encoder paths retain private file backing. Static workers receive a private anonymous result file through an inherited OS handle. They redirect Rust/native stderr before running codecs. Encoders write directly through a capped sink into that private backing. Other decoded pixels and prepared rasters are copied after codec execution. The host validates the exact file length and confirms writer-process tree termination before publishing a read-only mapped image. File-backed image owners retain both Pixels and TempDisk reservations until their final clone is dropped. Prepared rasters and encoded artifacts retain private parent-owned files. No writable result handle is exposed publicly. Animation frames remain bounded copies into host-owned immutable storage.

On macOS, eligible direct JPEG decode/preview paths instead write an immediately unlinked POSIX shared-memory object. A versioned Unix datagram transfers the regular backing and unallocated shared object as two OS handles, queued before spawn so parent death cannot strand a receiver awaiting its handoff. The worker reserves the page-rounded pixel allocation before sizing the object. The host requires a configured shared backing, checks descriptor bytes and kernel object type, proves all writer processes are gone, validates the exact page-rounded allocation, then maps only the payload read-only. Retained snapshots hold the rounded Pixels reservation without a TempDisk charge. This path has no filesystem writeback; regular file outputs retain their mandatory writeback checks. Fault tests cover premature replies, malformed allocation sizes, crashes, forked mapping writers and snapshot survival after service shutdown. Windows keeps the regular file publication path.

A Unix watchdog checks parent identity every 50 ms, independently of a blocked input reader, and also exits when the worker starts with parent ID 1. The latter is tested by stopping a launcher before `exec`, killing its parent while another process retains stdin, then resuming the orphaned worker. That regression timed out with the original parent-change-only check and passes with the startup-orphan check. Windows uses the parent-owned Job Object lifecycle.

For static inspection, decoding and previews on macOS, the host freezes its private input snapshot before transferring a read-only file capability. Freezing checks filesystem writeback, verifies the reopened file's identity, closes the writable owner and unlinks the name. Both transport endpoints reject writable, named or nonregular files; the worker reserves page-rounded input memory before receiving the capability, validates its exact length, then maps it read-only. The connection keeps the input's disk owner until its writer process has been reaped. Tests cover immutable snapshots, concurrent consumers without a shared consuming cursor, both sides of the former 2 MiB transport threshold, and retained disk ownership through success and cancellation. Other platforms stage static encoded input into the inherited private file using a reserved 1 MiB copy buffer; the worker validates its size and completes its admitted read before reusing that file for output. Persistent animation workers retain pipe input. The host proves output immutability through a terminated Windows Job Object or the absence of live members in the worker's Unix process group; macOS additionally resolves transient process-group permission results by inspecting group members. A zombie has no address space or writable descriptors. The Unix process-group contract assumes the supplied worker does not escape its group. It is not a sandbox for a hostile replacement executable.

Returned diagnostics and capability tables also retain their accounting owners. Drop a retained error before asserting an all-zero resource baseline; its diagnostic allocation is intentionally charged while the error remains usable. Diagnostic messages have a finite UTF-8 byte limit, and impossible queue capacities are rejected before queue allocation.

Prepared transforms optionally map their immutable backing under the shared 64 MiB prepared-pixel allowance. Clones retain one reservation until the final owner is dropped. Full or disabled caches and mapping failures preserve the already prepared file and its disk reservation, without copying or recomputing pixels. Tests cover aggregate memory admission, padded rows, bounded regions, malformed backing, clone lifetime and a real-worker export after cache admission is exhausted.

`allocation_failure` is a separate test executable with a test-only `GlobalAlloc` wrapper. A scoped atomic switch refuses exactly one 1 MiB, alignment-one pixel allocation while forwarding metadata, Arc and harness allocations to `System`. The test observes `AllocationFailed`, checks that admitted pixel and metadata reservations return to zero, and then repeats the same allocation successfully. This exercises actual allocator refusal in addition to budget admission failure; production core code still forbids unsafe Rust.

## Real filesystem exhaustion on macOS

The opt-in helper below creates and mounts its own 24 MiB APFS disk image without elevated privileges. It first decodes an opaque sRGB JPEG to obtain a pixel reference, removes that reference backing, then fills only the private filesystem until a real write returns `ENOSPC`. It sends the same decode directly to a disposable native worker with publication backing on the exhausted filesystem. The JPEG must be at most 4 MiB encoded and 8 MiB as decoded RGBA pixels; the generated 1920×1080 benchmark JPEG fits both limits.

```sh
python3 scripts/test_macos_enospc.py \
  --worker target/cargo/debug/snow-image-worker \
  --jpeg target/corpus/ui-1920x1080-jpeg.jpg
```

Run this separately from timing measurements. It is never mounted by ordinary Cargo tests. The image's physical allocation is checked against a 32 MiB ceiling. The helper owns its mount, image, filler, output and reference files, uses bounded child deadlines, and detaches before deleting those files. An uncertain attachment result is resolved using the exact private image identity; failed detachment preserves the image and prints its cleanup location. It does not change existing files or fill the host filesystem.

This reproduced a false successful publication on macOS ARM64: the worker returned `BackedImage` for 8,294,400 bytes and exited successfully; readback after exit still matched the reference, but an explicit filesystem sync returned `ENOSPC`. A separate disposable mapping probe showed that `mmap.flush()` could also succeed before `fsync` reported the error. Dirty cached pixels therefore do not prove that temporary backing can retain its content. After checking mapping flush and file writeback before publishing, the same worker test returns a failure containing OS error 28, publishes no image, and cleans up the private filesystem. This helper checks real worker writeback behavior; the service fault tests separately verify retained reservations, worker reaping and deletion of failed anonymous output files.

## Miri

The ownership and accounting code contains no unsafe code. `OwnedPixelStorage` adopts an external owner with a documented stable, immutable byte-slice contract; the host performs the unsafe OS mapping only after proving backing immutability. Miri exercises external owner lifetime, final-clone release, and explicit mutable copying without mapping real files. Run its memory-only tests with the installed nightly:

```sh
cargo +nightly-2026-09-08 miri test -p snow-image-core --lib -- \
  --skip temporary_rasters_use_disk_not_pixel_budget \
  --skip cancelled_and_truncated_snapshots_release_disk_budget \
  --skip private_file_producer_cannot_exceed_declared_size
```

The three skipped tests use real temporary files; they run under ordinary Cargo tests. Miri does not validate native codec implementations, subprocess supervision, or OS accounting.

## Fuzzing

The independent `fuzz/` workspace pins `libfuzzer-sys`, has its own lockfile, and exposes `inspect`, `decode`, `animation`, and `ipc`. Default fuzzing uses Rust codec backends. Policies cap input at 1 MiB, raster size at 4 MiB, admitted memory at 8 MiB, metadata at 64 KiB, frames at 32, and animation transition sequences at 16 commands. `animation` inputs start with 16 operation bytes followed by the encoded image. Every target checks that owned reservations return to baseline.

Generate original tiny PNG/APNG/GIF/BMP and protocol seeds:

```sh
python3 fuzz/make_seeds.py
```

With `cargo-fuzz` installed, run one target at a time:

```sh
cargo +nightly-2026-09-08 fuzz run inspect fuzz/corpus/inspect -- -max_total_time=60 -max_len=1048576 -timeout=2 -rss_limit_mb=256
cargo +nightly-2026-09-08 fuzz run decode fuzz/corpus/decode -- -max_total_time=60 -max_len=1048576 -timeout=2 -rss_limit_mb=256
cargo +nightly-2026-09-08 fuzz run animation fuzz/corpus/animation -- -max_total_time=60 -max_len=1048576 -timeout=2 -rss_limit_mb=256
cargo +nightly-2026-09-08 fuzz run ipc fuzz/corpus/ipc -- -max_total_time=60 -max_len=1048576 -timeout=2 -rss_limit_mb=256
```

A direct nightly build is available when `cargo-fuzz` is absent. On macOS ARM64, the following enables AddressSanitizer and coverage instrumentation:

```sh
RUSTFLAGS='--cfg fuzzing -Zsanitizer=address -C passes=sancov-module -C llvm-args=-sanitizer-coverage-level=4 -C llvm-args=-sanitizer-coverage-inline-8bit-counters -C llvm-args=-sanitizer-coverage-pc-table' \
  cargo +nightly-2026-09-08 build --manifest-path fuzz/Cargo.toml --release --target aarch64-apple-darwin --bins

target/cargo/aarch64-apple-darwin/release/inspect fuzz/corpus/inspect -runs=1000 -max_len=4096 -timeout=2 -rss_limit_mb=256
```

Substitute `decode`, `animation`, or `ipc` for the executable and corpus. Do not treat a plain, uninstrumented release build as a coverage-guided campaign. Passing `--features native-backends` links the prepared native dependencies; those libraries must also be compiled with sanitizers before claiming native sanitizer coverage. Native backend fuzzing should use an OS process memory limit in addition to libFuzzer's RSS limit.

## Observed validation

The final scoped validation on macOS ARM64 with Rust 1.97.1 passed all 231 Rust tests and eight offline build/packaging regressions. The script names each relevant package explicitly; it does not run the repository's unfiltered suite. Formatting, native-source verification, all selected packages' Clippy checks with warnings denied, and collection of 140 dependency notice files passed. The 15-command report is written to `target/validation/native.json` and the captured output to `target/validation-final.log`.

| Package | Passed tests |
| --- | ---: |
| Core, including allocator refusal | 24 |
| Protocol, including single-allocation input reads | 6 |
| OS-handle transport | 13 |
| Native adapters, including deliberately unaligned allocation | 23 |
| Codec/processing unit and workflow tests | 65 |
| Worker | 30 |
| Public service: 22 unit, 30 default fault, seven native fault, 11 native service | 70 |

Final integration uncovered two fixture races, which were reproduced and corrected before the clean run. The cooperative-input test now starts its failure timer after readiness rather than including Python startup. Both parent-death fixtures publish their complete JSON marker by atomic rename and own a private process group for cleanup even if setup fails. Neither correction changes production cancellation or parent-death behavior. The earlier failed logs remain under `target/validation-before-fixture-fix.log` and `target/validation-before-marker-fix.log`.

The staged macOS ARM64 release worker also passed the opt-in package smoke test:

```sh
python3 scripts/test_packaged_worker.py
```

This builds the existing export client and exercises decode, `ScreenshotSdr` preparation, PNG encoding and atomic save through the packaged worker. It verifies the worker and dependency-manifest hashes, all 140 notice files, every output PNG CRC, all 12,032 expected RGBA samples, and the 551-byte `Exact` receipt. Fixtures live in a temporary directory and subprocesses have finite deadlines. The result is recorded in `target/package-smoke-final.log`. The package is unsigned local staging; this test does not establish signing, notarization or execution on other architectures.

A fresh memory-only Miri run passed 20 tests on nightly 2026-09-08, including calibrated color descriptions, external pixel owners and retained diagnostic reservations. All four fuzz targets also completed 1,000 seeded runs each with AddressSanitizer and coverage instrumentation, using Rust codec backends; there were no failures in that smoke run. This is a small harness check, not a sustained fuzzing campaign.

After the final decode, animation and input-handle protocol updates, the three affected fuzz targets were rebuilt and rerun with nightly AddressSanitizer/coverage and native backends disabled. Regenerated IPC seeds include valid pipe, backing-file and input-handle requests. Each target completed another 1,000 runs with seed `20261024`, a 4 KiB maximum input, a two-second per-input timeout and a 256 MiB RSS limit; none reported a failure. Peak RSS was 50 MiB for decoding, 53 MiB for animation and 47 MiB for IPC. Build output, per-target logs and a summary are retained in `target/fuzz-smoke-final/`. Inspection was unchanged and was not rerun for these updates.

Cross-target compilation also passed for the facade and worker test targets on Windows x64, Windows ARM64 and macOS x64 with native backends disabled. That is compile evidence, not native execution on those platforms. These results do not establish the four-platform release gate or a sustained fuzz campaign. The separate seven-sample [performance comparison](legacy-performance.md) passes the common-path threshold on this macOS ARM64 machine only. Native sanitizer and dependency coverage is recorded separately in [native validation](native.md); do not infer it from these Rust tests.

macOS address-space limits are separate from physical footprint. The default is an explicit 1 TiB virtual-space ceiling because current dyld mappings reserve hundreds of GiB before decoding begins; 64 GiB was experimentally rejected with `EINVAL`, while 1 TiB succeeded. The physical-footprint policy remains 768 MiB. XNU rejects an address-space ceiling below existing VM usage; the worker reports the failure and never silently raises a configured limit. See [XNU resource-limit enforcement](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_resource.c).
