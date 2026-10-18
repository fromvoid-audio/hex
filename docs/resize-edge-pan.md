# clip resize edge panning

Ordinary Arrangement audio and MIDI resize handles use the existing captured
trim payload and 16 ms controller timer. Holding the pointer in either 24 px
horizontal edge band pans without further pointer motion, in steps capped at
50 ms. Leaving that band, release, Escape or a stale owner stops panning.
The left navigation bound is frame zero.

Previously zoomed views already panned in both directions, but panning stopped
at the original display horizon; a fitted viewport could not pan right at all.
The display horizon is not an authored content limit. A current resize may now
navigate beyond it. Only that gesture's validated, checked viewport end extends
the transient display horizon. Generation/revision, original clip/asset, lane
geometry and explicit viewport ownership must still match. Cancel/release ends
this transient ownership; normal canonical horizon handling resumes. Project
content, canonical duration and other move/selection/import scrolling are unchanged.

Audio/MIDI model previews still enforce source windows, minimum duration, looping,
warp/stretch and checked endpoints. Navigation stops when the unsnapped model
cannot advance further, so exhausted sources do not pan out of sight. An inward Shift-stretch step that would cross the opposite edge uses the last
canonical-valid viewport within that timer step, found in bounded integer-frame
probes. It does not discard the remaining valid travel or relax the positive
interval/source rules. Prepared warp capability limits (including the existing
1/8–8 stretch range) still refuse unsupported releases atomically; model-positive
preview alone does not establish preparation acceptance. MIDI retains its existing canonical clamp. Grid
quantization does not prematurely stop held scrolling. Preview and release use
the current snap modifier; Shift still chooses the existing audio stretch path.
No separate timer, DSP, waveform-cache, fade gesture or source mapping is added.

The bounded headless regression uses real worker/history and GPUI test windows,
rendered handle mouse events, and controlled controller tick times. It separately
covers the already-working zoomed path and the fitted-view failure, zero/source
limits, stale/cancel retirement, and stationary modifiers with release agreement.
Compiler/fixture failures are retained separately from product failure evidence.

Coordinator-owned optimized physical acceptance remains open: resize a looping
or extensible audio clip and MIDI clip to each edge, hold still, confirm continued
pan and exact preview geometry beyond the old view; pan left to zero. Move back
inside and outside the edge band, release, Escape, and change focus; confirm no
continued pan or stray edit. Check nonloop source bounds, minimum lengths, grid /
Command and Shift stretch, correct committed source windows, one Undo/Redo, and
waveform visibility. Lower fade controls and unrelated gestures retain their
existing contract. Cloud checks do not establish actual pointer capture/display,
GPU deadlines, audio/listening or delivery. Do not launch/replace the user's app.
