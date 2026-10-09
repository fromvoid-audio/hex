# midi local loop native 001 — prepared thumbnails and source editing

Status: partially observed. Independent Hex acceptance; no observed Live parity claim.

Fixture: `artifacts/native-midi-local-loop-start-20261008.hex`, schema 10,
48 kHz, SHA-256 `4d14b17bfe6020282c30b32dafdd8fc1f58ec1533ff9bd10c388e3d0cc465da6`. Original MIDI only, no external assets. Preserve this
baseline and all existing app/user projects; use an isolated acceptance bundle.

1. Open the baseline and inspect full Arrangement clip 2. It occupies 0–6.5
   quarter beats, while its source extent is 8 beats. Prepared thumbnail should
   show the intro and three looped occurrences at frame intervals
   [12000,36000), [36000,72000), [84000,144000), [168000,216000).
   Tempo changes at beat 4; repeated notes must not look like equal frame periods.
2. Select clip 2, fit the source piano roll and inspect all four authored notes.
   Notes 3/6 at source 0.5/6 remain editable despite not playing in this loop.
   Source start is 1, loop brace [2,4). Compare source and Arrangement domains.
3. Start/seek/Stop and export with create-new filenames. Shared graph has exclusive
   audible end 218400 frames, including release after Arrangement end 216000.
   Compare exported PCM with the independent headless render before listening.
4. Pan/zoom across a clipped Arrangement viewport. Sample shapes normalize to
   visible clip bounds; they should not slide to the wrong position when the left
   edge is cropped. Inspect at 880×540 and normal desktop size.
5. After compact source controls are implemented, change one marker at a time,
   save create-new, reopen and compare the entire project. Undo/Redo each edit,
   retain hidden-note IDs/order/channel/velocity and allocator next_id 7.
6. Exercise the bounded dense-thumbnail fixture separately: numeric shown/total
   badge reports approximation, and the row cannot exceed 128 note shapes.

Current evidence: numerical loop/event/partition/export tests pass for this
contract; prepared thumbnail and native control integration are still being
validated. Native input delivery was previously unreliable in isolated apps.
No native screenshot, saved-state, listening or sustained device result is claimed.

## observed Hex acceptance — 8 October 2026

48 kHz headless render: 218400 frames, stereo PCM16, SHA-256
`b3702553ffb6894e824a2f1b487b8bf99340dcb5b6bf7a4a890ba078a7a9705c`.
Native bundle `hex-midi-loop-thumbnail-check-20261008.app`, executable SHA-256
`6bfe4c9d08f4c5cc7bf331645aaf3f90304e7189c51e3447dcb9d1426f08901a`.
Initial canvas was black; exposed native Raise/zoom repainted it. Cmd+O and
picker input succeeded. Active device is CoreAudio 44100 Hz stereo; opening
the 48 kHz baseline refused with sample-rate mismatch and retained empty state.

Created separate `native-midi-local-loop-44100-start-20261008.hex` changing
only sample rate, SHA-256
`edb77d06d7ebda04bbb1dce1b75b8185891fcccce77509e65160a4bf98fed380`.
Native Open accepted at revision 1. Arrangement shows intro plus three repeated
gates; tooltip reports four authored notes/four occurrences. Clip selection and
piano Fit show eight beats and all four authored source notes, including the
hidden-playback notes at source 0.5 and 6. No project mutation was performed.
No cropping/minimum/dense, marker editing/save, listening or sustained device
acceptance is claimed by these observations.

Follow-up: zoom-in plus Arrangement pan right crops the clip at beat 4;
two remaining sample gates display with a truthful `2/4` badge. Play button
advances transport/cursor (0.487 s observed), Stop returns to zero. This is
transport evidence, not latency/listening acceptance. Device-rate headless WAV:
200655 frames at 44100 Hz, stereo PCM16, SHA-256
`408c1a7e465417056024c9f16f428b0cd9c34ae13cc36a3dd004ad4ba327785b`.
Native zoom restore and drag to 880×540 produced a 1760×1082 capture, but
rendered content appeared clipped from the larger canvas with lower controls
absent; minimum-layout acceptance remains inconclusive. No speculative resize
workaround added. Clean revision-1 test app closed and inventory confirmed exit.

## compact source control follow-up

Isolated executable SHA-256:
`9ea0313fee5c989554f2846a51dd466a848597c72664fdfaa3f7ea90fdd52fe0`.
The explicit field/direction ID repair makes all five numeric + controls commit:
Arrangement 6.75, source 8.25, start 1.25, loop A 2.25, loop B 4.25 beats.
Loop off commits revision 7; Undo restores on at 8, Redo off at 9, Undo on at 10.
Native Save As creates `artifacts/native-midi-source-controls-accepted-20261008.hex`,
SHA-256 `3fc612c2704bb8f612f26f9b916b30ca16559826b4c6ea06745428ee8f28dd78`.
A full parsed-document comparison against the 44.1 kHz baseline proves exactly
those five numeric changes, with looping on and every other field unchanged,
including all four authored notes, IDs, tempo and track settings. Reopening
visibly shows the same markers. The final fit interaction was interrupted; an
acceptance click entered an extra note, and attempted Undo was interrupted too.
Do not treat that app's current state as a clean fixture or discard unknown edits.
Fit/all-note reopen and minimum-layout/listening gates remain open.
