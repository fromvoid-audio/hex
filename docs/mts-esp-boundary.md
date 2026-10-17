# MTS-ESP master boundary

`third_party/mts-esp-master` is a standalone Rust package that contains the
only unsafe dynamic-library and raw ABI boundary for MTS-ESP. The parent Hex
workspace continues to forbid unsafe code. The package exposes only validated
frequency publications, typed client counts, and an owner-thread-only scoped
master handle; native pointers and the library handle do not escape.

The adapter follows ODDSound MTS-ESP commit
[`f214739b8832e7f297cb9970d0c0efbf783f1462`](https://github.com/ODDSound/MTS-ESP/tree/f214739b8832e7f297cb9970d0c0efbf783f1462),
especially `Master/libMTSMaster.h`, `Master/libMTSMaster.cpp`, and the upstream
README. Their interface files carry the permissive notice retained at
[`third_party/mts-esp-master/LICENSE-ODDSOUND.txt`](../third_party/mts-esp-master/LICENSE-ODDSOUND.txt).
Hex does not vendor or redistribute ODDSound's runtime library.

## ABI and ownership

The public master header declares a no-argument `MTS_RegisterMaster()` wrapper.
That wrapper is not the raw dynamic-library ABI: the pinned C++ wrapper resolves
`MTS_RegisterMaster` as `void (*)(void*)` and calls it with null. The adapter
uses that raw signature and null argument. It requires the exact registration,
deregistration, master-state, version, client-count, full-table, name, period,
map-size, map-start, and reference-key symbols. It rejects a missing symbol or
library version below `0x00010003` before registration.

Production loading uses only
`/Library/Application Support/MTS-ESP/libMTS.dylib`. There is no public API for
an arbitrary path or symbol. Registration refuses when `MTS_HasMaster()` is
already true and never calls `MTS_Reinitialize()` or attempts takeover. A
process-local atomic gate prevents duplicate registrations through this
adapter. The returned `Master` is `!Send + !Sync`, keeps the library loaded,
and calls deregistration exactly once before releasing that library handle.
All registration, publication, client-count, and teardown calls must run on
the same serialized control thread, never on an audio callback. Callers must
explicitly republish after reacquiring a master; the adapter has no retry
worker or hidden state synchronization.

MTS exposes a sequence of setters, not an atomic publication transaction.
The adapter validates the complete table and metadata before its first call,
but clients may observe table and metadata changes at different points. The
MTS API also cannot protect against another process racing between the
`MTS_HasMaster` check and registration, or against external/manual library
reinitialization while the adapter is active. Such cross-process races and
external resets are unsupported; Hex does not attempt recovery or takeover.
When the host isolates plug-ins into multiple processes, MTS-ESP's optional
IPC domain may share this master state across those processes.

The upstream README states that standalone macOS master or client apps need the
Disable Library Validation entitlement to load `libMTS.dylib`. Hex has made no
signing, entitlement, installation, or privileged-system changes. Runtime
availability still depends on the separately installed, compatible ODDSound
library at its fixed system path.

## Metadata and validation

The adapter accepts exactly 128 finite positive frequencies and bounds the
scale name to 16 non-NUL bytes. Period ratio is bounded to `[1, 16]`; map size
must fit a positive signed byte (`1..=127`). The MTS header documents negative
map values as unavailable and reserves `-1` as the sentinel. `ScaleMetadata`
derives map start as `root_key - root_degree`; when that value would be below
MIDI key 0, it sends `-1`, never a wrapped signed byte. Reference key remains
the explicit root MIDI key. A negative native client count is reported as a
typed error.

## Verification

The package is tested independently with controlled local C shared-library
fixtures. Those fixtures verify the raw null registration argument, existing
foreign-master refusal, duplicate local registration, required-symbol and
minimum-version checks, full table and metadata publication, validation before
native calls, client count, and deregister-before-unload ordering. Tests do not
load the installed MTS library or alter machine-wide master/client state.
Standalone package checks:

```sh
CARGO_TARGET_DIR=third_party/mts-esp-master/target cargo test --manifest-path third_party/mts-esp-master/Cargo.toml --locked
CARGO_TARGET_DIR=third_party/mts-esp-master/target cargo clippy --manifest-path third_party/mts-esp-master/Cargo.toml --all-targets --locked -- -D warnings
```

Actual installed-library validation and connection to a compatible external
client remain separate manual acceptance; no client connection is claimed by
the controlled fixture tests.

## Serialized desktop publisher

`hex-mts` is the safe control-side owner of the excluded adapter. Its worker is
created without registering a master; only an explicit project enable claims
ownership. One bounded latest-value mailbox coalesces tuning publications while
reserved epoch/attempt/shutdown state preserves disable, project switch, retry
and process teardown. A foreign master is never reset or replaced. Unavailable
or blocked registration requires explicit retry, including after ordinary edits.
Native calls, registration and deregistration stay on this worker, outside all
Hex callbacks. Desired and published revision identities remain separate; MTS
client count is native-reported, not a promise that every client consumed a table.

Linux source verification includes four injected-transport ownership/lifecycle
tests and five actual dynamically loaded C ABI fixture tests. The fixture uses
`-shared` on Linux and `-dynamiclib` on macOS; this does not enable the production
system-library loader on Linux. Installed macOS runtime, signing/library validation
and compatible-client behavior remain native acceptance requirements.
