# Canonical musical time contract

Status: implemented in project/storage/editor; [Pulse](https://pulse.iocore.dev/) tracks verification and remaining native
interaction checks.

Musical time belongs to the std-only canonical project model. Beats are
nonnegative Q32.32 quarter notes, persisted as u64 raw units. Tempo is integer
micro-BPM (1–1,000 BPM validation policy). Meter numerators are 1–99 and
denominators are powers of two through 64. Map entry caps are 1,024, beginning at
beat zero with strictly increasing subsequent beats. New and migrated projects
default to 120 BPM and 4/4. These are explicit Hex contracts.

Piecewise-constant tempo segments cache checked u128 Q32 fractional-frame
prefixes at the project rate. Segment arithmetic truncates only below Q32 frame
precision; beat-to-frame rounds the resulting position once, nearest frame with
half ties later. Inverse lookup uses these prefixes. An exact quantized boundary
frame resolves to the latest tempo point at that frame, making coincident
boundaries right-continuous; elsewhere choose the nearest representable raw beat
at the frame center, ties later. Multiple beats can resolve to one frame, so a
unique beat inverse is not promised. Representable frame→beat→frame identity,
monotonicity, boundary behavior and checked range errors are acceptance gates.

Meter markers may truncate a bar. At each marker, advance bar numbering by the
ceiling of the previous section's bar count and begin the next numbered bar.
Positions report the effective end and whether a bar is fragmentary. This is
Hex's deterministic numerical policy; exact Live bar labeling remains unverified.
The Live 12 manual documents arbitrary marker placement and fragmentary bars in
[Arrangement View §6.5](https://www.ableton.com/en/live-manual/12/arrangement-view/#time-signature-changes).
It lists denominators through 16; Hex's larger denominators are an extension.

Snap uses a positive fixed-point beat grid, nearest point with half ties later.
The first UI integrates opt-in snapping for pointer clip moves. Keyboard nudging
and edge trimming retain their current frame behavior until explicitly extended.
Map edits never implicitly move or warp current audio clips. Clip/source/fade,
loop and viewport coordinates remain frames, and schema migrations preserve them.
Explicit musical anchors and duration semantics are a subsequent prerequisite
for MIDI and warped audio, not an implied feature of this map.

Map construction and conversion run on the control side. Future MIDI/automation
compilation must resolve bounded frame events before callback consumption. No new
musical conversion, allocation or scheduler is added to the sample render loop.
Tempo curves, metronome, recording, musical clip anchors, warp and Live-set import
remain required roadmap work; this document does not claim their implementation.

Verification on macOS arm64: 241 workspace tests, two additional dense-map
quantization regressions and strict checks pass. Multi-rate inverses cover all
1,024 fractional tempo boundaries and monotonic frame round trips; a rational
golden prevents whole-frame prefix drift. The isolated native build saved and
reopened 121 bpm/3/4 maps, snapped a move to beat four (frame 87,471), restored
frame 75,154 with undo and reapplied it with redo. Snap-off then admitted a
non-grid start at 100,747; undo restored the snapped start. Clip source phase
4,909, duration 43,490 and fade domain remained intact through these moves.
Ruler labels fit the existing 24 px row after a native clipping repair.

Ruler labels now sample denominator-beat landmarks anchored to each visible meter
section, with at most 64 landmark attempts and a bounded 1,024-marker scan.
Fractional offsets and 6/8 half-beats pass 63 UI tests and native zoomed inspection.
A legacy schema8 muted audio fixture saved as schema9 with unchanged phase and
meter points. Held-drag preview/stale cancellation and minimum-window/dense
interaction remain open. The first native fixture session contained additional
clip edits before the measured snap check; no attribution or unchanged-clip claim
is made for that earlier interval. Listening and sustained device timing were
not assessed here.

## Prepared ongoing-playback timing prerequisite

`hex-engine::PreparedTempoMap` now owns a canonical map prepared off callback.
It validates the expected sample rate and 1024-point bound, then borrows the same
integer conversion kernel with typed static-text errors. There is no separate
rounding implementation or shared-ownership clone/drop in conversion calls.
Construction and eventual graph retirement must stay on the control side. Source
inspection finds no allocation in these borrowed calls; allocation instrumentation
and callback integration are not established. A future scheduler may call them
only at admitted block/event boundaries, never per sample. Current sample render
behavior remains unchanged.

51 engine tests pass, including seven prepared-map tests covering independent
constant-tempo arithmetic, exact half-frame ties, coincident quantized boundaries,
rate/range refusal, 1024 points, multi-rate canonical parity and reordered queries.
The prepared timing contract does not yet implement ongoing MIDI templates or
Session launches. Dense throughput measurements are in performance.md.

## Exact beat ranges for rounded frame queries

`TempoMap::beat_lower_bound_for_frame` returns a u128 raw-beat bound, the earliest
Q32 beat whose canonical forward rounded frame is at least the requested frame.
Frame zero returns raw zero. `TempoMap::BEAT_RANGE_SENTINEL` is u64::MAX + 1 and
means no representable beat reaches the requested frame. Cached fractional-frame
prefix lookup and checked ceil ratio solve a crossing without scanning the raw
beat domain. PreparedTempoMap delegates and returns a typed static-text error.
For a half-open frame window `[a,b)`, candidate beats occupy the half-open raw
interval `[lower_bound(a),lower_bound(b))`. The representative frame_to_beat
inverse retains its existing latest-boundary policy and is not used for this
range operation.

Final focused acceptance: seven canonical tempo tests and eight prepared engine
tests pass, including independent exhaustive small-domain lookup, predecessor
minimality, positive exact half-frame shared prefixes, dense maps, several rates,
large/seam/full-domain probes and one-past-domain results. Project/engine full
suites passed before final test strengthening; strict checks pass afterward.
This numerical primitive is integrated, while ongoing event queries remain
in implementation and callbacks remain unchanged.
