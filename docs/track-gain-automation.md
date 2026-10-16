# prepared track-gain automation

Pulse28 / PR184 adds a bounded canonical editing and prepared-playback stage.
The original ticket's broader graphs, automation recording, latency and native
acceptance remain incomplete. A compact Arrangement gain-lane editor now exposes
these commands; native interaction acceptance remains pending. There is no
device-parameter capability expansion or recording implementation in this stage.

Each canonical Track owns one `GainLane`; its base `gain_envelope` is carried by its ordinary
TrackSnapshot and history. Existing TrackId owns the lane; existing Q32.32
BeatTime keys its points. Replace, upsert, move, remove and clear are atomic
control commands. Upsert replaces an equal beat; moving onto another point
refuses. Persisted beats must strictly increase. Gains are finite linear0..2,
matching the existing manual gain domain. Point interpolation is step or linear
for its outgoing interval. No independent automation-node allocator or parallel
save/history envelope exists.

An active lane replaces static gain absolutely. Empty uses the original gain;
singleton holds everywhere; before first and after last hold the endpoint.
At a point boundary the new value applies. Linear intervals are authored in
beat space and split at tempo markers during control preparation. Canonical
beat-to-frame rounding is nearest, half ties later. When distinct beats map to
one frame, the latest beat wins for playback, without deleting authored points.
Prepared linear values use f64 endpoints and interpolation, cast to f32 at the
sample; tempo cuts use the authored beat ratio and prepared frame boundaries.
There is no accumulated ramp phase or implicit smoothing.

The lane follows Arrangement transport position for all current source owners,
including launched Session audio/MIDI. Launch phase does not restart it.
Transport loops repeat the lane; linear Arrangement export does not wrap.
Meter affects presentation/grid, not quarter-note anchors. Tempo edits reprepare
against the new accepted map at the accepted transport position. Session source
clock continuity does not imply a gain-lane smoothing or beat anchor at a cut.
Pause freezes transport/history; Stop/Seek select the destination value through
the existing reset/preroll contracts.

Automated tracks use bounded raw-source scratch and apply the prepared absolute
gain, balance and mute once after devices, before Main and post-fader sends.
Static gain zero therefore does not suppress an audible envelope. Existing
no-automation fast paths and arithmetic remain; ReturnFeed topology, pinned Main
summation, single Return-chain execution and single master application remain.
The public isolated Track convenience renderer refuses automation instead of
returning unscaled source output; full Project/Session/export renderers admit it.
Borrowed pages and post-node observations retain their existing transaction
contract. Observed processor samples remain before the post-device track fader.

Current track-owned processing follows the lane. Detached old source/effect
chains retain their existing frozen-mix policy with the effective old gain at
the exact transition frame. Removed/changed Return residuals remain Main-only
and never replay old feeds. Lane edits do not reset compatible processor history.
Existing admission, tail/release budgets, preflight and owner transfer rules
remain intact. A failed whole block zeros output and leaves committed clocks,
routes and DSP state unchanged; gain queries have no persistent mutable cursor.

Manual Gain/GainBy visibly refuse while automation owns the fader. Clear restores
manual ownership and its retained static fallback. Clip-only edits leave this
track-owned lane anchored. Affected time clipboard, paste/ripple and track-split
operations refuse until complete curve transforms are implemented. Track removal
owns lane removal in the same edit; undo restores it. No operation may silently
drop a curve to make an edit succeed.

Canonical bounds are1024 points per track and65536 per project. Control preparation
counts all authored points and interior tempo cuts before reserving gain plans,
with a conservative327680-knot project limit. Tempo-marker ranges use bounded
binary searches. Prepared vectors reserve the counted number; coincident frames
collapse only the prepared sequence. `gain_automation_capacity` reports actual
authored points, stored knots, allocated vector slots and knot bytes. These are
coefficient-storage observations, not process-memory or native deadline claims.
The dense256-track/65536-point/1024-tempo fixture measured327424 slots and
7858176 knot bytes on Linux, with24-byte knots; allocator overhead is not included.

Schema21 requires each track's strict gain_envelope point array, each containing
beat_raw, gain and outgoing. Unknown fields/modes, malformed values, duplicate
or unsorted beats, excessive points/tracks and unsupported backports refuse.
It preserves complete schema20 state, ReturnFeeds, sends, opaque device payloads
and exact bounded import provenance. Schemas1–20 migrate empty lanes. Ordinary
encoding selects21 only for nonempty lanes and preserves existing19/20 byte paths
otherwise. Older envelope entry points refuse automation loss; the document
ceiling and recoverable/create-new file behavior remain. Operator statev9/v10
and preset envelope1 are unrelated formats.

Recording still needs callback-confirmed applied values, revision/TrackId/parameter
identity, sample rate, exact application frames and loop/transport context,
bounded event delivery/overflow handling, touch ownership, release-to-read and
atomic gesture history. Wall-clock UI events do not establish recording.
Native lane discovery/editing, listening, Live reference behavior, optimized
macOS packaging and CoreAudio/hardware deadlines require separate evidence.

## selected-track gain pane

The Arrangement header's `a` control opens a 96 px pane below its virtualized
track list. Track rows remain 44 px and the curve uses the same time viewport,
right-hand mixer-header width and selected snap grid. The view includes the lane’s
last authored point while open; this display horizon does not change canonical
content duration or the default export extent. Opening it validates device
controls and closes the open detail body so real track rows remain operable at
880 × 540. Explicitly reopening the detail body closes the gain pane and cancels
its local draft; the last selected workspace keeps enough room for track rows.
The `auto` badge identifies an active lane; manual gain buttons visibly
refuse automation ownership and offer this editor. The retained static gain is
labeled as fallback, never described as the sounding value of an active lane.

Click the plot to add/upsert a point; drag a point's beat/gain as one local draft
and commit once on release. Previous/next selects every authored point, including
beats that coincide at one playback frame. The shared numeric field edits
percent gain; time dragging edits the exact beat; step/linear changes its outgoing mode; remove and clear use the
same canonical commands. An unchanged click retains the exact Q32.32 beat key.
Numeric gain editing never rewrites that key. Exact raw beat, prepared frame and
meter position are available in the readout tooltip. Command-modified placement
uses the existing free-placement grid behavior. A move onto an occupied beat
refuses the complete candidate and keeps the draft visibly unapplied.

Numeric gain uses a round-trip display codec without changing the shared finite
range validation. Its focused text context prevents letters such as `l` from
becoming transport/loop shortcuts. Numeric text validation stays in the shared
widget. Invalid drafts remain
visible and block point actions. Escape cancels; the first Undo cancels a local
pointer/text draft before a later Undo reaches canonical history. Changing the
selected track or replacing/revising the project invalidates local drafts and
numeric bindings. A typed `GainLane` actor request carries TrackId, generation
and revision; the worker checks all three before existing preparation/history.
A request delayed behind another committed edit refuses rather than overwriting
its lane. Device gestures must finish before this independent edit is admitted.

The worker caches a fallible, read-only `GainAutomationPreview` prepared through
the playback envelope law. An unavailable preview is explicit and cannot become
an apparently valid static value for active automation. Painting uses its tempo
cuts and collapsed prepared knots; authored points remain separately selectable.
Public prepared Track knot/value getters are immutable metadata accessors. They
perform no source-owner mutation and are not applied-gain acknowledgments.
`draft · not applied`, queued canonical edit, pending audio application and an
exact applied revision are separate presentations. The cursor's displayed gain
is expressly a prepared value; it is not an audio callback measurement.

## pending optimized native checks

On the approved native machine, use the existing release/optimized packaging
workflow against the exact candidate and its retained clean full local-CI report.
No unoptimized normal test artifact, Linux result, or revision label substitutes
for native acceptance. Preserve the original full Pulse28 scope and readiness
hold; do not merge, deploy or register a passing gate to obtain an artifact.

- At minimum and normal window sizes, inspect `a`, `auto`, fallback/refusal text,
  curve/header alignment, 44 px rows, font, focus and lowercase text.
- Exercise add/upsert, time/gain drag release, numeric gain, outgoing
  modes, remove/clear and all coincident-frame points. Check invalid text,
  occupied beats, Escape and Undo/Redo, Save/Reopen and owner/revision changes.
- Check grid/free-placement modifiers, zoom/pan, transport cursor readouts and
  pending/applied revision presentation during Arrangement and Session playback.
- Listen to source ownership changes, sends/ReturnFeeds, tails and linear export
  on actual native audio. Record exact source, hardware and observed outcomes.

Recording remains separate: applied-value/sample-frame/revision acknowledgments,
bounded touch/release ownership and canonical splice/history are not implemented.
There is deliberately no recording button. General ports, MIDI/sidechain routing,
latency propagation and compensation remain later stages of the original ticket.

Schema22 closed touch-span model/storage preparation is described in
[gain-touch-spans.md](gain-touch-spans.md). Nonempty spans are refused at normal
adoption/load and commands; this does not implement recording or application ACKs.

## combined persistence integration — 16 october 2026

New saves use the complete schema23 envelope. Historical schema18/19 branch
shapes dispatch by key presence into strict readers; schemas20–22 remain readable.
Device input/output host gains, opaque payloads, provenance, Return feeds, gain
points and epoch metadata share one lossless snapshot path. The old ordinary
writer selection described above is historical. See
[storage](../crates/hex-storage/README.md). Nonempty touch-span adoption and actual
recording remain refused; this adds no compensation.
