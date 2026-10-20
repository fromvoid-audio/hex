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
cannot establish native/macOS acceptance. Source merge follows the standing reviewed-PR policy in AGENTS.md; native
acceptance remains separate and open until renewed on the final source.

Linux full validation on `6838df4` with verified rootless native prerequisites
reproduced the independent xattr/libc errno failure. The already-merged PR161
implementation (`9b66bab6e7ba1a8bab8d754cc08d0bd6ec66b1f6`, merge `5a79795`)
is integrated here as a prerequisite, preserving its exact vendor files, guard,
license/provenance and unchanged package version. The two warning patches remain
identical. This integration creates a new source revision; earlier exact-commit
Mac CI/release evidence must be renewed before acceptance.

## current warning cleanup — pulse2872 / pr474

The historical 46 `missing_abi` warnings above are now addressed by explicit C
ABI on all ten external declaration/function sites in the actual block source.
The previous implicit external ABI was C, including function-pointer and macro
callback declarations; signatures, symbols, ownership and marker layout remain
unchanged. The existing provenance guard reverses this finite ABI spelling delta
as well as the opaque marker change, then verifies the original published hash
and complete patch. Original archive identity, retained files/licenses, package
version and lockfile remain unchanged. The actual dependency-lint compilation
now denies `missing_abi` too. This compiler evidence does not renew native block
callback/lifetime acceptance on macOS.

The declaration spelling follows the [Rust Reference's explicit C/default ABI
contract](https://doc.rust-lang.org/reference/items/external-blocks.html#abi).
The zero-context recorded vendor diff avoids whitespace-only patch context;
its exact reconstruction/hash guard still verifies every original byte.

### pinned clippy and declared pcm layout

The old PCM `Unknown: unsupported_wrapper_or_ambiguous_flags` warning in strict
Clippy was real: Cargo sets `RUSTC_WORKSPACE_WRAPPER` to its installed driver.
The default gate still refuses unsupported flags, targets, compilers, nested or
foreign wrappers, sysroot/bootstrap/build-std overrides and missing evidence.
A finite build-only exception now qualifies the pinned official Clippy component
under the existing ordinary installed-toolchain trust: exact native Rust 1.94.0
commit/std installation, exact non-symlink installed compiler/driver paths and
component manifests, matching complete compiler metadata, sysroot and pinned
Clippy version, and unchanged file identities before/after probes. Missing or
repaired component entries are tracked for Cargo reruns too.

The [actual pinned driver source](https://github.com/rust-lang/rust/blob/4a4ef493e3a1488c6e321570238084b38948f6db/src/tools/clippy/src/driver.rs)
uses embedded rustc, consumes `SYSROOT` and parses `CLIPPY_ARGS`; info queries
bypass those arguments. Consequently `SYSROOT` is refused independently and only
empty args or the exact observed Cargo `-D warnings` encoding is accepted. No
arbitrary option prefix, response file or wrapper filename is trusted.

Clippy adds `cfg(clippy)` and changes MIR passes. The concrete `Frame = [f32; 2]`,
`AudioData` retained Vec/optional reservation, source constructor and receipt
paths have no `cfg(clippy)` layout alternatives. The pinned std `ArcInner` is
`repr(C, align(2))` with strong/weak atomic counters followed by the payload;
receipt payload layout is measured from the actual compiled type. This supports
the existing declared-storage-layout law, not runtime equivalence or physical
allocator usage. A modified/hostile installed toolchain is outside the existing
installation trust; metadata/version checks are not binary authenticity proof.
No production PCM, Audio construction, audio callback or allocator law changed.

Native macOS compilation and optimized physical behavior remain separate checks.
The native owner reports PR395's original two Audio unused-host errors cleared in
its intermediate cfg(test) source (Pulse29631/29634); production promotion and
any other build diagnostics remain with that owner. This PR does not edit those
in-flight files or use Linux checks to establish Mac acceptance.
