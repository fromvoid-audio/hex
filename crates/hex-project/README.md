# hex-project

## Purpose

Canonical control-side arrangement: stable typed IDs, media references,
validated frame-based audio clips and beat-based MIDI clips, shared track controls,
and bounded undo/redo. Commands
are transactional; failed edits leave state and history unchanged. No PCM is
stored in history. Each asset records its original source rate/frame count and
its prepared project-rate playback frame count plus conversion version. Clip
source intervals use playback frames.
Optional musical audio edits use shared immutable ownership with copy-on-write
for crop, placement and processing changes. Neutral clips retain only an optional
pointer; public read-only edit access and owned snapshots keep the same contract.
See [musical editing](../../docs/musical-editing.md) for retained-domain semantics.
Track solo state is canonical metadata. Exclusive solo clears other solo states
when enabling; additive solo preserves them, and toggling off only clears the
selected track. The optional half-open loop region has validated positive length,
an independent enabled flag and transactional set/toggle/clear commands.
Tracks also carry a validated original Hex stereo balance in -1..=1, and the
project carries linear master gain in 0..=2. Relative edits resolve from
canonical state and clamp to those endpoints. Balance attenuates the opposite
channel without crossfeed; this is an explicit Hex policy, not a measured Live
pan law. Clips carry validated fixed-domain fade metadata. Setting fades anchors
them to current duration; moves and duplicates preserve the envelope, splits
retain its original span and advance the right segment offset, and trims reanchor
and clamp both fade lengths. The explicit original Hex fade curve is linear with
endpoint samples and multiplicative overlap, not a confirmed Live curve.
Looping clips retain a source-relative phase at their timeline start. Splits
preserve the full source interval and advance the right clip's phase; one-shots
keep their prior source partition behavior. Absolute left/right edge trims use
the same metadata-only preview helper as the canonical command. One-shot trims
preserve the clip's duration/source-length difference and shift source bounds
with the timeline; loop left trims move phase while right trims only change
duration. Actual trims reanchor fades, while boundary no-ops preserve them.

Finite Arrangement time rectangles use `TimeRange` and opaque `RangeClipboard`
metadata. Capture preserves audio source/fades and materializes selected MIDI
playback in frames; paste/delete/move are validated candidate edits with one-step
history. No PCM or OS clipboard is owned here. See [time selection](../../docs/time-selection.md).

## Dependencies

Depends on the standard library and `hex-lattice-contract` for opaque, bounded
native-device state envelopes. The engine consumes read-only snapshots to
compile playback; storage owns the versioned native metadata file. Device
payloads remain opaque here; the model does not depend on DSP or UI. Hybrid tracks
also own one optional generic instrument slot with globally unique device ID,
bypass/state edits and history. New/imported tracks receive the explicit neutral
sine envelope; removing the slot means MIDI silence. See
[instrument slots](../../docs/lattice-instruments.md).

The schema-17 return model adds up to 16 pinned-order return buses, each with a
stable `ReturnId`, gain/mute and up to 64 opaque native effects. Regular tracks
own sparse unique sends to returns in the finite linear gain domain `0..=2`;
zero removes the send. Return IDs and device IDs share global project ID
validation/high-water behavior. Return removal cascades target sends atomically,
and track removal drops its owned sends. This is metadata/history only: playback
does not compile nonempty return/send projects until the engine owns routed graph
processing. See [return tracks](../../docs/return-tracks.md).

Prepared browser imports can atomically insert a default hybrid track before a
stable `TrackId` or append at the end, then create its audio asset and clip at a
validated frame start. The legacy prepared-import command remains append-at-end
at frame zero. Track insertion, media ownership and clip creation share one
transaction/history step; failed validation leaves order and history unchanged.

Arrangement drag drops use separate overwrite commands and leave generic moves
unchanged. They carve the incoming half-open destination interval from both
audio and MIDI clips on the target track, excluding the moved clip itself. Audio
fragments use the canonical split phase and fixed fade domain. MIDI right
fragments materialize bounded note occurrences into finite one-shot sources;
the left fragment retains its original clip/source identity, while split right
fragments receive fresh IDs. Collapsed frame-domain MIDI fragments are omitted.
Prepared audio imports use the same operation for the incoming WAV and all
victims, so candidate compilation/history admission stays atomic. See
`../../docs/clip-overwrite.md` for interval, rounding and loop-expansion details.

## Does not own

PCM decoding, disk I/O, audio callbacks, render scheduling, GUI state,
resampling, warp, MIDI synthesis/playback, device DSP or compatibility import. Undo history is a bounded
control-side snapshot implementation, not a real-time API.

IDs are project-scoped, monotonically allocated and never reused by an Editor
across undo/redo branches. Do not mix IDs from independent projects. Timeline and
clip source regions use project-rate playback frames, not source-file frames,
beats or bytes. Conversion metadata validates duration and stable algorithm ID;
actual PCM preparation belongs to hex-media on the control side.

Musical position metadata is represented by `BeatTime`, unsigned Q32.32
quarter-note beats, integer micro-BPM `Tempo`, and validated `Meter` values.
Projects own bounded, sorted tempo and meter maps beginning at beat zero.
Tempo conversion uses checked fractional-frame prefixes and rounds once to the
nearest frame with ties later; inverse conversion uses those same prefixes.
The control-side lower-bound query returns the first raw Q32.32 beat whose
forward conversion reaches a requested frame, with `u64::MAX + 1` as the
no-representable-beat sentinel. It solves the crossing within a cached tempo
segment and is distinct from the inverse query's latest-boundary rule.
Meter markers may occur mid-bar. Hex starts a new numbered bar there and marks
the preceding truncated bar fragmentary. These maps drive ruler/snap queries;
existing clips remain frame-anchored and are not implicitly warped or retimed.
Every track owns both Arrangement audio clips and beat-anchored MIDI clips, with
one stable track identity and shared mix controls. MIDI clips retain independent
source length/start, loop markers, looping enable and finite arrangement duration. A shared
`MidiSource` owns those source fields and note data; `MidiClip` adds only stable
clip identity and finite Arrangement start/length. The sparse Session grid owns
independent `SessionMidiClip` values with a user name and `MidiSource`, stable
scene IDs, and typed MIDI or explicit stop cells. A missing cell is structurally
empty. Stop cells are valid on any existing track; setting a stop intentionally replaces
the cell, while create/copy/move/duplicate refuse occupied destinations. Clear
removes any cell. Removing a track or scene removes its cells atomically. Session
clips have no Arrangement placement and do not refer back to Arrangement clips.
Project clones share immutable source allocations with `Arc`; source mutation
detaches only the changed source. Prepared-cache identity is an opaque owning
token holding that `Arc` and comparing allocation identity, so an allocation
cannot be recycled while cached artifacts still use its key. Arrangement-to-
Session copies and duplicated clips receive independent source allocations and
fresh clip/note IDs.
`DuplicateScene` copies the scene name and each populated cell, preserving track
associations, Stop cells, MIDI source fields, notes, and clip names. Copied MIDI
clips and notes receive fresh project-wide IDs and independent sources; empty
cells remain empty. The duplicate is inserted immediately after its source.
`MoveScene` reorders by stable scene IDs, placing before a target or appending
when no target is supplied. These edits preserve cell content and IDs while
reordering and use the same transactional project/history path as other edits.
Notes use source-local
Q32.32 positions and may overlap; they require channel 0..15, pitch 0..127 and
velocity 1..127, and must fit inside the source length. Track order and identity
are shared by both clip collections. One global ID allocator covers assets,
tracks, Arrangement and Session clip kinds, scenes and notes. The project bounds
combined clips at 16,384, notes at 65,536, scenes at 256 and sparse Session cells
at 65,536. Scene and cell maxima are format-safety caps, not performance promises.
`SplitMidiClip` divides one finite Arrangement region while retaining the left
clip and source identity. The right region receives a fresh clip ID and an
finite one-shot source whose notes materialize the original playback occurrences
inside that region. Gates crossing the cut are clipped at the cut and resume at
the right region's start; later loop occurrences are expanded explicitly, so a
carry note is not repeated at each loop seam. The right fragment receives fresh
IDs for its generated notes and resets source/loop markers to its finite region.
The left retains the original authored source, notes and loop settings. This
keeps the finite note timeline while making the right region independently
editable; changes to its source or loop settings no longer affect the left.
The split refuses occurrence expansion beyond the project note cap. As with
overwrite fragments, a carried note is retriggered at the boundary, so this does
not claim seamless instrument-envelope equivalence.
`SetMidiClipSource` replaces all source and arrangement
fields atomically and refuses to shrink a source past any authored note, even if
that note is hidden by the current playback offset or loop.

`MidiSourcePlayback` is an allocation-free beat-domain projection for finite
one-shot or looped source regions. It validates source/loop bounds and produces
an O(1)-count per-note occurrence plan whose iterator carries loop iteration,
timeline onset and gate duration, including source-seam and arrangement-end
cuts. It does not implement MIDI scheduling or synthesis.

`UpdateMidiNotes` and `RemoveMidiNotes` operate atomically within one MIDI source,
whether its owning clip is in Arrangement or Session.
Nonempty batches are capped at `MAX_NOTES`; IDs must be unique and owned by that
clip. Update records replace all canonical note fields, validating every record
before mutation. Untouched order/IDs are retained; removals compact in one pass.
One successful changed batch is one history step. `hex-session` additionally
compiles the candidate before committing, so runtime budget rejection preserves
the prior project/history. Selection and group preview remain UI responsibilities.

`AddMidiNotes` inserts a nonempty clip-scoped batch of `MidiNoteInsert` records.
All fields, final project-wide note count, storage and the complete ID range are
admitted before note mutation. Records append in input order; identical musical
values are permitted. `Created::Notes` reports the owning clip and inclusive
contiguous first/last IDs while remaining Copy. A successful batch is one history
step; undo retains the allocator high-water and redo restores the same notes/IDs.
Mixed insertion/removal is provided by `EditMidiNotes`; continuous draw previews
remain a UI dependency.

`midi_batch` contains shared admission, stable compaction and ordered append for
insertion, removal and mixed commands. Mixed removals and insertions validate as
one transaction; storage reserves only net growth and global capacity reflects
removed notes. Erase-only uses no IDs. Rejected candidate preparation preserves
notes slated for removal and does not publish a replacement playback graph.

`Editor::undo_prepared` and `Editor::redo_prepared` prepare the target history
snapshot before moving either stack. A failed compiler or validator leaves the
current project, allocator high-water and undo/redo order unchanged; successful
preparation returns its artifact with the committed history transition.

Arrangement MIDI edge resizing uses `TrimMidiClipEdge` with beat units and a
metadata-only `MidiClip::preview_edge_trim` / `MidiClipTrim` candidate. Authored
notes and IDs remain intact; only timeline bounds and the playback start marker
change. The original one-shot/intro/loop phase policy is in `docs/midi.md`.

`InsertTrack` creates an empty hybrid track before an optional stable anchor
(or appends), with existing validation/history and no schema change.

The pure `preview_clip_fragment` helper exposes validated control-side audio
fragment metadata for view previews. It preserves source mapping, inherited fade
domains and the original clip ID without editing the project or copying PCM.

Selected-row `InsertTimeRange` opens a frame-based gap and pastes a finite time
rectangle atomically; bounded shifting/materialization lives in `time_range/insert.rs`.
No project schema or device callback responsibility is added.

The bounded project-level import provenance envelope is independent of
native device state: exact original input bytes (1 MiB maximum), source kind and
version, and adapter version. Clones/snapshots/history share immutable evidence;
normal edits retain it unchanged. Debug output excludes contents and private
paths. This is unchanged-input pass-through, not editable Ableton export or
proof of valid/importable Live XML. See [import provenance](../../docs/live-set-import.md).

Routing proposals use existing owners and validate the complete DAG, including
implicit Main paths, canonical sends and admitted Return feeds. Generic proposals
remain closed; typed stereo Return/track feeds and MIDI source selection use
separate canonical commands; see [routing](../../docs/routing.md). No device/sidechain bus support is
inferred from opaque state.

Canonical track gain envelopes now support atomic bounded point editing/history
with strict BeatTime order. Manual fader and affected time transforms refuse while
a lane owns gain. See [track gain automation](../../docs/track-gain-automation.md).

## Playable routing metadata

Typed `AudioFeed` edges carry stable global route IDs and finite positive gains
through 2.0. A Track or Return post-fader source adds stereo audio to a Track
input before its effect prefix; the source retains its own Main path. Gain zero
removes the edge. Gain edits retain identity; recreation allocates a fresh ID.
At most 4096 inputs are admitted, and the complete audio DAG includes canonical
sends and Return feeds, so changing any of these cannot introduce feedback.

`SetMidiSource` selects one incoming MIDI bus per instrument destination; `None`
restores local authored clips. Sources may have no instrument. Fanout is allowed;
acyclic chains resolve to their original authored source through
`midi_source_track`. Destination instrument/effects/mix remain local. Self routes,
cycles, stale endpoints and duplicate destinations are refused. Removing a Track
or Return removes incident routes in the same undoable edit; removing an
instrument with an incoming MIDI route requires clearing that route first.
Project owns validation/history only; engine owns bounded source scheduling,
audio processing, latency alignment and runtime admission.

Nested audio-event sequences and typed edits belong to this canonical model.
They retain immutable source coordinates, use bounded nonoverlapping intervals
and participate in normal transactional history; see
[audio events](../../docs/audio-events.md).

Arrangement and Session clip labels are canonical independent metadata.
`RenameClip` validates a stable clip ID/name and shares normal transactional
history. Splits/copies retain names. See [clip names](../../docs/clip-names.md).
