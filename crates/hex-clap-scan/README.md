# controlled CLAP discovery and offline fixtures

## Purpose

Own a safe Rust standalone Linux x86_64 supervisor and a separately built native
CLAP descriptor-only helper. Only ORIGINAL controlled fixtures are admitted. The
approved native boundary owns dlopen/entry/factory pointers entirely inside a
fresh child; no Rust FFI/wrapper/unsafe exception is adopted. The descriptor probe never instantiates or processes plugins; the separate offline
child described below owns the approved original-gain lifecycle. CLAP descriptor keywords are metadata, never
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

Third-party/installed-directory execution, security sandboxing, production plugin instance
or DSP lifecycle, realtime ownership, general buses/events/parameters/state admission,
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


## controlled offline gain unit (legacy version1)

The same offline builder additionally produces `controlled-offline.json`, bound
by digest to the complete scanner build receipt and five original offline sources.
`hex-clap-offline <controlled-offline.json> <original-name> <request.json>
[deadline-ms]` uses the existing owned supervisor with bounded duplex IPC.
Only the twelve freshly built ORIGINAL processing fixtures are accepted; descriptor
scanner identities, catalog retention and its no-instantiation behavior remain.
No installed module or real-time/editor/engine admission is introduced.

Version1 owned request/response packets are limited to1MiB, opaque state64KiB,
32blocks of1..1024frames,8192frames total,128gain events/block and1024total.
Sample rates are44100/48000; a checked nonnegative steady frame clock is echoed
with request ID/source/platform/capability metadata. This is a free-running offline
clock, not DAW transport, wall-clock recording or hardware application evidence.
The single global plain gain parameter0x200001 spans0..2 (default1); events are
steps at sorted sample offsets, with latest input event winning coincident offsets.
No MIDI/note buses, polyphonic targeting, output events, modulation or GUI exists.
The exact supported profile is one stereo Float32 main input/output, zero latency,
and explicit `clap.render` offline approval; other capabilities are refused.

The native control thread remains the main thread from create through destroy.
It owns initialization, copied extension/port/parameter metadata, inactive initial
parameter flush or opaque state load, activation and deactivation. A single joined
logical audio thread serializes start/process/stop. No parameter flush overlaps
processing. Deferred callbacks are serviced on the main thread; callbacks during
destroy are retained as diagnostics without calling the destroyed instance.
Host/thread-check context survives destruction. Failed instance init still destroys
the instance; entry init success is paired with deinit, followed by unload.

Partial stream adapters transfer at most7bytes per read/write. Original state load
validates fully before committing; unknown versions/truncation/trailing bytes refuse.
Loaded state owns gain/history; `initial_gain` applies only to an empty-state start.
State save occurs inactive after joined stop. All processed PCM must be finite;
ERROR/unknown status, unexpected output events or restart requests discard the
whole attempt. No earlier processed prefix is published on failure.

Sleep stops processing. Subsequent zero-input/event-free blocks are explicitly
reported as host skip status5 and emit zeros; nonzero input/events/request_process
wake through start_processing. The original sleep fixture is zero-tail and quiet.
CONTINUE/CONTINUE_IF_NOT_QUIET remain callable; TAIL uses only explicit caller
blocks. The original tail fixture declares2frames and demonstrates stateful tail
continuation; final TAIL status means residual work remains, not a fully drained
export. The helper never silently appends or truncates an unbounded tail.

Run the actual optimized native unit with HEX_CLAP_OFFLINE_RECEIPT and
`cargo test -p hex-clap-scan --test offline_native --locked -- --ignored
--test-threads=1`; native tests remain visibly ignored without explicit fixtures.
Process-group cleanup is still not an OS sandbox. Third-party containment, general
extension/callback negotiation, realtime scheduling/ownership, routing/latency,
project/UI/device integration, editors and optimized macOS packaging/signing/native
listening remain required for full20 and are unproved here.

## controlled offline MIDI1 CC7 profile

Version2 messages add a distinct ORIGINAL `org.hex.controlled.offline-midi-gain`
fixture and `midi1-cc7-gain-v1` profile. The same CLI/helper supports both versions;
legacy version1 gain fixtures retain their plugin ID/state/PCM laws and refuse MIDI
or output events. Module names and message profiles must match before launch.
No production event adapter, instrument, device/editor/project or realtime
capability is introduced. The16 MIDI fixture variants are original source only.

The fixed inactive main-thread note-port contract is one MIDI1 input (index0,
stable ID0x200101), MIDI1 preferred/supported, zero output note ports. Main stereo
Float32, plain global gain0..2, offline mode, zero latency and zero tail remain
required. Unsupported dialect/port metadata refuses. Host note-port extension
advertises MIDI1 representation; any rescan invalidates this fixed attempt.
General params callbacks/rescan/flush scheduling are not advertised. Extension
contexts survive destroy; process input storage is borrowed only during process.

Request events form one ordered list of global gain changes and three-byte CC7:
status0xb0..0xbf, controller7, value0..127, port0. Each CC7 sets gain to2*value/127;
it applies before its addressed sample. Equal-offset events retain supplied order;
the last gain change controls that sample. This law belongs to the original fixture.
Other MIDI messages, running status, notes/MPE/MIDI2/SysEx, modulation, targeted
parameters, custom event spaces and flags are refused. No implicit sorting occurs.
A nonempty MIDI event list wakes sleep even with zero PCM.

MIDI-derived parameter output carries DONT_RECORD and is copied synchronously
inside try_push as owned scalar fields. Cookie is never dereferenced, retained or
serialized; output is never reinjected. A report can precede a later same-offset
host gain change, so it does not assert the final audible value or hardware/recording
ACK. The response contains final main-thread queried gain separately. The fixture
mutates its stack output after try_push; tests verify that owned reports survive.

Existing payload1MiB/state64KiB,32blocks,1..1024frames/block,8192frames/request
bounds remain. Input gain+MIDI counts COMBINE at128/block and1024/request.
Output has independent128/block and1024/request bounds. Native aligned variant
slots and output scalar arrays are preallocated with checked sizeof arithmetic,
not unaligned byte casts. Pinned Linux x86_64 sizes are MIDI24, parameter56,
input slot64, output slot32bytes; static assertions and response decoding refuse
other layouts. Input slot reservation is blocks*128*64 (max262144bytes); output
slot reservation32768bytes. These are slot charges, excluding PCM, state, container
metadata, logs and process/runtime overhead; no OS memory containment or realtime
allocation/deadline claim follows. try_push allocates no storage. Any malformed
header/type/space/target/flag/order/value, wrong-thread/late push or capacity refusal
latches failure even if the fixture ignores false. The whole request then exposes
no successful PCM, state or events. Earlier successful blocks remain unpublished.

Version2 continuation requires a matching state owner (profile, state_version2,
module SHA-256), checked before child launch. The original fixture additionally
validates its distinct opaque magic, exact length, finite gain, frame bound and
zero unsupported residual fields before committing load. State preserves held
gain through7-byte partial transfers; input/output events are never replayed.
Changed fixture identity needs an explicit later compatibility decision. Caller-owned
receipts/owner records are provenance, not third-party authenticity or containment.

Use the same explicit HEX_CLAP_OFFLINE_RECEIPT native test invocation above.
Tests cover independent Float32 bit PCM and emitted-event oracles, all channels,
coincident ordering, partitions, state/endpoints/sleep-wake, malformed raw packets
and output, ignored try_push failure, both output limits, late discard, unsupported
profiles/dialects/rescan/thread calls, crash/hang/cancel and kernel descendant reap.
Legacy native/scanner/protocol checks remain. Third-party containment and the
full20/native blockers listed above are unchanged; this is offline controlled CC7
support only. Rust unsafe-forbid and the existing dependency boundaries remain.

## controlled offline finite delay

Protocol3 adds only the ORIGINAL `org.hex.controlled.offline-finite-delay` module
and `finite-delay-gain-v1` profile. The five reference variants declare fixed
processing latency0/1/16/31/32samples at44100 or48000Hz. This is gain **before** a
stereo FIFO: `y[t] = float32(x[t-D] * gain[t-D])`, with cold history zero. Global
PARAM_VALUE events apply before their unchanged input sample offset; coincident
ordering remains supplied ordering. No MIDI port, shifted input timestamps,
filter/group-delay law or device/engine admission is introduced. Isolation startup,
IPC buffering and scheduling overhead are explicitly unknown and separate fromD.

The pinned1.2.10 latency ABI permits get on the main thread while activating or
active; changed is accepted only on that thread during activate. The host latches
notifications and queries after activate returns. Active latency drift, restart,
wrong-thread/phase calls invalidate the whole attempt. Tail get supports main or
logical audio thread. Tail.changed is accepted only during serialized audio
start/process/stop; queries occur after those calls return. The attempt retains
maximum observed finite tail, including stop, across continuations. Values>32,
including the ABI infinite sentinel>=INT32_MAX, refuse. No dynamic graph restart,
parameter flush/rescan scheduler or realtime promise follows.

Each request block explicitly marks content or drain. Drain must be a contiguous
suffix, zero input on both channels and event-free, supplied by the caller. No
frames are silently appended. Content resets the current drain ledger; tail
notifications never reset it. Caller drain is capped at32samples cumulatively
across continuations, within the existing32block/8192frame/1MiB request limits.
Short drain yields a truthful pending bound `max_tail - drained`, saturating at0;
it is a conservative support bound, not a measurement of nonzero queued samples.
New content may continue the FIFO. Drain-only continuation retains held gain,
queue/cursor, frame and ledger and never replays events. ReportedPCM has exactly
the supplied length, including drain; request/activation ordinals are not graph
publication epochs or hardware/recording ACKs.

The actual fixed32stereoFloat32 queue is256bytes; the measured Linux x86_64
DelayMemory struct adds32metadata bytes (including ABI padding). Process input
parameter vectors reserve128aligned56-byte slots/block before activation and
report actual checked reservation (maximum229376bytes). These charges exclude
PCM/result/container/IPC/state/plugin/process overhead and are not an OS memory
limit. Opaque continuation is360bytes: strict288-byte plugin state plus8-byte
maximum-tail/drain ledger and64ASCII module digest. The fixed state validates
version/profile owner, module, rate, delay, frame, deterministic cursor, finite
queue, zero unused cells, and completed-drain zero history before load/adoption.
Ledger bytes must match the explicit owner; old states/profiles refuse. This
controlled provenance does not authenticate arbitrary third-party binaries.

All outputPCM/state stays provisional until every block, tail/latency recheck,
state save, destruction/unload, supervisor reap and immutable input/build identity
check succeeds. Illegal callback, capacity, drift/restart, malformed state, ignored
output rejection or late process/stop/destroy failure exposes no successful result.
Process-group cleanup is tested through real artificial descendants; it is not a
security sandbox. Artificial nested namespace UID mapping remains refused by this
cloud host. Installed/third-party admission stays closed without OS containment.

Run `cargo test -p hex-clap-scan`, and the explicit native receipt commands above
plus `cargo test -p hex-clap-scan --test delay_native -- --ignored --test-threads=1`.
Original fixtures prove impulse/gain/event timing, both rates, partition/ring wrap,
short drain/state continuation, owner/cursor/queue and suffix/capacity refusal,
maximum tail after shrink/stop, callback phase/thread/drift/restart/late failures,
hang/crash/cancel and actual descendant reap. The builder includes each new source
in its pinned closure and source digest, preserving SDK/system notices. Legacy
version1/2/scanner regressions remain required. Full20 hosting and full28 graph
latency compensation remain unfinished; macOS/native realtime/editor/reference
acceptance and packaging/signing are unverified.

## controlled arm64 Mac portability unit (native verification pending)

The Mac entry points admit only descriptor12 and legacy gain11 original modes,
excluding `descendant` and `process-descendant`. MIDI, delay, foreign/installed
modules, translated architectures and other profiles are refused before launch.
Linux schema1/ELF behavior and all three existing profiles remain separate.

The operator builds a new private canonical artifact directory outside source and
SDK with `native/build_macos.py`. This builder requires actual arm64 Darwin, clean
committed Hex source, the exact already-fetched MIT CLAP pin, explicit compiler/
SDK notice files and their audited origins. It builds26 C++17/O2 artifacts with
Apple clang/warnings-as-errors, audits escaped depfiles against the native, pinned
CLAP, selected Apple SDK and selected toolchain roots, and retains command logs,
image inspection, source/compiler/header/notice hashes. Only helpers' non-loading
`--runtime-identity` modes execute during the build. No fixture module is loaded
by the builder; no signing, provisioning, sandbox or resource policy is installed.
Missing closure, runtime UUID or notice evidence refuses the build.

`controlled-darwin.json` is strict schema2. Rust tools are independently enrolled
at build time with `HEX_CLAP_DARWIN_ROOT`, `HEX_CLAP_DARWIN_RECEIPT_SHA256`, and
`HEX_CLAP_DARWIN_SOURCE_TREE`. They refuse an unenrolled or different runtime root,
receipt bytes/source/build audit changes, symlinks, different UID ownership,
group/other writable root/files, unknown modes, wrong image roles, and non-thin
arm64 Mach-O. The root/operator build is the trust anchor; neither an arbitrary
caller hash nor filename authenticates foreign code. Same-UID operator compromise
is outside this controlled fixture contract. This is not an OS containment policy.

The native Mac envelope contains actual OS release and ordered loaded system
image LC_UUID identifiers, taken before module load and after worker join,
destroy/deinit/unload. Runtime identity must match the enrolled helper's baseline
and remain identical through the attempt. UUIDs are identifiers, not dylib content
hashes; SDK `.tbd` hashes belong only to the build closure. `shared_cache_uuid` is
explicitly null: this unit does not invent a cache identity from public APIs.
Enumeration occurs only in these original, non-concurrent-loader phases. A
missing identity, extra image or incomplete unload refuses success. Actual dyld
behavior, availability and native warnings-as-errors remain a Mac acceptance gate.

Mac supervision owns exactly one direct never-fork child. Safe signal registration
repairs inherited auto-reap; observation with NOWAIT retains PID ownership, actual
`Child::wait` reaps, and PGID is checked while the PID is reserved before TERM/KILL.
Deadline/cancel/crash/IPC or cleanup failure discards the whole result. Cleanup
failure retains the owned lease and drives actual wait; OS-stuck cleanup has no
promised finite completion. `descendant_policy` explicitly says not applicable to
audited original never-fork fixtures. No process observation, group emptiness or
Linux subreaper result proves Mac descendant cleanup. Hostile/descendant-producing
admission remains closed pending a separately decided OS containment/lifecycle
boundary. No filesystem, network, memory, CPU, App Sandbox or XPC claim follows.

Mac descriptor invocation is `hex-clap-scan <enrolled-root> <original-name>
[deadline-ms] [previous-report.json]`. Mac gain invocation is `hex-clap-offline
<enrolled-root> <original-gain-name> <gain-input.json> [deadline-ms]`. Gain input
wraps the unchanged version1 request as `{ "request": ..., "state_owner": null }`.
Continuation uses the returned owned state and `state_owner`, bound to profile,
platform, module/helper/source hashes, sample rate and exact next frame before
child launch. Invalid gain/residual/tail/version/clock state refuses. No successful
PCM/state/owner escapes a failed request; packets are removed from public reports.

On actual Mac, run `cargo test -p hex-clap-scan --test macos_native --locked --
--ignored --test-threads=1` with the same enrollment variables used for tool build.
Eight tests cover descriptor inventory/refusals, independent gain PCM/partition/
state/sleep/tail oracles, late whole-result discard, cancellation, and inherited
auto-reap followed by actual direct wait. They are explicitly ignored in default
CI and refuse Linux execution. Linux direct-child canaries exercise shared Unix
mechanics only; they do not prove Darwin ABI, dyld, native process or Mac behavior.

Full20 still requires third-party containment (actual Linux UID-map refusal is
retained), broader extension/bus negotiation, production state recovery and
project/editor integration, realtime ownership/deadlines and #28 routing/latency
contracts. This source portability unit is not optimized Mac packaging/signing,
CoreAudio, listening/reference, native interaction, full hosting or release proof.


### exclusive original artifact custody prerequisite

Mac build requires `--exclusive-artifact-custody`: the operator asserts sole
custody of the original artifact directory during construction and every later
attempt. The builder seals its flat output files/root read-only after recording
provenance. Rust tools require `HEX_CLAP_DARWIN_CUSTODY=exclusive-original-readonly-v1`
at compilation and each invocation, as well as the existing root/receipt/tree
enrollment. Missing/mismatched attestation, a writable root/file, a symlink,
nonregular entry or more than256 artifact files refuses before child launch.
Native tests validate enrollment/seal and original launcher identity before even
using the inherited-auto-reap launcher. If exclusive custody cannot be maintained,
do not attest or execute; keep native admission closed. Requests live outside the
sealed artifact root. Rebuild into a fresh root to change any enrolled artifact.

Read-only modes and an operator declaration are advisory prerequisites, not an
OS-enforced immutable mount, lock against a hostile owner, or cryptographic
binding of checked bytes to exec/dlopen. The owner can chmod or replace and
restore paths. A deterministic no-execution original-byte canary demonstrates
that pre/post hashes can match while an intervening path read consumes different
bytes. It deliberately violates custody and does not claim an attack-resistant
loader. Foreign admission requires a separately reviewed supported validated-byte
to-load binding and OS containment; current third-party admission remains refused.

Notice inputs still require actual complete applicable terms and audited origins,
not arbitrary short bytes or an unrelated LLVM section. The512KiB input guard is
unchanged. Receipt hashes protect supplied notice bytes; completeness and source
PDF/extraction/section applicability remain an operator audit. Compiler depfiles
record source/header closure; retained otool reports and loaded-image UUIDs do not
establish full hashed linker/tool/SDK-stub closure. This unresolved provenance
limit must not be described as complete link closure or loaded-image hashes.
