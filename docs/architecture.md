# Architecture

Foreign CLAP admission is specified in [the admission and containment
contract](clap-admission.md) (Pulse #2523). It proposes a disposable macOS guest
boundary for a finite offline profile; no foreign execution tuple is qualified.
This design enables no plugin loading, entitlement, dependency or Rust FFI change.
Original fixture implementation/history remains in frozen PR199/#20.

The production macOS VST3 design selects a per-instance native helper with an
explicit asynchronous audio bridge; see [the VST3 hosting decision](vst3-hosting-boundary.md).
It is a reviewed design boundary, not implemented installed-plugin admission.
The separate CLAP VM profile above is unchanged.

## Implemented

The native device DSP system is named **lattice**. Its staged implementation,
canonical model boundary and mutable-state dependencies are specified in
[lattice](lattice.md). Canonical registered linear effect chains are implemented
in the prepared engine; desktop device controls and mutable processors remain
separate dependencies.

`hex-engine` owns immutable compiled `Project -> Track -> Clip -> Audio` playback data.
`hex-project` owns canonical editable Arrangement and Session metadata. Audio uses
shared immutable stereo f32 buffers. Each clip holds a start frame and exclusive
end frame. Looping repeats the selected source region (the whole buffer for the legacy constructor); a non-looping source emits silence
when exhausted. Sources must match the project sample rate. Clips on a track
sum when overlapping; this is a prototype choice, not a Live compatibility claim.

`Renderer` borrows the project, owns an integer frame cursor, and writes into a
caller-provided block. It starts paused. Pause emits silence and freezes the
cursor; seek sets an absolute frame position. The caller chooses block sizes.
Canonical tempo conversion uses checked integer Q32.32 beat positions and
fractional-frame prefixes in hex-project, outside the callback. Bounded tempo
and meter maps are persisted and undoable; existing audio remains frame-anchored.
The separate fixed demo retains its legacy f64 scheduling helper. Audio warp,
fractional playback and live tempo automation remain unimplemented.

Rendering allocates no memory. It scans each clip once per block, skips inactive
spans, and processes only intersecting frames, preserving summation order.
Span discovery remains O(total clips per block); sustained native deadline safety
is not established. Finite input values are validated; sufficiently large sums
can still overflow f32. No limiter or normalization runs in the engine.
WAV export quantizes and clamps at the output boundary.

Canonical track balance is validated in -1..=1. Its original Hex linear law
keeps unity at center and attenuates only the opposite channel, without boost or
crossfeed; master gain is validated in 0..=2 and applies after summing without a
limiter. These coefficients are Hex-defined, not measured Ableton behavior.

## Desktop runtime

`hex-diagnostics` owns per-session background log I/O and ten-file retention.
Desktop owns its worker lifecycle; UI forwards changed control-side status and
errors. Audio publishes only fixed atomic fault snapshots, without a logging
dependency. See [session diagnostics](diagnostics.md).

The default GPUI editor uses hex-session's canonical model and worker to import,
edit, load/save and compile immutable playback. hex-audio publishes compiled
projects through bounded queues and reclaims old graphs away from the callback.
The former fixed generated demo remains separate behind --demo.
See [runtime.md](runtime.md) for queue, transport and lifecycle contracts.

`PreparedRevision` combines the compiled Arrangement, independent Session source
inventory and control-prepared summaries for one committed canonical revision.
Candidate preparation succeeds before history commits. Session MIDI sources own
beat-domain timing; unwarped Session audio one-shots own half-open project-rate
PCM bounds and a performance-frame launch anchor. They share resident immutable
PCM without retaining an Arrangement placement. The mixed kernel keeps a
separate Arrangement clock and monotonic performance clock, and each track has
one authoritative Arrangement, Session or Silence route. Exhausted audio keeps
Session ownership until an explicit route change. See [session-audio.md](session-audio.md)
for the source, discontinuity, handoff and acceptance contracts.

Ordinary revisions inherit compatible runtime state. Successful project Open
publishes with `InstallFreshRuntime` and supplies a fresh paused graph with
Arrangement ownership, even when another file reuses track and clip IDs. The
UI retains that policy while coalescing same-project candidates before admission.
Retirement backpressure delays installation without delaying priority Stop;
graph and shared-media destruction remain on the control side. The policy itself
does not sanitize an arbitrarily pre-mutated candidate.

`hex-audio` also owns bounded observational spectrum analysis for backend
telemetry. Its fixed-window FFT runs on control/worker threads and does not
process the audible device chain. Native musical DSP stays in `hex-devices`;
the kernel alone does not install live capture or an analyzer UI. See
[spectrum analysis](spectrum-analysis.md) for the finite boundary and pending
integration gates.

## Further boundaries, not implemented APIs

- General musical scheduler: automation, external MIDI, broader launch modes and follow actions.
- General device graph: device chains, routing and latency compensation.
- External plugin hosting behind a separately decided native boundary.
- Input/recording/streaming workers and recovery journals.
- Full project updates, relinking and measured Live compatibility adapters.

Current arbitrary audio graphs are immutable arrangements, not device graphs.
Future graph scheduling retains bounded publication/retirement and explicit
discontinuity/ownership contracts. The callback must avoid allocation, locks,
file I/O, logging and final-reference destruction.

## Verification boundary

Unit tests prove Hex contracts. Golden audio/MIDI fixtures compare a particular
Live version under recorded settings. Device stress tests establish timing on
one machine. Manual host and listening checks establish usable behavior. None
of these substitutes for the others.

## Enforced crate ownership

| Crate | Owns | Depends on |
| --- | --- | --- |
| hex-lattice-contract | Neutral device identifiers, descriptors and versioned opaque state | std |
| hex-dsp | Host-independent stereo kernels and validated amplitude/decibel units | std |
| hex-lattice | Native processing registry, prepared/runtime lifecycle and compatibility reexports of stereo kernels | hex-lattice-contract, hex-dsp, std |
| hex-devices | Registered original device algorithms and their state codecs | hex-lattice, hex-lattice-contract, hex-dsp, std |
| hex-project | Canonical editable arrangement, stable IDs, transactional edits, history | hex-lattice-contract, std |
| hex-media | Bounded control-side WAV decode and cached waveform peaks | hex-engine, std |
| hex-session | Canonical editor controller, retained decoded assets, import preparation, compilation and bounded export coordination | hex-project, hex-media, hex-engine, hex-storage, hex-export, hex-lattice-contract, std |
| hex-storage | Versioned project metadata and recoverable file creation | hex-project, hex-lattice-contract, serde, serde_json, std |
| hex-export | Fixed-block linear PCM16 WAV rendering and create-new file publication | hex-engine, std |
| hex-warp | Pure bounded control-side stereo transformation; no project/I/O/UI/thread ownership | std |
| hex-engine | Compiled playback data, timing, note scheduling, playback commands, rendering and transform preparation/cache | hex-project, hex-warp, hex-devices, hex-lattice, hex-lattice-contract, sha2 (control-side source identity), std |
| hex-audio | Device stream lifetime, command delivery, callback boundary | hex-engine, CPAL, rtrb |
| hex-library | Bounded folder/WAV/project listings, recursive WAV catalog and atomic derived cache with original paths | std |
| hex-widgets | Reusable native control rendering, focus and local edit gestures with caller-supplied style/text | GPUI, std |
| hex-ui | GPUI views, interaction/display state, required typography and browser navigation | hex-engine, hex-audio, hex-session, hex-project, hex-library, hex-preferences, GPUI |
| hex-desktop | Startup and composition; constructs backend and injects it | hex-ui, hex-audio, GPUI lifecycle |
| hex-cli | Headless project edits, import/load/compile and streamed WAV export | hex-project, hex-engine, hex-media, hex-storage, hex-export, std |

Each README defines purpose/dependencies/non-goals. Run
`python3 scripts/check-architecture.py`; all new crates and dependency edges
require an explicit policy entry. Cohesive subsystems get separate crates;
helpers and empty speculative modules do not. This policy follows the
[PhotoCraft reference adaptation](reference-project.md) while retaining a separate
real-time audio plane.

`hex-engine` may use pinned `sha2` 0.10.9 only for bounded, cancellable control-side
verification of original PCM in its existing sealed input bank. This registered
source-identity dependency does not permit hashing in sample/process callbacks
or add bank ownership/admission authority. Other runtime hash boundaries and the
separate `hex-stream` build-only exception remain unchanged. See
[native device resource scopes](native-device-resource-host.md).

## Editable project foundation

`hex-project::Editor` applies a typed command to an isolated metadata candidate.
Success becomes one undo step; a rejected command leaves current state/history
unchanged. History capacity is 1..128 and evicts the oldest step. Track, clip and
asset IDs remain unique across undo/redo branches. Names and paths retain original
case; display normalization belongs only to the UI. Referenced assets cannot be
removed. Limits and known resource costs are in [issues.md](issues.md).

Each regular track stores up to 64 serial effects and one optional instrument. The
instrument's canonical chain position is a validated gap in the effect vector,
persisted in schema16 and changed through stable-ID transactional edits. A
borrowed ordered-chain view reports explicit node class/slot references. The
first instrument added to an empty slot defaults to the front; it can then be
moved freely among effects. Arrangement, Session and offline rendering honor
the stored gap: audio enters the effect prefix, the instrument adds audio from
MIDI at the gap,
and the suffix processes their sum. An absent or bypassed instrument adds no
instrument audio while source audio continues through the effects. See the
[instrument ordering and release contract](lattice-instruments.md).

Schema17 adds ordered return buses with separate typed `ReturnId`s, generic
effect chains, mix gain/mute, and sparse regular-track sends. The return model
does not depend on a device registry and does not admit clips, instruments,
Session cells or return-to-return routing. The engine compiles regular-track
post-fader sends through registered Return effects into Main; Return gain/mute
precedes the single master gain. Unknown/nonzero algorithmic latency refuses
Return effect admission, while general path compensation remains future work.
See [return tracks](return-tracks.md).

`hex_engine::compile::project` resolves the snapshot's asset IDs through a decoded
PCM bank, checks exact rate/frame metadata, and produces immutable playback data.
Clips keep a half-open timeline interval and explicit source offset/length. Looping
repeats that region; a one-shot emits silence after its region is exhausted.
Compilation and history allocate on the control side and must not enter a callback.
The render hot path shares original PCM through Arc and scans clips once per block.

The generated `--demo` retains its fixed bank. The default desktop editor uses
the canonical project through `hex-session`, which compiles immutable
`PreparedRevision` candidates on control and constructs audio-owned playback
before publication. The bounded audio handoff preserves the running graph on
refusal; retired playback/graph owners return to control for final destruction.
See [runtime ownership](runtime.md) and the finite device limits in the
[instrument contract](lattice-instruments.md). This does not establish general
mutable chase, unrestricted device density or physical audio deadlines.

## Modern CPU and latency requirements

The compiled graph must support independent-path parallelism, bounded real-time
workers, reusable audio buffers and measured architecture-specific DSP paths.
The engine owns graph scheduling, device timing and path compensation; the audio
boundary owns platform scheduling/device lifecycle; UI and media preparation
remain outside the deadline-critical path. New cohesive subsystems receive named
crates and ownership documents when implemented, not a catch-all engine expansion.

Graph scheduling must preserve serial device dependencies and compensate latency
at merges, including event/automation timing. Device loading and schedule/delay
buffer preparation happen before publication, never on Play. Ownership, worker
completion, reclamation, overload and latency-change semantics need a concrete
reviewed contract before implementation. See [performance.md](performance.md)
for 100-track/large-device acceptance and [latency.md](latency.md) for responsive
transport, pre-roll and monitoring constraints. These are requirements, not
capabilities of the existing fixed-bank demo.

## Import, persistence and prepared playback foundation

hex-media caps input bytes and decoded frames, validates RIFF chunk boundaries,
encoding/rate/channel metadata and finite samples. Mono duplicates to stereo;
source rate/case remain intact. Peak summaries are bounded control-side caches.
ImportPreparedAudioTrack creates source/playback metadata and IDs as one validated
undo transaction. Source conversion and waveform preparation run on the control
worker; callback PCM and clip source intervals use project frames. Identity
conversion shares PCM. Other rates use versioned windowed-sinc conversion with
independent source/output limits. Opening prepares assets one at a time.

hex-project's std-only Snapshot DTO validates global ID uniqueness/high-water,
collection limits, refs and intervals on reconstruction. hex-storage alone owns
serde/schema 9 (strict schema-1–8 migration; legacy solo/loop defaults, neutral
mixer/fade/source-phase defaults and 120 bpm/4/4 maps), bounded 8 MiB documents and create-new same-directory synced
publication. It persists every current model field; undo stacks/decoded media are
not project state. SaveNew refuses existing destinations. SaveCurrent uses a
captured exact-byte FileToken, token-checked replacement and one validated rolling
recovery sidecar; changed content conflicts rather than silently replacing it.
Create-new save errors distinguish pre-publication failure from post-publication
cleanup or directory-sync failure; the latter names the complete published file,
and session dirty state remains conservative. Asset relinking and crash recovery
remain unimplemented.

hex-export shares the offline stereo PCM16 renderer between CLI and native paths.
It renders fixed 256-frame blocks, validates finite output and RIFF bounds, and
publishes through an exclusive same-directory temporary plus no-clobber link.
Native session export captures the latest compiled project Arc and revision, then
runs on a separate one-job worker. It renders linearly from frame zero through the
latest clip end; loop metadata does not repeat. Completion is a separate event and
cannot install an old graph or alter project dirty state. Native export refuses
empty projects and an existing destination. Progress, cancellation, overwrite,
stems, arbitrary ranges and alternate formats are deferred.

hex-engine::playback::Playback owns a compiled immutable project and transport.
hex-audio::prepared uses a one-slot publication queue, two-slot retirement queue,
32-command queue and independent Stop flag. It checks retirement room before a
swap, applies at most one graph per block, preserves transport and acknowledges
revision. Reclamation runs on the control side. Shutdown freezes submissions,
returns active/queued graphs and acknowledges completion while emitting silence.
These operations are tested hardware-independently and integrated with the
native CPAL prepared-output runtime; sustained device acceptance remains open.

The lifecycle worker implements non-RT retention/retry when callback pause fails;
it does not assume an explicit pause proves quiescence or wait on GPUI. The pinned
CPAL/coreaudio-rs backend disposes callback closures off the render callback after
stopping. Physical disconnect, persistent pause failure and device-reopen behavior
still require hardware acceptance.

## Application controller and stream lifetime

hex-session owns one Editor and retained decoded assets. Prepared imports compute
waveform summaries before committing metadata. Media frame and waveform budgets
remain charged through undo/deletion so redo never references destroyed audio;
raw asset-creation edits are refused without decoded data. Native controller
integration uses a dedicated control worker and bounded requests/events so import,
metadata history and graph compilation do not block GPUI.

hex-audio now leases one output stream per application. StreamOwner schedules
stop/disposal on a non-RT lifecycle worker without waiting in UI Drop. Stop failures
retain the same stream and retry at 250 ms intervals; the lease prevents reopen
from accumulating failed streams. If the worker unexpectedly exits, a static
quarantine retains the sole stream and reopening stays disabled. This deliberately
bounds retained resources, but cannot promise recovery from persistent OS failure.
Unit tests establish retention/disposal-thread and lease behavior using a fake
resource; physical stop-failure/disconnect acceptance remains unverified.

The canonical editor can request a default-output reopen through this lifecycle
worker. One open reservation remains held until its result is consumed or an
abandoned late stream retires; pause/disposal and CPAL construction do not run in
GPUI or the render callback. The session restores its current compiled graph
without changing project metadata. Native recovery interaction and physical
device-failure behavior remain unverified.

PreparedOutput adapts compiled block playback to CPAL, using fixed stack blocks,
rate checks, bus peaks, callback timings and fault flags. A nonfinite/failed render
silences its block and stops transport; no invalid float reaches the output.
The default native editor now uses this worker and prepared output; --demo retains
the old fixed runtime. Native import/edit/play/stop/save-new passed short checks;
virtualized row layout repair and sustained/fault acceptance remain open.

### Existing-document replacement and session association

hex-storage exposes opaque bounded exact-byte FileToken, load_with_token and
read-only check_unchanged, plus replace_existing and load_recovery. Replacement
requires the captured token, validates one target-bound recovery sidecar, copies
the observed prior bytes, and publishes prepared files by same-directory rename.
Unknown sidecars and direct symlinks are refused. The result carries the new token,
recovery path and optional postcommit durability warning; the session actor adopts
that token even when retaining a dirty warning. Open associates a file only after
candidate validation and compilation. Save As serializes relative references using
the original media base without mutating the live model. Basic modes are retained
for both copied files, with private Unix staging before writes. ACL/xattr fidelity
and filesystem CAS are not established. The native Cmd-S/Cmd-Shift-S wiring is in
source; fresh native dialog/save interaction acceptance remains open.

### Clip fade domains

Canonical clips retain a validated fixed frame-domain envelope so splitting
preserves sample gain through recursive edits. Setting fades and trimming have
explicit reanchoring policies; move/duplicate retain phase. Storage schema 9
persists the entire domain and migrates legacy clips neutrally. Prepared engine
clips store no envelope for neutral playback; non-neutral envelopes are boxed
on the control side. The renderer dispatches once per active clip, without
allocating, mutating ramp state or dropping envelopes in the sample loop.
See [clip-fades.md](clip-fades.md) for the original linear law, validation,
history and native acceptance limits. This does not establish Live fade parity.

Compiled clip phase/fade state shares optional control-owned metadata. Neutral
clips keep a 72-byte stride on the measured macOS arm64 build and allocate no
metadata. Renderer resolves the metadata once per active span and dispatches
the zero-phase path before the sample loop. Builder allocation/reclamation stays
on the control side, using the existing prepared-graph retirement contract.
This is an internal representation change, with no project schema/API change;
paired profiles and their limitations are in `performance.md`.

### Canonical musical time

hex-project owns nonnegative Q32.32 quarter-note beats, integer micro-bpm tempo,
and validated piecewise-constant tempo/meter maps with at most 1,024 points each.
Checked fractional-frame prefixes avoid truncating each segment to a whole frame;
inverse conversion uses the same prefixes and resolves quantized boundaries to the
latest point. Meter markers may truncate bars. Point commands and snapshots retain
normal transactional history. Schema 8 persists authoritative points and rebuilds
prefixes on load. The audio compiler still consumes frame positions: tempo edits
do not move or warp existing audio. See [musical-time.md](musical-time.md).

### MIDI model and prepared edges

Canonical tracks keep one ordered list with tagged audio or instrument contents.
Compatible audio accessors remain explicit; MIDI clips/notes use separate getters
and commands. Musical clip anchors and note times use BeatTime, with positive
bounded intervals and global monotonic note IDs. Schema 9 persists this model,
including the current one-shot policy; legacy tracks become audio contents.

Control-side MIDI preparation converts each absolute onset/off through TempoMap
and sorts bounded event edges, off before on at equal frames. Instrument audio
uses prepared absolute-frame spans and an immutable interval index; hex-devices
owns original sine/envelope DSP. Native and offline playback share Renderer.
Session prepares candidate compilation before committing history and publishes
that exact graph. Worker Open also compiles before replacing the current session.
See [midi.md](midi.md) for the original contract and required next runtime gates.

## Ongoing MIDI scheduling prerequisites

The engine now exposes an immutable prepared canonical tempo map and exact
first-raw-beat range lookup. It retains one canonical rounding kernel in
hex-project. Prepared source edge and instrument-span templates now provide
fixed-storage, stateless window queries. This explicitly ongoing
source view is distinct from finite Arrangement placement and saved metadata.
Prepared template admission must retain sample-rate/tempo ceiling compatibility,
so an incompatible query map cannot invalidate its edge/voice bounds. Instrument
preparation reuses hex-devices voices and indexes gate/release spans. Queries
reconstruct sounding notes without history replay and render through the shared
sine/envelope kernel into caller-owned scratch. Control-side density admission
is separate from the bounded query path. No runtime wiring or second project
model is introduced. Aggregate track work, stateful device releases, launch
transitions and the prepared Session graph/runtime remain integration contracts.

Canonical Session data is now project-owned: shared MidiSource values underpin
finite Arrangement placements and independent named Session clips. Sparse cells
reference stable track/scene IDs and own MIDI or explicit-stop content. Global
ID/cap/snapshot validation and schema-11 persistence cover both locations.
Scenes/cells are data, not a second runtime project. The source-oriented engine
adapter prepares either source owner; the current finite graph still renders
Arrangement only. The session worker must later prepare and publish the Session
inventory before launch commands/UI expose it as playable.

## Prepared Session revision contract — implementation in progress

The canonical Session grid lives in hex-project alongside Arrangement data.
Internal shared MidiSource values use copy-on-write for candidate/history clones;
this preserves unchanged source identity without a second editable model. Stored
snapshot/schema shapes and independently allocated copied clip/note IDs remain
the authority. Engine preparation validates every cell, including inactive cells.

A control-side PreparedRevision groups the finite Arrangement graph, immutable
Session inventory and Arrangement preview from one candidate. hex-session must
prepare the whole artifact before committing history or replacing an open
project. Event publication and restore share that exact artifact at its revision;
exports retain only the finite Arrangement graph. Inventory may travel with
Playback, but no Session rendering follows until explicit runtime activation.

Inventory work envelopes sum each track's maximum across its cells, since tracks
can independently launch cells from different scenes. Structural query/storage
bounds are separate from measured CPU admission. Replacement tails and active
phase anchors require fixed runtime banks and additional aggregate accounting.
Stable shared sine kernels are prepared off-callback and retained across revisions.
Source pointer identity does not establish continuity across tempo-map changes.

The source identity token owns its Arc<MidiSource> and compares allocation
identity; prepared inventories retain it to prevent recycled-address cache hits.
Reuse also requires exact canonical timing, sample rate, clip/track identity and
the stable sine kernel. Runtime phase continuity is not implied by cache reuse.
Legacy controller mutation return types remain compatible but must share the
complete candidate preparation contract with worker mutations. Finite standalone
compile/export APIs retain their Arrangement-only products.

Prepared Session inventory acceptance — 8 October: internal COW and owning
source-identity tokens, immutable prepared cell inventory, timing/kernel-safe
reuse, structural per-track envelopes and exact precommit PreparedRevision are
implemented. All controller mutations/history, including legacy signatures, and
worker initial/import/Edit/Undo/Redo/Open/Restore use full preparation. Export
retains the Arrangement graph; Playback retains the complete accepted artifact
without activating Session. Existing bounded Publication retirement moves the
whole boxed Playback, so inventory ownership follows the established control-side
reclamation path; no CPAL runtime behavior was changed.

Root discriminating inventory/worker/controller/COW acceptance passes 15 tests;
finite Arrangement regressions pass 17, audio boundary tests 8. Executor full
engine/project/session/storage tests and strict Clippy pass. Desktop build, fmt
and both architecture/UI guards pass. Source acceptance does not prove native
Session playback, mixed-route deadlines, stateful plugin lifetime or launch parity.
Next prerequisite is a shared borrowed per-track Arrangement render/mix seam,
followed by explicit ownership/quantization/continuation/retirement runtime tests.

Borrowed track-render/mix acceptance — 8 October: `Project::tracks()` exposes
compiled tracks in project order. Each `Track::render_additive` borrows its finite
source and adds per-clip/per-voice contributions using its prepared track mix;
range overflow is checked before destination mutation. `Track::mix()` supplies
the same validated coefficients/mute policy to alternative sources, and
`Project::master_mix()` supplies the prepared post-sum stage, applied once.
Renderer delegates to the extracted track loop without changing traversal or
floating-point multiplication/sum order. No allocation, new runtime ownership,
schema, CPAL or UI behavior was added. This enables selecting a source without
rendering/subtracting its overridden Arrangement source. Runtime ownership
selection remains unimplemented; the additive source prerequisite follows below.

Additive template prerequisite accepted — 8 October: a borrowed
`MidiInstrumentRenderContext` carries launch beat, prepared timing and sine table.
`PreparedMidiInstrumentTemplate::render_additive` queries the complete <=256-frame
window before mutating caller output, adds each sorted voice using prepared track
mixing, validates muted requests, preserves destination on refusal and clears
visible query storage. Master gain belongs to the complete track mix; the original
standalone overwrite render is unchanged. This adds no runtime ownership,
Session launch command, CPAL publication or UI. Whole-block failure, routing,
aggregate admission and bounded continuation remain dependent runtime work.

Headless Session kernel accepted — 8 October: `session_playback` owns only runtime
state over one immutable PreparedRevision. Core construction/views, command
policy and rendering live in separate modules. Compiled Arrangement tracks retain
optional canonical IDs for revision-order validation; standalone track builders
remain unchanged. The kernel selects Arrangement/Session/Silence per track,
supports sparse scenes and immediate/four-quarter pending actions, splits queries
at <=256 frames, loop wraps and due transitions, and applies master gain once.
Construction reserves route/scratch and Copy release storage. On any later render
error, requested output is zeroed and persistent state remains unchanged.

Outgoing finite or ongoing sine spans preserve oscillator onset anchors. Held
gates clamp at a route cut; already-releasing voices retain their gate/end. Copy
entries retain no graph handles; checked capture order preserves deterministic
per-track accumulation. Capacity is 64 voices per track MIDI source, pooled and reserved
conservatively before commands commit; refusal preserves other pending actions
and rendering can advance to expire tails. Same-owner Arrangement/Silence changes
capture no duplicate voices. Source inspection finds no render/apply allocation
or owning Arc changes; native callback lifetime/allocation auditing is still open.

Sixteen kernel tests and 38 related root regressions pass, including real mixed
audio/MIDI/silence composition, held/releasing phase, capacity recovery, late
refusal after a due pending transition, retry PCM and repeated-cut ordering.
Full engine tests/strict Clippy, desktop build, formatting and both guards pass.
The canonical 100-track release profile is in performance.md. The periodic
timing seam now supports multi-point tempo maps with enabled Arrangement loops.
Graph-revision/tempo-edit active-voice handoff remains required;
native Playback, CPAL publication and the editor still use the Arrangement path.

Periodic timing is prepared with the immutable Session inventory as Copy loop
metadata plus the owned canonical map. Borrowed views perform bounded conversions
at event/block boundaries, never per sample. Canonical center inverses define
the frame-anchored loop's beat period; inverse lower bounds include prior-cycle
beats quantized to a seam. Template admission and reuse include the effective
rounded periodic rate. Unrepresentable periodic metadata retains its typed error
without rejecting a finite Arrangement revision; Session kernel construction
returns `SessionTempoUnavailable`. Finite compilation/export remain canonical.

Revision transfer accepted in source — 9 October: a candidate
`SessionPlayback` is constructed off-thread and reconciles through the
`try_inherit_state_from(&previous)` API. At the cut, one shared clock stores the
old effective musical beat and the new base-map beat at the same performance
frame. Future conversions translate between those anchors; Arrangement retains
its separate frame cursor. Historical template onsets are filtered before frame
conversion, while sounding voices carry copied phase/gate data and bounded
suppression keys. This prevents tempo edits from jumping musical position or
attempting to represent past onsets as unsigned new-clock frames.

Stable IDs reconcile routes, notes and pending target snapshots. Carried held
gates may follow the edited clock; already-releasing envelopes retain sample
anchors. Repeated edits normalize the same carried occurrence. Deleted tracks
need copied old mixing coefficients for their tails, without retaining the old
graph. Capacity/error refusal must preserve both runtimes; the old graph remains
active until the accepted swap and control-side retirement. These are engine
acceptance requirements, not yet a native callback lifetime guarantee.

The implemented shared continuation/release bank reserves `MAX_TRACKS * 64`
entries off-thread, including orphan tails from removed tracks. Route cuts
reconcile already-carried held voices in scratch and suppress duplicate capture;
due retriggers clear the outgoing route's occurrence keys. Seek validates the
base clock before clearing inherited timing/tails; transport Stop clears them
while retaining selected route owners. Releasing gates are never converted
through the edited clock. Late-chunk capacity refusal zeros the complete block,
preserves committed carry/suppression/route/order state and permits exact retry
against a twin.

Accepted shared-sample launch policy: a pending action retains its
requested quantization beat. If its old scheduled frame is already due at the
revision cut, retain the cut frame without converting that historical beat
through the edited clock. At application, anchor the new source to the effective
musical beat at the actual sample boundary. Several nearby beats can round to
one frame; using the boundary's canonical inverse keeps the first source onset
eligible without clamping the clock or chasing historical events. Future pending
actions still remap by their requested beat. This is an explicit Hex contract,
with Live's precise shared-sample launch behavior still unverified.

Native integration contract — 9 October, source accepted: legacy Playback
constructors keep finite Arrangement rendering. An explicit prepared Session
constructor allocates the kernel off-thread; native workers expose a specific
capability reason and retain Arrangement playback when Session is unavailable.
Session-capable native graphs use the mixed renderer from the outset, subject to
all-Arrangement PCM parity over wraps/partitions. Both paths retain separate
Arrangement and monotonic performance counters; capability transitions seed
those counters without discarding musical transport time. Legacy finite clocks
have explicit exhaustion bookkeeping; promotion validates the candidate's base
Session beat domain before changing it, and rejects exhausted clocks atomically.

A candidate without Session capability cannot replace live overrides, pending
actions or tails. Kernel replacements reconcile transactionally. Publication has
one queued/deferred candidate, guarded until accepted or permanently rejected;
transient release-bank refusal retries while the old graph and priority Stop
continue. Permanent rejection returns ownership through retirement and publishes
its rejected revision/error separately from applied revision. Failed command
tickets likewise have a distinct acknowledgment. Shutdown reclaims active and
deferred ownership on the control side. Source tests now pass (see plan.md and
native-session-root logs). Canonical controls are the next active unit; native
interaction, allocator auditing and physical deadlines remain acceptance gates.

Audio publication tests may depend on `hex-project` as a dev-dependency to build
canonical revision fixtures. The architecture guard permits that single test-only
edge; production audio dependencies remain engine/CPAL/rtrb, with no ownership of
project editing, parsing or UI. Do not replace fixtures with a competing model or
add production reexports just to bypass this boundary.


## Long-project source access

`hex-media::WavFrameReader` now provides bounded control-side random-access
classic WAV decoding. It shares RIFF topology and sample decoding with resident
imports, owns the original descriptor, uses 32 KiB decode scratch, and applies
separate file/source/header/window limits. Descriptor length/mtime checks detect
ordinary mutations but do not establish immutable source identity. Full resident
Audio objects remain the production native Session/Arrangement model. Shared
source-window conversion, verified byte identity and a bounded worker/cache now
pass headless engine acceptance. Bounded callback page ownership and canonical
source integration are still required before native long-source playback; see
[media-streaming.md](media-streaming.md). No file IO belongs in the engine render
or audio callback.

`hex-stream` owns worker IO, immutable generation-scoped source catalogs and
fixed page payloads; `hex-engine` knows only source facts and a borrowed page
provider. Canonical resident/streamed compilation must share the existing
Arrangement loop, including MIDI preparation, gain, solo and clip semantics.
Unused retained media remains legal; referenced bindings require exact canonical
metadata and generation. An Arrangement bridge is not a streamed PreparedRevision:
Session inventory and rendering must adopt the same resolver before native wiring.
`StreamRuntime` remains a headless facade with `split()` into `StreamControl`
and `StreamCallbackPages`. Only control owns worker wake/join, cancellation and
catalog readiness. Pages own bounded queues/cache/coalescing and borrowed access;
failure draining clears pending keys there. Matching control/pages shutdown joins
and accounts for all buffers; a private pair token rejects unrelated runtimes
without losing either owner. Pages destruction still frees storage and must occur
off realtime after callback quiescence. The worker owns actual descriptors:
control must outlive all graphs using any key in its immutable catalog. This is
not a dynamic catalog lease or native endpoint-retirement implementation.


## Application browser preferences — 9 October 2026

hex-preferences is the narrow application-preferences boundary: versioned bounded
browser-place data, platform path encoding and recoverable filesystem publication.
It has no workspace dependency and owns no project edits, GPUI, enumeration or
audio. UI coordinates background load/save with bounded serialized/coalesced
work. hex-storage remains project-only and hex-library remains listings-only.
See browser-places.md for persistence/error and restart acceptance.


## pinned external GPUI input and cursor repair

The 9 October primary boundary decision uses an excluded `third_party/gpui`
copy of verified published GPUI 0.2.2 with a pinned native input/cursor delta.
`[patch.crates-io]` and the lockfile resolve it reproducibly. It preserves
Apache-2.0/license headers and remains the existing external GPUI/AppKit boundary;
Hex workspace unsafe prohibition is unchanged. No new framework/app FFI.
See `native-input.md` and `scripts/check-gpui-patch.py` for scope and checks.

The next 9 October cursor decision extends only this existing boundary with
original left/right bracket cursor variants, cached NSCursor images on macOS
and directional native fallbacks elsewhere. Native cursor creation/retention
remains GPUI-owned, never Hex FFI or per-hover image construction. No reference
assets are extracted. Exact changed files remain pinned by the provenance guard.

## external xattr compatibility — 11 October 2026

The existing GPUI → zed-async-tar → xattr boundary retains xattr0.2.3 through
a checksum-verified excluded `third_party/xattr` patch. Only Linux/Android's
missing-attribute errno name changes to libc::ENODATA; macOS/BSD retain ENOATTR.
There is no new workspace FFI or unsafe exception. See
[xattr compatibility](xattr-compatibility.md) for provenance, locked compile
regression, macOS query tests and explicit cross-compilation limits.

### bounded native accessibility boundary — 14 october 2026

Ticket29/PR188 additionally authorizes the excluded GPUI semantic-control/AppKit
patch described in [native-accessibility.md](native-accessibility.md). Native
object/selector handling stays inside that dependency; Hex retains safe Rust,
canonical commands and existing gesture/history ownership. The exact original
archive, changed files and added source remain covered by the vendor guard.
Mounted generic-control source coverage is separate from native AX/VoiceOver and
offscreen-navigation acceptance; this decision does not establish those passes.
Musical audio roots, preparation/retention bounds and the engine→warp boundary
are specified in [musical editing](musical-editing.md#preparation-and-retained-memory).

## never-installed constructor custody

The production `hex-audio -> hex-stream` edge is restricted to real catalog
leases/pair identity and control-side disposal for admission-pending constructor
candidates. Engine still has no stream/media/device-I/O dependency. The actual
publication queue returns this opaque candidate before inheritance or rendering;
legacy publication remains separate. [Never-installed disposal](never-installed-audio-disposal.md)
describes terminal anchoring, retained failure and the unresolved native gates.

## controlled standalone VST3 tooling

`hex-plugin-scan` has no workspace dependencies and is not imported by the app,
engine, audio or session crates. Its Linux-only supervisor admits explicit
original SDK fixtures through caller-owned build receipts. Separately built C++
children own factory discovery or bounded offline component processing; foreign
objects and SDK pointers never enter the Rust supervisor or audio callback.
Production/installed plugin scanning and application hosting remain refused.
The receipt records pinned MIT SDK provenance, compiler/header/runtime closure
and licenses; process cleanup contains ordinary fixture faults, without claiming
a security sandbox. See the crate README for the finite wire, lifecycle and test
contracts. Other platforms expose refusal stubs, not a translation fallback.

`hex-clap-scan` owns the corresponding separate CLAP format boundary, with no
workspace dependency edges. Its descriptor and controlled offline children cover
original audio/MIDI/state/delay fixtures on Linux. The arm64 Mac portability unit
requires its own enrolled immutable artifact root and native evidence; Linux
results do not open that eligibility. It shares no process-global signal owner
with the primary application or the independent VST3 supervisor.
