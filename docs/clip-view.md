# unified clip view

Requested 9 October 2026. User audio/MIDI screenshots define the visual direction:
a compact shared bottom panel, source parameters on the left, detailed waveform
or note editor consuming the remaining width, fine dividers and small controls.
All app text remains lowercase and uses Ioskeley Mono. Native visual acceptance
belongs to the user; matching the pictured composition does not establish DSP
or reference parity.

## targeted delivery sequence

1. Shared open/close toggle, retained selection, audio waveform/detail and the
   existing MIDI editor in one bottom-panel slot. Reuse current canonical
   source/Arrangement edits and note controls. Add only the narrow audio loop
   setter needed to expose existing finite source looping. Show actual source
   start/end, play start, Arrangement position/length, and media metadata that
   the canonical model actually retains. Looped audio play start includes its
   source phase; source offset alone is the loop-domain start. Source-interval
   edits reset phase by the existing TrimClip contract and must not masquerade
   as an independent looped play-start setting. Preserve Session MIDI and existing
   unwarped Session audio controls. No decorative gain/pitch/warp controls.
2. Audio clip gain as a canonical saved/history parameter, wired through common
   native/offline rendering and bounded prepared compilation. Review schema
   migration, source/track/master gain interactions, waveform gain visualization
   and neutral-path equivalence together. This is clip gain, not track volume.
3. Audio pitch controls with declared semitone/cent units and an explicit
   unwarped playback-rate/source-length policy. Cover source conversion, loop
   phase, trims/splits, source views, compiled replacement and save/undo. Do not
   change duration implicitly before that contract is defined and tested.
4. Warp enable and real time-stretch modes, with tempo/source beat anchors,
   bounded background preparation, latency/readiness reporting, source cache
   ownership and complete native/offline/save/undo behavior. Do not equate pitch
   resampling with tempo-preserving time stretch or expose inactive mode menus.
5. Detail gestures and numeric entry refinement, including source/loop braces,
   play-start markers, detail zoom/pan and applicable MIDI note tools. Keep each
   PR targeted and preserve tested interactions while adding the next one.

Pitch, warp modes, broad Live transformations, envelopes/MPE, groove and
scale workflows are requested follow-ups, not implemented merely by displaying
a panel. Full Live clip-view equivalence remains an acceptance goal. No new
proprietary UI assets or extracted application code are needed for this layout.

## detail focus and visibility

Pulse2117 keeps pane visibility independent from its current content. An explicit
Arrangement or Session clip focus selects clip detail; an explicit track focus
selects devices, even when an older clip selection would otherwise remain. A
scene-level focus with no applicable track or clip shows the compact empty state.
Changing focus never opens a closed pane or closes an open pane. When a clip
selection disappears, the open pane shows a truthful empty state at the last
valid clip-detail body height (228 px initially, or 38 px after Session audio).
Device content uses the fixed 213 px geometry. These heights are view-local
and are not persisted. Worker snapshots, selection reconciliation and Undo/Redo
do not choose the pane content; they may clear a selection, leaving the open pane
truthful about the missing context.
Switching away from device controls uses the existing gesture-finalization path,
and switching away from the MIDI editor returns keyboard focus to the editor.
Session cell/clip keyboard navigation uses the same explicit context transitions
as pointer focus.

Source checks cover real Arrangement track/clip clicks, a closed-pane track focus
followed by explicit reopen, Session keyboard focus, stable empty-pane height after
selection removal, and worker snapshot/Undo selection reconciliation. Native
track/clip focus and minimum-layout checks remain for user acceptance.

## first panel acceptance

- One compact icon toggle opens/closes audio and MIDI detail without changing
  project data or selection; MIDI shortcuts follow actual focus/open state.
- A selected Arrangement audio clip shows a nonzero-size detailed cached stereo
  waveform, time ruler, source region and start/end/play-start information.
- Supported parameter edits dispatch canonical validated commands; no direct UI
  project mutation. Generation/revision and pending-edit checks prevent stale
  widgets from overwriting newer values. Errors preserve the project/history.
- Audio source/Arrangement/loop edits and MIDI source/note edits are undoable and
  round-trip through existing metadata. Unsupplied fields are not fabricated.
- Existing Session controls remain usable; no duplicate detail panels. Empty
  selection uses minimal space. Native small-window appearance, focus,
  selection switching, toggle behavior and source edits remain user checks.

## first panel implementation

The shared chevron retains selection while showing/hiding the bottom detail pane.
Closing it cancels active MIDI gestures and returns focus to the Arrangement,
so hidden notes do not receive editing shortcuts. Arrangement audio shows a
cached stereo waveform across the full source, a seconds ruler and source/end/
play-start markers. MIDI uses the existing note editor with source controls in
a compact left column. Session audio retains its existing source panel.

Arrangement audio controls edit position, duration, source in/end, play start
and looping. Step buttons use one millisecond by default and one project frame
with Option held; direct numeric entry and draggable markers remain follow-ups.
For a looped clip, play start changes source phase within the loop interval.
For a one-shot, it changes source in while retaining source end. Disabling
looping resets phase to zero. Source/length trims retain the existing phase-reset
contract. Commands are canonical, validated and undoable; captured revision,
selection and generation reject stale controls. These use existing saved fields
and require no schema change. Pitch and warp remain separate stages. Audio clip gain is described below.

## source verification and native checklist

Workspace regression: 715 tests passed, 1 ignored. Feature-enabled GPUI suite:
248 passed, 1 ignored, including rendered toggle/focus, waveform layout and stale
control rejection checks. Typed audio loop/phase tests cover invalid bounds,
undo/redo and no-op/stale history behavior; existing storage tests cover saved
loop/phase fields. Strict affected-crate lint, desktop build, formatting and
architecture/UI/GPUI guards pass. These establish source behavior, not native
visual or listening acceptance.

Separate review bundle: `artifacts/hex-clip-view-20261009.app`; identity/check logs:
`artifacts/clip-view-manifest.json`. User checks:

- Select audio, then MIDI: correct detail appears; toggle closed/open retains selection.
- Audio waveform, ruler and markers are visible; source/play/loop edits update
  the clip, Undo restores it, and save/reopen retains values.
- MIDI note edits still work; collapsed MIDI does not receive note shortcuts.
- Check a small window and existing clip drag/resize; Session source panels still open.

Pitch, beat anchoring, source envelopes and warp now use the separate
[musical audio contract](musical-editing.md#canonical-musical-audio). Detail
zoom/pan, draggable source markers and direct numeric entry remain targeted changes.
This panel is not full Live equivalence.

## canonical audio clip gain (musical editing, issue 27 / PR 185)

Arrangement clips and Session audio one-shots own independent `ClipGain` values:
integer millidecibels in -60,000..=24,000 (-60..=24 dB), with exact unity at 0 dB.
This is a finite attenuation/boost control, with no mute sentinel, limiter or
normalization. It does not change source bounds, loop phase, duration, tempo or
pitch. `SetAudioClipGain` validates either audio domain by its stable clip ID;
invalid edits preserve metadata and history. Trim, split, duplicate and audio
copy operations retain gain, including Arrangement-to-Session copies. Undo/Redo
and snapshots retain the exact integer value.

Project schema 25 requires a complete ID-keyed `audio_clip_state` table containing
validated integer gain and required-nullable transform state. Historical main
schemas migrate audio gain to unity; strict, explicitly selected development
20/21 readers preserve their authored gains. Missing/null/noninteger or
out-of-range current gain refuses admission. Existing recoverable save/journal/
publication paths remain in use; see the [storage contract](../crates/hex-storage/README.md).

Preparation converts dB to one finite f32 coefficient on the control thread.
The shared Arrangement resident/streamed kernel applies fades, then clip gain,
then track processing/balance/volume, sends and master output. Session uses the
same source-stage gain before its track chain. Native prepared playback and
offline render/export consume these prepared paths. Unity keeps the existing
compact clip path and sample operation order; no coefficient conversion,
allocation, media mutation or history work occurs during rendering. Changing
only Session gain retains the current one-shot launch anchor across compatible
revision replacement. Boosted overlapping sources may exceed full scale;
existing output quantization/clamping and engine overflow limitations still apply.

Both audio detail panels expose real `gain` step buttons: 1 dB by default,
0.1 dB with Option. Captured revision, generation, selection and pending-edit
checks protect stale widgets. Arrangement clip/detail waveforms and Session
source bars scale cached source peaks by clip gain, with visual clipping to the
available channel height; the underlying waveform/media cache remains unchanged.
Retained geometry includes gain in its cache identity. Fades retain their own
existing overlay rather than changing the stored source peaks.

Source regressions cover range rejection/history, source edits and copies,
strict persistence/migration, exact unity PCM, gain with overlapping clips/fades/
track/master levels, partitioned resident/streamed parity, Session replacement,
and rendered gain command/stale-widget dispatch. They establish source behavior;
native visual/listening acceptance is separate. Optional user checks for the
final optimized build:

- Select an Arrangement audio clip; increase/decrease gain and use Option for
  fine steps. Hear the level change and inspect waveform height; 0 dB restores it.
- Undo/Redo, split/trim/copy, and save/reopen: the gain values and source timing
  remain correct. Overlapping clips keep their independent levels.
- Launch a Session audio clip and adjust gain during playback: level changes
  without restarting the one-shot. Check the source bars and save/reopen value.

Pitch, warp, beat anchoring, source envelopes and remaining issue 27 gestures
remain separate work; this stage does not establish full musical editing.

Clip selection and Command-R inline names are described in [clip names](clip-names.md).
