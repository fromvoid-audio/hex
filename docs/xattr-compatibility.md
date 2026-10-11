# xattr dependency compatibility

Pulse #2349 / PR #161 repairs a source incompatibility exposed by Linux cloud
builds after native prerequisites were available. It is separate from environment
maintenance #2348 / PR #160. The dependency chain is GPUI 0.2.2 →
zed-async-tar 0.5.0-zed → xattr 0.2.3 → libc 0.2.190.

## finite scope and boundary decision — 11 October 2026

Preserve the parent dependency, public xattr API, licenses and macOS behavior;
repair Linux missing-attribute errno compilation; retain a compile regression
and verify native macOS missing-attribute behavior plus the desktop build.
No application DSP, UI, storage, environment setup or registry-cache changes.

The parent requires xattr ^0.2; 0.2.3 is the last published compatible release.
A broad parent upgrade or libc downgrade would affect more dependencies. Instead,
copy the checksum-verified published xattr archive to excluded `third_party/xattr`
and resolve it through `[patch.crates-io]`. This is the same existing external
native dependency chain, not a new workspace FFI boundary. Hex still forbids
unsafe code; existing external syscalls and unsafe blocks are unchanged.
All MIT/Apache-2.0 licenses, archive files and metadata are retained.

Only `src/sys/mod.rs` changes: Linux/Android use libc::ENODATA for the internal
missing-attribute sentinel; macOS/BSD retain libc::ENOATTR. Public absent-attribute
queries still return Ok(None), while unrelated errors propagate. Unsupported
platform handling is unchanged. Cargo.lock only loses the two registry identity
lines for xattr; every package version and all other checksums remain unchanged.
See the exact [patch](../third_party/xattr/hex-patches.diff) and
[provenance](../third_party/xattr/hex-provenance.json).

## regression and evidence

Before repair, `cargo +1.94.0 check --locked -p xattr --target
x86_64-unknown-linux-gnu` reproduces E0425 at upstream `src/sys/mod.rs:12`:
libc::ENOATTR is absent. This selects the actual Linux source/configuration,
without GPUI/ALSA or a Linux linker. The reproduced source failure matches the
reported cloud failure; no native prerequisite workaround can fix it.

```sh
# Rust's Linux standard-library target must already be installed.
RUSTUP_TOOLCHAIN=1.94.0 python3 scripts/check-xattr-compat.py --target x86_64-unknown-linux-gnu
python3 scripts/check-xattr-compat.py
python3 scripts/local-ci.py
```

The compatibility runner checks every vendored baseline/patched hash, dependency
exclusion and lock identity. It builds the locked real dependency and compiles a
public API test using Cargo's exact artifacts. On its native host it also runs
that test: absent attributes through both path and descriptor APIs return None;
a removed file still returns NotFound. Cross runs explicitly do not execute it.
The script honors the existing Cargo target configuration; do not run it while
local CI owns the same target. It does not install tools or modify setup.

Rust 1.94.0 Linux compile regression and Rust 1.98.1 macOS native regression
passed. Before/after logs are retained in this PR worktree under ignored
`artifacts/xattr-compatibility/`. Full local CI is recorded separately against
the final commit and unchanged source digest in the PR and Pulse ticket.

This establishes dependency source compatibility and macOS query behavior.
Linux syscall execution, a complete Linux desktop build and native UI/listening
are not established by cross compilation. Android/BSD runtime acceptance is also
unclaimed. Refreshing the external dependency must review the verified archive,
exact patch and whether the parent now permits a maintained upstream release.
