# midi editor pointer gestures

Status: public workflow specification; independent implementation in progress.

- Reference: Ableton Live 12 public manual, Editing MIDI sections 10.5.4/10.5.5,
  https://www.ableton.com/en/live-manual/12/editing-midi/ (read 8 October 2026).
- Installed target identity remains in docs/research/reference.md. This observation
  does not claim a fresh installed-app experiment or binary comparison.
- Public workflow: note body movement changes time/pitch; either edge adjusts note
  length; grid affects movement and sizing. Modifier/free-grid and multiselection
  need separate work. Notes may be auditioned; preview here initially means visual.
- Hex contract: preview on control/UI side, typed release-time canonical update,
  one undo transaction, retained opposite edge for resize, local clip bounds,
  retained channel/velocity and nonfinite/stale geometry refusal.
- Distinguishing acceptance: move an authored note both horizontally and vertically;
  save and inspect onset/pitch/gate. Resize each edge and inspect fixed opposite end.
  Undo returns the complete prior note. No-op/Escape/off-roll release submits none.
- No reference export/audio, sample-level matching or decompilation is needed to
  establish these basic public interactions; musical playback correctness retains
  independent engine/export tests.
- Remaining gaps: exact Live modifier/snapping thresholds, overlap overwrite,
  audition, multi-selection, extended navigation, measured dense input/paint timing and all
  expressive/recording workflows. Keep gaps visible rather than treating this
  initial implementation as one-to-one acceptance.

## grid workflow reference — 8 October 2026

Public Live 12 manual sections 10.2.1 and 10.5.4–5 were re-read for configurable
grid work. They describe preserving original placement offsets, initial free
movement until encountering a grid line, and temporary snap inversion with Cmd
on macOS. Hex currently preserves offsets by quantizing displacement relative
to the original pointer, with a no-op snap cell; that is not a measured match
for Live’s initial magnetic threshold. Configurable fixed/free steps are the
active independent implementation unit. Temporary modifier inversion is now source-tested; native saved-value acceptance
and exact magnetic threshold matching remain explicit compatibility work. This source reading is
not an installed Live behavior experiment or a decompilation result.

Native Hex evidence now includes all 600 visible notes, late-note edit/save,
six coincident-note cycling, both edges at a two-beat span and 880 × 540 window,
plus exact saved metadata and off-roll cancellation. Native held-drag Escape and
stale-window behavior remain open; pure state/geometry tests do not replace them.

## selection workflow reference — 8 October 2026

Re-read the same public Live 12 manual, sections 10.4.1 and 10.5.2. With draw mode
off, an empty click places an insert marker and an empty drag selects a time/note
region; notes are inserted by double-click. Shift adds to a selection or removes
an already-selected note; Cmd+A selects all and Escape deselects. Draw mode uses
single-click/drawing for insertion and clicking an existing note removes it.
These public behaviors guide the next native selection unit after atomic group
commands; current Hex single-click insertion is an explicit parity gap, not a
selection-mode behavior to retain as a substitute. Keyboard selection ranges,
pitch-ruler selection, note-boundary navigation and invert selection remain
separate follow-ups. This is a manual specification, not installed-app proof.

The manual uses Alt for temporary snap inversion on Windows and Cmd on macOS.
Hex currently uses GPUI's platform modifier (Control outside macOS), so Windows
modifier parity remains open; the initial native compatibility target is macOS.

## Continuous drawing specification — 8 October 2026

Rechecked installed bundle version: Live 12.4.6 (2026-09-10_0de5c8fa9a). Public
[Live 12 manual §10.4.1](https://www.ableton.com/en/live-manual/12/editing-midi/#draw-mode)
read on this date specifies pitch-locked and melodic drawing, with Option on
macOS temporarily inverting the preference. Melodic traversal can erase an
existing note; pitch-locked backtracking retracts drawn notes. This is public
specification evidence, not installed-app pointer experiment evidence.

An insertion-only stroke is insufficient for the requested reference behavior.
Atomic AddMidiNotes is useful for insertion/import and a drawing prerequisite;
complete drawing needs one mixed insert/remove transaction (or an equivalent
validated clip-note edit) so encountering existing notes cannot create multiple
undo steps. Interpolation between sparse pointer events, retracing, overlapping
note hit order, off-grid drawing and preference/modifier transitions require
explicit Hex contracts plus native/reference experiments. Keep those gaps visible
rather than silently substituting additive-only painting.

## Draw integration reference check — 8 October 2026

Re-read public manual §10.4.1 during native integration. It confirms pitch-lock
preference inversion with Option, melodic erasure and locked backtracking, but
not exact sweep rasterization, Off spacing or overlapping-note selection. A
read-only CUA inspection found Suite currently playing an existing user project;
no reference editing, preferences, stop, save or replacement action was taken.
Defer disruptive fixture experiments until the reference can be isolated. This
is an experiment availability limitation, not a blocker to original Hex drawing.
