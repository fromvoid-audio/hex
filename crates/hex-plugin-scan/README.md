# controlled VST3 discovery

## Purpose

Own a Linux x86_64 standalone safe-Rust supervisor for explicit original SDK
fixture modules, a separately built native factory probe and bounded offline
component attempts, plus a distinct Darwin arm64 bootstrap and explicit
user-trusted, factory-only Mac scan. Linux installed-directory/third-party scans
and Mac automatic directory scans remain refused. Processes isolate ordinary
crashes; they are not a security sandbox against arbitrary same-user code.

## Dependencies

No workspace crate dependencies. Exact existing serde/serde_json encode bounded
receipts/results. sha2 identifies inspected input/build bytes, not authenticity.
rustix1.1.5 process/event safe APIs implement scoped child ownership, WNOWAIT
observation, termination and readiness polling on Linux and Mac. Subreaping is
Linux-only, never transplanted to Mac. Hex Rust still forbids
unsafe. Scoped signal-hook0.3.18 (default features off, MIT/Apache2.0) installs
a safe atomic SIGCHLD handler in each dedicated standalone owner before spawn,
removing inherited SIG_IGN/SA_NOCLDWAIT auto-reaping. Its existing
signal-hook-registry1.4.8 action flags were audited; the controlled native launcher
proves the negative control and direct-child repair. Linux adopted-descendant
checks remain separate; Mac never claims descendant reaping. Native SDK-interface
loading occurs only in the separate C++ executable,
never in this supervisor, engine or audio process. No Rust VST3 wrapper, audio
backend or GUI toolkit is adopted. Component/controller/processor interfaces
remain inside the separate controlled offline child; editors are unimplemented.

## Does not own

Production plugin hosting, realtime queues, general buses/automation, latency compensation,
canonical project state, state recovery, editors, provider/audio/root UI,
third-party security containment, production Mac packaging/signing or full Pulse21 acceptance.

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

## original macOS bootstrap

The separate `native/macos_bootstrap.py` tooling builds a native arm64
`hex-vst3-host.app`, two original SDK processing fixtures, and the separate
factory scanner/failure fixtures below. It does
not port or invoke the Linux supervisor, subreaper, scanner receipt or offline
success-after-exit protocol. The [adopted production boundary](../../docs/vst3-hosting-boundary.md)
remains a staged design: the bootstrap self-test is not a persistent host or an
installed-plugin admission path. Its local identifier is `org.portalsurfer.hex.vst3.bootstrap`;
it does not replace or launch the installed Hex app.

From this checkout, with an already fetched SDK at the five exact pins in
`native/build.py`:

```sh
python3 crates/hex-plugin-scan/native/macos_bootstrap.py build \
  --sdk artifacts/vst3-sdk --output artifacts/vst3-native-2933/owned-build
python3 crates/hex-plugin-scan/native/macos_bootstrap.py verify \
  --receipt artifacts/vst3-native-2933/owned-build/controlled-macos.json --run
HEX_VST3_MACOS_RECEIPT=artifacts/vst3-native-2933/owned-build/controlled-macos.json \
  python3 -m unittest discover -s scripts/tests -p test_vst3_macos_bootstrap.py -v
```

Build refuses an existing output or any destination outside this checkout's
`artifacts`, modified/untracked SDK inputs, a pin/license mismatch or a different
host architecture. It performs no fetch or installation. Compiler commands,
compiler/linker/signing/inspection tool binaries, compiler version, Apple SDK
settings, every emitted source/header input,
all five SDK notices, source digest/commit and dirty state are retained. The
receipt binds every bundle file, including nested fixtures and code signatures.
The local bundles use ad hoc signatures without entitlements or notarization;
Developer ID packaging and production signing remain unimplemented. Runtime
provenance records linked install names and the exact OS build; individual dyld
shared-cache images are **not** hashed or compatibility-qualified. No SDK
implementation library is linked; the original fixtures use its interfaces.

The verifier uses one fixed package lookup, validates actual bundle contents,
package identifier, native arm64 Mach-O role and code signature immediately
before execution, then rechecks contents after each attempt. Execution also refuses a changed
OS build. Its caller-owned
receipt is an explicit local trust root, not third-party authenticity. Linked,
missing, changed or stale source/bundle inputs refuse; no path-only trust or
architecture translation fallback is used. Current controlled bounds are 256
bundle members, 16 MiB per file/receipt and 32 MiB total bundle files; these are
not production-host limits or bounds on arbitrary foreign memory/CPU. Each owned
bootstrap command has a 120-second control-side timeout; Python owns only its
direct child, with no production descendant or hung-plugin cleanup claim.

The helper accepts only `--identity` and `--self-test`. Self-test loads exactly
the two bundled original modules after checking their compiled-in binary hashes.
Before loading, CFBundle's actual executable URL must match the fixed, nonlinked
fixture path and expected executable/identifier metadata; hashing a different
fixed file is insufficient. It calls Mac `bundleEntry`, factory/component
interfaces and `bundleExit`, with
interfaces released before unload. Main-thread setup/teardown and a joined
processing thread exercise one fixed Float32 stereo, 48 kHz, 64-frame block.
The original effect multiplies each sample by 0.5; the original instrument is
an explicitly constant 0.125 generator with no audio input. It is a bootstrap
instrument fixture, not a playable MIDI instrument. No parameter/state/editor,
event, latency/tail, arbitrary bus or production processing claim follows.
Changing or supplying an installed module path cannot select another module.

The explicit native suite executes the actual helper and compares both exact
PCM outputs. Refusal cases remove the helper, change helper/fixture bytes,
compile a real x86_64 executable and update the caller manifest (so architecture
refusal cannot be confused with a stale hash), link a member, change source
identity and pass an installed-directory argument. Another regression changes
CFBundleExecutable to a real marker-writing alternate module, reseals both
bundles and updates the caller manifest; both native and Python boundaries must
refuse before the alternate initializer runs. A signing-timeout injection must
fail without publishing a receipt. Without the explicit receipt,
these native cases are visibly skipped; synthetic Mach-O header tests are parser
evidence only. Two builds from the same inputs must have identical bundle-file
hashes; retained commands/receipts remain independent provenance, not a claim
that different compilers or OS versions produce the same binaries.

Persistent processor/controller ownership, realtime queues/deadlines, primed
playback, per-bus presentation latency, state/editor integration and native
compatibility remain future leaves. No Rust unsafe exception, workspace
dependency, CPAL or UI replacement is introduced. Existing Linux controlled
behavior is unchanged.

## explicit trusted Mac factory discovery

`hex-vst3-scan-macos` is a separate native-arm64 safe-Rust control executable.
It admits **one explicitly selected, user-trusted** `.vst3` bundle for factory
metadata, using the pinned `hex-vst3-factory-scan` child. Loading a factory runs
foreign code with the user's authority. It is ordinary crash/hang isolation,
not malicious-code containment, authentication or an atomic filesystem sandbox.
There is no directory autoscan, browser/catalog persistence, component creation,
editor, playback or audio-host admission in this leaf. An installed module path
can use this factory-only profile; actual third-party compatibility remains
unqualified by original fixtures.

```sh
cargo run -p hex-plugin-scan --bin hex-vst3-scan-macos --locked -- \
  artifacts/vst3-discovery-2934/owned-build/controlled-macos.json \
  --trust-user-code /absolute/path/selected.vst3 10000
HEX_VST3_MACOS_RECEIPT=/absolute/path/owned-build/controlled-macos.json \
  cargo test -p hex-plugin-scan --test macos_native --locked -- \
  --ignored --test-threads=1
```

Use the same native builder above to create a fresh owned output. Receipt trust
is explicit: the caller selects a known local build receipt, not one supplied
by an arbitrary plugin. Before spawn, Rust verifies exact SDK/native-source
identity, the fixed scanner lookup, every helper-bundle file/hash and actual
thin-arm64 executable role. This is caller-owned build provenance, not a signed
claim proving how a third party built its binary. Installed vendor binaries are
never substituted into the helper package or rewritten by the scanner.

Two-phase, versioned duplex IPC separates metadata inspection from loading.
The child reads bounded `Info.plist` metadata and returns CFBundle's actual
selected relative executable and identifier without requesting executable load
or factory lookup. Rust independently confines that nonlinked path to the
selected bundle, checks its Mach-O role/architecture and seals the executable
hash. Its ACK binds request/generation, metadata digest, selected path, image
kind and executable digest. The child rechecks path/metadata/hash immediately
before load. Normal thin arm64 and big-endian fat32/fat64 containers with a
validated arm64 slice are supported; the initial universal profile permits
arm64 and ordinary x86_64 slices only. All slice offsets, sizes, alignments,
inner headers, roles, duplicates, overlaps and load-command framing are checked.
No x86-only, arm64e or Rosetta fallback is attempted. POSIX filesystem
canonicalization is used consistently with Rust; Foundation's `/var` versus
`/private/var` presentation is not treated as a different selected file.

The helper calls Mac `bundleEntry`, `GetPluginFactory`, bounded factory/class
queries and releases the factory before checked `bundleExit` and an unload
request. It never calls `createInstance` or infers a processor/controller pair
from names/categories. Sixteen-byte class UIDs and exact UTF-8 metadata are
retained; invalid/duplicate IDs, malformed strings/counts, stale generations,
unexpected frames and trailing bytes refuse the whole attempt. Unknown factory
flags remain visible and do not establish a durable class inventory. A void
CFBundle unload call does not prove dyld unmapped all code. Complete valid
terminal data, normal direct-child exit/reap and unchanged helper, metadata and
selected executable are all required before a discovery record is offered.

Optional `IPluginFactory2` discovery copies class flags, subcategories, class vendor,
version and SDK version. Its owned reference is released before the original
factory and module exit; no instance is constructed. UID/category/cardinality must
match the basic class. Unknown flag bits and original UTF-8 bytes are retained;
unterminated, oversized, invalid UTF-8 or inconsistent optional records refuse the
whole scan. Native Mac class packets use version2 with an explicit per-class
optional marker; the active Linux basic packet remains version1. The current Mac
path refuses obsolete versions. `Instrument` and `Fx` tokens inform downstream
reported labels only; `Fx|Instrument` is a valid dual type in the pinned SDK.

Host I/O and supervision run on the standalone control owner; foreign factory
calls stay on the child main thread. The owner uses distinct nonblocking
UnixStream endpoints for protocol/stdout/stderr; plugin stdout/stderr cannot
become metadata. Limits: 1 MiB framed payload, 4096-byte path, 64 KiB metadata file,
16 MiB executable, 1024 classes, SDK-sized factory/class strings and 64 KiB raw
bytes per diagnostic channel. Each poll reads at most four 4096-byte chunks per
channel and writes at most 16384 bytes. Metadata decodes once, cancellation and a
monotonic deadline are checked between bounded work and before publication.
Any stdin byte cancels, including a fragmented command; normal stdin EOF does
not cancel. Scoped SIGINT/SIGTERM flags in this dedicated owner use the same
latched cancellation and retirement, including repeated signals during cleanup.
Cancellation/deadline reasons remain latched through cleanup.
These byte/work limits do not bound arbitrary foreign allocation, side effects
or OS-stuck filesystem/kernel operations, and establish no physical UI/audio
responsiveness or callback deadline.

Only this dedicated executable installs the scoped waitable SIGCHLD and
SIGINT/SIGTERM cancellation dispositions;
no primary-app/GPUI signal policy changes. `process_group(0)` creates the owned
initial group. `waitid(EXITED|NOHANG|NOWAIT)` observes without reaping, keeping its
direct PID reserved through scoped group TERM/KILL attempts. A still-running
helper can also be killed through its exact unreaped Child even if it changed
groups. Exactly one direct reap follows; no PID/PGID operation follows that reap.
Unexpected lost custody (including ECHILD) is terminal failure with unconfirmed
cleanup and no further numeric operations, not an indefinitely polled fictitious
lease. SIGKILL or an owner-process crash cannot run this graceful retirement; no
parent-death daemon or universal recovery is claimed. A genuinely owned but
unreaped child exceeding the 500 ms cleanup budget
retains its real control owner; an early valid reply is never success while
cleanup is pending.

Mac group permission errors after observed exit are recorded as ambiguous, not
silently relabeled as group cleanup success. The independent original probe and
[Apple's public XNU implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sig.c)
show the zombie-only group case: its group iteration can find no signalable
member and return EPERM. Only separately observed exit followed by normal exact
direct reap can qualify metadata despite that outcome. Live permission failure,
cancellation, timeout, malformed data and failed native teardown still refuse.
No Darwin subreaper, `waitpgid` adoption or descendant-reaped claim is made;
descendants are always reported unknown, even after group signal success or EOF.

Original native fixtures cover thin/universal processor/controller metadata,
load/entry/enumeration/exit crashes and hangs, malformed count/string/UID,
diagnostic flooding, inherited open channels, a group-escaping hung child and
inherited auto-reaping. A separate compile-time helper variant sends valid
terminal data then hangs, proving the exit/reap and cancellation gates; no
production runtime fault flag exists. Owned-copy mutations check changed helper
rejection before spawn and changed/redirected module admission before a
crash-on-load constructor can execute. Kernel-operation traces independently
check lost custody, exact-once reap, no post-reap signals, live versus exited
permission errors and cancellation at publication. Native suites are visibly
ignored without an explicit receipt; they must be run deliberately for the final
candidate. Synthetic/parser and original-fixture passes are not installed-plugin,
physical-device, listening, Windows or complete VST3-host acceptance.

### Packaged discovery tools

`macos_bootstrap.py build --discovery-only` produces a minimal factory scanner
bundle with SDK notices and no bootstrap instruments, fault helpers or fixture
libraries. Its receipt has the separate `native-macos-factory-discovery` scope.
The ordinary original-fixture build remains the native regression profile.

The optimized app bundler's `--vst3-discovery-receipt` option accepts only this
minimal profile from the exact clean candidate. It builds the Rust standalone
scanner alongside the desktop under the same release target/source lock, signs
both nested tools with the configured local Developer ID, updates their final
byte inventory, then seals the outer app. These local development signatures do
not establish notarization or third-party compatibility.

The fixed tools in `Contents/Helpers/` are `hex-vst3-scan-macos` and the minimal
`hex-vst3-host.app`; their receipt is `Contents/Resources/vst3-discovery.json`.
Invoke the packaged scanner as
`hex-vst3-scan-macos --bundled --trust-user-code /selected/module.vst3`.
`macos::package::Package` resolves and checks only these application-relative
paths and source/hash bindings, with no PATH, Python, SDK or checkout fallback.
Filesystem work belongs on a control worker, never UI/audio callbacks. The
selected sealed app/local receipt remains a caller trust root; this is not
malicious tamper resistance or an authentication service. Discovery remains
factory-only, with no processing instance or native browser insertion.


`macos::report` contains copied serde records shared with the catalog/control
client. Its cleanup publication predicate is the standalone owner's existing
normal-direct-reap gate; exporting data does not export signal registration or
native child ownership. The application links only neutral records/package
inspection through `hex-plugin-catalog`, never `macos_owner.rs`.
