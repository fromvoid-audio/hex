# prepared track-gain automation

Normal saves now use the strict schema23 union. The schema19–21 formats below
remain frozen migration contracts; explicit schema20 encoding still refuses
unrepresentable gains or gain envelopes. See [backend integration](streaming-prerequisite-integration.md).

Pulse28 / PR184 adds a bounded canonical editing and prepared-playback stage.
The original ticket's broader graphs, automation recording, latency and native
acceptance remain incomplete. There is no native lane UI, device-parameter
capability expansion or recording implementation in this stage.

Each canonical Track owns one `gain_envelope`, carried by its ordinary
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
