# GPUI and audio runtime

## Native device integration

Canonical device state is an opaque versioned envelope; registered definitions
parse it and prepare ordered memoryless matrices or stateful processors on the
session worker. `PreparedTrackMix` applies the regular effect transform before
track gain/balance and Main summation across Arrangement and Session sources.
Captured outgoing Session voices keep their source revision's mix. Payloads and
device codecs never enter the callback.

Native parameter requests carry device/parameter IDs and typed values. The
worker patches the latest canonical state and compiles the candidate before
committing history. These are discrete edits; continuous gesture coalescing and
grouped undo are separate Pulse ticket 65. Full graph latency compensation
remains a separate dependency. See [lattice](lattice.md) for the contract and
acceptance limits.

## Return audio routing

The compiled topology is regular track → Return → Main. A regular source and
effect chain are evaluated once; sparse sends tap the post-track-fader result.
Each Return processes its sum through the registered chain and bus mix, then Main
applies master gain once. Arrangement rendering, SessionPlayback, and prepared
linear export use the same routing. Only registered return effects that declare
explicit zero algorithmic latency are admitted. Fixed 256-frame scratch and
runtime chains are constructed on control. Compatible Return IDs inherit state
only after all chain copies have passed preflight. Seek and Stop reset Return
state; arrangement loop wraps preserve it. Zero-input blocks still process
return filters so their stateful output can decay. Removed regular-track sends
stop at the revision boundary, existing release tails remain Main-only, and a
removed Return is cut at the boundary and retired on control. This is not a
general compensation or Live-equivalent tail policy; see
[return tracks](return-tracks.md).

## Default canonical editor

Desktop composes PreparedOutput and the hex-session worker; GPUI owns only
display/selection and bounded command endpoints. The worker request queue holds
32 requests, its event queue two committed views/graphs; GPUI retries at most 32
local requests and polls two events per 16 ms tick. File decode, waveform peaks,
metadata history, load/save and compilation stay on the worker. Track rows are
virtualized; peak arrays are cached.

Prepared audio handoff has one publication slot, two retirement slots and 32
transport commands. Each callback accepts at most one graph and 32 commands.
Retirement-room checks precede ownership transfer; control-side reclamation drops
old graphs. Prepared Stop has an independent atomic ticket cutoff: it stops first,
discards older requests and preserves later admitted commands in FIFO order.
Ordinary edits preserve transport through graph replacement. Opening a
project stops at the start and clears pending UI transport/selection.

Fresh candidates may opt into `InstallFreshRuntimeIfUnchanged` with a
`FreshRuntimeFence` captured on the control side. Capture reads one coherent route
header, then Acquire-loads the admitted-command ticket. At the callback's final
pre-swap point, the fence must still match active revision (zero when empty), the
active runtime's route-state generation, and the admitted ticket, with no pending
priority Stop. A changed fence returns the candidate through normal retirement,
reports a typed `runtime-fence-changed` rejection and clears the publication gate;
it never clears a pending Stop. The normal command cut then affects the old graph.
An action or Stop admitted after successful validation follows the accepted graph
swap and therefore applies to the new graph. If retirement capacity is full, the
candidate remains deferred while the previous graph continues, and is revalidated
when a slot becomes available. The existing unconditional fresh and inherited
handoffs are unchanged. This fence only protects audio runtime freshness; it does
not replace the Session worker's project-generation/revision admission checks.

Edited model revision and applied audio revision remain visibly distinct until
the callback accepts a graph. Canonical session history commits after successful
control-side compilation; later callback admission failure does not roll back the
project, undo/redo state, dirty state, or worker save/export snapshots. The editor
tracks the current audio admission by project generation, canonical project
revision and audio revision, with pending/applied/rejected status. A rejected
audio revision is resolved against its exact retained candidate before candidate
acknowledgments are pruned. Only a rejection matching the current generation,
project revision and latest publication changes the current footer state; stale
or coalesced rejection telemetry cannot overwrite a newer accepted or pending
revision. Numeric device/send previews reuse a canonical project revision but
remain transient overlays: preview rejection stays visible as a preview error,
preview application does not change durable canonical status, and cancel/restore
does not create a canonical rejection. The displayed last-audible identity is
the last admitted canonical graph, not the latest transient preview; the editor
keeps the actually promoted preview candidate separately for playback and uses
its validated source revision for existing transport gating. Queue-full and
deferred publication remain pending until a later publication receives a
terminal acknowledgment. Audio recovery resets audio admission for the current
canonical revision while retaining no stale audible identity. The callback
protocol is unchanged and exposes only its existing high-water/rejection status;
the UI's bounded candidate mapping supplies project identity. See Pulse2180 for
the open acceptance gate and remaining native validation.

A failed open leaves the current model/audio intact; new media and save-as paths
preserve case. Device faults retain offline editing.
A nonfinite render block becomes silence and stops transport. Finite floating
output preserves headroom; integer output formats necessarily saturate conversion.

The compact ⟳ control requests a default-output reopen while the editor is offline.
The lifecycle worker validates the actual default rate against the project rate
before stream construction and allows only one pending open. Request/result polling
is nonblocking in GPUI; the reservation remains held until the result is consumed
or an abandoned late stream retires. The session restores the latest compiled
graph through `RestorePlayback`, without changing revision, history, dirty state
or document association. A generation token drops stale UI completions. Recovery
clears old control acknowledgments, remains stopped, restores the captured callback
position (or offline cursor), waits for the current graph and a newly consumed seek,
and lets Stop override the pending cursor with frame zero. Play, seek and split are
gated while recovery is pending; editing and saving remain available. Failure
leaves the app offline with a retry path.

This is a source-implemented recovery path, not verified physical device recovery:
native failure/reopen, rate-mismatch interaction and listening remain untested.

## Arrangement seek candidate preparation — Pulse2188

`hex-session::ArrangementSeekWorker` owns a dedicated control thread with an
explicit 8 MiB stack reservation, independent of canonical edits and export.
Each Service therefore reserves one bounded worker stack in addition to the
candidate storage budget below. It admits one running request and one replaceable
pending request, and retains at most one completed result. Admission is
nonblocking with respect to preroll and result consumption. New admission cancels
the running request, replaces pending work, and removes an unread result;
explicit invalidation cancels and clears the same state. The candidate is
prepared directly from the exact immutable `Arc<PreparedRevision>` through the
bounded engine preroll API, avoiding a throwaway source runtime. Wrapping the
result in `Playback` preserves that runtime, exact graph Arc, performance and
Arrangement frames, and requested play state without calling `Seek` afterward.

The result echoes request ID, caller generation/revision, exact prepared Arc,
destination frame, work budget and desired play state for a later caller-side
admission check. Engine limits remain 8,388,608 replay frames and 32 MiB accounted
candidate runtime storage, with <=256-frame cancellation checks and a 2,048-byte
render scratch allowance inside the preroll storage budget. Shared immutable
prepared graph data is not duplicated or charged to the runtime budget. There
are bounded running/pending request metadata and two fixed completed-runtime
slots for defensive stale-result retirement. A completed but unread
runtime moves into one worker-owned retirement slot on new admission or
invalidation. If that slot is occupied, the stale ready value stays in its own
fixed slot; both are drained before any pending candidate construction. The
normal path retains one candidate runtime. Defensive double-stale state is
bounded by two 32 MiB runtime slots, and performs no new preroll until drained.
Teardown cancels, wakes, and detaches without joining; the worker drains its
owned pending/retired/in-flight/result state before exit. Replaced pending
request metadata and its prepared-revision `Arc` are dropped by the submitter
after unlocking, while unread runtime `Playback` destruction stays on the
worker. A latency-sensitive caller must retain its audible/current prepared
revision owners across supersede or invalidation until worker ownership drains;
this API does not guarantee that final immutable graph release avoids the
caller thread. The linear candidate API remains Arrangement-only and does not
support Session route actions. The GPUI editor's stateful Arrangement seek path
retains the exact prepared revision and captures a fresh runtime fence at
admission. It refuses admission unless the current canonical revision is
already audible, every route belongs to Arrangement, and no edit, replacement,
gesture, recovery, or queued playback action is pending. The same fence is
carried through preparation and installed with
`InstallFreshRuntimeIfUnchanged`; it is never recaptured for an admitted
request. Pointer seek preserves the pending play intent, while Spacebar
PlayFrom starts playback. A second Spacebar press sees a pending or latched
play intent as playing and queues Pause even before the callback admits the
first transport command. Direct analytic seeking requires a retained prepared
revision that identifies the source as analytic; an unknown active source is
refused rather than sent through the fast path.

Audio device/render faults and shutdown refuse both initial admission and ready
candidate publication; the editor tick invalidates retained seek intents after
reclaim when it observes those flags. Before publication, invalidation cancels
preparation or returns the uninstalled candidate. After conditional publication,
Pulse2208 arbitrates cancellation against callback installation with one
revision-keyed atomic decision cell allocated on control. UI invalidation CASes
pending to cancelled; the callback CASes pending to applying immediately before
active graph replacement. If cancellation wins, the candidate is rejected and
returned through existing retirement ownership while the old graph stays active.
If application wins, the UI waits for the exact terminal revision and the newer
canonical graph follows. The original route/command fence remains in force.
Ordinary inherited publication is unaffected. The callback moves the token with
its Publication and never allocates/clones/finally drops it. See the bounded
[decision and ownership contract](prepared-seek-cancellation.md), including
Stop ordering, shutdown and backpressure evidence.

An unconsumed worker result is replaced through the worker's fixed retirement
slot. A consumed result that becomes stale is returned intact to that slot; if
it is occupied, the UI retains one result and retries before admitting more
work. During editor teardown, the result is handed to the worker's existing
retired slot, or its empty ready slot, for worker-side shutdown destruction.
The worker's ready and retired slots are each bounded by the existing 32 MiB
candidate ceiling. A blocked UI handback may add one retained UI result, so the
defensive aggregate upper bound is three candidate ceilings (two worker slots
plus one UI-held result), not only the worker's 64 MiB. The consumed-result/
teardown flow assumes the
normal invariant that the ready slot was vacated before the UI retained a
result and no later seek was submitted; if both worker slots were occupied,
teardown handback would remain unresolved and must not be treated as a proven
worker-side drop. This defensive two-full-slot case is refused by the API; it
is not silently overwritten or converted into another owner queue.

This admission path does not compile or warm a graph on the callback. Existing
`Control::reclaim` still destroys callback-returned owners on its caller (the
current editor tick), so this work does not claim that every graph destruction
is off the UI thread. Native output/listening acceptance and physical-device
deadline behavior remain open.

`ArrangementSeekMode::LinearArrangement` remains the default and preserves the
export-style Arrangement-only runtime. `ArrangementSeekMode::Interactive`
constructs a normal Session runtime first and then advances its Arrangement
history, so the result remains eligible for Session route commands and normal
prepared-revision state inheritance. Both modes reject enabled Arrangement
transport loops. Results echo the selected mode; the caller must still validate
current project/route/Stop admission before publication. `Playback` wrappers do
not establish the target-frame or readiness proof themselves.

Interactive mode reconstructs Arrangement history from frame zero while keeping
the exact `Arc<PreparedRevision>`, runtime clocks, mutable voices, release tails,
track/return processor history and requested play state. It uses the same
<=256-frame cancellation checks, 8,388,608-frame work ceiling and 32 MiB
accounted candidate storage ceiling as linear preroll. The separate 8 MiB thread
stack is reserved once per Service. An initial high-residency test fixture
overflowed the platform-default test-thread stack; a reduced fixture passed on
a 2 MiB test stack after sequentializing runtime construction. The dedicated
worker still owns the supported preparation path. `size_of::<SessionPlayback>()`
(104,568 bytes here) is not a peak-stack measurement, and 100-track worst-case
stack use remains unmeasured. Enabled transport loops are refused because the
operation currently models one monotonic Arrangement pass.

`PreparedRevision::arrangement_seek_requires_preroll()` reports whether direct
absolute-frame seeking would omit mutable Arrangement instruments or stateful
track/return processors. Built-in Sine and Operator with zero effective feedback
remain on the direct path because their notes are deterministic functions of
note age and gate duration. Nonzero effective Operator feedback uses the bounded
mutable runtime and therefore requires the existing prepared-history seek path;
enabled transport-loop seek/preroll and other unsupported mutable paths remain
explicit refusals. See docs/operator-feedback.md for its actual admission scope.
This is a control-side policy query, not a full eligibility or publication
fence. Native UI/listening acceptance remains open. A 100-track interactive readiness
profile is recorded in docs/performance.md; it is not a physical deadline.
Canonical startup now opens output on the same bounded lifecycle worker. A
font-validated loading window owns the pending handle; a weak polling task and
non-owning window handle replace its root with the editor when output is ready.
The project binds to the actual default-device rate; failed output open alone
selects the existing 48 kHz offline fallback. Closing the loading window abandons
the request and late output retires through the worker. Demo startup remains
synchronous. Startup timing and close-during-open acceptance remain unmeasured.

Native Command-Shift-E export is submitted through the bounded session request
queue, but a separate atomic reservation permits only one active export. The worker
accepts only the current compiled revision and captures its immutable playback
Arc plus revision before dispatching to a dedicated export worker. That worker owns
disk rendering and reports on a separate one-entry result queue; native export
releases its reservation when the UI consumes the completion. Later edits and
graph publication continue independently, and completion never changes canonical
state, transport or dirty status. Closing while an export is pending warns that
it may not finish; shutdown does not join the worker.

The export range is linear project frames [0, latest clip end), regardless of
global loop state. Empty native projects and destination collisions are refused.
WAV output uses fixed 256-frame blocks, finite-value checks and synced
create-new publication. Progress reporting, cancellation, stems, selected ranges
and overwrite are not implemented.

StreamOwner uses a single-stream lease and an off-RT lifecycle worker. Failed
pauses retain and retry the stream, preventing unsafe callback closure disposal
or accumulating reopened streams. Persistent failure can retain one stream until
process exit. Physical device-loss and sustained callback acceptance remain open.

Worker views include an unsaved flag. Successful model changes/undo/redo mark it;
successful Save As, clean Save Current and open clear it. A committed save with a
durability warning keeps it set. Ordinary native close and Cmd-Q
consider that flag plus pending local/worker requests, duplicate intent and open
state. A coalesced cancel-first asynchronous prompt freezes new edit admission.
Already accepted requests and duplicate continuations keep their bounded ordering
while the prompt is open. Discard stops output before closing. Pinned GPUI's
app-level quit observer cannot veto termination; OS/forced quit and crash recovery
remain separate unfinished work.

Create-new project saving distinguishes a pre-publication failure from a cleanup
or directory-sync failure after publication. The latter reports that the complete
file exists at its path; the editor keeps the project conservatively unsaved.

Native editor source routes Cmd-S to Save Current for an associated project and to
create-new Save As while untitled; Cmd-Shift-S always opens Save As. One save dialog
and one save job may be active per view. Explicit `save_complete` events release
the job flag on success or failure; a closed worker also releases it. The compact
toolbar hint, create-new path/extension validation, and pending indicator are in
place. Fresh native dialog and save acceptance remains open.

## Fixed demo slice (--demo)

hex-ui uses GPUI 0.2.2 for main-thread display state; hex-audio uses CPAL 0.17.3
for device streams. hex-desktop creates the backend and injects it into the UI.
The chosen dependency pins are in Cargo.lock. GPUI's native macOS GPU/UI backend
and CPAL/CoreAudio are allowed OS boundaries. GPUI is documented by the
[upstream project](https://github.com/zed-industries/zed/tree/main/crates/gpui);
CPAL's [stream API](https://docs.rs/cpal/0.17.3/cpal/traits/trait.DeviceTrait.html)
defines output/error callbacks.

Before stream startup, LiveEngine builds eight immutable stereo buffers (four
tracks × two variants), using the device's negotiated sample rate. At 48 kHz
this is about 6.1 MB. Engine and buffers move into the audio callback once and
remain there throughout playback. No graph/media replacement happens in a block.
The callback generates no media, performs no file I/O or locks, and its Rust
render path contains no allocations. This is source inspection, not allocator
instrumentation or proof about the native backend.

UI sends fixed scalar commands through a 32-entry rtrb SPSC queue. Exactly one
producer lives in the GPUI entity and one consumer in the output callback. At
most 32 commands are consumed per callback. Queue-full rejects the edit and
reports it; UI values only update after successful enqueue. Delivery is next-block,
not sample-timestamped external automation. There is no dynamic graph yet.

Stop uses a separate atomic flag. At the next callback it suppresses queued
transport commands, preserves queued mixer/tempo/mode edits, then resets and
pauses transport. Therefore Stop cannot be lost to a full queue. Stop has priority
over Play requested before that callback; Play after the callback resumes.

Atomics expose eventually consistent play state, bounded loop phase, active
scene, track peaks, callback maximum, deadline misses, and device-error flag.
UI polls at 30 Hz. Atomics never drive scheduling. Callback timing measures
this closure only; it does not count every OS underrun or scheduling delay.

## Prototype playback contracts

- Default is paused Arrangement; its eight-beat loop plays A then B for all tracks.
- Session holds one scene shared by all tracks. Selecting a scene enters Session.
- While playing, launches occur at the next four-beat boundary; at an exact
  boundary they may launch immediately. Last request replaces the pending one.
- While paused, a scene changes immediately but does not start transport.
- Pause holds loop phase. Stop resets phase and cancels pending launches.
- Switching views also switches playback ownership; this is a Hex prototype
  convention, not measured Live behavior.
- Tempo is 20–300 BPM. Linear source interpolation changes rate and pitch.
- Mono outputs receive a stereo average; outputs above stereo receive silence
  on extra channels. f32, i16, and u16 device formats are supported.
- Final output clamps to [-1, 1]; no limiter, normalization, or gain smoothing yet.

Keep the hex-audio output handle owned by the main-thread UI entity for its lifetime. CPAL/CoreAudio
teardown must stop callbacks before callback-owned media is freed. Closing Hex
releases the stream; explicit disconnect/restart stress acceptance is pending.
The canonical editor has a manual default-output restart action, but device
selection and automatic recovery are not implemented. Physical recovery
acceptance remains open.

## Further general DAW architecture

One canonical editable project owns stable IDs and asset references. A control
worker compiles it into a render graph with block schedules and preallocated
buffers. The audio thread owns the active graph. Graph swaps require bounded
handoff plus deferred reclamation on a control thread, including a policy when
that handoff fills. Add that protocol only with lifecycle tests.

Disk workers handle import, recording writes, and streaming with explicit
underrun/overrun behavior. The callback never waits for them. DSP state remains
with the audio thread. UI draws a cached viewport and waveform summaries rather
than traversing the audio graph. MIDI/automation carry frame offsets; seek, loop,
and graph edits define discontinuity/reset semantics. Plugin hosting and latency
compensation need measured contracts before architectural promises.

## Native layout iteration — 8 October 2026

The view is split into state/dispatch plus transport, timeline and mixer layout
modules. It uses a 38 px toolbar, 188 px browser (collapsible to 30 px), compact
44 px arrangement lanes and 40 px Session rows, 150 px mixer and 72 px inspector.
Transport/mute/gain icons have lowercase native tooltips. The practical minimum
window is 960 × 600; interactive final minimum-size acceptance remains pending
after native automation interruptions. The source inspector identifies generated
media and tempo/pitch behavior; no decorative device or import controls exist.
Audio commands/queue/runtime contracts are unchanged.

Source-rate conversion runs on the canonical session worker before media commit
and graph publication. Original source rate/count and converter version remain
in schema-9 metadata (schemas 1–8 remain readable); all prepared PCM and source intervals use project frames.
Open validates source shape then converts one asset at a time. No conversion runs
on Play or in the audio callback. Device/project rate mismatch remains refused.

Prepared transport commands can return monotonic admission tickets. The callback
publishes the highest consumed ticket after transport state (Release; UI reads
Acquire). Consumption includes commands discarded by priority Stop, so it is not
a promise of application. Failed admission does not advance tickets; exhaustion
refuses further queued commands. The largest ordinary ticket is `u64::MAX - 1`,
reserving a representable cutoff-plus-one Stop marker even at exhaustion. The
marker is one atomic value (0 means no request). Admission publishes its
high-water only after a successful queue push. Callback entry acquires that
high-water before swapping the marker; safe queue peek retains commands beyond
the captured prefix for the next block. Stop applies before commands and discards
only tickets through its cutoff. Repeated Stop after the swap remains pending,
even with the same cutoff. Capacity/work bounds remain 32 commands per block:
the same constant defines queue capacity and pop budget, ensuring the entire
pre-Stop prefix is drained. Increasing capacity independently would require a
persistent discard watermark.
The UI can gate cursor-dependent edits until an accepted seek is consumed without
inferring acknowledgment from a moving playback position.

## Canonical Arrangement loop

The optional project-frame region [start,end) and enabled flag are canonical,
undoable schema-4 metadata. Legacy projects migrate to no region. Cmd-L uses
selected audio clip bounds; MIDI Cmd-L region setup remains deferred. The compact
transport indicator and loop rail show canonical state; the existing revision
status reports when audio has applied it. Region changes compile on the session
worker.

Prepared Playback renders an intro before start, wraps at end, and normalizes
playing seeks beyond end into the region. Empty outputs do not normalize; Pause
emits silence without moving; Stop returns to zero. Graph replacement inherits
transport before applying new bounds on render. Linear Renderer/CLI export ignores
loop repetition. A complete stateless source period can be repeated from the
caller output buffer with no allocation; stateful devices/events/fades need separate
contracts. This is Hex behavior, with no measured Live loop-parity claim.

### Native Arrangement loop rail

The compact rail below the ruler is a GPUI view over this same canonical loop
region. A drag paints a local preview and dispatches one `SetLoopRegion` on
release; newly painted ranges start disabled. Command bypasses the active
adaptive snap grid for both anchor and endpoint. Resizing either visible edge
preserves the region's enabled flag. Clicking a disabled range enables it, while
L and the toolbar continue to toggle playback looping. The tooltip documents
these gestures and Command's free-placement modifier. Pointer capture remains
owned by the rail through release outside its bounds. Generation, project
revision or canonical loop-region changes discard the preview. No per-move
project edits or audio-thread state are introduced.

The canonical mixer adds validated per-track balance (-1..=1) and master gain
(0..=2). Center balance is unity; this original Hex linear law attenuates only
the opposite channel, with no boost or crossfeed. Master gain applies after track
summing without limiting. Schemas 1–4 migrate to centered balance and unity master.
The compact native mixer strip is 26 px tall beneath the existing 44 px track
rows. Live pan-law parity and fresh native mixer interaction acceptance remain open.

The canonical editor continues bounded polling every 16 ms but notifies GPUI only
when visible state changes. Worker/export completions, graph/seek acknowledgment,
faults and playing position/meters refresh promptly; paused/stopped cursor moves
also invalidate. Callback-duration diagnostics refresh at a one-second cadence
when otherwise unchanged. This removes unconditional idle repainting without
slowing queue consumption. Pure policy checks pass; dense native responsiveness
and input-latency percentile gates remain open.

The session actor now owns the selected document path and exact file token.
SaveCurrent uses recoverable replacement; SaveNew remains create-new Save As.
Successful Open adopts the association only after media preparation and compilation.
Save As retains the original live relative-media resolution base while serializing
absolute references into the new file. Saves change neither revision nor playback.
A committed durability warning adopts the new token, retains dirty state, and
reports both target and recovery paths. Event.save_complete acknowledges every
processed save, including errors, so UI admission never relies on status wording.
File serialization/sync remains on the canonical worker; slow filesystem operations
can delay subsequent canonical requests while UI polling/feedback remains active.

Canonical clip dragging uses a bounded metadata-only GPUI payload and cached lane
bounds pruned to current track IDs. It captures project generation, revision,
source identity/start/duration and viewport, revalidates them at gesture creation
and drop, and refuses pending canonical work. Only a valid non-no-op drop enqueues
MoveClip; pointer motion does not edit or compile. Canonical completion reconciles
selection to the clip owner, including undo/redo. Escape has a cancel-first action
while explicit Stop always cancels any drag and stops playback. Native gesture,
resize and dense-load responsiveness acceptance remain open.

The canonical editor reads tempo/meter maps from worker snapshots. Compact ruler
controls submit undoable point edits at the cursor's snapped quarter. Pointer
clip moves opt into quarter/eighth/sixteenth grids; preview bounds and release
share a control-side resolver, with revision/generation/viewport/snap guards.
No new map conversion or allocation runs in the audio callback. Existing audio
stays frame-anchored when tempo/meter changes. Native tempo/meter save/reopen
and quarter-snap undo/redo passed in the isolated timing fixture; held-preview
and dense interaction acceptance remains open.

Canonical MIDI sources on hybrid tracks render original sine voices from immutable
absolute-frame spans shared by offline export. Preparation and ownership remain
on the control worker. Candidate edits compile before canonical history commits,
and the service publishes that exact graph. Failed preparation preserves IDs,
redo, revision, dirty state and prior playback. Seek analytically chases active
notes; pause freezes position, Stop returns to zero, and global loop wrap repeats
the same absolute-frame region. This contract does not apply to future stateful
plugins or external MIDI. See midi.md for density, envelope and pitch limits.


Direct piano-roll gestures use UI-owned transient metadata,
not a second editable project. Pointer motion must not enqueue canonical edits or
compile graphs. Release updates onset/gate/pitch together through the existing
prepared worker admission, preserving velocity/channel and one history step.
Revision/generation/owner/view/pending guards invalidate geometry; no-op and
cancellation leave both canonical history and audio untouched. Native body move, both edges, off-roll cancellation, undo and saved exact metadata
pass. Held-Escape/dense acceptance remains open; see midi.md and plan.md.

Session reference contract correction — 8 October: playback ownership must be
per track, allowing Arrangement and Session sources on different tracks together.
A Session launch overrides only its track; clip stop keeps it silent until an
explicit Back to Arrangement operation. A global exclusive playback mode is not
the final parity contract. Sparse no-op and explicit-stop cells remain distinct.
See research/observations/session-routing-001.md for manual provenance, installed
UI evidence and unperformed timing/launch experiments. Inventory-only work is
unaffected; runtime design/aggregate work must include mixed ownership.

Historical Session kernel work — 8 October: an engine-owned headless implementation
will eventually replace/delegate through Playback. It uses monotonic performance
time for ongoing sources and a separate wrapped Arrangement cursor, preserving
the existing native cursor contract. One selected source per track uses shared
prepared mix coefficients, followed by master gain once. Outgoing Session voices
move as Copy spans to a bounded release bank. Carried shared analytic voices
retain at most one `Arc` per unique context; they do not retain the prior playback
graph or allocate one owner per voice. Construction allocates all
route/pending/scratch storage; command/render paths must allocate and drop no
graph ownership. A refused render zeros the entire requested output and leaves
persistent clocks/routes/pending/releases unchanged.

The initial kernel uses explicit Hex test policies: per-track replacement of
pending immediate/four-quarter actions; absent scene cells do nothing. Pause
holds phase; Stop clears pending/releases, resets time and active launch anchors
while keeping owner selection. Seek reanchors selected sources and clears pending
releases as a discontinuity. These are not verified Live semantics. Keep native
transport/publication unchanged until graph/tempo continuity and retirement
acceptance pass; the Session manual's two-Stop rewind behavior remains a separate
parity gap recorded in issues.md.

Periodic Session timing acceptance: control-side inventory preparation derives
loop beat/frame periods using canonical center-inverse endpoints and stores a
typed error when no exact endpoint is representable. Arrangement preparation
remains usable; headless Session construction reports that deferred error.
Sources admit the loop-specific effective rounded rate and reuse compatibility
includes it. Borrowed timing views preserve seam lower-bound minimality and
canonical shared-frame endpoint selection. Engine tests and independent finite
100-track PCM comparisons pass. This changes the headless kernel only; active
tempo-edit/graph continuation and native publication are still pending.

Native prepared integration — 9 October: the optional mixed Session runtime is
constructed on the worker and boxed inside Playback. Finite constructors and
export keep their legacy Arrangement behavior. Native transport status exposes
loop-wrapped position and independent performance_frame; pause holds, Stop
resets and fallible Seek reanchors. Legacy clock exhaustion is explicit and
promotion checks the base beat domain before candidate mutation.

One atomic publication gate admits one queued/deferred candidate. The callback
attempts one transfer per block; release-capacity refusal keeps the candidate
while the old graph renders, permanent rejection returns it through retirement.
Active Session routes/pending/tails cannot silently demote to finite playback.
Priority Stop works during deferral and defensive retirement pressure. Shutdown
returns active/deferred/queued ownership for control-side disposal. Coherent,
bounded rejection snapshots distinguish consumed actions from accepted actions.
UI revision waits resolve on permanent rejection and recovery resets channel
acknowledgment high-water marks. Source tests cover this seam; native interaction,
Session-state device recovery and sustained physical deadlines remain open.

Canonical Session UI — 9 October: the normal editor has a compact Session mode,
paged track columns and virtualized scene rows, independent of the fixed demo.
Shared source/note editing dispatches owner-correct canonical commands. Session
requests are guarded by project generation, revision and audible revision;
launch/start is one atomic PlaybackAction, with typed refusal preserving transport.
Bounded UI feedback tracks callback consumption separately from applied routes.
Ticketed rapid launch/space intent is source accepted; native interaction remains
an open acceptance gate.

`Status::session_routes` now reads a coherent route list into caller-owned
storage in at most three attempts. Ignore that buffer on None. Raw stable IDs
and beat metadata are tagged with accepted audio revision/capability and the
snapshot's performance-frame cut; that cut is not a live transport counter.
Snapshots publish only on accepted revision or committed route-stamp change.
Shutdown publishes an empty revision-0 snapshot once active ownership retires;
historical applied-revision acknowledgment remains separate. Grid active/queued
indicators consume this API only against matching accepted revision and project
generation, with bounded retained candidate views. Ownership labels distinguish
transport state from actual sounding audio, including exhausted one-shots.

## Session audio and project replacement — 9 October

Unwarped Session audio clips retain independent asset/source bounds. Prepared
PCM is shared with Arrangement assets, but Session owns no Arrangement position,
duration, fades or loop phase. Launch uses a performance-frame anchor; tempo
changes do not stretch audio. After the half-open source ends, output is silent
and the track retains Session ownership until an explicit stop/return/relaunch.
Pause freezes position; Seek reanchors the selected source at the requested
performance frame; Stop resets clocks/anchors while retaining selected owners.
These are chosen Hex contracts; installed-Live equivalence is unverified.
See [session-audio.md](session-audio.md) for the complete contract and evidence.

Successful Open uses `PublicationPolicy::InstallFreshRuntime` with a freshly
prepared paused/Arrangement-owned graph. Ordinary edits, history, import and
recovery use inheritance. A failed Open preserves current state. Same-generation
candidate coalescing retains fresh installation until queue admission; later
edits inherit behind that publication. Reused IDs in a different file must never
carry the old Session route. Actual retirement-ring backpressure keeps the old
graph owned and priority Stop effective while the new installation waits.
Control-side reclamation frees retired media; callback code never drops its last
graph reference. Source and GPUI event-sequence regressions pass; native
dialog/publication timing, listening and allocator instrumentation remain open.


Prepared scene launch index (9 October): the immutable inventory preserves public
canonical cells/scenes and builds a separate sorted cell-index vector plus scene-ID
ranges on the control side with fallible reservations. Launch finds a range and
visits only its cells in canonical relative order; empty scenes are valid no-ops.
The callback dereferences index positions into canonical cells without allocation
or sorting. Runtime rollback/ownership rules are unchanged. Reordered/interleaved/
empty tests and 100-track PCM regressions pass; measured dense-grid timings and
memory overhead are in performance.md. Native deadlines remain unverified.

## edit-position playback restart — 9 October 2026

The canonical editor's Space action pauses active playback. Starting again uses
the normalized time-selection start, otherwise the generation-valid clicked edit
cursor, otherwise frame zero. It does not resume from the paused audio playhead.
`PlaybackCommand::PlayFrom(frame)` performs seek/reset and start in one existing
fixed-size queue action and ticket. Prepared Session playback starts only after
seek succeeds; legacy Arrangement uses the same position fields as Seek. Priority
Stop and bounded admission/drain remain unchanged. Pending restart acknowledgments
are tracked as seeks; no compilation, decoding or callback allocation is added.

Focused tests cover legacy/prepared restart, seek refusal, single-ticket position,
Stop cutoff, a zero-allocation/deallocation callback fixture, and real Space routing
for reverse time ranges, point cursors, no cursor, pause/restart and MIDI focus.
Targeted strict Clippy, formatting and source guards pass. These are source gates;
native audible start timing and interaction remain user acceptance. Check Space
from a clicked point and reverse time selection, pause then restart, and clear
selection/cursor to confirm project-start behavior.

Native review, 9 October 2026: the user reported all other issues in the combined
`hex-editing-menu-20261009.app` build look good and authorized merging PRs #12–#16.
Those PRs are merged. This records acceptance of the reviewed editing/menu
behavior, not physical timing measurements or exhaustive listening. Dragging time
selections beyond visible lanes/window edges is a separately reported defect and
the active follow-up.

### retained return effect output

A revision that changes or removes an audible nonquiescent Return captures the
complete old native processor chain into a separately preallocated64-slot bank.
It receives zero input once per256-frame render chunk and feeds the current Main
before the current master stage, using the old bus matrix/gain; old sends are not
replayed. Exact unchanged chains inherit once under candidate bus controls.
Transfer planning shares the existing track-tail ceilings (256 stateful
processors,2MiB paired payload,4096 entries and a checked finite10-second
horizon) and completes before any owner moves. Exhaustion is retryable and
preserves the previous runtime. Failed render blocks keep committed history;
Stop/Seek reset tails and Pause freezes them. Quiescent owners stay in their
Playback for control reclamation. See [the contract](return-effect-tails.md).
Focused PCM/boundary/rollback and callback allocation checks pass; full local CI
and native listening remain pending at this source checkpoint.

## mutable source-loop admission — 10 October 2026

Interactive SessionPlayback admits prepared mutable Session source loops and
finite Arrangement source loops through the existing bounded template queries
and occurrence ledger. Source iteration and transport pass remain separate;
authored notes before source start are not chased, intro-only notes do not repeat,
and held gates end at the exclusive source loop boundary. Source-loop counter
exhaustion and query failures retain transactional DSP/ledger/clock rollback.
Enabled Arrangement transport loops use a separate control-side density and
wrap-chase admission path; see [mutable-transport-loops.md](mutable-transport-loops.md).
Mutable Arrangement source-loop linear export remains refused. See
[mutable-source-loops.md](mutable-source-loops.md) for source-loop evidence and
remaining native validation.


## mutable transport-loop admission — 10 October 2026

Interactive Arrangement playback proves bounded onset, event-edge and voice
density before admitting a mutable instrument to an enabled transport loop.
The first traversal continues authored gates without a second attack at loop
start. Actual wraps create distinct checked occurrences; pre-loop notes whose
gates cross loop start receive a fresh attack with the remaining authored gate,
clamped to the exclusive loop end. Device feedback and effect history continue
across wraps. The loop pass, mutable state and occurrence ledger commit with the
whole render block; failed blocks are silent and preserve the prior state.

Changing loop bounds during an active Arrangement mutable gate is refused at
handoff. Prepared seek/preroll still refuses enabled loops because it models a
monotonic traversal; transport-loop admission does not change that policy.
See [the focused contract](mutable-transport-loops.md). The144 SessionPlayback tests (3 ignored), strict engine/audio Clippy and actual
wrap callback allocation probe pass. Full local CI and native acceptance remain
pending at this checkpoint. Pulse2215 owns this work. Pulse2220 separately tracks
the pre-existing finite/source-loop aggregate event and device-capacity proof.
