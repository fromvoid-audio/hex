# sample bpm and arrangement edge stretch

Follow-up to Beats2771/PR379: Pulse2781/PR387. Selected resident Arrangement
audio clips have a sample bpm numeric field alongside Warp. Enable Warp, then
enter the source's original tempo (1..1000 BPM, up to six decimal places). This
is a declaration, not automatic tempo detection. Before declaration, the field
offers the local project tempo as a starting value. Warp off retains the saved
sample bpm and disables editing that field.

The first declaration maps the source region's duration to its musical length.
For example, a two-second source declared at60 BPM contains two quarter notes;
at project120 it lasts one second. Project tempo changes resolve the same saved
beat extent again. Later declarations scale the existing musical map by new/old
sample bpm, retaining custom marker proportions and any manual stretch factor.
The original tempo value is independent of the project's tempo and manual edge
stretching. Original PCM, source region, pitch and loop phase remain separate.

Hold Shift while dragging an ordinary Arrangement audio clip edge to stretch
instead of trimming. The opposite visible edge stays fixed. The retained root,
view and warp markers scale together, so existing trims and source loops keep
their mapping. An unwarped clip enables transient Beats for that edit. Existing
algorithm settings remain when present. Shift press/release at a stationary
pointer recomputes the candidate; release modifiers are authoritative. Header
Shift selection, MIDI edges and lower fade handles keep their separate bindings.

Pointer motion changes bounded metadata and visible preview geometry only.
It does not decode, render PCM, compile a revision or submit a worker command.
Release submits one canonical edit and one Undo step; the existing worker must
successfully prepare audio before publishing/committing it. Existing limits,
including supported stretch ratios and resident-only transformed audio, apply.
Esc, loss of focus, stale project/revision or an invalid drop cancel the draft.

## verification status

Initial model/storage tests pass for duration ratios, project tempo changes,
both edges, cropped roots, loop phase, pitch, one-step history and roundtrip.
Actual-input numeric/gesture, PCM and full exact-source checks are in progress.
Cloud6 owns the waveform/cache disappearance fix in2764/PR372. The existing
transformed trim overlay calls an original-source mapper which returns no ranges
for an audio-edit domain. The coordinated preview mapping must land before this
follow-up is considered complete. No substitute waveform or audio-preparation
work on pointer motion is acceptable. No native acceptance is claimed.

## finite coordinator-owned native check

Use one launcher-delivered optimized build containing the final merged PR387 and
waveform fix372. Verify their merge ancestry in the build's source checkout;
record build/source provenance. No separate cloud-triggered Mac build/install.
Use disposable projects and original copies of both a sample that previously
lost its waveform and an unaffected sample, including stereo percussion.

1. Select the clip, enable Warp/Beats and enter original sample BPM. Verify the
   two-second/source60/project120 example, fractional BPM, invalid input, Esc,
   Undo/Redo, save/reopen and project tempo changes. Listen for unchanged pitch.
2. Drag each ordinary edge with Shift, and press/release Shift without moving.
   Verify ordinary trim versus stretch, fixed opposite edge and continuously
   remapped waveform before mouse-up, including both zoom levels and stereo.
3. Check trimmed/looped clips and existing warp markers; test autoscroll, release
   modifiers, Esc, focus loss and stale-revision cancellation. Canceled previews
   must restore the original waveform and produce no history entry.
4. After release compare playback, seek and export; verify one Undo restores the
   previous interval/mapping and Redo restores the edit. Inspect attacks and loop
   seams and record audible defects with source/settings rather than claiming
   sonic parity.

Pointer/keyboard interaction and listening are required. Normal save/quit is only
needed if the coordinator replaces a running app. Never force-quit or discard
unsaved data. The coordinator schedules one local verifier; launcher retains
combined native delivery ownership.
