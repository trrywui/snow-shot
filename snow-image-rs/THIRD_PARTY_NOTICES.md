# Snow Image Rust third-party notices

Snow Image Rust is GPL-3.0-or-later. Its native dependencies are built statically
into the worker; the public host library does not link them. This project does
not change the dependency set of the existing Qt applications.

| Component | Pinned version | Upstream license family |
| --- | --- | --- |
| png | 0.18.1 | MIT OR Apache-2.0 |
| gif | 0.14.2 | MIT OR Apache-2.0 |
| image | 0.25.10 | MIT OR Apache-2.0 |
| memmap2 | 0.9.12 | MIT OR Apache-2.0 |
| libjpeg-turbo | 3.2.0 | BSD/IJG/zlib; retain all upstream terms |
| libwebp | 1.6.0 | BSD-3-Clause and patent grant |
| libjxl | 0.12.0 | BSD-3-Clause and patent grant |
| libavif | 1.4.2 | BSD-2-Clause |
| dav1d | 1.5.3 | BSD-2-Clause |
| libaom | 3.14.1 | BSD-3-Clause and patent grant |
| Little CMS | 2.17 | MIT |
| zimg | 3.0.6 | WTFPL |
| Highway | 1.2.0 | Apache-2.0 |
| Brotli | 1.1.0 | MIT |

This software is based in part on the work of the Independent JPEG Group.

The AOM encoder also incorporates the bundled FAST feature detector (Edward
Rosten, BSD-3-Clause), vector implementation (Peter Goldsborough, MIT), and x64
assembly helpers (x264 contributors, ISC). Their separate upstream notices are
included at their original `third_party/fastfeat`, `third_party/vector`, and
`third_party/x86inc` paths inside the AOM notice directory.

The authoritative native source URLs, archive names and SHA-256 checksums are in
[`native/sources.json`](native/sources.json). Cargo's complete independently
resolved dependency graph and registry checksums are in `Cargo.lock`. Transitive
Rust dependencies include their upstream notices in the generated distribution
bundle; the table is not a replacement for that bundle.

Run `cargo xtask licenses` after dependency changes. It gathers actual license,
copyright, notice and related texts from the verified native source trees and the
resolved non-development Rust graph for all four supported targets. It fails if a
dependency lacks a distributable notice. Output: `target/third-party/INDEX.md`
and `manifest.json`, with per-package texts. `cargo xtask package` ships that
bundle beside the worker.

Package staging also includes `Cargo.lock` and `native-sources.json`; its manifest
records hashes for these inputs and the worker executable. The package command
selects an explicit supported native target and absolute Cargo output directory.
It replaces a previous completed bundle only after collecting all notices and
writing the new manifest; a failed publication rename restores the previous
bundle. It stages an unsigned package and does not publish a release or establish
the outstanding native validation gates.

`cargo xtask licenses --repository-check` additionally runs the repository's
`scripts/collect-third-party-licenses.ps1` against a notice-only compatibility
prefix generated from these verified native sources. That check's legacy
"vcpkg" category describes its input schema; these dependencies are not built
with vcpkg. Its separate output is validation evidence, not the standalone
distribution bundle.

Upstream patent grants, copyright notices and license conditions must accompany
redistribution. Source availability and GPL obligations apply to the combined
distribution. Generated license expressions summarize package metadata; the
collected upstream texts control.
