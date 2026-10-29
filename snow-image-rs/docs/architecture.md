# Ownership and process architecture

| Package | Responsibility |
| --- | --- |
| `snow-image-core` | Checked descriptors, immutable image storage, row sources, shared metadata, reservations and typed options/errors; no unsafe Rust |
| `snow-image-rs` | Runtime-neutral jobs, finite queues, process supervision, animation actors, input snapshots and atomic publication |
| `snow-image-codecs` | Worker-only codecs, color/HDR processing, orientation, linear-premultiplied resizing and animation composition; no unsafe Rust |
| `snow-image-native-sys` | Checked-in FFI and safe lifecycle wrappers; the only codec-facing unsafe Rust |
| `snow-image-protocol` | Private bounded control frames and binary transfer schema |
| `snow-image-transport` | Private macOS descriptor handoff, shared-memory creation, and kernel object/size validation |
| `snow-image-worker` | Versioned dispatch, cancellation reader and native operation containment |
| `xtask` | Explicit dependency preparation, license collection, packaging and targeted benchmarks |

## Resource ownership

`Budget` reserves categories before allocating. Reservations are unique owners;
shared storage retains a single reservation instead of duplicating charges.
`MutableImage::seal` consumes the mutable owner. An `ImageView` cannot outlive its
image. Descriptors validate dimensions, multiplication/addition, strides, sample
types, significant bits, orientation, alpha and color conventions.

The default Snow Shot policy separates these limits:

| Resource | Default |
| --- | --- |
| Imported encoded input | 256 MiB |
| Retained metadata | 8 MiB |
| Animation frames | 10,000 |
| Per-job accounted working allocation | 512 MiB |
| Aggregate accounted memory | 1 GiB |
| Logical raster | 8 GiB |
| Prepared pixel allowance | 64 MiB |
| Fit preview | 2048 pixels |
| Encoded artifact | 1 GiB |
| Aggregate temporary backing | 16 GiB |
| Queued jobs / active codec operations / codec threads | 16 / 2 / 8 |
| Worker physical-footprint threshold or Windows commit limit | 768 MiB |
| macOS virtual address-space limit | 1 TiB |

Admission reserves a child memory envelope from the remaining aggregate allowance
and passes that smaller finite policy to the worker. A static operation reserves
at most half the remaining memory; a persistent animation worker initially
reserves at most a third. Disk backing uses parent-owned envelopes too. This is
conservative reservation accounting, not a claim that every reserved byte is
resident. Admission can fail when retained images/sessions consume the budget.
No limit is silently increased, no quality setting is lowered, and no hidden
full-image copy occurs on shared ownership.

Native allocators without hooks are additionally supervised by OS containment.
On Windows the parent creates a Job Object, installs process/job commit limits
and kill-on-close, and only then resumes the suspended child. On macOS the worker
sets `RLIMIT_AS` before codec execution and the parent samples physical footprint.
Sampling permits transient overshoot and is not a portable hard RAM cap. Modern
dyld reservations require a large virtual limit: the 1 TiB default is independent
of the 768 MiB physical threshold. An explicitly configured virtual limit that the
OS rejects fails closed. Allocation estimates, Rust ownership and process limits
do not replace native sanitizer/lifecycle tests.

## Worker lifecycle

Two bounded static supervisor lanes keep disposable workers prestarted. Idle
workers have no admitted job envelope. Each static lane retains one input executor
and one output executor across disposable workers, using capacity-one task and
completion channels. Animation connections retain their own executor pair across
commands. Idle executors hold no image buffers, child handles or resource leases.
A reply does not complete a call until its input transfer also finishes under the
same deadline. Each worker handles one static operation and exits; results are
published only after exit is observed and the process is reaped. The completed
static connection releases its process and backing owners before its executor
pair moves to the next worker. Two animation actors lazily create persistent
workers, multiplex sessions,
execute one command per actor at a time, and retire their process after its final
session closes. All four lanes share the two-operation and CPU-thread admission
limits. Session recovery reopens immutable input and replays at most 64 frames
within a two-second replay allowance.

Persistent workers are sampled again before a successful command completes and
every 50 ms while idle. Idle checks also detect process exit. An over-limit
connection is terminated; both failed and exited workers are reaped and their
process trees checked before their envelopes are released. Retained sessions can then use the same bounded
generation reopening/replay path; previously published snapshots remain owned by
the host. Sampling still permits transient overshoot between observations.

Cancellation queues a cooperative command on the input executor after its payload
transfer finishes, then terminates the worker after the default 100 ms grace.
Deadlines, footprint excess,
producer failure, malformed replies and pipe failure also terminate it. The
parent kills the Windows Job or Unix process group to close inherited pipes,
waits for the child, drains both I/O tasks, and then releases reservations. Even a
blocked cancellation write cannot prevent the supervisor from enforcing its grace
period. Service destruction starts cleanup off the caller's thread. Completed
snapshots remain independent.
Custom application row producers must cooperate with cancellation; Rust threads
cannot safely be forcibly stopped.

On macOS, a prestarted connection registers a kqueue `EVFILT_PROC`/`NOTE_EXIT`
notification. After a static response and input transfer complete, the supervisor
can wait for this exit hint instead of a polling sleep. Each wait is bounded by
two milliseconds and the remaining operation/cancellation deadlines. The event
never reaps a process or authorizes publication: `Child::try_wait` remains the
source of exit status, and the separate process-tree check still proves that no
writer remains. Other platforms retain the existing bounded polling cadence.

Capability creation through worker spawning is serialized process-wide because
macOS pipe/socket creation, kqueue creation and close-on-exec configuration are
not atomic in the Rust standard library or OS calls. At Unix
worker startup, descriptors other than stdin, stdout and stderr are closed before
the worker opens its own files or starts threads. This also prevents descriptors
inherited from unrelated application threads from retaining a sibling's pipe.

## Storage and transport

External inputs are copied into private anonymous backing. Statically prepared
rasters can also be file-backed. Worker temporary files are anonymous (or
delete-on-close), so abrupt termination closes the underlying OS handles.
`EncodedArtifact::save` stages in the destination directory, flushes and syncs,
then atomically persists with explicit create-new or replace semantics. A failed
save leaves the previous destination intact.

Control frames are length-prefixed JSON bounded to 1 MiB; pixels and metadata use
separate length-checked binary payloads, ordinarily in 64 KiB transfers. Large row
sizes are explicitly reserved. Received generations, request IDs, descriptors,
metadata lengths, preview/transform bounds, animation canvas/count and artifact
precision/fidelity receipts are checked before publication. Parsing reserves a
conservative envelope for JSON collection expansion. Capabilities and bounded
remote diagnostics retain metadata leases after their jobs finish.

Bounded byte reads reserve the complete payload fallibly, then fill that capacity
through `Read::take(...).read_to_end`. In the pinned Rust 1.97.1 standard library,
`Take` uses the default spare-capacity reader; its exact-fit EOF probe stops at
the declared limit without another allocation or reading the next message.
The protocol's separate allocator-guard test refuses every allocation after the
initial reservation, including growth, and covers interrupted/short reads,
truncation and a following pipe sentinel. Toolchain upgrades must preserve this
tested allocation contract.

Static workers receive an anonymous publication file through an inherited OS
handle. Before running codecs, the worker duplicates the handle and redirects
native and Rust diagnostic output away from it. Results use the file plus bounded
control/metadata messages. Before any successful file-backed pixel, raster or encoded-artifact reply, the worker
checks mapping flushes and filesystem writeback for the completed output. On Unix,
this uses checked `fsync` with interrupted calls retried; it does not request the
macOS hardware-cache flush used by `File::sync_data`. Windows uses the corresponding
mapping and file flush operations. These checks detect deferred allocation and I/O
failures, including a full filesystem whose dirty cached pixels still read correctly.
A failed writeback produces an error instead of publishing a result. The host validates
its exact length only after the worker is reaped and the entire process group or
Windows Job is confirmed empty.
Decoded pixels then use a read-only mapping with retained pixel and disk leases;
encoded artifacts retain the private file directly. Prepared rasters first try
the shared 64 MiB prepared-pixel allowance. Successful admission produces a
read-only mapping with one shared cache reservation and its disk reservation;
clones do not duplicate either charge. When cache or aggregate admission fails,
the same immutable raster stays file-backed, without recomputing or copying its
pixels. No writable handle or file path escapes. Mapped image clones do not copy pixels.

On macOS the startup handoff uses a private Unix datagram socket and transfers
both the regular file and an immediately unlinked POSIX shared-memory object.
The bounded initial descriptor message is queued before spawn. The worker receives and
marks both descriptors close-on-exec before starting threads; malformed messages
close their received handles. Darwin allocates all SCM_RIGHTS handles before
copying ancillary data and can omit their numbers when the control buffer is too
small. The receiver accommodates the kernel's bounded 512-handle maximum and
then rejects every count other than two; the 64-handle regression and near-limit
254-handle tests verify cleanup.

Eligible direct JPEG decoding and previews that need no resize use this separate
shared-memory object. After descriptor validation the worker reserves page-rounded
pixel memory, sizes the object once, decodes through an exclusive writable mapping,
and unmaps it before replying. Darwin shared-memory objects cannot be resized or
read/written as regular files. The host checks the kernel object type and rounded
length, observes process-tree quiescence, then retains a read-only pixel mapping
with its memory reservation. There is no filesystem writeback or temporary-disk
charge for these pixels. Other operations and Windows retain the checked file
publication path. This optimization does not change persistent animation copying.

On macOS, input snapshots are written and checked for filesystem writeback, opened
read-only, unlinked, and stripped of their sole writable owner before publication.
Static inspect/decode/preview jobs transfer this read-only input capability through
the retained private datagram channel before sending Run. Both endpoints require
an unlinked regular file and read-only access; the worker checks the exact declared
length, reserves page-rounded input memory, then maps it read-only. This avoids a
second input copy and never changes a shared file offset. The host retains the
snapshot's disk lease through process reaping. The mapping and its input reservation
live through codec execution. Invalid handle messages close every delivered handle.

Other platforms copy static encoded input into the inherited publication file
before Run. The worker validates its size, admits owned input memory and disk,
finishes reading it, and only then permits output replacement. An output mapping
cannot exist during this phase. Animation input and raw raster rows use bounded
pipe transfers. PNG/JPEG/BMP consume a three-buffer
256 KiB streaming queue; other operations can use bounded private backing.
Animation snapshots always cross through copying I/O into host-owned storage:
persistent workers can never publish a shared writable mapping. Benchmark results
include transport, process supervision and publication costs.

## Processing

PNG, JPEG and BMP encode rows without a full source materialization. JXL uses its
chunked input/output API and allocation callbacks. WebP and AVIF explicitly
reserve materialization and scratch; file-backed input does not make these
encoders constant-memory. zimg scratch and row-buffer requirements are queried
before processing. Reductions above 16× per axis use staged reductions. The
resident path reserves its intermediate pixels; row-source processing uses
bounded temporary backing when intermediates cannot remain in the row pipeline.

ICC profiles stay tied to the pixels. Relative-colorimetric transforms with
black-point compensation use the one private Little CMS build. Resizing operates
in linear light with premultiplied alpha. `ScreenshotSdr` adapts Snow Capture's
screenshot tone/gamut mapping and produces sRGB without imported EXIF/XMP.
`PreserveDynamicRange` keeps the explicit scRGB/PQ/HLG convention; floating-point
storage alone does not classify a raster as HDR. See format contracts for exact
encoding and precision restrictions.
