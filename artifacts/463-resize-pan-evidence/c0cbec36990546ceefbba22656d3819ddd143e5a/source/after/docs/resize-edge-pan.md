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
preview alone does not establish preparation acceptance. Audio and MIDI timer
steps consume only the portion applied by the canonical source/minimum clamp.
Grid
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

## Exact terminal retirement follow-up

Accepted enqueue ends pointer/timer capture but transfers the owned transient
viewport to one `PendingTrim` core: generation, typed resize command, actual
submission FIFO count and viewport. UI deferral does not stamp submission;
matching actual submit stamps only after the pending count increments. Every
request completion counts, including typed gestures. The own terminal takes a
local viewport token before receive installs state and normalizes only after
canonical/display state is installed. Generation, exact explicit viewport
ownership and absence of another active trim protect newer document/navigation
state. No success-path original-clip equality, queue-empty heuristic or blanket
viewport reconciliation is used.

The previous accepted/refused release tests demonstrate actual hidden raw pan
resurrection after later project growth, using the real two-second/eight-second
service horizon. Added controls exercise real earlier ordinary/device/tuning
acks, an earlier ack while resize is still deferred, actual matching queue drain,
and a deterministic receive batch with own terminal then later growth and no
intervening render. They also preserve explicit fit/zoom/pan, actual new-document
replacement, current loop/open-gain display extents and a new pointer capture.
The latter local-token guard and generation/failure hook cases are direct
component controls; no impossible runtime interleaving or worker crash is claimed.
A full UI queue keeps its already-deferred owner; failure retirement is
idempotent and leaves independent queued work intact. Actual channel disconnect
and resize `SubmitError::Busy` are not fabricated (Busy currently applies only
to export submission).

PR403 extends this same record with revision/track/original/prospective/asset
preview metadata and display horizon. It must retain one key, counter and
viewport owner; its original preview/Undo tests and old native receipts stay
separate until the composed exact source is checked. Dedicated merger owns
combined qualification; physical viewport/capture/audio acceptance remains open.

## Returned FIFO and partial-clamp checks

Actual successful `RestartAudio` submission increments the same existing worker
pending count exactly once. Its earlier completion cannot retire a later trim,
and outstanding restart work fences new trim capture. Real offline Service tests
exercise the normal `audio adapter unavailable` completion without opening a
backend or substituting acknowledgments; refused submissions add no count.

A valid but clamped full timer candidate is distinct from a fully applied step.
The bounded prefix search finds the first viewport that reaches that same
canonical clip, so the view moves only by the actually applied edge displacement.
Unconstrained movement stays constant-work; each integer search takes at most
64 metadata-only probes. Real nonloop audio-source and MIDI-minimum tests cover
partial steps, stationary no-drift, release and Undo. No model minimum, snap,
source limit or prepared-DSP admission bound is relaxed.

The MIDI model's minimum is one Q32 beat unit. At the tested tempo its endpoints
both project to integer frame96000, and existing painting omits that zero-frame
rectangle. The regression asserts this exact projection and actual minimum
release/Undo, not invented visible geometry. Existing nonzero-extent rendered
cases still check alignment. This is not a new sub-frame rendering solution.

An unrelated lower audio-event editor must not take focus on an outside release
when it owns no gesture: doing so cancels a valid Arrangement trim before drop.
Its outside handler now changes focus only when it actually retires its own
audio-event gesture. Active outside cancellation still retains event focus,
clears its preview and submits no edit. Deliberate foreign-focus trim cancellation
remains unchanged.

## Pointer delivery after the original handle leaves view

Edge-pan can move the original canonical clip completely outside the viewport
while its prospective resize remains visible. Pointer updates belong to that
captured resize payload, not to the original handle's rendered lifetime. The
existing typed audio/MIDI move handlers now live on the stable editor root;
original handle listeners are removed to avoid duplicate updates. Current-owner,
stale/cancel, modifiers, canonical preview and valid-position checks are retained.
The timer and its edge-band calculation are unchanged.

Real rendered regressions first pan until the original handle is absent, then
move the held pointer into the interior: further ticks must leave the viewport
fixed while capture continues. Re-entering either edge resumes only its current
direction; a second interior entry stops again. Normal audio, MIDI, temporary
Shift stretch and enabled Warp cover both edges, exact displayed-to-worker
extent and one Undo/Redo. Existing Escape/stale/release/pending-horizon cases
remain separate controls. Track-height resizing is not a timeline-pan gesture.
These headless results do not claim native physical input acceptance.
