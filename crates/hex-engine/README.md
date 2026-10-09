# hex-engine

## Purpose

Own musical state, typed playback commands, timing, and deterministic rendering.

## Dependencies

hex-project, hex-devices, and the Rust standard library; no UI/device-stream dependencies.

## Does not own

GPUI, device streams, file dialogs, packaging, UI state, or control networking.

Read ../../AGENTS.md and ../../docs/architecture.md before extending this crate.

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
