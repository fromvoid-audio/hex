# musical editing — pulse 27 / pr 185

The canonical issue requires clip gain and pitch, all six requested Live-style
warp modes, beat-anchored audio, source envelopes, metronome, scrub and loop
handles, snap nudges and drag autoscroll. The user clarified on 16 October 2026
that Beats and Re-Pitch come first, followed by Tones, Texture, Complex and
Complex Pro. Distinct algorithms and meaningful output checks are required;
mode labels alone do not establish either functionality or Live equivalence.

## implemented source stages

Clip gain uses the canonical model/history/storage and common resident/streamed
rendering described in [clip view](clip-view.md#canonical-audio-clip-gain-musical-editing-issue-27--pr-185).
Snap nudges retain [their directional landmark contract](snap-nudges.md).
Existing Arrangement transport-loop handles retain [their own contract](arrangement-navigation.md).
These stages do not complete the full issue.

The transient metronome belongs to interactive `Playback`, outside saved project
history and linear export. Its normal and accented 20 ms click samples are
prepared on control, with at most 30,720 sample bytes at 192 kHz. The owning
playback graph retires on control. Callback rendering retains scalar sample
position and expected Arrangement frame, without allocation or ownership copies.
Prepared canonical tempo and meter provide denominator-beat landmarks; meter
markers begin accented bars. Arrangement transport remains the clock when
Session clips own track routes. Seek, Stop, pause and loop wrap discard the old
click tail; a pulse exactly at the new position retriggers. Compatible revision
replacement preserves the toggle and tail; changed timing reanchors. Legacy
and linear-export graphs refuse enabling the click. The toggle is admitted
through the existing bounded command queue. The UI displays acknowledged state
and prevents another toggle until the current request is consumed or rejected.
The click is a monitoring signal after the musical mix, independent of track
and master gain. It is not a separate hardware cue output or a physical timing
acceptance claim.

Captured Arrangement clip move/copy, browser-audio and audio/MIDI edge trims now
scroll horizontally while a held pointer is within a timeline lane's 24 px edge
bands (capped to a quarter of narrow lanes). Scrolling uses the existing viewport
horizon, at 1.5 viewport lengths per second, with at most 50 ms credited to a
single delayed UI tick. It does not extend the horizon or scroll vertically.
Each payload shares its internally advanced viewport with the native drag;
source grab offsets and musical anchors remain captured. Preview recalculation
does not edit project/history. Release still uses the existing canonical edit;
manual viewport changes, source revision changes, busy edits and canceled drags
cannot apply a stale preview. Entering the browser or toolbar does not scroll.
Time-selection/range scrolling retains its existing independent contract.

Ruler scrubbing is drag seeking through the existing transport, preserving the
current playing/paused state. It is not stopped-transport audio audition. Pointer
events replace one latest target; the UI tick admits at most one seek after
pending command/publication and seek acknowledgment work clears. A released final
target survives queue pressure. Changed project identity, viewport/geometry,
focus, edits, Escape or Stop cancel future targets, without claiming that an
already admitted seek was undone. Prepared stateful seeks retain their existing
bounded worker and publication-fence lifecycle.

## reference and algorithm boundary

The required mode families and control behavior are described in the public
[Ableton Live 12 warping manual](https://www.ableton.com/en/live-manual/12/audio-clips-tempo-and-warping/#warp-modes).
In particular, Re-Pitch derives pitch from playback speed; its independent
transposition controls are inactive. An independent implementation must state
its own bounded algorithms and signal checks. No proprietary DSP code or assets
are copied, and a mode name does not establish matching output.

The installed reference was rechecked on 16 October 2026: version
`12.4.6 (2026-09-10_0de5c8fa9a)`, executable SHA-256
`5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`,
matching [the recorded reference](research/reference.md). This is identity
verification, not a new native behavior or listening experiment.

## verification and optional native checklist

Source tests and native evidence remain separate. Run the full local runner on
the clean final commit before readiness/merge; targeted development runs and
source guards do not replace it. Include the callback allocation probes for
metronome and clip gain. New tests cover click timing/partitioning/seek/loop/
inheritance/export refusal, rendered clip scrolling and stale cancellation,
bounded delayed ticks, and the gain cases listed in clip view.

The optimized exact-source candidate should be checked with this finite list:

- Adjust Arrangement and Session clip gain; hear independent levels, inspect
  waveform height and verify Undo/Redo and save/reopen.
- Enable the metronome while stopped and playing; check bar accents and tempo/
  meter changes, seek, pause, Stop and transport-loop wrap. Export stays click-free.
- Hold a clip move/copy or audio/MIDI trim near each horizontal edge; preview
  follows scrolling and release makes one edit. Cancel and Undo preserve source.
- Drag browser audio near an edge; the final import uses the scrolled destination.
- Check ruler drag seeking and final release after queued or prepared seeks.
- Exercise pitch, beat anchors, source envelopes and every requested warp mode
  only after their corresponding implementation and source checks are present.

Physical audio deadlines, listening quality, native interaction, Windows and
reference compatibility are not established by these source tests.
