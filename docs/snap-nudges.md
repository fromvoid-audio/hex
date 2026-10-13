# Arrangement snap nudges — pulse27 / draft pr185

This is the snap-nudge stage of the original musical editing request, not its
complete acceptance. Arrangement MoveLeft/MoveRight uses the previous/next
DISTINCT selected-grid landmark, including off-grid starts. Fixed grids use
canonical quarter-note units; Adaptive reuses the painted meter-section origins
and density at the current viewport width. Tempo conversion remains control-side.
Audio start remains frame-anchored and MIDI start beat-anchored. Source bounds,
length, media, other clips and routing are unchanged; one nudge is one existing
canonical worker command and Undo step.

With snap disabled, existing audio quarter-second and MIDI one-beat free steps
remain. Effective-grid inversion follows the existing platform modifier contract
when an action is dispatched. Session navigation and focused piano-roll note
nudges retain their existing paths. Busy, stale/unpainted geometry and
unrepresentable endpoints enqueue nothing; the left edge is a no-op on the grid.

Rendered/action, directional landmark, modifier, boundary and real-worker
Undo/Redo tests are source evidence. Native keyboard/focus/modifier behavior and
visible positions on a fresh optimized Mac candidate remain pending. Linux does
not prove physical input/paint latency or Live parity. The broader requested
clip gain/pitch/warp, beat-anchored audio, source envelopes, metronome, scrub and
remaining drag-autoscroll acceptance are still required. Existing loop-handle
and time-range autoscroll implementations retain separate native gaps.

PR185 remains blocked/draft under the readiness/evidence HOLD. No passing gate
registration, native delivery, acceptance, merge, deployment or Done is claimed.
