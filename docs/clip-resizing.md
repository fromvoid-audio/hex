# live clip resizing

Audio/MIDI source implemented, 9 October 2026. Native acceptance remains pending.

Audio resizing uses the existing canonical `TrimClipEdge` command and
`Clip::preview_edge_trim` validation. The preview is display-only during the
gesture; release enqueues one edit. Preserve source bounds, looping phase and
the deliberate fade-reanchoring policy in clip-fades.md. Invalid or stale
candidates must not change the project. An unchanged edge is a no-op.

The former 20-pixel eligibility threshold is removed. Invisible full-height
left/right edge strips use up to 4 pixels each, narrowing with the rendered
clip width (minimum 3 pixels for audio, 4 for MIDI) to preserve at least one
pixel of central move target. Between the edges, the upper-third header moves
the clip; the lower two thirds accepts time selection and clip painting. Distinct
original left/right bracket cursors identify each edge on macOS; no colored handles or extracted reference cursor assets are used.

During a gesture, the fixed original hit geometry stays active but transparent;
a separate in-track clip preview redraws its geometry and waveform/note marks.
There is no floating numeric drag readout.

Use the existing Arrangement grid resolver for the edited edge: adaptive/fixed
when enabled; Command temporarily inverts snapping. Pressing or releasing the
modifier at a stationary pointer must update the preview; final release uses
actual release modifiers. Grid visibility remains independent of snapping.

This modifier rule follows the official Live 12 manual, section 6.10:
https://www.ableton.com/en/manual/arrangement-view/#using-the-editing-grid
Narrow handle geometry and live preview are independently implemented Hex
interaction choices. Exact Live hit thresholds and extra magnetic targets are
not established. MIDI Arrangement resizing now uses `TrimMidiClipEdge` and a metadata-only
`MidiClipTrim` preview; source marker and loop-phase rules are in midi.md.
Authored notes remain intact when hidden by the resized Arrangement region.

## native review

1. At a wide and a zoomed-out clip, drag the header to move it; use either full-height
   resize edge to adjust its edge. Header movement must remain easy to grab.
2. While holding the edge, check that clip geometry and waveform update before
   release. Resize inward and outward within the source limits.
3. Check grid-aligned resizing; press and release Command while holding still,
   then release with Command held. Preview and resulting edge should agree.
4. Cancel with Escape, then resize and Undo/Redo. Cancellation must preserve the
   original clip; a completed edit must undo in one step.

Source tests establish candidates, rendered geometry and simulated event
behavior. User native tests establish physical interaction and appearance.


## audio/midi resize source milestone complete — 9 October 2026

Thin bottom-corner handles (up to 4×6px; narrow widths derived from the rendered
3px audio/4px MIDI minimum) preserve the upper move target. In-lane previews
redraw waveform/note geometry immediately; original hit geometry remains fixed
and transparent. Existing grid/Command inversion updates during motion,
stationary modifier changes and authoritative mouse-up. Release commits one
validated edit; cancellation/no-op/stale state preserves canonical data.
MIDI metadata-only previews retain authored notes and source/loop markers.

233 UI tests pass, one ignored after the final narrow-width repair. Workspace
integration: 695 tests pass, one ignored before that UI-only repair. Project/
storage/session/UI strict lint, desktop build, fmt and architecture/UI-style/
GPUI-patch guards pass. Logs and source/binary identity are in
`artifacts/clip-resize-manifest.json`. Existing upstream compiler warnings remain.

Review `artifacts/hex-clip-resize-20261009.app`; native audio/MIDI left/right
edge usability, real-time appearance, Command release agreement and Undo
acceptance remain open. Checklist: docs/clip-resizing.md. No commit/push or
automated native interaction; broader roadmap stays paused.

Full-height edge source checks pass: 233 UI tests/one ignored, strict UI lint,
desktop build, fmt and architecture/style/GPUI patch guards. Review build:
`artifacts/hex-clip-edges-20261009.app`; identity in
`artifacts/clip-edges-manifest.json`. User native cursor/edge usability check
pending. Uncommitted; prior apps and unsaved work preserved.


User acceptance: the full-height invisible edge build works, as reported on
9 October 2026. The following visual iteration replaces the shared resize
cursor with distinct brackets and makes clips flush with a black border.
That new appearance, cursor scaling and through-clip grid remain pending review.
