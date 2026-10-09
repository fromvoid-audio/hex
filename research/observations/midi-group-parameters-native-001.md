# selected-group parameters native acceptance 001

Status: planned. Hex-only behavior acceptance; no installed-Live parity claim.

Use an isolated bundle built after the group parameter source checks, and open
the original `artifacts/native-midi-selection-start-20261008.hex`. Do not replace
any user's active or unsaved project. Keep the entire baseline JSON authoritative.

1. Select clip 2 and notes 6/7, leaving notes 8–12 unselected. Notes 6/7 start
   at 0.5/1.5, have gates 0.25/0.125, pitches 60/62, channels 2/4, velocities 64/96.
2. Click v+ once. Save create-new: only velocities become 72/104; IDs, order,
   channels, timing, other notes/clips and allocator next_id 13 remain identical.
   One revision/history action. Undo once restores baseline; Redo restores the edit.
3. Click length + once at 1/16 grid. Only gates become 0.5/0.375. Undo once
   restores the preceding state; Redo restores exact gates.
4. Click pitch up once. Only pitches become 61/63. Retain the two-note interval,
   timing, channels and velocities. Save and compare the complete project.
5. Select all notes; note 11 ends at clip length 4. Length + must refuse the
   entire edit, preserve revision, saved values and existing redo. Inspect the
   lowercase error rather than inferring refusal from note appearance.
6. Inspect the existing six compact parameter controls at minimum 880×540.
   Icons remain visible for groups; no extra toolbar row or dead space is added.

Pitch/velocity boundary cases, missing IDs and stale/pending admission require
source evidence too. Pointer group edge resizing and expression lanes are not
part of this parameter-button check. If automation cannot deliver input, leave
this protocol pending; successful compilation is not native acceptance.

## startup attempt — 8 October 2026

`hex-group-parameters-check-20261008.app`, identifier
org.portalsurfer.hex.groupparameterscheck20261008, uses the successfully built
desktop binary. The process remains running. Initial capture is a black canvas
with native traffic lights; Raise/zoom then paints the ready empty editor.
Restoring zoom succeeds, but Cmd+O still shows no file dialog. No fixture or user
project was loaded, edited or discarded. A one-second read-only process sample
finds the main thread mostly waiting in AppKit and editor update tasks executing;
this does not prove a deadlock or explain the input/initial paint gap.
All group-interaction acceptance checks remain pending on this same process.
