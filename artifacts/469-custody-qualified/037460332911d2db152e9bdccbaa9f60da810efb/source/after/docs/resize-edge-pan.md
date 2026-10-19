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


## Captured release outside the source track (Pulse2867 / PR469)

A captured Arrangement trim has one source clip and track. Moving vertically into
another track, a header, ruler, blank region, browser or another panel does not
retarget or cancel that owner when the same window delivers its left-button
release. The existing root release path now finalizes against the captured source
track. Horizontal positions within its timeline use the same final-pointer and
modifier resolver; foreign horizontal positions retain the existing valid
timeline point. Model/source/Warp admission, pending FIFO custody and canonical
worker completion remain authoritative. There is no new release/timer owner.

Real GPUI input and Service completion checks cover both edges for audio, MIDI,
Shift stretching and enabled Warp, exact displayed/committed geometry, untouched
other tracks, and one full-snapshot Undo/Redo. Controlled pan tests also unmount
the original handle, stop on interior entry, and release over the actual ruler.
The former other-track/outside cancellation tests are intentionally updated for
this new user requirement. Escape, stale generation, lost focus and actual worker
refusal retain their separate cancellation/retirement controls.

Preserved diagnostics distinguish two actual other-track failures and the
corrected changed-extent generic-outside failure from an earlier no-op fixture
assertion and an initial test-only private-field compile error. Physical OS
outside-window delivery, optimized waveform appearance and native capture remain
Root-owned acceptance; cloud input does not establish platform-wide capture.

### upper-rail legacy oracle correction — pulse2867 / pr469

The previous `rendered_audio_trim_release_outside_cancels_without_edit` case
asserted the obsolete cancellation law at a point proven outside the browser,
Arrangement body and hovered row. It now tests captured-source acceptance on the
same upper rail. This case uses a real disposable WAV/Service project and actual
worker events, retains the region proofs, and verifies exact ClipId/left edge/
release modifier/command frame, unchanged canonical state before ACK, pending
preview ownership, exact accepted clip and untouched other track, and one
Undo/Redo of the complete canonical snapshot. No production behavior changed.

The fixture's raw command point is 192000 while the ordinary trim model projects
that point to the valid one-frame extent 95999..96000. It intentionally retains
the exact command, rather than reconstructing it from displayed geometry. A
first test-only equality assumption between these distinct values failed and is
retained as diagnostic evidence; it was not a new production failure. The
original required full-run failure remains failed/historical until the merger
qualifies a fresh combined candidate. Native captured-release acceptance remains
separate, as do the inherited focus-cancel paint issue and warning-cleanup PR474.

The PR469 upper-rail regression fixture now creates one existing test Environment
before Service/import/startup and injects that same Arc into the editor. Its WAV,
samples, four preferences and derived catalog all belong to that root. The
existing pre-ACK retention pattern disarms only on the actual matching successful
Import response (48kHz, 96k frames). A paused real catalog save proves the root
survives view/caller retirement until filesystem completion, then cleans up;
a real missing-import refusal preserves evidence without leaking an Arc. This
is fixture custody, not a Service join or new production release behavior. The
original pointer/geometry/typed192000/pre-ACK/worker/UndoRedo assertion block is
byte-identical. Earlier 56-case evidence remains tied to its original source.
