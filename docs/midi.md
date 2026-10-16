# canonical midi contract

Status: model, persistence, prepared events and original instrument playback/export
implemented. Native piano-roll creation, parameter edits, history and save/reopen
are verified. Direct body movement and both edge resizes now pass native acceptance.

## ownership and anchors

hex-project owns editable musical data, stable identifiers and history. One ordered
track list retains shared name/mixer/mute/solo controls. The hybrid-track model
now gives each track both audio and MIDI clip collections under one TrackId.
The existing audio `clips()` and `Project::clip()` remain audio-only accessors;
MIDI accessors are explicit. Placement and moves require a valid destination track
without a source-kind capability restriction. There are no parallel top-level
track lists or duplicate mixer identities. Hybrid rendering/persistence and integrated UI/Session source checks pass;
user native hybrid acceptance remains pending. See
[hybrid tracks](hybrid-tracks.md) for the current contract and evidence.

Audio clips retain their existing frame anchors, source phase and fade domains.
MIDI clips have nonnegative Q32.32 quarter-note start and positive beat length.
Source length, source start and retained loop markers are independent of finite
Arrangement length. Defaults are one-shot with source length equal to Arrangement
length, start zero and the full source loop brace disabled. Enabled loops play
from the start marker to loop end, then repeat the loop region. Intro notes play
once; notes outside the playable range remain authored and editable.

Notes have globally unique NoteIds from the existing monotonic project counter,
clip-relative beat start/positive duration, channel 0–15, pitch 0–127 and velocity
1–127. Checked note end must not exceed source length; checked clip end must fit
the beat domain. Same-pitch overlaps are allowed and retain distinct identities.
The whole project caps notes at 65,536 and combined audio/MIDI clips at 16,384.
All IDs, including notes, share snapshot uniqueness/high-water validation.

Typed MIDI commands create tracks/clips/notes, update/remove notes, move/remove
clips and duplicate clips. One success is one history step; failures are atomic
and no-ops preserve redo. Moves preserve local note timing. Duplicates begin at
the source clip's exclusive end and allocate a new clip ID and new IDs for every
note, retaining source settings. `SetMidiClipSource` validates all source/loop/
Arrangement fields and authored notes together before mutation. Source shrink
that invalidates a note is refused. MIDI Arrangement edge resizing uses the policy below; MIDI splitting is implemented with bounded right-region materialization; see
[edit cursor and split](edit-cursor.md) for carry-note and loop-setting limits.

## persistence and prepared events

Schema 11 retains schema-10 tagged tracks and required per-clip source/start/loop
metadata with validated MIDI notes. It also stores independent Session sources,
scenes and sparse MIDI/explicit-stop cells. Schemas 1–10 default the Session grid
empty. Strict schema 9 migrates its one-shot defaults.
Schemas 1–8 explicitly migrate tracks to audio contents,
retaining their IDs, frame intervals, media/fade/phase and tempo/meter maps. Storage
remains strict: unsupported tags, missing/unknown fields, malformed notes, duplicate
IDs and over-limit collections fail rather than silently dropping content.

hex-engine prepares MIDI event edges outside the callback. Add clip and local note
beats with checked arithmetic, convert absolute on/off beats independently through
the canonical tempo map, and reject zero-frame gates after quantization. The event
cap is 131,072, with a separate whole-project prepared-occurrence cap of 65,536.
Both edge and instrument-span compilers reuse the validated source projection;
canonical clips derive their validated one-shot or looped mapping. Admission counts
all occurrences before allocating event/span buffers. Sort by absolute frame,
note-off before note-on, then ordered track, clip ID, note ID and iteration.
No per-sample beat conversion occurs. Prepared edges alone do not produce audio.

## original instrument runtime

hex-devices owns a shared 2,048-entry sine table with interpolation, prepared
Q0.64 oscillator steps, a 5 ms linear attack and 50 ms linear release. Note-off
releases from the current attack level, so short gates are continuous. Velocity
is linear with 0.1 maximum per-voice amplitude; no hidden limiter is applied.
Preparation rejects pitch above MIDI 127, invalid rates/velocity and frequencies
at or above Nyquist. Channel metadata persists; expression/MPE is not yet rendered.

hex-engine resolves note gate and release endpoints on the control worker.
Per track, including release tails, at most 64 notes may overlap and at most 128
onsets may occupy any sliding half-open 256-frame window. Invalid graphs are
refused rather than stealing or dropping voices. An immutable balanced interval
index streams intersections in stable onset/clip/note order with a fixed stack;
render chunks are at most 256 frames with at most 192 intersecting spans. The
source implementation bounds stack depth and whole-project note count; sustained
device deadline acceptance remains open. No callback allocation, table creation,
trigonometric preparation or beat conversion occurs by source inspection.

Phase and envelope derive from absolute frame position. Seek chases notes at the
destination, pause freezes position, Stop returns to frame zero, replacement
inherits transport and global looping repeats the region's same samples. Native
and offline Renderer share synthesis; these policies are original Hex behavior
and do not establish Live parity or future stateful-plugin reset semantics.

Candidate session edits compile before history commits and publish that exact
graph. Failed preparation preserves project, IDs, redo, dirty state and revision.
Open still validates its candidate before replacement. CLI/native export includes
clip endpoints and release tails, and MIDI-only projects need no audio assets.

## next runtime and editing gates

Finite Arrangement loops use admitted prepared occurrences. Ongoing Session
looping still needs bounded templates and tempo-aware scheduling rather than
unbounded repeat materialization. Stateful plugins require
a separate voice/reset contract; analytic sine behavior does not prove it.

Add a compact piano-roll editor only when canonical note commands can prepare
playable audio. Controller data, velocity editing, expression, chance, MPE, MIDI
import/recording/external I/O, tuning and device/plugin routing remain required
fundamental workflows. The public reference in
[Editing MIDI](https://www.ableton.com/en/live-manual/12/editing-midi/) establishes
workflow requirements; it does not establish Hex's numerical or retrigger policies.

Acceptance evidence: device/engine tests cover finite frequency/envelopes, limits,
partition invariance, seeks, pause/Stop/global loops and replacement. Saved mixed
and MIDI-only CLI tests verify audible notes at tempo-crossing frames 24,000–80,000,
release tails and unchanged audio outside MIDI spans. Worker tests verify playable
Open and atomic failed Nyquist edit/Open candidates with usable redo and graph.
Workspace tests pass. An isolated 44.1 kHz native fixture opens, starts Play and
returns to zero on Stop. Native/CLI exports both contain 176,400 stereo PCM16
frames and hash eea5ecf8cb5d996ce02afa155fb77b194a0dae8a33c340babdd53fef4de2c242.
Instrument clips now have native previews and a compact note editor. Native
create/parameter/delete/undo/move/duplicate/save/reopen acceptance is recorded in
docs/plan.md. Listening and sustained physical-device stress are unverified. Legacy audio export retains SHA-256
952c8d29e303b10942d47300381b87b074d75828077e1b759f744b29a2bba681.

Storage's 8 MiB document cap remains separate from the canonical 65,536-note cap;
a sufficiently verbose project may reach the persistence cap earlier. Future
large-note projects need measured metadata/history/save limits, not an assumption
that every maximum-sized canonical snapshot fits the file ceiling.

## direct note gestures

One pointer gesture previews one selected note without modifying the canonical
project, worker queue or audio graph during motion. Body dragging preserves gate
and pointer grab offset, changes local onset using the selected grid and pitch by
key rows. Leading/trailing edge dragging changes gate while retaining the opposite
edge. Clamp to clip bounds and MIDI pitch range; retain channel/velocity. Body motion
retains any positive original gate and authored timing offset.
Resize keeps unchanged metadata for a click/return to the starting pointer cell;
Changed resizes use the selected grid's minimum gate capped by the available
clip span; free mode uses a positive raw-unit minimum. Releasing a changed valid gesture sends one typed update
and creates one undo step; no-op/cancel sends none. Escape cancels a gesture before
transport Stop. Project generation/revision, clip/note ownership, viewport and
pending-work changes must invalidate old geometry. Dense geometry remains bounded.

The public Live 12 MIDI manual sections 10.5.4/10.5.5 establishes body movement and
both edge resizing. Hex's fixed grid, clamp policy and overlapping-note identity
are explicit original contracts; full modifier/selection parity, audition,
left/right keyboard gate editing and Live's note-overwrite behavior remain parity
gaps rather than silently inferred equivalence. Native body move, both edges, off-roll cancellation, undo, saved values and short
off-grid click/move now pass. Native 600-note painting/edit/save, overlap cycling,
zoom/fit/pan and minimum-window checks also pass. Held-preview/Escape, measured
input/paint timing and physical listening remain separate open gates.

The transient piano-roll selector cycles whole-note divisions 1/4, 1/8, 1/16,
1/32 and off. Default 1/16 is a quarter of a quarter-note beat. It controls
insertion, initial gates, parameter nudges and direct gestures. Off retains exact
pointer timing with a quarter-beat insertion/nudge fallback. Selector changes
invalidate a gesture without committing it. The implementation passes grid/domain
unit checks. Native grid authoring, snapped/free movement and both free edges
pass at minimum window size, with independent whole-project saved-value checks.

Hold Cmd on macOS (the platform primary modifier elsewhere) to invert snapping
temporarily for insertion, body dragging and either edge. A fixed selector becomes
free; off temporarily uses 1/16. The selector itself stays unchanged. Dragging uses
one raw pointer-down anchor and the original note; modifier changes recompute the
candidate without rebasing from the previous preview. The release modifier decides
the final candidate. Source tests cover toggles, no-op and opposite-edge preservation;
native insertion inversion passes in both directions. Held-modifier dragging,
mid-drag modifier changes and Live's initial magnetic threshold remain open.

## atomic group edits

The control model exposes clip-scoped `UpdateMidiNotes` and `RemoveMidiNotes` with
nonempty, unique target IDs and the existing 65,536-note maximum. All targets and
replacement fields validate before mutation. Updates preserve note order and IDs;
removal uses stable linear compaction. One changed command is one undo/redo step,
and session compilation precedes history commit. Tests cover whole-project
undo/redo, invalid late records, foreign/missing/duplicate/oversized targets and
actual onset-budget rejection with unchanged project/revision/dirty/redo state.
These commands underpin native multi-selection; they do not add schema fields
or change callback behavior.

## native selection and group movement

The piano roll starts in normal mode. Empty clicks place a local marker; empty
drags select intersecting notes, Shift-click toggles members, Shift-marquee adds
hits and Cmd+A selects the clip. Normal double-click inserts/removes; the compact
B toggle enables draw authoring; continuous strokes are implemented in source
with partial native acceptance, detailed below.
Selection is sorted and Arc-backed; cached Canvas painting avoids note layout
nodes. Dedicated piano focus routes note deletion and movement separately from
Arrangement editing. Group parameter-button integration is the current work
unit; group edge dragging remains deferred. Shared body movement clamps one time/pitch delta across immutable
originals, preserving spacing, gates, channels and velocity. Release commits one
atomic batch; deletion is one batch. Stale context and off-roll release cancel.

Source tests and strict workspace lints pass. Native Shift, marquee, select-all,
Escape, normal/draw insertion and group move/delete pass. Saved-value comparison
confirms only the intended notes change, including nonzero channels and unequal
velocities. Additive marquee, group redo/cancellation, focused arrow routing and
minimum-size layout still need native checks; source checks do not close them.

Native selection follow-up: focused single-note Up/Right passes exact full-project
comparison (`native-midi-selection-keyboard-20261008.hex`). Group no-op and
off-roll release produce an identical project (`native-midi-selection-canceled-20261008.hex`).
Group deletion undo/redo runs as single actions; saved redo removes exactly IDs
6/7 and preserves every other value (`native-midi-selection-redo-20261008.hex`).
Native additive marquee and minimum-size layout remain open. The documented
automation drag has no modifier parameter, so Shift-marquee has source evidence
only. Held-preview Escape remains separate from selection-clearing Escape.

Piano-focused Left/Right now moves a selected group by the current grid movement
step; Up/Down transposes it by one semitone. Pointer and keyboard group edits use
one common bounds resolver and payload builder. Every member preserves its gate,
expression and relative offset; a boundary-clamped zero submits no command.
Native pair movement, exact saved metadata, one-step undo/redo and redo preservation
after a clip-ending all-note no-op pass. Fresh 880×540 group layout also passes.
Continuous drawing now uses the accepted mixed edit transaction; native drawing
acceptance remains partial. Group gate/parameter editing,
Shift-octave keys and broader Live gesture parity remain open.

## selected-group parameter buttons

Selected-group pitch/gate/velocity buttons are now integrated. One uniform
relative delta produces one `UpdateMidiNotes` batch; any member crossing a field
boundary refuses the whole edit. IDs, channel, order and untouched values remain
unchanged. Rendered clip/revision/generation/selection epoch and pending work guard
admission. Single-note buttons and clamped group arrow/body movement retain their
existing behavior. Resolver/capture and whole-project history tests pass within
121 UI tests; primary inspected event wiring. Native saved-value and compact
group-control checks remain pending; see midi-group-parameters-native-001.md.
Expression-lane/keyboard parity is separately tracked in issues.md.

Piano-focused Shift+Up/Down now requests octave transposition for single notes or
selected groups. Shift+Left/Right also adjusts group gates through the same
transaction used by the compact buttons. Existing Arrangement trim behavior is
retained. Group octave changes use the existing common pitch-boundary clamp,
preserving intervals; blocked zero deltas submit no command. Source tests include
offset/gate/expression preservation, common bounds and one-step history with
unselected notes unchanged. 124 UI tests pass, one existing benchmark ignored;
native keyboard delivery and measured Live boundary behavior remain unverified.

## atomic batch insertion

`AddMidiNotes` accepts nonempty `MidiNoteInsert` records within one existing
instrument clip. Input order, gates, channel and velocity remain exact. The batch
and resulting project-wide count are capped at MAX_NOTES; all fields and allocator
range validate before append, and storage reservation is fallible. Created IDs
form an inclusive contiguous range returned with the clip. Successful insertion
is one history step; undo retains allocated-ID high-water, preventing reuse.
The session prepares the candidate before history commit, preserving revision,
dirty state, redo, allocator and working playback on admission failure. This
control API has no new schema/callback behavior and does not yet expose native
stroke authoring. Mixed insert/remove edits remain a prerequisite for full drawing.

Insertion source acceptance: ten project MIDI and five session admission tests
pass, alongside strict workspace Clippy/build/fmt/guards. Actual service evidence
covers valid spaced insertion and simultaneous-onset rejection without replacing
playback or changing canonical project/revision/dirty/redo/allocator. Native stroke
authoring remains unimplemented.

## mixed insertion and removal

`EditMidiNotes` combines unique clip-owned removal IDs with new note records.
Either list may be empty; both empty rejects. Shared midi_batch admission checks
all fields, final project-wide count and allocator range before compaction/append.
Reservation accounts for final size, so a full-cap replacement can reuse space
freed by removal. Untouched order/IDs remain exact and new notes append in input
order. Erase-only consumes no IDs and returns no created range. One successful
edit is one history step; actual service tests confirm compile refusal restores
all canonical state and does not publish replacement playback. Thirteen project
MIDI and six session tests, strict workspace lints, build, fmt and guards pass.
This does not yet implement pointer strokes, pitch-lock or backtracking.

## continuous draw strokes

Draw mode now captures an immutable bounded baseline on pointer-down and previews
insertions/erasures locally through a shared Arc. Motion never submits canonical
commands. An inside release with unchanged context sends one EditMidiNotes,
with inserted IDs selected only if the created clip is still selected. Normal
mode retains its marker, selection, double-click and direct edit behavior.

Pitch lock defaults on; the compact l/m control toggles locked/melodic drawing.
Option inverses pitch lock and Cmd inverses the selected grid at pointer-down.
Changing either modifier mid-stroke cancels rather than committing a different
interpretation. This transition policy has not been measured against Live.
Fixed anchors clamp to the final valid grid line and gates clip to available
length. Off starts exactly at the raw pointer beat and uses a quarter-beat lattice
anchored there; that spacing is original Hex policy pending reference evidence.
The actual scene hit controls first-click deletion, including short off-grid
notes; an empty down inserts. Subsequent sweep cells choose the lowest baseline
NoteId among overlaps and deduplicate removals. This overlap policy is unverified
against Live. Baseline erasure never toggles against a mutated local preview.

Locked backtracking retracts the provisional active path, including removals;
melodic movement traverses crossed time/pitch cells. Dwell/revisits deduplicate.
Interpolation is deterministic for each sampled segment, with tested horizontal,
vertical, negative and lattice-aligned cases; integer pitch/cell sampling does
not establish universal event-density invariance. Work is capped at 65,536
traversed cells and 1,048,576 baseline probes per stroke; overflow cancels the
whole stroke, never commits a truncated prefix. Baseline/index/preview allocations
are control-side with fallible vector/map reservations. No audio contract changed.

Escape, piano focus loss, window deactivation/resize, pending work, project/
revision/clip/grid/view/mode changes and outside release cancel. Shared previews
rebuild only when active cells change. Source tests cover context/modifier guards,
clip-tail snapping, clicked-note override, preview sharing, created-ID selection,
and mixed history. 116 UI tests pass (one existing benchmark ignored), strict
workspace Clippy, desktop build, fmt and architecture/style checks pass.
Native initial locked stroke produced five quarter-gate pitch-60 notes at
0.5–1.5 beats with one revision; further edits entered that test instance before
saved comparison, so complete project/history acceptance remains open. Native
held-preview, Option/Cmd, backtracking and detailed Live parity remain separate.


## finite source projection prerequisite

`hex-project::MidiSourcePlayback` separates source extent, start marker, optional
beat-domain loop brace and finite Arrangement duration. Schema 10 connects this
projection to canonical clips; strict schema 9 migration retains old behavior.
An allocation-free per-note plan counts in constant time and emits stable iteration
identities. The first pass starts at the source marker, then repeats the loop;
intro notes play once. Notes preceding the start are not onset-chased. Gates stop
at the local seam or finite endpoint and instrument releases remain independent.
These are original Hex policies pending the planned reference experiment in
`research/observations/midi-local-loop-001.md`.

The descriptor is integrated in both compiler paths and canonical saved metadata.
Source-aware editor bounds and control-prepared Arrangement thumbnails are
integrated. Preview samples use absolute gate frame endpoints and publish with
the accepted graph/project revision, including prepared history/import. Painting
uses at most 32 samples per clip and 128 note shapes per row. Compact native
source controls are integrated; independent listening/reference and complete
minimum-layout/native reopening acceptance remain required.
Tests compare closed-form plans against independent small source-segment walks
and cover huge counts/overflow; existing tempo, render and session refusal tests
remain applicable. Prepared templates for ongoing Session playback are a separate
required stage; finite expansion is insufficient for potentially unbounded loops.

## Ongoing template event query dependency

Finite Arrangement projection remains canonical and saved. Ongoing source
templates are a separate engine contract under implementation; they must not
implicitly make Arrangement clips unbounded. The next prerequisite is an exact
raw-beat range lower bound on canonical rounded frame conversion. A representative
inverse beat (especially the latest beat at a coincident tempo boundary) can skip
earlier beats mapping to that same frame. Admission must bound periodic, intro
and seam contributions before a caller-owned at-most-256-edge query is considered
realtime-ready. Runtime voice reconstruction, launch quantization, MIDI output and
canonical Session slots/scenes remain required after this primitive.

Ongoing source edge template source acceptance: PreparedMidiSourceTemplate
control-prepares sorted intro and repeat edges and admits conservative density.
Fixed MidiEdgeBuffer queries are stateless and seekable: at most256 frames,
256 events / 128 onsets and260 period windows, without internal allocation/history
replay. They refuse incompatible timing maps and domain exhaustion, preserve
iteration identity and off-before-on ordering, and reset visible output on
failure. Independent density and finite-projection tests pass. This is an engine
primitive, not an active voice renderer, MIDI device output or Session launcher.
The finite canonical Arrangement compiler/render behavior remains unchanged.

## Ongoing instrument span prerequisite — source accepted

PreparedMidiInstrumentTemplate reuses source gate projection and the existing
PreparedVoice/SineTable kernel. Its fixed 192-span buffer reconstructs gate and
50 ms release intervals at arbitrary frame positions, including prior loop
iterations. Queries admit at most 256 frames; timing maps must match the sample
rate and remain within the prepared tempo ceiling. Intro/repeat indexes and
period arithmetic avoid scanning source notes or prior playback history.

Control-side admission includes release overlap, capped at 64 sounding voices.
Separate intro and periodic maxima plus a bounded one-sided first-tail sweep
cover actual nonnegative repeat occurrences. The valid source-start-0 32-note
loop matches finite PCM at first/later seams; delayed-tail overload still refuses. Rendering overwrites
one source's output; query errors clear spans and silence that output. Graph
aggregation, track/master processing and transport/launch transitions remain
required before runtime integration. Finite PCM oracle and density/seek/error
regressions pass; this does not establish native ongoing Session playback.

## Mutable instrument source-loop admission — Pulse 2214

`SessionPlayback` now admits bounded mutable instruments on finite Arrangement
MIDI clips with enabled source loops and on looping Session MIDI sources. The
existing control-prepared templates already materialize only the intro and one
repeat period; each query reports the occurrence iteration in its span/event
key. The mutable host combines that iteration with the Arrangement transport
loop pass (zero for Session routes) before assigning its monotonically checked
runtime occurrence ID. Authored gates crossing a source loop end are clipped
there, so their matching NoteOff remains bounded and is emitted at the mapped
loop boundary.
Existing source-start semantics are preserved: a note onset before the initial
`source_start` is not chased, and notes beginning before `loop_start` occur only
in the intro even when their gates extend into the loop interval.

Mutable Arrangement instruments now support bounded interactive transport-loop
wrapping. The initial pass preserves already-sounding notes across the first
natural arrival at `loop_start`, without an extra attack. On actual wraps, active
notes authored before `loop_start` receive fresh attacks whose gates end at the
authored gate end or exclusive `loop_end`, whichever comes first. Notes authored
inside the loop keep their phase. The checked transport pass joins source
iteration in the occurrence identity. Before runtime construction, admission
proves the 256-frame onset ceiling, periodic voice/release overlap, ordered
NoteOn/NoteOff edge capacity and intro-tail first-wrap seam for each mutable
track. Event and voice limits respect the smaller of each device's declared
capacity and the engine capacity. Spans are bounded and collected fallibly; dense
or unsupported loop configurations are refused. Tempo-map edits are refused
while a mutable Arrangement loop gate is active, preserving the old playback
until gate-deadline reconciliation is supported. Finite source
loop occurrences still stop at the Arrangement clip end. Linear Arrangement
export retains its monotonic source timeline and continues to refuse mutable
Arrangement source loops. Seek/preroll note chase remains unsupported.

Source tests compare trimmed Session intro/repeat events with an authored-frame
oracle, check loop-end gate clipping and unique iteration IDs, compare render
partitions of 1/17/127/256 frames, enumerate finite Arrangement spans, and
compare interactive transport-loop PCM with an independently expanded
Arrangement oracle across multiple wraps. They also cover dense short-loop
rejection, first-wrap release density, device-declared capacities, aggregate
ordered-event density, release-only handoff filtering, tempo-edit refusal with
old-PCM continuity, linear-export refusal and transport-pass overflow rollback.
An allocation-probe test covers repeated Session source events through the
prepared audio callback and requires zero total and signed-net allocations.
These are headless source checks; native device, audible deadline and broader
musical editing acceptance remain open.

## Mutable device capacity admission — Pulse 2220

Before publishing a finite Arrangement revision or ongoing Session inventory,
the control-side compiler proves both event-edge density and release-inclusive
voice density against each prepared mutable device's declarations. The effective
limits are the smaller of the device declaration and the host capacities (256
events and 64 active occurrences). Every sliding half-open 256-frame window
counts both NoteOn and NoteOff edges, including dense short notes across multiple
clips; exact-boundary NoteOffs sort before NoteOns. Active voices include their
declared release tails. Zero or over-host factory declarations and candidates
that exceed either bound are rejected before project history or playback
publication. The audio callback continues to use the same fixed bounded event
and voice storage, so it never needs to drop an admitted NoteOff or allocate a
larger buffer. Session template/work reuse also keys on the copied declarations,
so swapping a same-ID registry implementation with tighter event limits or a
longer release tail re-runs admission instead of retaining stale proof data.

Tests cover the 320-edge multi-clip event-only counterexample with bounded onset
and voice density, exact event and NoteOff/NoteOn boundaries, lower registered
device capacities, first-seam source-loop tails, and a rejected editor candidate
that preserves undo/redo, project contents and already-prepared PCM.

## Canonical Session data ownership — source accepted

MidiSource owns source markers, loop settings and notes. Arrangement MidiClip
delegates its existing source getters and keeps finite placement. SessionMidiClip
has an independent ID, name and MidiSource with no Arrangement timing/reference.
Copies allocate fresh clip/note IDs; moves preserve IDs across valid track/scene
destinations. MIDI cells require instrument tracks; explicit stop cells can live
on any existing track, and absent cells are structurally empty. Launch behavior
is not defined by this data representation.

Generic note commands/batches resolve both owners. Scene/cell/source commands
use canonical transactional history; removal cleans owned cells, and failed
commands preserve project/allocator/redo. Combined limits are 16,384 clips,
65,536 notes, 256 scenes and 65,536 sparse cells. Snapshot/schema validation
includes global scene/clip/note IDs and cell ownership. Schema 11 round trips
complete data; literal schemas 1–10 migrate empty grids. The source-oriented
engine adapter prepares Session clips without finite Arrangement placement.
Prepared Session inventory and exact precommit revision publication are source
accepted, including all inactive cells and componentwise per-track maxima.
Mixed-route physical admission, quantized launches/phase/releases and UI remain
required. Existing finite Arrangement render/export stays unchanged.


## arrangement edge resize — 9 October 2026

`TrimMidiClipEdge` is a beat-anchored, atomic edit;
`MidiClip::preview_edge_trim` returns O(1) metadata through `MidiClipTrim`,
without copying or expanding notes. IDs, authored notes, source length and
loop markers remain unchanged. Right resizing preserves the timeline start
and source marker and clamps to a positive length; one-shot extension beyond
the source exposes silence, retaining notes for later editing. Left resizing
preserves the exclusive timeline end and changes the source start marker.
One-shots clamp to available source data and allow reversing a prior trim.
Loop intro positions can rewind to source zero; forward edits pass through
the intro then wrap the loop. Once starting inside the loop, left edits advance
or rewind modulo the loop interval. This original policy does not persist an
extra unwrapped phase or restore a previous intro automatically. Existing
playback does not chase notes beginning before the source start.

No-op edits preserve source identity and redo. Source marker changes copy the
shared source only on canonical commit, not every preview frame. Existing
schema-12 fields persist the result without a schema change. Audio trim and
MIDI resize preserve their current neighboring-overlap behavior; overwrite
editing remains specific to drop commands. Native resize acceptance is pending;
see clip-resizing.md. Exact Live source-boundary parity remains unverified.

## computer midi keyboard — #2642 / PR257

The toolbar keyboard toggle and unmodified M enable a transient computer MIDI
keyboard, off at startup. Select a track with an enabled native instrument.
The physical keys `a w s e d f t g y h u j k o l p ;` play seventeen chromatic
semitones from MIDI60 (Live's c3). Z/X shift the base by one octave, bounded to
MIDI0–120; keys above127 are silent. C/V step through velocities20,40,60,80,100,
120,127, initially100. Held notes retain their original pitch/velocity when these
settings change. The toolbar tooltip shows the current base and velocity.
These documented controls follow [Live12's computer MIDI keyboard](https://www.ableton.com/en/live-manual/12/routing-and-i-o/#playing-midi-with-the-computer-keyboard).
The octave labels are Live's convention; exact musical response parity is not claimed.

Audition works with transport stopped or playing, adds polyphonic notes at the
selected instrument output, and uses the existing suffix, track mix, sends,
Returns and Main routing. It creates no notes, history, dirty state, saved toggle,
recording or external MIDI output. Unmodified note keys take precedence over
single-letter editing shortcuts only in main-editor/piano focus. Shift and other
modifiers retain editing shortcuts, and text/numeric focus retains typing.
Auto-repeat never retriggers; physical key-up releases its original note even
if modifiers or octave changed. Disable, Stop/seek, focus loss, track/project
change and successful graph replacement cancel the current audition lease.
After a cancellation, release and press a key again to rearm. Device previews and
prepared seeks suspend audition so a same-revision canonical bank cannot override
their graph. Native input delivery and physical listening remain user checks.

Preparation runs on the existing session worker and publishes an ephemeral graph
for the exact canonical revision through ordinary graph custody. It does not
publish a canonical edit. The selected bank prepares128 pitches at the seven
finite velocities, using existing inline/shared analytic samplers or a separate
mutable native runtime pair. Unsupported pitches and capacity refusals surface
an error. All-unsupported configurations refuse preparation. Selected-bank owned
storage is capped at3MiB and charged to the graph runtime budget; owners retire
with that graph on control. Mutable audition has its own bounded pool, so
instrument-global modulation is independent from authored voices; it does not
establish a shared authored/live mutable voice contract.

Seventeen independent atomic desired-key tokens and a terminal cancellation
lease bypass the transport ring and cannot lose NoteOff under queue pressure.
The callback has64 fixed live voice slots and128 fixed event slots; device
capacity declarations still apply. Capacity pressure cancels the live pool,
requiring a new activation, without stealing authored voices. Block rollback
covers live voices, note identities and the independent monitor clock. Stopped
monitoring freezes Arrangement and Session performance clocks and consumes no
authored source/launch events. Existing shared effect tails may ring while
monitoring. Voice age is independent; musical modulation follows the effective
performance clock while playing, and an anchored constant-current-tempo clock
while stopped. Positive-latency graph preparation explicitly refuses keyboard
monitoring to preserve paused authored delay history; support is deferred to
Pulse#2644 / PR260. Zero-latency typed audio routing is supported.

Source checks cover independent sine PCM/polyphony/release across block sizes,
authored continuity, mutable Operator release, typed routing and one suffix,
nonzero/anchored tempo clocks, invalid inputs, late cancelled preparation,
voice pressure, worker revision/history preservation and positive-latency refusal.
GPUI dispatch checks cover M, reserved S/L, repeat, modified key-up, text entry,
focus cancellation and held-repeat suppression after focus reset. Callback probes
cover publication, live notes/release, a full transport queue plus priority Stop
and graph retirement with zero total and signed-net allocations/bytes. These are
headless checks, not native deadlines or listening acceptance.

Finite native checklist for the optimized exact-source test build:

1. Select an enabled sine, FM4 or Vector instrument; enable the toolbar keyboard,
   play chords while stopped, and confirm the cursor stays still.
2. Play the same keys during authored playback, release them, and confirm authored
   notes continue. Check track gain/mute, a suffix effect and a Return send.
3. Hold A, use X/V, release A, then press A again; check the changed octave/velocity.
   Verify C from127 returns120, and M switches the toggle off/on.
4. Hold notes, then separately disable, Stop, change track, focus search/numeric
   input, and switch away from the app. Confirm no stuck notes; release/repress
   rearms. Type normally in text fields and exercise Shift+S/edit shortcuts.
5. Change an instrument parameter or project during a held chord. Confirm the old
   audition stops without retriggering; release/repress after preparation. Confirm
   undo/save data has no performed notes and no audition-only dirty change.

Keyboard review repairs: while audition is active, direct Arrangement audio and
MIDI sources enter the instrument gap before the track fader. Authored and live
notes receive the final track gain once, including on tracks with no effect
chain. A silent Session route still admits live notes without restarting its
authored source. Live keyboard owners explicitly refuse the separate closed
audio-only candidate owner inventory. Source regressions cover these boundaries;
native listening and deadline acceptance remain open.
