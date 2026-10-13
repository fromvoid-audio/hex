# controlled CLAP discovery

## Purpose

Own a safe Rust standalone Linux x86_64 supervisor and a separately built native
CLAP descriptor-only helper. Only ORIGINAL controlled fixtures are admitted. The
approved native boundary owns dlopen/entry/factory pointers entirely inside a
fresh child; no Rust FFI/wrapper/unsafe exception is adopted. This stage does not
instantiate or process plugins. CLAP descriptor keywords are metadata, never
assertions of bus/event/parameter/state/latency/editor capabilities.

## Dependencies

No internal workspace edges. Existing exact serde/serde_json encode bounded
receipts/results; sha2 hashes only controlled scanner build/module identity.
Pinned rustix1.1.5 process/event and signal-hook0.3.18 (default features off) own
waitable children, process groups, safe atomic SIGCHLD registration, subreaping
and readiness polling in the STANDALONE executable only. Their use is explicitly
registered for this boundary; unsafe remains forbidden throughout Hex Rust.
Official CLAP1.2.10 headers195b42a004144fab0b3cf95e9c067187d15365b7
are separately supplied and MIT notices retained. No Clack/clap-sys/libloading
or new UI/audio framework is adopted. Original native sources build C++17/O2
with all warnings treated as errors; source/header/compiler/system-library and
license closure enter a bounded build receipt. Builder performs no fetch/install.

The process owner is derived from frozen PR1934262556b and lives in this new
format-owned crate. That branch and its supervisor are not changed or extracted.
There is no API dependency between format scanners or production engine/audio.

## Does not own

Third-party/installed-directory execution, security sandboxing, plugin instance
or DSP lifecycle, realtime ownership, buses/events/parameters/state admission,
latency/routing, editors, project/UI integration, native Mac packaging/signing,
reference/listening/deadline acceptance or complete Pulse20 hosting.

## controlled invocation and guarantees

`native/build.py --sdk <already-fetched-pinned-clap> --output <owned-folder>`
produces `controlled-clap-fixtures.json`. Invoke
`hex-clap-scan <controlled-clap-fixtures.json> <fixture-name> [deadline-ms]
[previous-report.json]`; optional stdin `cancel` terminates the owned attempt.
Only receipt-listed ORIGINAL names/paths are accepted. ELF64LE x86_64 ET_DYN,
exact source/helper/module hashes and immutable before/after receipt checks are
required. Caller-owned receipts establish build provenance, not authenticity of
arbitrary code. No credential/environment dumps or installed-folder search occurs.

Native entry compatibility is explicitly CLAP major1; invalid/future major
profiles refuse. Successful entry init is paired with exactly one deinit; failed
entry init is never followed by another CLAP call or deinit. Factory/descriptor
pointers are borrowed until deinit and never freed by the host. No create_plugin
call exists in the probe. Copied IDs must be nonempty and unique per module;
mandatory names are nonblank. UTF8 is strict on Rust ingress, unknown feature
strings remain unchanged. Descriptor count1024, strings512bytes, features64 per
plugin and feature strings128bytes are controlled unit limits; aggregate packets
fit1MiB. Native strings/features are checked before copying. Malformed pointers
can still crash a foreign child; only original controlled fixtures are supported.

Dedicated length-delimited/versioned metadata IPC remains separate from bounded
stdout/stderr diagnostics (64KiB each). Success requires deinit/unload before the
terminal packet, normal helper exit, complete reap, strict packet validation and
unchanged identities. Deadline1..30000ms/cancel/crash/overflow/malformed/changed
attempts retain the catalog's prior valid record and a current error. The direct
PID remains unreaped while scoped TERM/KILL target its process group; no later
signal risks PID reuse. Adopted descendants are reaped with a retained PID ledger.
Cleanup beyond500ms retains a live owned lease and failure while driving reaping;
no universal finite termination for OS-stuck tasks is promised. Inherited ignored
SIGCHLD/SA_NOCLDWAIT is repaired through the audited safe registration API.

This is process crash isolation, NOT filesystem/network/host process containment.
The actual shared Linux namespace probe failed UID mapping with read-only
filesystem; installed/untrusted loading remains refused, with no weaker fallback.
No system policy change is made by this crate. Native Linux tests require explicit
HEX_CLAP_FIXTURE_RECEIPT and `cargo test -p hex-clap-scan --test native --locked
-- --ignored --test-threads=1`; default workspace runs visibly ignore those tests.
