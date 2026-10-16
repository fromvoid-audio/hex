# controlled VST3 discovery

## Purpose

Own a Linux x86_64 standalone safe-Rust supervisor for explicit original SDK
fixture modules, a separately built native factory probe and bounded offline
component attempts. Installed-directory and production third-party scanning remain
refused pending a reviewed OS containment policy. Processes isolate ordinary
crashes; they are not a security sandbox against arbitrary same-user code.

## Dependencies

No workspace crate dependencies. Exact existing serde/serde_json encode bounded
receipts/results. sha2 hashes controlled binary/build identity only. rustix1.1.5
process/event safe APIs implement scoped Linux child ownership, WNOWAIT
observation, subreaping, termination and readiness polling. Hex Rust still forbids
unsafe. Scoped signal-hook0.3.18 (default features off, MIT/Apache2.0) installs
a safe atomic SIGCHLD handler in this standalone executable before spawn,
removing inherited SIG_IGN/SA_NOCLDWAIT auto-reaping. Its existing
signal-hook-registry1.4.8 action flags were audited; the controlled native launcher
proves the negative control and repaired child/descendant ownership. Native SDK-interface loading occurs only in the separate C++ executable,
never in this supervisor, engine or audio process. No Rust VST3 wrapper, audio
backend or GUI toolkit is adopted. Component/controller/processor interfaces
remain inside the separate controlled offline child; editors are unimplemented.

## Does not own

Production plugin hosting, realtime queues, general buses/automation, latency compensation,
canonical project state, state recovery, editors, provider/audio/root UI,
third-party security containment, Mac builds/signing or full Pulse21 acceptance.

## Controlled build and invocation

`native/build.py --sdk <already-fetched-pinned-sdk> --output <owned-fixture-folder>`
checks the SDK and four submodule commits, rejects tracked source changes, checks
MIT licenses and records compiler, exact commands, all emitted SDK/system headers, compiler binary identity, loaded
helper runtime-library hashes and retained SDK/system license notices. The
controlled receipt ceiling is512KiB (measured notices alone occupy200969B here).
It performs no fetch, SDK installation or installed-plugin scan. SDKroot
3cdf9ca5/public.sdk586dc5e6 are exact pinned3.8.1 inputs. SDK notices are retained
in native/SDK-LICENSE.txt; no Hex distribution license is chosen.

`hex-plugin-scan <owned-folder>/controlled-fixtures.json <good|hang|crash|descendant|discardable> [deadline-ms] [previous-report.json]`
returns bounded JSON. Optional stdin `cancel` cancels the request. Only the
five original factory-only fixtures built from this source are admitted. The
caller-owned build receipt is the explicit trust root, not an authenticity or
security sandbox. Binary hashes bind the attempt, not arbitrary plugin trust.
Pre/post receipt and helper/module identity checks refuse changed inputs. Source
identity/architecture/version checks never load a library. ELF64LE x86_64 ET_DYN
is the only currently supported binary target; no translation fallback.

One fresh helper per request receives minimal environment and its own process
group. Protocol is length-delimited/versioned on a dedicated socket descriptor;
plugin stdout/stderr are separate bounded diagnostics. The probe enumerates
factory class metadata, releases its factory, calls ModuleExit and dlclose before
writing success. It never creates a component/controller/processor/editor.
Factory/class strings are length-delimited, not raw JSON5 interpolation. Unknown
factory flags remain visible; dynamic/discardable or unknown flags never establish
cacheable future class inventory. Vendor strings are self-reported, not provenance.

The direct child is observed with WNOWAIT before termination: its unreaped PID
pins the process-group identity. Scoped TERM/KILL occur before direct reaping;
no later group signal risks a reused PID. The dedicated supervisor subreaps and
reaps controlled descendants. Timeout/cancel/crash/malformed/changed attempts
retain prior valid records. Cleanup exceeding500ms is reported as pending failure
with an owned lease retained by the live supervisor; it continues to drive reaping
without claiming completion. No universal bounded termination for OS-stuck tasks
or protection against escaped hostile processes is promised. This executable
must not be embedded as a process-global subreaper in Hex's primary process.

Real Linux tests require an explicit controlled fixture receipt through
HEX_SCAN_FIXTURE_RECEIPT. Without it native cases are visibly ignored; fake
protocol/identity tests still run. The final exact-source validation must supply
that receipt and retain build plus test provenance. Linux fixture enumeration,
unload and fault containment do not prove production plugin compatibility,
Mac native packaging/signing, processing/state/editor support or physical audio.

## controlled offline component stage

`hex-vst3-offline <controlled-offline.json> <process-fixture-name> <request.json>
[deadline-ms]` owns one fresh native child for a bounded non-realtime attempt.
Only separately built ORIGINAL processing fixtures are accepted. The existing
factory-only scanner/CLI is preserved. No engine, audio, service, editor or project
host integration exists. Installed/third-party modules and realtime processing
remain refused; a same-user child is not a security sandbox.

The pinned SDK interfaces now exercise a component plus separate controller,
bounded host messages/attributes and edit callbacks, bidirectional connections,
state synchronization, negotiated stereo Main buses, Float32 `kOffline` setup,
activation, a joined processing thread, deactivation, state capture, disconnection,
termination and complete release/unload. Failed initialization releases without
terminate. Processing errors unwind only successful lifecycle transitions. Missing
or combined controllers, event/MIDI/sidechain buses, other sample formats/rates,
nonzero latency/tails, restart requests and other context requirements are refused.

Request JSON has version1, nonzero request ID, sample_rate44100/48000,
start_frame (checked signed sample clock, fresh fixtures require zero), controller_value0..1, component_state
and controller_state byte arrays, and blocks with left/right Float32 arrays and
ordered unique `{offset,value}` points. Controller presentation values and callback
reports are not processor application acknowledgements. Processor queues carry
actual changes. The ORIGINAL effect maps normalized gain0..1 to multiplier0..2,
default unity; it follows SDK linear queue segments including previous-block
carry and adjacent points for jumps. Its opaque versioned component state retains
gain/carry and processed-frame count; request start_frame must match continuation.
Controller state retains a separate presentation preference and cannot change PCM.

Limits belong to this controlled unit:32 blocks,1..1024 frames/block,8192 total
frames;128 parameter descriptors;128 points/block and1024 total;64KiB per state;
128 lifecycle/callback records;32 simultaneous messages and32 attributes/message,
128-byte IDs,256 UTF16 code units/string,1KiB binary attributes. Input and output
packets each fit1MiB, with diagnostics independently capped64KiB. These are not
new production engine caps or proofs of arbitrary plugin memory/process bounds.
Event capacity is zero. The single original connection message does not establish
general recursive-message handling or arbitrary foreign-thread compatibility.

Strict versioned binary IPC is duplex on a dedicated inherited socket; no pointer
or foreign object escapes the child. Rust validates the whole request before
spawn; native independently validates wire bounds before loading. Output is
published only after successful processing, finite PCM, strict response validation,
normal helper exit, native unload, complete reaping and unchanged request/module/
helper/receipt identity. Any failed later block or teardown discards the whole
attempt; this is not a hardware callback transaction or recording ACK. The existing
owned deadline/cancel/process-group/reaper contract and cleanup-pending failure
apply unchanged. Request/result packets are absent from persistent diagnostics.

The offline receipt binds its native source/helper/modules to the original scanner
receipt (which carries pinned SDK, compiler, all emitted headers, runtime closure
and license notices). It is caller-owned build provenance, not plugin authenticity.
The builder audits runtime libraries for ALL scanner/offline fixtures and helpers.
No additional Rust dependency or native SDK implementation library is added.

The supervisor requires local Unix-domain IPC. Earlier default-sandbox `EPERM`
attempts remain historical evidence; the current Linux environment executed all
22 controlled native cases without a permission or network-policy change. Tests require `HEX_OFFLINE_FIXTURE_RECEIPT` and explicit
`cargo test -p hex-plugin-scan --test offline_native --locked -- --ignored
--test-threads=1`. Default workspace tests visibly ignore native fixture suites.
Run the existing scanner native suite with `HEX_SCAN_FIXTURE_RECEIPT` as well.
Native Mac packaging/signing, editors, realtime deadlines/queues, production
containment, broad buses/events/automation, graph/state integration, listening and
whole VST3 hosting remain unverified or unimplemented.

## current-main review — 16 october 2026

The pinned official SDK and four submodules were fetched without modification;
the builder verified their exact commits, MIT notices and actual compiler/header/
runtime closure. Ten scanner and twelve offline native fixture tests passed with
explicit receipts, alongside default protocol tests and strict crate Clippy. The
final exact-source native rerun and full workspace CI are recorded with PR193.
These results cover the original controlled fixtures, not arbitrary third-party
modules or production application hosting.
