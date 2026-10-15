# hex-engine

## Purpose

Own musical state, typed playback commands, timing, and deterministic rendering.

## Dependencies

hex-project, hex-devices, hex-lattice, hex-lattice-contract and the Rust standard
library; no UI/device-stream dependencies.

## Does not own

GPUI, device streams, file dialogs, packaging, UI state, or control networking.

Read ../../AGENTS.md and ../../docs/architecture.md before extending this crate.

Canonical native effect chains resolve through a borrowed validated registry.
Device implementations own codecs and preparation; the engine composes admitted
memoryless matrices in chain order on control. `PreparedTrackMix` shares the
result across audio/MIDI Arrangement and Session paths, before track and master
mixing. Unknown types, future state versions and invalid payloads refuse
preparation even when bypassed. The default registry admits Utility and the
stateful registered eq eight processor;
`compile::project_with_registry` also supports independently registered test
definitions. Stateful devices and instrument replacement remain separate work.
See [lattice](../../docs/lattice.md).

SessionPlayback preserves detached Arrangement track effect suffixes while their
committed state can still produce output. It feeds them zero input, keeps the
captured old matrix and track mix, and retires them only after a committed
successful block reports quiescence. Unchanged prepared chains transfer their
history once instead of spawning a duplicate suffix. Finite tail bounds above
ten seconds at the prepared rate and aggregate retention beyond 4,096 suffix
entries are refused before runtime ownership moves; unbounded processors are
retained until observed quiescence. Return-bus changes retain the whole old
chain while its committed state can still produce audible output. The old bus
is zero-fed once per render chunk and contributes through its captured matrix,
gain and mute state to the candidate's current Main before the current master
stage. It does not receive old sends or feed another Return. Exact unchanged
chains inherit history once; changed candidate chains start fresh. The
preallocated 64-slot Return tail bank shares the bounded Arrangement release
budget. Native publication/deadline acceptance remain open. See
[native effect tails](../../docs/native-effect-tails.md).

The control-side compiler resolves canonical metadata through a prepared media
bank and creates immutable playback snapshots. The explicit
`compile::project_with_prepared_audio_sources` entry accepts one mixed bank of
`PreparedAudioSource::Resident` PCM or `Streamed` verified catalog metadata for
Arrangement compilation. `PreparedRevision::prepare_with_prepared_audio_sources`
uses that same resolver to prepare Arrangement and Session inventory together.
Referenced streamed bindings must match the expected nonzero project-open
generation and canonical asset, rate, frame-count and conversion facts. Unused
retained entries remain legal. Catalog owners verify fingerprints; the engine
performs no IO.

The headless Session kernel can play an independently stored streamed audio
one-shot through a borrowed `MediaPages` provider. It reads project-rate source
frames from the clip's half-open source window, requests missing pages, and
continues other tracks and the performance clock while a page is unavailable.
`Playback::render_with_pages` preserves Arrangement loop segmentation and uses
the same Session route kernel. Providerless rendering refuses a nonempty playing
streamed project before changing output or runtime state. This is an engine
rendering contract only: native catalog admission, prewarming, and publication
are not wired here. Arrangement export remains resident and finite.

`SessionPlayback::prepare_arrangement_preroll` builds a fresh Arrangement-only
candidate and advances it from frame zero in control-side blocks of at most 256
frames. Callers provide an explicit replay-frame budget, bounded by 8,388,608
frames; candidate-owned runtime storage, including the fixed render scratch, is
preflighted and checked against a 32 MiB accounting ceiling. Cancellation is
checked before construction, at each block boundary and before return. The candidate preserves the exact immutable
prepared revision and the source playback is untouched. Enabled Arrangement
transport loops are refused. This API does not inspect live route ownership,
restore Session routes, or establish native seek/publication readiness; live
route fencing and supersede/Stop admission remain a separate integration layer.
Interactive Session preroll uses the same bounded replay contract with the
dedicated seek worker's 8 MiB control stack reservation. The opt-in
[`playback stack probe`](../../docs/playback-stack.md) records isolated
debug/release completion thresholds for Utility and dense mutable-instrument
fixtures; it measures requested-stack completion, not peak stack residency.
Prepared Playback honors the
compiled half-open loop region with bounded spans; Renderer and CLI export remain
linear arrangement renders. Compilation never runs inside a callback.
Compilation prepares track balance coefficients and validates project master
gain. Renderer and Playback share the same stereo balance and post-mix linear
master stage without clipping; center balance and unity master preserve the
existing default render. The original Hex balance policy is left gain
`1 - max(balance, 0)` and right gain `1 + min(balance, 0)`, with no crossfeed.
This has not been measured against Live's pan law.

`Project::tracks()` exposes borrowed compiled track sources in project order.
`Track::render_additive(start_frame, output)` renders only that finite
Arrangement track into an existing caller-owned mix, preserving per-clip and
per-voice channel multiplication before summation. A caller can select one
borrowed source per track without rendering and subtracting Arrangement output.
`PreparedTrackMix` shares validated channel coefficients and frame accumulation
with other source paths; `Project::master_mix()` supplies the validated final
post-sum stage. The master stage must be applied once after selected tracks have
been accumulated. These helpers do not add Session activation, track override or
audio-thread state.

`PreparedMidiInstrumentTemplate::render_additive` queries at most 256 frames
into reusable `MidiSpanBuffer` storage before touching the destination, then
adds each individual mono voice through a borrowed `MidiInstrumentRenderContext`
and `PreparedTrackMix`. It does not clear the destination or apply master gain;
the caller composes source contributions first and applies the master stage once.
Muted mixes still validate timing and fill/clear the span query result normally,
but add no samples. Failed queries leave destination samples unchanged and clear
the visible span count. The existing standalone `render` retains its overwrite
and silence-on-error contract. These are stateless control-prepared source
helpers, not Session routing or callback deadline evidence.

The compiler also attaches each canonical clip's fixed-domain linear fade.
Zero-length fades use the original render loop without per-frame envelope work;
faded clips multiply their two endpoint-defined ramps in timeline phase. Splits
preserve phase through the canonical offset, including under source and global
looping. This original Hex law is not verified against Live.
Loop clips also compile their canonical source-relative phase. Source lookup
uses modular frame arithmetic so long timeline positions do not overflow, while
existing constructors continue to start at phase zero.
Source phase and non-neutral fades share optional control-owned metadata rather
than enlarging every compiled clip. Neutral clips allocate no metadata; active
spans resolve it once, with a dedicated zero-phase sample path. Builders may
allocate or reclaim metadata and therefore belong off the audio thread. Render
does neither. Phase reset preserves an existing fade and releases phase-only
metadata on the control side.
Playback can render one stateless source period and repeat it in the caller's
output buffer with bounded doubling copies. This preserves current frame values
and phase without heap storage. The original stateless sine instrument is safe
with that path because phase and its linear attack/release envelope derive from
absolute note-relative frames; this does not establish reset behavior for future
stateful devices.

`midi_compile::midi_events` is a bounded control-side compiler that projects
source-local MIDI notes through finite clip offsets and optional local loops,
then resolves note-on and note-off edges independently through the canonical
tempo map. It counts expanded occurrences project-wide against a 65,536 cap
before reserving buffers. Events sort by frame, note-off before note-on, then
track/clip/note/loop iteration order.
The PCM compiler also prepares immutable frame-anchored note spans and a
balanced max-end interval index for each hybrid track with MIDI content. It rejects the whole
compile if more than 64 notes, including release tails, sound at once, or more
than 128 onsets fall in any half-open 256-frame window. Across a query chunk,
the match bound is 64 spans active at its first frame plus 128 starts, or 192.
The project note cap bounds a tree at 65,536 nodes; each query therefore visits
at most 65,536 nodes, with max-end pruning, and uses a fixed 64-entry traversal
stack. In-order traversal preserves onset/clip/note summation order without a
callback match buffer.

`hex-devices` prepares one shared 2,048-entry sine wavetable and Q0.64 phase
steps off the render path. Each voice uses linear 5 ms attack and 50 ms release,
velocity-scaled 0.1 gain, and mono-to-stereo output through the track's existing
balance coefficients. Frequencies at or above Nyquist are rejected. The sampler
uses absolute note-relative frame positions, so block partitioning, seeking,
pause, stop, graph replacement, and global-loop repetition do not need mutable
voice state. `Project::audible_end_frame()` reports the exclusive maximum across
audio clips, MIDI clip timeline ends (including empty clips), and release tails.

For immutable instruments with configuration-heavy sampling, `PreparedRevision`
may also own one shared analytic context table used by both Arrangement and
Session note banks. Compact per-note payloads resolve a checked table index at
sampling; copied old-law releases either remap to a compatible candidate owner
or use a bounded retained-context lease. Revisions reuse an owner only when
stable device identity, complete state, instrument type and sample rate match.
Lease admission is preflighted before graph handoff, and final context drops
remain attached to graphs for control-side reclamation. The built-in Sine and
FM4 samplers keep their existing inline representation. See
[`docs/lattice-instruments.md`](../../docs/lattice-instruments.md) for bounds
and the specific callback allocation/destructor evidence.

`examples/profile_instrument.rs` measures the shared renderer in release mode
with 100 tracks, 16 notes per track, and 128/512-frame blocks. It is a CPU
microbenchmark, not device-deadline, audible, or DAW-compatibility evidence.

`examples/profile_session_audio.rs` exercises a canonical 100-track Session
audio fixture at 48 kHz with 128-frame render blocks, two independent finite
one-shots per track across two scenes, Arrangement companions, and a Stop scene.
Run it in release mode with `cargo run --release -p hex-engine --example profile_session_audio`
for one shared stereo asset, or add `-- --distinct-assets` for 100 separate assets. It verifies direct-frame
finite-source PCM across irregular partitions and source ends, Stop and
Arrangement return, and source-window plus tempo handoff. It reports cold
preparation, full prepared-revision gain/source edits, route command actions,
active source-window rendering, and exhausted rendering. Setup, oracle checks,
launch/retrigger, and PCM/graph drops stay outside render timers. Host metadata
must be recorded externally for each run; this profile is offline CPU evidence,
not callback, device-deadline, UI, plug-in, audible, or Live-compatibility
acceptance.

`examples/profile_session_grid.rs` isolates stored-cell scaling with 256 audio
tracks and two versus 256 scenes (512 versus 65,536 cells), using track-major
cell order and shared original short PCM. Run `cargo run --release -p hex-engine
--example profile_session_grid`. It verifies exact Audio/Stop/Audio PCM before
timing single launches and bursts of 32 direct actions, advancing playback outside
timers. Fixture construction, revision preparation, rendering and final drops are
excluded. The burst models the native action-count bound without exercising the
callback queue. It does not measure near-limit audio/MIDI clip counts, voice
capacity, plugin chains or device deadlines; retain host metadata and outliers.

Both MIDI edge and instrument-span compilation consume the same validated
hex-project source projection. Whole-project admission checks a separate 65,536
prepared-occurrence cap before event/span allocation; absolute onset and gate end
are tempo-converted independently. Canonical clips derive validated one-shot or finite-loop source mappings from
stored metadata; editor source controls/native loop acceptance remain pending.
Finite Arrangement projection and ongoing Session templates remain distinct.

`compile::project_with_midi_preview` returns the same playback graph together
with a sorted immutable clip index derived while instrument spans are emitted.
Each MIDI clip summary includes exact Arrangement frame bounds, authored and
admitted occurrence counts, and at most 32 evenly spaced exact onset/gate/pitch
samples. Empty clips are represented. The project-wide occurrence cap also
bounds total preview samples to 65,536; `MidiArrangementPreview::get` uses a
binary search by stable clip ID, so painting needs no source-note or tempo-map
scan.

`PreparedTempoMap` moves the canonical `hex-project::TempoMap` into immutable
playback-owned state, checks its expected sample rate and the 1,024-point cap,
and delegates conversions to the canonical checked integer implementation. This
includes the first-beat-at-or-after-frame lower bound and its
`u64::MAX + 1` no-beat sentinel; the query is different from
`frame_to_beat`'s latest quantized-boundary choice. Conversions borrow the map
and return typed, static-text errors; they add no callback-owned `Arc`
ownership. Construct and retire the graph off callback, and use conversion
only at bounded event or block boundaries, never per sample.

`PreparedMidiSourceTemplate` snapshots one canonical MIDI clip's source-local
intro and repeat edges for ongoing playback. It deliberately ignores the finite
Arrangement duration: one-shots stop at source end and loops repeat indefinitely.
Preparation uses fallible control-side allocation and admits density against the
fastest tempo in the prepared map, checking intro, cyclic repeat, and the
first-pass seam for both 256 total edges and 128 onsets in the conservative
258-frame beat horizon. The seam check sums the final intro window and the
repeat pattern's first tail window, so it can reject some otherwise playable
templates. The one-frame gate/period proof subtracts the full 1,024-Q32 prefix
truncation allowance. It may refuse an exact one-frame constant-tempo interval
because the proof cannot guarantee it, even when direct conversion would produce
one frame.

Queries accept a launch beat and a half-open frame window of at most 256 frames,
and fill a reusable fixed `MidiEdgeBuffer` with at most 256 events/128 onsets.
They derive beat candidates with the canonical lower bound, binary-search the
sorted intro/phase arrays, and calculate repeat iterations arithmetically. The
beat horizon is at most 258 frames; the admitted one-frame period floor permits
at most 260 overlapping period windows, checked before entering the loop. A
period endpoint note-off retains its owning iteration; simultaneous edges sort
off before on and then by stable track/clip/note/iteration identity. The query
borrows the prepared map, performs no allocation or historical loop replay, and
resets the visible buffer length on failure. The template itself does not define
transport, launch quantization, callback policy or project persistence; the
separate `SessionPlayback` harness below defines only a limited headless policy.
`cargo run --release -p hex-engine --example profile_midi_template` benchmarks
repeated 256-frame queries against a prepared dense 128-note source, with all
project/template/buffer setup outside the measured loop. It reports query count,
maximum edge operations, and elapsed time; this is not callback deadline or
allocation-instrumentation evidence.

`PreparedMidiInstrumentTemplate` derives source-relative gate spans during the
same control-side note projection and prepares the existing `hex-devices`
`PreparedVoice` values once. Its balanced interval indexes answer stateless
gate/release overlap queries into a caller-owned 192-slot `MidiSpanBuffer`
(64 voices sounding at the window start plus 128 admitted onsets). The index
has at most 65,536 nodes, a fixed 64-entry traversal stack and a 16,384-visit
query guard; periodic span queries also have a separate 326-window guard. The
query lower-bounds `[start_frame, end_frame)` directly and indexes gate ends
extended by release time. Admission proves at most 192 interval hits; with at
most 327 balanced-tree queries of height 17, the interval reporter charges one
root-to-hit path per raw interval hit plus two boundary paths per tree:
`17 * (192 + 2 * 327) = 14,382`, below the guard. The 192-hit limit applies
before exact frame filtering: at most 64 intervals are active at the lower
bound and at most 128 admitted onsets begin within the 258-frame raw window.
To keep the period bound mechanically valid, a nonempty repeating source must have a period long
enough for the admitted tempo and the 50 ms release horizon; empty repeat
sections skip period traversal. The separate period guard can refuse short
periods, while the 64-voice cap usually rejects those first. The interval query
uses half-open source gates,
retains an iteration's release tail across seams, and orders spans by absolute
onset/track/clip/note/iteration. The render helper borrows both the prepared
map and sine table, calls the existing `SineTable::sample`, and performs no
allocation or voice preparation in the query. It overwrites one source's output
and zeros that output on query error; a graph-level caller can render to
caller-owned scratch, then merge templates in stable order and apply track
balance/master gain. The source's finite Arrangement duration remains
intentionally ignored.

Voice admission counts half-open gate-plus-release intervals in intro, cyclic
repeat, and the finite first-pass tail. Periodic intervals contribute
complete-period overlap plus a half-open cyclic residual arc; coincident
residual endpoints do not add a phantom voice. The first-tail sweep includes
only repeat iterations at or after the first seam and measures their overlap
with intro releases until those releases end. It does not add a bi-infinite
steady-state maximum to the intro peak. This admits a 32-note, four-beat chord
whose first seam and steady-state maxima are both 64, while still refusing
repeat onsets that pile into a live intro tail.

The sweep is finite and mechanically bounded. Intro gates end by the first
seam, so its horizon is at most the admitted raw release width. If
`q = floor(release_raw / period)` is zero, each repeated source note starts at
most once during that horizon, for at most 65,536 occurrences. If `q >= 1`,
each repeat voice contributes at least `q` to periodic baseline overlap;
periodic admission therefore proves `note_count * q <= 64`, and first-tail
starts are bounded by `note_count * (q + 1) <= 128`. The event allocation is
bounded by 65,536 intro ends plus two events per admitted repeat occurrence,
with checked capacity and fallible reservation. Release beat width uses the
admitted fastest tempo, two extra frames for frame rounding and the 1,024-point
Q32 prefix truncation bound. Frame queries use the canonical lower-bound
conversion; the latest raw beat mapping to frame `f` is
`beat_lower_bound_for_frame(f + 1) - 1`, so same-frame quantized onsets remain
distinct. Tiny-domain finite and cyclic-oracle tests check the admission
formula; finite-render comparisons cover first and later seam release tails,
including the 32-note chord across varied partitions. Multi-template mixing,
long-duration stress, and hardware callback deadlines remain unverified.

`cargo run --release -p hex-engine --example profile_midi_spans` measures a
single prepared source and 100 repeated source queries over 256-frame windows.
Setup and warmup are outside the timed section; output values are consumed.
This local microbenchmark is not callback deadline or allocation-instrumentation
evidence.

The native `Playback` wrapper exposes project-order `SessionRouteStatus` entries
without allocation. Its opaque `session_route_state_generation()` wraps on a
committed visible route/reanchor change and is compared only for equality. It
does not advance with ordinary render blocks. `MAX_SESSION_ROUTE_TRACKS` is the
callback route table capacity and mirrors the canonical project track limit.
`PlaybackAction::LaunchAndPlay` accepts only clip and scene launches and applies
Play only after that route request succeeds.
`cargo run --release -p hex-engine --example profile_tempo_conversion` measures
dense prepared event/block queries with the input map and query vectors prepared
before timing. It reports operation counts and elapsed time, not callback deadline
or allocation-instrumentation evidence.

`PreparedSessionInventory` validates and prepares every stored Session MIDI cell
on the control side, including cells that have not been launched. It retains
project-order scenes, sparse typed MIDI/stop cells, track mix metadata, and a
clip-ID-sorted template index; absent cells remain absent. Each source stores an
owning identity token, so unchanged copy-on-write sources can reuse source and
instrument templates only when sample rate, canonical tempo map and stable sine
table also match. Each track's work envelope is the component-wise maximum over
its cells; the inventory envelope sums those per-track maxima. Empty tracks
contribute zero. Voice, onset, span-hit and query/period guard values describe
structural admission ceilings, not measured CPU or callback deadline budgets.

`PreparedRevision` groups the Arrangement graph, Session inventory and MIDI
preview prepared from one candidate before the editor commits it. The worker
retains that exact artifact for restore and constructs Playback from it. Capable
native Playback uses the mixed Session kernel; unavailable optional Session
construction keeps the finite Arrangement graph with an explicit reason.

The hidden `instrument-test-support` feature exposes only a preparation wrapper
for integration tests that inject a lattice instrument registry. It is disabled
in ordinary builds and does not register or construct any proof instrument.

`SessionPlayback` is a separate headless mixed-route kernel over one immutable
`PreparedRevision`. It chooses one borrowed source per project-order track:
finite Arrangement, one prepared Session MIDI clip, or silence. Sparse absent
scene cells leave their track routes unchanged; explicit Stop cells silence a
track. Scene launch updates only represented cells. Four-quarter launch
quantization uses one replaceable pending action per track; while paused,
selection applies immediately without starting transport. Play resumes, Pause
holds both cursors, transport Stop resets both cursors and tails while retaining
selected routes, and Seek clears tails and reanchors selected Session sources.
These are current Hex kernel policies, not verified Live behavior.

The kernel keeps a monotonic Session performance frame separate from the
loop-wrapped Arrangement cursor. Arrangement clips render in loop-split chunks;
Session templates continue on performance time. A control-prepared periodic
clock repeats the canonical tempo segment over the loop beat/frame periods and
uses the canonical lower bound at seams so tail events that quantize to a seam
remain on the preceding cycle. Source templates are admitted against the
periodic loop's effective rounded rate, computed with an integer ceiling from
its Q32 beat period and frame period; changing tempo points outside the loop do
not change repeated Session timing unless a point's rounded frame exactly
matches a loop endpoint. Endpoint center inversion follows the canonical
latest-boundary rule. Inventory reuse checks this admission rate,
so a loop edit that tightens the rate rebuilds affected source templates. If
loop endpoints cannot be represented or their derived periods overflow, the
Arrangement revision remains preparable with a retained typed Session timing
error, while Session kernel construction refuses. Native publication now uses
this policy; its coupling to Arrangement loops remains unverified against Live.

Route transitions copy active sine voices into a preallocated release bank.
Held notes are cut at the transition frame and use the ordinary release ramp;
already-releasing voices keep their original exclusive end. The shared
release/continuation capacity is `MAX_TRACKS * 64`, including removed-track
tails, and is reserved at construction. Both `release_storage_capacity` and
`release_voice_capacity` report this shared Hex policy cap. Route transitions
and inherited tails consume the same capacity, so a smaller candidate graph
cannot strand copied tails or reject unrelated routing solely because it has
fewer MIDI-bearing tracks. Commands that cannot fit fail transactionally and can
be retried after rendering advances beyond expired releases. Entries are grouped
by track with a monotonic capture order, so each track mixes outgoing tails
before its selected source without rescanning the whole bank or changing
same-track floating-point accumulation order.

`SessionPlayback::try_inherit_state_from` reconciles a fresh runtime with an
accepted predecessor without cloning graph ownership or allocating. It preserves
stable-track routes, pending musical targets, both cursors and transport state;
pending targets whose track or clip disappeared are canceled in the bounded
`SessionHandoffReport`. A changed timing map reanchors the effective Session beat
at the current performance frame. Selected Session notes strictly before that
beat are not chased; an onset exactly at the cut remains eligible. Active
Session voices are copied with their phase, held gates are remapped, already
releasing gates retain their old end, and matching occurrences are suppressed
from the replacement template until the copied voice ends. Deleted tracks keep
copied tails with their old track mix. Gain-only and reorder-only revisions keep
the existing active source path without a recapture. Arrangement MIDI remains
frame-anchored and does not acquire a new revision-continuity policy here.
An inherited quantized request already due at the cut keeps that frame while
retaining its original requested beat for status; Session source phase is then
anchored to the effective beat at the actual application frame. This handles
shared-frame quantization plateaus as a Hex policy, not a verified Live rule.
The test-only mutable instrument path also moves a live runtime pair and its
occurrence ledger into a bounded old-revision tail group. That group keeps its
old effect suffix and track mix through gate/release completion; a disabled or
removed candidate instrument sends NoteOff at the cut. Tail exhaustion is
preflighted with the shared release-bank and declared runtime limits. Stop
resets active and retained mutable state, while Seek refuses a nonquiescent
mutable tail. Effect processors do not yet declare residual-tail quiescence.
`playback::Playback` keeps the legacy finite Arrangement constructors and export
behavior. Its explicit prepared-with-Session constructor opts into the headless
Session kernel; all-Arrangement PCM parity is tested across loop boundaries,
tempo maps and partitions. A prepared revision that cannot construct the
optional Session runtime can still retain and render its Arrangement graph with
an explicit capability reason. Session action and revision-handoff errors have
compact callback-safe status categories; their original typed errors remain on
the engine control API.

Rendering accepts any caller block and processes it in at most 256-frame
chunks, splitting again at pending route frames and Arrangement loop wraps.
Route, release-bank and clock changes commit only after the whole requested
block succeeds. A later-chunk refusal zeros the full output and leaves committed
runtime state unchanged. Route storage and the bounded release bank are
fallibly reserved at construction; the kernel borrows the exact revision's
track coefficients, timing map, sine table, source templates and master stage.
The callback integration uses bounded action/publication queues and returns
retired graphs for control-side destruction. It does not establish device
deadline performance or Live parity. Arrangement export remains finite and does
not activate Session routes.

For the additive shared-context sampling comparison, run
`cargo run --release -p hex-engine --features instrument-test-support --example profile_shared_context`.
It prepares an injected 100-track/1,600-note fixture, checks inline/shared PCM
outside timing, and reports render versus control construction separately.
It is a headless benchmark, not a native callback deadline test.

Canonical selected TrackId/ReturnId outputs are available through control-built
`OutputCapturePlan` and borrowed `OutputCapture` slices, transactionally committed
with Main. See [the capture contract](../../docs/output-capture.md) for admitted
Arrangement-only modes, original retired-tail identity, bounds and export limits.

## Opt-in analytic parameter continuation

Prepared analytic providers may approve bounded coefficient continuation between
compatible contexts. Current-owner notes and detached tails have explicit binding
provenance; detachment snapshots the finite transition and retains its target
context lease. Rapid edits start at the actual applied boundary, preserve onset
and gate, and preflight before ownership mutation. Unsupported providers retain
the existing old-context policy. Two control-allocated fixed `MAX_TRACKS` tables
are charged to preroll accounting; handoff/render never resize or drop their
allocations. Stop and accepted seek clear the current transition. Arrangement,
Session and output capture use the same sampled frame. See
[contract and validation gaps](../../docs/operator-enable-continuation.md).

`SessionPlayback` and `playback::Playback` expose the combined borrowed
`render_with_pages_and_post_node_observer<P: MediaPages + ?Sized>` path. It uses
the existing Session render transaction with both inputs; page and observer
borrows are never retained. Observer code must not allocate, block, clone owners
or retain sample slices. Post-node samples are speculative until both block
render and caller final-output validation succeed. With no Session runtime,
Playback's new observed path refuses before output/clock/provider/state mutation;
with no observer it preserves existing page rendering. Existing public methods
retain their contracts. This adds no stream dependency or native admission.

Canonical stereo Return feeds use a bounded control-prepared topological plan
with pinned ready ties in Arrangement and prepared Session/export kernels.
Post-chain/matrix/gain/mute source output feeds destination input additively;
current Main sums stay pinned, master runs once, and captured old chains remain
Main-only. The no-feed loops keep their original arithmetic order. See
[routing](../../docs/routing.md); this adds no latency compensation or native
deadline/listening evidence.

Prepared track-gain lanes apply absolute sample-offset gain once after devices
and before Main/sends in Arrangement, Session and export. No-automation paths
remain; detached tails freeze the effective transition gain. Actual prepared knot
capacity is reported separately from process memory and native deadline evidence.
See [track gain automation](../../docs/track-gain-automation.md).

The new opt-in `OwnedAudioInputs` bank owns every explicit caller alias; callers
receive nonowning tokens and borrowed leaf views. Its typed input-storage result
reports constructor-declared data, actual bank/ledger backing capacities and
unresolved independent handle obligations. This is input-bank accounting only;
it neither filters generic report Unknowns nor permits publication admission.

`AudioPreparationCustody` implements only the Arrangement source-binding compiler
stage. Before any source clone, it validates supported fields, asset/rate/region
bindings, checked identities, destination and handle capacities, and declared
reservation arithmetic. Each construction alias is installed directly into its
persistent exact track/clip slot before later work. It produces no Session
inventory, prepared revision or renderable runtime. Legacy compilers are separate.
The host must retain custody: native audio uses the existing single pending slot
and cleanup extraction capsule, not a UI-owned request. Cancellation is scalar;
actual partial-owner disposal runs outside the ordinary root mutex. Original
bank recovery requires normal disposal of every construction alias. Panic pins
the bank, remaining aliases, payload and tombstone without retry. Abandoned bank
recovery after terminal close remains pending and backpressures that fixed slot;
it is not automatic cleanup success. Data-capacity declarations are not allocator
bookkeeping/RSS or proof of whole-graph, domain or lifetime completeness.

`AudioArrangementInput::with_session_sources` explicitly adds Session audio
source/name construction in that same custody. It borrows and validates every
audio cell's track/scene/asset/rate/interval before bank transfer, reserves the
actual cache/cell/name capacities, then installs independently recorded cache
and clip aliases into persistent destinations. Repeated cells reuse the current
bank's private cache allocation identity, with a separate obligation for every
physical handle. Names copy exactly into pre-reserved persistent Strings; no
temporary source map, path/name clone or raw previous inventory is adopted.
Legacy previous-inventory source aliases remain untracked and are not inputs to
this stage. Every source/name/cached-alias disposal is pending before destruction
and normally complete before original-bank recovery. Metadata/data charges stay
retained through cleanup; no lifetime-closed fact is emitted. This is still a
source/name compiler stage, not complete scene/Stop/index/preview/tempo/template
inventory, PreparedRevision accounting, or a renderable Session publication.

The same opt-in custody now also constructs ordinary Session structure: scene
and track names/scalar mix metadata, Audio/Stop cells, track indices, and sorted
scene-cell indices/ranges (including empty scenes). Each Audio cell binds the
private exact source-handle row; identical content or another custodian does not
substitute for that binding. Every vector backing and independent String backing
has a control-borrowed actual owner plus private charge location. Capacity reports
use owning private element sizes, actual returned capacities and checked growth
overlap; inline headers are included only in their parent storage. These are
unresolved exclusive-allocation observations, not global admission/retirement
authority. A failed visitor requires discarding its partial sink. Names construct
directly in persistent fields, become Pending before disposal, and are normally
disposed before original-bank recovery; panic retains remaining owners/payload
without retry. Empty retained backings remain charged on subsequent requests.
The same custody now reserves both canonical tempo-copy backing destinations
before copying exact validated points and Q32 prefixes into persistent fields.
Borrowed prepared-tempo validation happens before ownership transfer; no consumed
canonical map is passed to a fallible legacy constructor. A vacant private engine
slot receives the buffers by fixed moves after the byte/overlap checks. Its two
unresolved allocation obligations change owning location without a duplicate data
charge. Control-borrowed owner visitation binds actual custody and field location;
failed partial sinks must be discarded. Cancellation returns buffers to explicit
copy custody and clears only scalar contents, retaining and charging spare backing
capacity even across later Arrangement-only requests. Panic retains partial
buffers and the first payload without retry or original-bank consumption.
This remains preparation-only: no complete inventory or publication is produced.
Complete prepared inventory assembly, cross-ledger/runtime accounting and lifetime
closure remain unfinished. Existing legacy constructors and Unknown/refusal behavior are intact.

The opt-in Session stage also creates the mandatory sine table and strictly empty
shared-context table inside persistent custody. The audited sine payload is only
its fixed inline sample array; the owning context module exhaustively refuses
nonzero payload, nested handles, or even empty spare Vec backing. Only these
engine-created constructors use the existing pinned requested-layout declarations;
raw/legacy Arcs and nonempty provider contexts remain Unknown. No provider trait
or DSP behavior changes. Before construction, three fixed physical-handle slots
and checked identities are reserved: Session sine and separate Arrangement/Session
context handles. The two context handles share the actual allocation by
Arc::ptr_eq, with equal declarations, so data is charged once but both obligations
remain. This is not a refcount-based ledger. All handle/receipt/slot headers are
part of the containing custody size; outer Arc payloads include their own headers
and inline fields, with declared Arc overhead separate. Existing caller byte and
construction-overlap checks precede bank moves and the context alias; no hidden
shared owner leaves the custodian. Every owner release is Pending before take,
outside the ordinary root mutex, and normal cleanup precedes bank recovery.
Panic keeps remaining owners, original bank and first payload without retry.
Positive fixtures explicitly reserve the extra actual handles; the former
eight-handle declaration still refuses when nine are required. This is caller
reservation pressure, not a changed graph/project/domain/component cap.

These are unresolved storage/handle observations for fixed inventory metadata,
not full inventory assembly, aggregate completeness, callback admission or
retirement closure. Final prepared containers/bindings, graph and
runtime/caller cross-ledger coverage, and lifetime contracts remain unfinished.
The legacy report's Unknown reasons and require_complete refusal are unchanged.

Tempo assembly now installs the retained canonical buffers into a private actual
PreparedTempoMap destination. A private control-only helper validates by borrow
and checks destination vacancy before taking the source; refusal preserves the
original slot/pointers. Its successful commit moves only the canonical owner and
scalar maximum. The preparation stage never uses the legacy consuming fallible
constructor. Periodic Session metadata/error and admission-rate metadata use the
same existing conversion kernel and fallback semantics as the legacy inventory:
failed periodic conversion remains an explicit error with canonical timing,
not a manufactured valid cycle. No transport, callback or clock behavior changes.

Capacity/owner observation distinguishes the copy buffers, temporary canonical
slot and actual prepared-map slot, charging the two data allocations only at their
current owning locations. All wrapper, periodic, error and scalar headers are in
the containing custodian size once; periodic metadata has no heap owner. Actual
normal cancellation returns the original buffers through the persistent canonical
slot to retained copy custody before bank recovery. Failure before installation
retains that slot; panic after installation retains the actual prepared owner,
remaining graph owners, bank and first payload without retry. These observations
do not close either allocation obligation or complete aggregate admission.

`AudioArrangementInput::with_session_inventory` explicitly adds sealed final
Session assembly; `with_session_sources` retains the prior staging behavior.
The existing native pending slot can host either mode. A single assembly
custodian reserves all seven final-vector/pair-record backings before bank moves.
Scene/track targets initially contain only copied scalar metadata and empty
Strings. Final audio order is established before moving any owner; the complete
pair row (clip/track/scene/asset/interval, original source-row/bank location and
conserved physical handle) sorts together. Borrowed batch preflight validates
all destinations and exact bank-backed allocation identities. Subsequent moves
transfer existing names/source owners rather than clone or regenerate handles.
Only bare-Arc fields use a private consuming Arc+declaration split, with both
parts installed in the same sealed custodian before later fault points. The
Session context owner remains OwnedShared. No raw adoption/receipt constructor
or owning error is public; legacy inventory/clip getters and Clone never escape
through the new strict view. Only IDs, names/cell scalars and borrowed streamed
metadata are exposed by SessionInventoryMetadata (compile-fail escape controls).

The actual private PreparedSessionInventory value covers every existing field:
owned prepared/periodic timing, sine/context owners, scene/track/name containers,
canonical sparse cells and scene indices/ranges, sorted paired audio clips,
installed-source streamed flag, and explicitly empty MIDI/mutable vectors with
zero MIDI/mutable work and counters. Unsupported input fields still refuse.
No PreparedRevision/Arrangement root or renderable publication is constructed.
Pair declarations are constructor-layout evidence, never a retirement receipt.

The extra assembly charge is actual final/pair Vec backing plus checked old-growth
construction overlap. Existing source/structure name, source-bank data, tempo and
fixed-shared charge groups follow those SAME allocations into final destinations;
adding their data again would double-charge them. Staging/cache/final/pair backings
all remain charged simultaneously, including vacated spare capacity and inline
headers in the parent size. Source bank/caller/cache handles remain independently
owned. Observations remain partial until the full cross-ledger contract is built.
Cancellation first unpacks the final value into vacant persistent fields outside
the ordinary root mutex, then disposes individual names and sources with Pending
before take. Source and name teardown fields are separate, retaining the latter
if source disposal panics. Exact timing returns to its original copy custodian;
remaining context handles dispose through their existing owner. Original-bank
recovery requires normal disposal of all aliases. Panic pins pairs, remaining
owners, actual payload and tombstone without retry. No callback work, cap change,
new registry, admission enable, whole-issue readiness or native proof is implied.

Remaining convergence: final Arrangement/return/gain/source containers; sealed
PreparedRevision/preview/empty-slice origins and descriptors; exhaustive caller,
bank, graph, runtime and domain cross-ledger accounting; all publication-ending
lifetime closures; connected installed demand/preroll/native lifecycle; optimized
native100-track mixed-rate/edit/seek/launch/fault/CoreAudio/deadline acceptance.

The further explicit `with_audio_clip_inventory()` preparation mode composes
that Session stage with **private final Arrangement clip targets**. It is not a
compiled Track/Project/PreparedRevision and cannot be published. Legacy builders
and the earlier source-only/Session-only opt-ins keep their previous behavior.
The private paired row retains exact original source-slot/track/clip/bank-source
and independent handle identity. Resident PCM receipts move with Audio; streamed
Arc and its constructor declaration move together into the sealed destination.
No raw Clip, Arc, mutable runtime or legacy recovery API is exposed.

The entire destination batch and all required ClipMetadata boxes are constructed
before bank consumption. Streamed clips, phased loops and faded clips receive
preallocated metadata; neutral resident clips keep the compact None case. This
preserves source geometry, loop phase and fixed fade metadata without calling a
legacy consuming-error builder. Final source moves perform no allocation or
cloning. Actual pair Vec capacity (including inline Clip/receipt/handle state),
metadata Box pointees, retained spare pair capacity and old/new growth overlap
are checked in the existing caller prepared/construction reservations. Data
allocations remain charged once by their actual bank identity; physical source
handles remain independent obligations. Allocator bookkeeping/physical rounding
are not measured, and process-fatal OOM/abort remains outside recoverable API
failures, as for the existing constructors.

Control cleanup stages source and metadata into separate persistent fields,
marks destruction Pending before taking each owner, and uses the existing
borrowed-custody catch. Failure retains metadata, remaining clips, bank, first
payload and an irreversible incomplete state, with no destructive retry. Original
bank recovery follows normal final/source/cache/Session cleanup only. Actual
headless native pending-root tests exercise this same opt-in, cancellation and
control recovery; they are not CoreAudio or native streaming acceptance.

Remaining finite assembly/accounting checklist after this bounded clip unit:

- Arrangement Track fields: mixes, gain-envelope knots, sends, empty chain
  constructor provenance, instrument meter owner, and their exact paired final
  locations. No absent instrument or empty chain may hide a meter allocation.
- Arrangement return buses/feed plan, root scalar metadata and shared-context
  handle; persistently prepared gain/return temporaries and capacity overlap.
- Restricted PreparedRevision/root assembly, explicit empty MIDI preview slice
  provenance, root/Session/Arrangement shared allocation and mutable-revision
  metadata. No legacy inventory/clip getter may escape through strict custody.
- Exhaustive cross-ledger actual allocation/location and independent handle
  obligations across caller bank, construction, final graph/runtime/domain and
  reclaim-pending custody; unsupported providers/raw origins/Weak stay Unknown.
- Actual never-installed/final-active lifetime closure, production admission,
  installed engine demand/preroll and real optimized native long-project
  playback/edit/seek/launch/fault/deadline evidence.

Existing numeric caps and graph exclusions are unchanged; this stage does not
mint a complete storage report, lifetime receipt, readiness or admission.

`with_audio_track_inventory()` is the next explicit opt-in: the same retained
preparation custodian now owns private actual Arrangement Tracks. It composes the
previous clip/Session stages; earlier opt-ins and all legacy compiler APIs keep
their behavior. Strict metadata returns only Track IDs, copyable mixes/send
indices/gains and borrowed scalar gain knots. It exposes no Track, Clip,
cloneable owning child, Arc, Deref or mutable runtime.

Every current Track field is explicit. Instruments, state, device processors,
mutable/MIDI providers and ordered matrices are absent/refused for this class.
Mandatory meter ownership is not inferred absent: the lattice owning module
exhaustively declares DeviceMeter's two AtomicU32 fields, separately from its
constructor-paired Arc requested layout and independent handle. Empty chains use
a DISTINCT explicit `Arc::new([T; 0])` allocation followed by safe unsizing,
retaining the same pinned constructor declaration. Legacy raw/default/empty-Vec
Arcs remain Unknown. Requested layout is not allocator rounding/RSS or universal
std ABI. No gate/toolchain/policy is weakened.

All Track rows, per-Track clip/send/pair destinations, gain-knot backing and
control-only source/destination bijection scratch are reserved before source
moves. Gain preparation fills the owning module's persistent Vec directly through
a borrowed helper; partial conversion/fill errors never unwind a local owning
Vec. Legacy preparation uses the same numerical filler and retains its original
public/error behavior. Quantized duplicate frames retain the existing last-knot
law and tempo-marker interpolation. Nonempty sends or automation set source_mix
to identity. Automation uses unity static gain, authored absolute gains and
balance; static gain is not applied twice. Solo/mute and canonical Return send
indices match the existing compiler, without claiming a prepared Return graph.

Exact clip/binding/declaration/handle custody moves into each pre-reserved Track
location together. A unique preallocated destination map rejects duplicate or
missing source associations before any moves. Control cleanup validates the
complete return destination, unpacks into vacant persistent fields, returns exact
clip associations, then disposes meter/chain owners Pending-before-take. Gain
buffers return to persistent copy custody; every spare capacity remains charged.
A panic retains remaining owners, first payload and an incomplete tombstone; no
retry or original-bank recovery until actual normal cleanup. No callback path is
changed or gains a preparation traversal, lock, allocation or owner clone.

Reports include actual row/clip/send/knot/pair/scratch backing, meter and explicit
empty-chain declared payload/header layouts, original staging headers, retained
spare storage and construction growth/caller overlap. Data allocations and
independent owning handles remain different obligations. Immutable graphs remain
outside the existing runtime 32MiB cap; caller prepared/construction reservations
and all other existing caps remain unchanged. Unsupported ownership still refuses
complete accounting/admission.

`with_audio_return_inventory()` further composes private actual Return buses and
the existing ReturnFeedPlan representation into that same retained preparation
custodian. Every bus field is explicit: canonical ID, authored gain/mute, absent
matrix, and one distinct constructor-tracked allocated empty chain. Device-bearing
Returns remain refused. Scalar metadata exposes buses, canonical indices and
feed values only; no ReturnBus, plan, Arc or mutable/cloneable owner escapes.

The new private feed constructor borrows the validated canonical Project without
a snapshot clone or temporary owning graph. It pre-reserves persistent per-source
feed buffers, final outgoing headers, topological order and indegree/visited
scratch. Copies land directly in those fields; local identity/endpoint/gain/pair
checks and deterministic Kahn ordering precede installation. Unique feed sort
keys preserve the legacy ordering without a temporary stable-sort allocation.
All destinations are validated before fixed vector moves into the actual plan.
Legacy Return preparation and propagation kernels are unchanged; tests compare
actual plans and propagation, including backward canonical-index edges, empty,
disconnected and the existing maximum sixteen-Return DAG.

Control cleanup unpacks the actual plan into vacant persistent vector fields,
returns each nested feed backing to its original source row, and clears only
exhaustively scalar leaves. Buses dispose Pending-before-take under the existing
borrowed root catch. Partial copies, partially installed buses, an installed plan
and spare buffers remain accounted through failure/cancellation. Panic retains
remaining owners, first payload and irreversible pending state without retry;
original bank recovery still requires actual normal alias cleanup. Reports add
actual row/order/outgoing/feed/scratch capacities, chain declarations and separate
physical handle obligations, including retained spares and checked old/new growth
overlap. These are stated data-accounting charges, not allocator/RSS measurements.
No callback or publication path changes, caps increase or completeness is implied.

Remaining finite assembly work: restricted Arrangement Project and
PreparedRevision/root construction, explicit empty preview provenance and root
descriptors. Full cross-ledger allocation/handle obligations, all ending lifetime
cases, production admission/readiness/preroll, and optimized native 100-track
mixed-rate/edit/seek/launch/fault/deadline acceptance are still pending.
