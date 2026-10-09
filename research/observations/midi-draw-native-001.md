# continuous draw native acceptance 001

Status: partial; remaining checks pending. Hex-only acceptance, not installed-Live parity.

Use a fresh uniquely identified bundle from the successfully built desktop binary.
Open `artifacts/native-midi-grid-start-20261008.hex`: first clip id 2 has four
quarter-note beats and no notes; clip 4 contains id 5 with channel 0/velocity 92.
Retain the whole original JSON as comparison authority. No user-owned project
or active Live session may be replaced or discarded for this experiment.

1. Select clip 2, focus piano roll, enable Draw; default 1/16 grid and pitch lock.
2. Drag from local beat 0.5/pitch 60 to beat 1.5/pitch 64. Expect five notes,
   starts 0.5, 0.75, 1, 1.25, 1.5; gate 0.25, pitch 60, channel 0, velocity 100.
   One revision/history publication. Save create-new; compare all project fields.
3. Undo once: clip 2 empty, all other musical/project fields preserved; allocator
   high-water intentionally remains advanced. Redo once restores exact new IDs.
4. Erase the authored run with another locked stroke. Save and compare emptiness,
   unchanged second clip and unrelated metadata. Undo restores exact authored IDs.
5. Toggle pitch lock off and draw a melodic/vertical path; save note start/gate/
   pitch metadata, verify preview/history counts rather than trusting appearance.
6. Release outside the roll; expect no new revision or changed saved project.
7. Inspect all controls at minimum 880×540; retain lowercase/bundled typography,
   selection counts and compact contiguous editor. Normal double-click remains.

Mixed baseline case: open a fresh copy of
`artifacts/native-midi-selection-start-20261008.hex`, Draw locked on pitch 60 from
0.5 to 1.5. Expect removal of exactly id 6 and insertion at 0.75, 1, 1.25, 1.5;
all existing notes 7–12 and clip 4 remain byte-value identical. Save, Undo, Redo;
Undo must restore original expression of id 6 (channel 2, velocity 64, gate 0.25).

The current documented CUA drag accepts only two points and no held modifier or
intermediate pointer samples. Live Option/Cmd transitions, held-preview Escape,
backtracking within one gesture and paint visibility during a held gesture require
manual evidence or another explicitly supported API. Source tests are useful
but do not close these native gates. Exact Off sweep/overlap parity is separate.

## partial result — 8 October 2026

Initial `hex-midi-draw-check-20261008.app` run: open original grid-start fixture,
select clip 2, enable Draw, drag raw screenshot coordinates [400,1244] to [980,1185]
in 1240×760 logical window (2480×1522 screenshot). Five notes appear on pitch 60;
revision 1→2 and five-note selection. Saved `native-midi-draw-locked-20261008.hex`
contains exact expected note IDs 6–10 but also further edits (next_id 41). Preserve
this file; it is partial initial-note evidence, not whole-project acceptance.

Fresh `hex-midi-draw-acceptance-20261008.app`, unique bundle identifier
org.portalsurfer.hex.mididrawacceptance20261008, loads the original fixture. Current
AX/screenshot shows revision 1, two clips and no selected clip. Native pointer
calls repeatedly fail window lookup despite successful read-only inspection and
running-process inventory; rebind by unique identifier also failed to restore
input. No restart or test fixture overwrite was performed. Resume on this exact
process after input recovers. All remaining native gates are still open.

Follow-up: the same process is confirmed running. Its AX tree and screenshot
still show the original fixture, revision 1, with no selected clip. The exposed
Raise action succeeds, and pointer calls now return without the earlier lookup
error, but single/double clip clicks and Cmd+O produce no visible editor/dialog
change. The fixture was not replaced or restarted. Input delivery remains
unverified; the process being alive is not evidence that a gesture was accepted.

Further read-only/window check: native zoom repaints the same process at both
maximized and restored size. The refreshed status now reports revision 2,
"project opened", no selected clip and no unsaved marker; the earlier cached
capture reported revision 1. The origin of that revision change was not observed.
Do not assume this instance remains a pristine acceptance fixture or discard its
state. Zoom alone does not establish successful clip input or saved-note data.
