# VST3 macOS hosting boundary

Decision: adopted design; executable implementation and admission remain pending.
21 October 2026, [Pulse2932](https://pulse.iocore.dev/#/issues/2932), canonical
[PR517](https://github.com/PORTALSURFER/hex/pull/517). Source assessment is pinned
to `d4a80a0cec6bcdb2b12530ca03446659b21db2ee`. This decision enables no installed
module loading, scanner access, entitlement, dependency or Rust unsafe code.

## Decision and alternatives

Use one native macOS arm64 helper process per plugin instance. The helper owns
its module, factory, processor, controller and optional editor together, whether
processor/controller are separate objects or one combined object. Hex remains
native Rust/GPUI with CPAL confined to hex-audio. No VST object, ABI pointer or
plugin-owned view enters the primary process.

| Boundary | Assessment |
| --- | --- |
| In-process hosting | Avoids the bridge delay, but places foreign ABI, crashes, hangs and arbitrary DSP inside Hex's process. Rejected for the initial production host. |
| Per-instance native helper | Selected: independent instance lifetime and ordinary crash isolation, at the cost of explicit transport delay, copies, supervision and deadline failures. |
| VM/guest | Not selected for this VST3 profile. It needs a separate guest, editor/audio integration and qualification. The separate [CLAP offline admission policy](clap-admission.md) does not silently become a VST3 prerequisite. |

The intended trust profile is explicit user selection of installed plugins they
trust to run as their user. A helper is **not a malicious-code sandbox**: a plugin
may access same-user resources, launch processes or attack other processes.
Identity hashes bind an attempt to inspected files; they do not make those files
trustworthy. Users' installed modules, projects and media are never rewritten by
discovery or a failed attempt. Installed execution remains refused until the
selected Mac loader, process owner, transport and relevant host capabilities exist.

The SDK's `kDistributable` capability permits separating processor and controller;
its absence forbids that separation. This design co-locates both in all cases,
so a separate controller class does not imply a separate process. Combined
objects are initialized/terminated once per actual object, with each acquired
interface reference released correctly. A missing editor does not imply that a
processor cannot run. These are SDK constraints, not measured plugin compatibility.
See the [official API model](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical%2BDocumentation/API%2BDocumentation/Index.html).

## Current source and intended ownership

Current [hex-plugin-scan](../crates/hex-plugin-scan/README.md) owns Linux x86_64
original factory/offline fixtures only. Its [receipt gate](../crates/hex-plugin-scan/src/lib.rs),
[standalone supervisor](../crates/hex-plugin-scan/src/supervisor.rs) and
[native offline lifecycle](../crates/hex-plugin-scan/native/offline.cpp) do not
provide a persistent macOS host. Its Linux subreaper/process-group contract must
not be embedded in Hex or assumed to work on macOS.

[Session](../crates/hex-session/src/service.rs) already owns canonical edits and
control work. [Prepared audio](../crates/hex-audio/src/prepared.rs) owns bounded
publication/retirement, and [latency compensation](../crates/hex-engine/src/compensation.rs)
owns control-prepared DAG delays. Preserve these boundaries. Add no speculative
crate now; the implementing leaves must register actual new responsibilities in
the architecture policy when code is introduced.

| Owner/thread | Owns and may do | Must not do |
| --- | --- | --- |
| Session/control | Canonical instance identity, opaque saved states, configuration generation, preparation, helper supervisor and retirement leases; report failure and reconcile accepted edits | Load a plugin or treat a controller callback as processor application acknowledgement |
| Hex callback | Prepared bridge endpoints and fixed pool slots; bounded PCM/event/parameter copies and exact output selection | Spawn, socket I/O, wait/spin, allocate/free, log, call foreign DSP/UI, or finally destroy an endpoint/process owner |
| Bridge worker | Bounded duplex IPC, copying/validation, deadlines and sequence reconciliation; return pool ownership on control | Become another musical scheduler, block the callback, or publish an unmatched reply |
| Helper AppKit main/UI thread | Bundle/factory lifetime, initialize/deinitialize, host/controller calls, state operations and optional native editor/window | Put a plugin view pointer in GPUI or call these interfaces from an arbitrary IPC reader thread |
| Helper processing thread | Serialized `process` and processing-state transitions, negotiated preallocated buffers | Concurrent process calls for one instance, synchronous UI/host IPC, or an invented rollback of foreign DSP |

The SDK assigns initialization/teardown and controller work to its UI thread;
`process`/`setProcessing` have the processing-thread exception. Host callbacks
are handled on their required thread; a violated thread contract must not be
"repaired" by synchronously waiting from processing for the parent UI. Use a
bounded fault/status handoff. See [processor sequence](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical%2BDocumentation/Workflow%2BDiagrams/Audio%2BProcessor%2BCall%2BSequence.html)
and [controller sequence](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical%2BDocumentation/Workflow%2BDiagrams/Edit%2BController%2BCall%2BSequence.html).

Orderly retirement closes new requests and detaches/removes the editor while its
native parent view is alive; stops processing and joins its thread; deactivates
buses/component and clears handlers/connections; terminates successfully
initialized distinct objects exactly once; releases all interfaces/factory; then
exits/unloads the module. Failure unwinds only completed lifecycle steps. A hung
foreign call is not forcibly unwound in-process: off-callback supervision owns
bounded termination attempts and retains its child/cleanup lease until actual
exit/reaping is confirmed. Do not claim OS-stuck tasks or escaped descendants
have a universally bounded cleanup. The primary app and unrelated processes are
never termination targets for plugin recovery.

## Transport, latency and transactions

Select copied PCM with safe-Rust preallocated ownership queues between callback
and bridge worker, using the already pinned rtrb0.3.4 boundary in hex-audio, and
bounded local duplex IPC between worker and helper. No new Rust shared-memory
FFI or unsafe exception is needed for this first design. The transport leaf2938
must qualify concrete queue/pool/packet/event/state/diagnostic bounds, deadlines,
maximum work per callback and full-queue refusal before enabling the profile;
this ADR does not raise existing engine or resident-memory caps.

Each prepared stream epoch has one fixed positive, validated quantum `Q` frames.
The selected pipeline adds **2Q frames**: input quantum `n` is gathered before
its worker/helper processing window and its result has the fixed output slot
`n+2`. The first two output quanta are silence. Variable CPAL callback partitions
are accumulated/split against this same sample timeline; latency cannot depend
on callback count or segment size. Preserve ordered sample offsets for notes,
parameter changes and transport context in the input frame domain. The helper
does not invent a second tempo, seek or scene clock.

Replies are bound to instance/configuration generation, stream epoch, block
sequence and exact frame range. Validate format, lengths, finite PCM and bounded
metadata before offering a result. Stage callback-side input/output ownership
locally, dispatch foreign work **only after the local render transaction commits**,
and commit consumption of a validated reply once. Stale/duplicate/wrong-frame
results never install and return through control reclamation.

Foreign DSP is nonrollbackable: after a dispatched `process` call, local rollback
cannot restore its private state. Never resend that block as a retry or model the
plugin as a cloneable [NativeInstrumentRuntimePair](../crates/hex-lattice/src/instrument_runtime.rs)
or [owned pair](../crates/hex-lattice/src/instrument_pair.rs). A future explicit
asynchronous external graph node must implement this commit/dispatch boundary;
it must not weaken native paired-runtime transactions or callable-source guards.

Preparation/discard preserves Hex canonical state, not arbitrary plugin file or
network side effects. A confirmed opaque snapshot is recovery data, not a copy
of an executing foreign runtime.

A missed deadline produces deterministic silence in the scheduled slot, never
late-shifted audio, callback waiting or replay of an old block. Queue overflow,
malformed output, processing failure or a missed deadline invalidates the epoch;
control exposes unavailability and recovery from the last confirmed saved state.
Exact DSP/tail continuity after recovery is not promised. Priority Stop silences
and invalidates outstanding output immediately in Hex; a slow helper cannot
resurrect it. Slots/endpoints retire through bounded control ownership even on
fault/full queues. Normal plugin/host bypass and tail policies require their own
implemented, latency-correct contracts; silence is not a successful bypass ACK.

The prepared node reports `2Q + plugin_latency_frames` to the existing
[LatencyPlan](../crates/hex-engine/src/compensation/plan.rs), with checked arithmetic
and existing cumulative/storage limits. Serial helpers accumulate that delay;
PDC aligns paths but cannot remove causal live-input latency. Rate, quantum, bus
or latency changes require control reconfiguration and a new epoch, including
handling `restartComponent(kLatencyChanged)`. State/parameter presentation and
actual processor application are distinct acknowledgements. The [SDK processing
FAQ](https://steinbergmedia.github.io/vst3_dev_portal/pages/FAQ/Processing.html)
describes latency notifications and inactive setup changes; these do not make
Hex's future transport or deadlines qualified.

## Native packaging and ABI policy

Retain SDK3.8.1 superproject `3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96`,
pluginterfaces `4f547e8e102b47de4a8b8aaf343c73b700786372`, and the other exact
submodule pins in the existing [builder](../crates/hex-plugin-scan/native/build.py).
Keep its source/compiler/header/runtime closure checks and MIT/component notices;
no implicit SDK fetch/update or Hex distribution-license decision. The pinned
[root license](https://raw.githubusercontent.com/steinbergmedia/vst3sdk/3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96/LICENSE.txt)
is source evidence, not a license for third-party plugin content.

The native C++/Objective-C++ helper owns VST ABI and AppKit access outside safe
Rust. Its signed, source-identified application bundle is intended under
`hex.app/Contents/Helpers/hex-vst3-host.app`, sealed by the repository's eventual
helper-aware packaging. The parent verifies the bundled helper identity and
owned connection before dispatch. Qualify that packaging in2933/2959; today's
single-executable development bundler does not already supply this helper.
Required foreign-library loading privileges must be reviewed and scoped to the
helper; never relax the main app's signing/security or silently grant broader
JIT/debug privileges. This decision changes no entitlement or installer.

macOS module load/entry/factory/exit stays inside that helper, including required
`bundleEntry`/`bundleExit`, rather than reusing Linux `ModuleEntry`/`dlopen` by
analogy. See the [official module architecture](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical%2BDocumentation/VST%2BModule%2BArchitecture/Index.html).
The initial architecture is native arm64, including an inspected arm64 slice in
a universal bundle. x86_64-only/Rosetta, Windows and Linux production hosting are
separate qualification work, not inferred from Linux fixture success.

The optional editor is attached to a helper-local AppKit NSView/window on its
main thread, with bounded size/focus/close events. No cross-process NSView pointer
or replacement of GPUI is proposed. Plugin-owned typography/UI is foreign native
content; Hex-owned labels keep the existing lowercase/Ioskeley policy. The pinned
[IPlugView contract](https://raw.githubusercontent.com/steinbergmedia/vst3_pluginterfaces/4f547e8e102b47de4a8b8aaf343c73b700786372/gui/iplugview.h)
defines NSView attachment and view removal/resize behavior. Required synchronous
resize callbacks complete within the helper-local view call stack; they never
wait for a GPUI round trip.

## Finite capability and acceptance matrix

| Area | Current source evidence | Selected implementation boundary / admission gate |
| --- | --- | --- |
| Discovery/module loading | Original Linux x86_64 factory fixtures; installed directories refused | Native arm64 inspected modules, Mac entry/exit and explicit user trust;2933/2934 |
| Processor/controller | Bounded original offline separate-controller fixture | Persistent helper-local combined/separate objects and exact lifecycle;2936/2937 |
| Audio/transport | Bounded non-realtime Float32 stereo fixture, not a Mac live host | First finite Main-bus Float32 profile, fixed-Q 2Q bridge and full resource/deadline qualification;2938/2939/2940 |
| State/history | Controlled opaque component/controller state only | Canonical current-format envelopes, confirmed snapshots and explicit unavailable recovery;2941/2942/2957 |
| Parameters/MIDI/PDC | Controlled offline queues; existing native DAG compensation | Sample offsets/stopped flush, proper events and bridge+plugin compensation;2943/2947/2948/2953 |
| Editor | Unimplemented | Helper-local NSView, main-thread lifecycle/resize/focus/close;2949/2950 |
| Product qualification | No installed-module/native compatibility claim | Actual signed helper, real effects/instruments and finite compatibility/listening workload;2958/2959 |

These are existing [VST3 goal2931](https://pulse.iocore.dev/#/issues/2931) leaves,
not a new backlog or a claim they are active. Unsupported interfaces, formats,
buses, rates or required host capabilities refuse explicitly; advertising an
interface requires implementing it. Consult [minimum host requirements](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical%2BDocumentation/Host%2BRequirements/Index.html)
and [host interfaces](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical%2BDocumentation/Change%2BHistory/3.0.0/Host%2BInterfaces.html)
before each capability is admitted.

This first leaf is complete only when this ownership/ABI/thread/drop/transport
and failure decision is reviewed against the pinned current seams and official
sources, the prose/link checks and exact repository merge plan pass, and its
canonical PR merges. It supplies no executable host, probe success, deadline or
production compatibility certificate. Installed-module refusal remains intact.
