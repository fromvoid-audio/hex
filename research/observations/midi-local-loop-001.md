# midi local loop 001 — start marker, seam gates and release

Status: planned. Public manual establishes workflow; numeric parity is unmeasured.

- Reference: installed Live 12 Suite, last inspected 12.4.6
  (2026-09-10_0de5c8fa9a), universal arm64/x86_64 executable SHA-256
  5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949.
  Recheck inventory before running the experiment; this is prior provenance.
- Public source: https://www.ableton.com/en/live-manual/12/clip-view/#looping-clips
  identifies a start marker independent of the loop brace, permitting an intro.
- Host/sample rate/block size, exact fixture and output hashes: not recorded yet.
- Do not disturb an existing user set. Use a new disposable set with original MIDI
  data and an explicitly documented instrument; no factory/proprietary export.

## distinguishing fixture

Use source extent 8 quarter-note beats, start marker 1, loop [2,4), Arrangement
length 9. Place distinct pitches at 0.5, 1.5, 2.5 and 3.5, each gate 0.25.
The note at 0.5 distinguishes onset chase/hidden-note behavior; 1.5 identifies
one-time intro; 2.5/3.5 identify repetition and the partial final pass. Save a
baseline before changing the start marker to 3, which skips 2.5 on the initial
pass but permits it after wrap. Change only one marker at a time.

Then give the 3.5 note a gate of 1 beat, crossing the loop seam. Observe separate
note-off/retrigger behavior using a documented external MIDI capture path or
another independent observable endpoint; instrument audio alone does not prove
MIDI note-off timing. Compare sustained and short-release instruments separately.
Test starts/seeks on each side of the seam, transport Stop/restart and global loop
wrap. Repeat with a tempo change inside the source projection on the Arrangement
timeline; document absolute beat boundaries rather than assuming frame periods.

## original hex policy and acceptance

Hex's proposed finite projection starts [start,loop_end), then repeats the loop.
It does not onset-chase source notes preceding the start marker. Gates end at a
local seam or Arrangement endpoint, followed by the normal instrument release;
there are no implicit ties across seams. Each absolute beat onset/off is converted
independently through the tempo map. These are explicit original policies, not
observed Live results. Stable source note IDs plus occurrence iteration identify
repeats without allocating canonical IDs.

Project/compiler tests cover this arithmetic, empty/partial passes and bounded
repeat admission. Native source controls, schema roundtrip, external MIDI,
listening, sustained device timing and reference comparison remain required.
Finite Arrangement expansion does not meet ongoing Session playback requirements.

## results

No new installed-Live or native Hex loop experiment has been run. Measured
outcomes, capture alignment/tolerance, artifact hashes and comparison are pending.
