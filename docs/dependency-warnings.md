# vendored dependency compiler compatibility — pulse2143 / pr153

Two pinned external packages now resolve through excluded local patches:
`block` 0.1.6 and `proc-macro-error2` 2.0.1. Versions and transitive dependencies
are unchanged. Hex's unsafe-code prohibition and existing GPUI baseline/guard
remain intact. No warning suppression or unrelated vendor formatting is used.

`block` replaces only `enum Class { }` with an inhabited `#[repr(C)]` opaque
struct containing a private zero-length byte array. The external symbol
`_NSConcreteStackBlock`, its address use and pointer field remain unchanged.
This marker does not describe Objective-C class contents or their actual size;
the crate neither constructs this marker nor reads the native class contents.
It removes `uninhabited_static` without changing block signatures or ownership.

`proc-macro-error2` changes only `extern crate proc_macro;` to
`pub extern crate proc_macro;`. Its existing macro-support module can then
publicly re-export the crate without `pub_use_of_private_extern_crate` / E0365.

## provenance and guard

Every file from both checksum-verified published archives is retained, including
package metadata, README files, tests and the proc-macro package's MIT/Apache
licenses. The historical block archive declares MIT in Cargo.toml but contains
no separate license text; the upstream 0.1.6 tag likewise contains none. No
license text or upstream attribution is invented. The source archive checksum
is the baseline identity; an upstream branch is not a replacement baseline.

Each directory contains `hex-provenance.json` with original/patched SHA-256
hashes, `hex-patches.diff` with the single source delta and `HEX-PATCHES.md`.
`python3 scripts/check-dependency-patches.py` verifies these hashes, exact source
deltas/diffs, retained file set, pinned package/lock identities, path resolution,
workspace exclusion and Hex's unsafe prohibition. Both quick and full local CI
run this guard. The independent GPUI guard is unchanged. Full local CI also runs
`check-dependency-future-lints.py`: the actual block source must compile under
its denied lint, and a downstream macro consumer must expand/run with the
actual proc-macro patch under its denied lint, default and no-default features.
These standalone fixtures are not shipped application crates. The provenance
guard uses only Python 3.9 standard-library APIs, matching documented `python3`;
it does not require `tomllib` or a backport. The Python Cargo regression step is
explicitly marked as target-using, so the runner claims and locks a fresh target
before invoking it. Unknown nonempty targets still fail closed without adoption.

Primary sources inspected before choosing these changes:

- [published block upstream](https://github.com/SSheldon/rust-block), including
  tag 0.1.6 (`47178790cfc9d4a8b092051d8b413b78bd31254a`).
- [proc-macro-error2 upstream](https://github.com/GnomedDev/proc-macro-error-2).
- [Clang's Apple Blocks ABI](https://clang.llvm.org/docs/Block-ABI-Apple.html)
  and [code generation](https://clang.llvm.org/doxygen/CGBlocks_8cpp_source.html).
- [Apple libclosure data](https://github.com/apple-oss-distributions/libclosure/blob/main/data.c).
  Its `!HAVE_OBJC` arrays describe a legacy runtime and do not independently
  prove the actual macOS Objective-C symbol representation.

## native prerequisite evidence — 11 october 2026

Read-only Mac support chat `01a12a1a-8aae-7ae3-b9e7-c18d7db3c037` supplied
`pr153-native-evidence/evidence.json`, uploaded to this coordinator (SHA-256
`d889ca935bfa26be16eadd3e662aad66fca6b718aede7e5cd6fdeb571d9d68f3`). The retained
actual Cargo report confirms block src/lib.rs:64 and proc-macro-error2
src/lib.rs:494, following its private import at line 277. Historical full CI
passed on `3f50b884f58d90633be1b60e3159033dad669879`, unchanged source digest
`d3183933bad18aee64f9ff81add68970ae8eb6ce5f4ec0c7faef0f51e8c370ea`.
Those earlier successful builds still emitted the notices.

The scratch candidates match the two deltas above. On macOS 26.6.2 arm64,
stable Rust 1.98.1 and installed 1.100 nightly compiled the block candidate
with denied `uninhabited_static`; the original declaration failed. Local Apple
Clang IR uses the native symbol address. C/native and Rust block `isa` values
matched `dlsym` for stack and heap symbols. Stable debug/optimized and nightly
optimized fixtures compiled, linked Foundation/System and passed stack
invocation both ways, copy, escape, clone/release and exactly-once capture drop.
This proves the tested symbol-address contract, not Objective-C internal layout.

The actual proc-macro scratch copy passed its denied future lint and downstream
`#[proc_macro_error]` consumer on stable/nightly with default features and stable
without default features. The original declaration reproduced the error.
Direct block compilations retain 46 pre-existing `missing_abi` warnings; these
are outside this ticket's two specified future-incompatibility notices.

The native evidence is retained on the Mac at
`/Users/portalsurfer/.codex/visualizations/2026/10/11/01a12a1a-8aae-7ae3-b9e7-c18d7db3c037/pr153-native-evidence/`.
The uploaded summary references detailed fixture/source hashes in the adjacent
`provenance.json`; that file and full logs have not been independently read by
this cloud coordinator. This is reported prerequisite evidence, not final-tree
Hex CI. macOS x86_64, iOS, non-Apple runtimes, all block signatures/callback paths,
whole-app native behavior and physical deadlines remain unverified.

## delivery validation

Cloud checks and final commit identity are retained in Pulse2143 and PR153.
Full exact-commit Mac local CI and a fresh locked release build without the two
specified notices remain required on the pushed implementation. Linux checks
cannot establish native/macOS acceptance. PR153 stays draft; merge is not
within this task's authorization.
