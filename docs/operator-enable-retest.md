# Operator held-note listening retest — Pulse2372 / PR247

Status: repeatable fixture and checklist; native listening results are **not recorded**.
This follows [PR164](https://github.com/PORTALSURFER/hex/pull/164) without
changing its enable implementation. Broad live MIDI/recording #26 remains separate.

## Supported input route

Use canonical Arrangement MIDI playback through the native editor, not live MIDI
or computer-keyboard note input. An authored positive-duration note supplies the
held gate and its scheduled NoteOff supplies release. This can exercise enable
changes during both stages while leaving the pointer free for the Operator panel.
It does not test the physical timing or lifecycle of future live input.

Generate disposable native projects from the candidate checkout:

```sh
mkdir -p artifacts/operator-enable-retest
for mode in analytic serial feedback free sync; do
  cargo run --locked -p hex-session --example operator_enable_fixture -- \
    "$mode" "artifacts/operator-enable-retest/$mode.hex"
done
```

The helper refuses to overwrite an existing project. Keep each edited test copy
under a new filename if repeating the original fixture is necessary. It uses the
registered Operator codec, canonical commands and ordinary project storage; no
new input/runtime path is introduced. Its source tests establish fixture timing,
persistence and audible gate/release followed by silence, not toggle correctness
or listening acceptance.

Each project has one MIDI track, no external media, one C3 note (MIDI 48), a
2-second lead-in, a 30-second gate and 3-second oscillator releases. A longer clip
keeps playback available after the tail. At the default 120 BPM, note onset is
beat 4 and NoteOff is beat 64 (beat positions here are zero based).

The analytic, feedback, free and sync fixtures use four parallel carriers so every
badge can affect an audible contributor. Feedback is authored at 0.3 on each
oscillator. Free LFO uses 1 Hz; sync uses one quarter-note beat per cycle; both use
35% pitch amount. The serial analytic variant exercises removal of modulation
without rerouting the authored graph. These are original diagnostic patches,
not measured Live presets or numerical reference laws.

## Exact candidate and native test

Use the optimized stable development app supplied by
[the native delivery workflow](test-builds.md). Record its bundled
`test-build.json`, full source revision, package version, included PR heads and
full local-CI report. Verify the version/revision shown by the running app after
launching that candidate. Disk installation alone does not establish the running
identity. Preserve any unsaved work; quit/relaunch normally if needed.

Open each generated `.hex` through the native project-open command. Show its track's
Operator device panel. Test one project at a time; changing processing class in
the middle of a held note is outside this enable-only check. Replay from the
beginning whenever more time is needed.

1. Play from zero and hear the note enter after the lead-in. During its gate,
   click each a–d badge off/on separately. In parallel patches each removes and
   restores its contribution; in serial, disabling a stage removes that stage's
   outgoing modulation/carrier contribution without bypassing it. Check for smooth
   response and no duplicated onset. Record each oscillator's result in each mode.
2. Reverse several toggles rapidly while still held, including an off/on/off
   sequence. Check for clicks, jumps, stuck sound or restoration of an older mask.
   End with all enabled and verify authored levels, routing, tuning, feedback,
   envelopes and LFO settings remain unchanged. Try Undo/Redo of a badge edit.
3. Let NoteOff occur at 32 seconds; toggle during the following 3-second release.
   Verify a continuing tail rather than a fresh attack. After the tail, toggle
   again: no note should resurrect. All-off may leave already-detached/effect
   tails under the [documented ownership policy](operator-enable.md).
4. Stop during a held note, then restart; repeat during release. Seek before the
   onset, into the held interval and past NoteOff, then restart. Record the
   observed sound and any explicit refusal/error. Mutable feedback/LFO seeks use
   the controller's bounded worker preroll; a refusal is not a passing result.
   Existing [LFO discontinuity limitations](operator-lfo.md) and separately tracked
   Stop issue #2305 must remain visible if encountered.
5. Save a disposable edited copy and reopen it. Verify the authored settings and
   final enable mask, then replay to check for stuck/resurrected notes. Provide
   explicit correctness confirmation or a precise failing step for every mode.

## Result record

| Field | Result |
| --- | --- |
| Package version / full source | pending |
| Running identity verified | pending |
| PR247 head / related PR164 | pending / historical source context |
| Full local-CI report / unchanged digest | pending |
| Optimized app receipt / installed path / launch | pending |
| Fixture files / SHA-256 | pending |
| Input route | authored Arrangement MIDI gate / scheduled NoteOff |
| Analytic parallel a–d / rapid reversal / release | not tested |
| Analytic serial modulation / reversal / release | not tested |
| Feedback a–d / reversal / release | not tested |
| Free LFO a–d / reversal / release | not tested |
| Synced LFO a–d / reversal / release | not tested |
| Stop / seek / restart in each mode | not tested |
| Authored settings / Undo/Redo / save/reopen | not tested |
| User's explicit correctness confirmation | pending |

If the authored gate cannot reliably exercise the reported interaction, retain
Pulse2372's testing blocker and record the failing action. Define only the missing
bounded gate/input seam; do not pull the whole #26 roadmap into 0.1. Automated
render/allocation tests, packaging and merge do not fill any native result cell.
Physical audio deadlines and Live parity remain separate claims.
