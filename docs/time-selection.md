# arrangement time selection

Work unit, 9 October 2026. This extends the clicked edit cursor; native acceptance
and measured interaction timing remain pending.

Click empty track space to place a point selection. Drag horizontally to select
a half-open frame interval, and vertically to include contiguous tracks. Reverse
dragging normalizes the interval without changing the endpoint focus: the last
track reached receives a subtle background highlight. One-track selection focuses
that track. Audio and MIDI clips have a visibly shaded upper-third header with
an open-hand cursor for clip selection and movement. Their lower two thirds
accept point/time selection and double-click-and-hold clip painting, just like
empty track space. Narrow full-height resize edges retain trim priority. Only
the topmost rendered clip reserves its handles; obscured edges do not block
selection or painting through the visible body.
Inside a selected area, its upper third is a move handle, including over clips;
dragging below that handle replaces the selection.
A highlighted area is steady translucent cyan; only a zero-width point selection
smoothly pulses white/cyan with a 1.2-second cycle. Track dividers remain above these overlays. Selection is
transient view state, cleared on project replacement and invalidated track removal.

The selection uses the current adaptive grid and Command snapping override.
Cmd-C copies, Cmd-X cuts, Cmd-V pastes at the selection’s left boundary
(or at a point cursor when no time area is selected), and Delete clears the
area without closing the gap. These shortcuts apply at Arrangement workspace
focus; browser text input and MIDI note editor shortcuts keep their own context.
The upper third of each selected track area is normally invisible as a handle.
Hovering highlights it and shows an open-hand cursor; dragging there moves the
whole area. Below it, dragging creates a new selection, including from inside
the existing selection and over selected clips. These gesture zones own input
inside the area; clip headers resume individual clip dragging outside it.

## canonical edits and clipboard

`hex-project::TimeRange` identifies an ordered contiguous set of stable track IDs
and a positive frame interval. The Session control worker owns one internal
clipboard for the open project; no PCM or OS clipboard is copied. Successful
project opening clears it, while a refused open retains it. Copy changes neither
history nor compiled playback. Cut publishes the new clipboard only after the
candidate project compiles successfully. Delete, paste and move each commit as
one prepared history operation. Refused edits preserve the project and redo.
Undo restores musical content; the existing stable-ID allocator never rewinds.

Paste replaces the **entire destination rectangle**, including copied silence,
with the same track count and frame duration. Keyboard paste of a single copied
track uses the currently targeted row. Multiple copied tracks retain their
original stable track IDs, using only the new horizontal position. Intervening
inserted tracks remain untouched; missing source tracks refuse the entire edit.
Visual time-chunk dragging still maps to contiguous destination rows. Neither
path creates tracks implicitly. Source audio fragments preserve
source phase, offsets and inherited fade domains. New clips receive fresh IDs.
Overlapping moves capture before erasing either rectangle. Moving to the same
origin is a no-op that retains redo.

MIDI capture materializes selected playable note occurrences as finite one-shot
fragments. A gate crossing the selection start becomes a carry note that
retriggers there; source loop markers reset in the pasted fragment. Gates and
clip bounds shift in frames, then convert to destination beats, retaining audio/
MIDI alignment across tempo changes within the timebase's frame rounding. This
is deliberately bounded by existing clip/note/track limits. The expansion budget
counts all occurrences in each intersecting source clip, so a narrow selection
of a very densely looped clip may be refused. Authored loop/source metadata remain
in the original and Undo; exact Live editing parity or envelope continuity at
carry-note boundaries is not established.

Cmd-E continues to split the clip at the point cursor's anchor track. Splitting
both edges of a multi-track time area, system clipboard exchange,
delete time with gap movement and Session time-area editing are deferred.
Cmd-Shift-C copies the same time rectangle; Cmd-Shift-V inserts it and shifts
later content right on only the destination rows. Ordinary paste still replaces.
No schema change is introduced.

## acceptance

Source checks cover mixed audio/MIDI tails, overlapping moves, replacement of
silent cells, fresh IDs, tempo changes, finite loop capture, malformed destinations,
prepared rejection, undo/redo and worker clipboard lifecycle. Native checks belong
to the user: point blink and row dividers; forward/reverse area drags and endpoint
focus; cut/copy/paste/delete/move/undo; normal clip dragging and resizing; browser
and MIDI editor shortcut focus. Listening at sustained-note boundaries remains
separate from source tests.

Source acceptance: 729 workspace tests passed (2 ignored); after the final focus/
move-release repairs, 255 feature-enabled UI tests passed (2 ignored). Strict
workspace and feature-enabled UI Clippy, format, source guards and release desktop
build pass. Fresh review bundle: `artifacts/hex-time-selection-20261009.app`;
manifest: `artifacts/time-selection-manifest.json`. No native input/listening or
paint-time claim is made.

Selection refinement, 9 October: the existing 16ms UI timer samples a cosine
color interpolation with elapsed-time phase, requesting redraw only when its RGB
value changes. Nonzero time areas remain steady and do not drive pulse redraws.
Source tests cover intermediate colors, exact cycle endpoints, skipped timer
ticks, handle extent/hand cursor, reselection inside a previous area, and both
gesture zones over selected clips. Native animation smoothness remains a user check.

## time-chunk landing preview — 9 October 2026

While the upper-third handle moves a time rectangle, draw its audio fragments
and cached MIDI note shapes at the proposed destination. Hide the selected
portions at source and hide content inside the replacement destination rectangle;
keep outside tails visible. The preview includes gaps and preserves relative row
and frame offsets, including overlapping moves. At the original location or an
invalid project state, restore the ordinary source display. Outside pointer
positions clamp as described below. Keep a submitted
move preview until its worker result arrives, and restore the source on rejection.

`hex-project::preview_clip_fragment` slices only validated audio metadata with
the original clip ID, source mapping and fade domain. It does not mutate the
project or allocate PCM. UI previews borrow the existing waveform cache and
prepared finite MIDI thumbnails; pointer updates do not compile, capture a
clipboard or expand MIDI occurrences. Render work is limited to visible rows and
the corresponding source rows, with the existing MIDI shape budget.

Native checklist: move a mixed audio/MIDI time chunk across rows and earlier/later
in time; check source and destination tails, silent cells and overlapping moves;
leave the valid lanes and confirm the preview remains clamped; cancel and confirm
the original display returns; release,
Undo and confirm the preview matches the committed clip positions. Native
smoothness, detailed waveform appearance and listening remain user acceptance.

Native review, 9 October 2026: the user reported that the separately launched
`hex-time-chunk-preview-20261009.app` looks good and works as expected. This records
acceptance of the presented preview behavior, not measured paint/device deadlines
or sustained-note listening. The bundle manifest identifies clean source commit
`4181d68` and its release binary. Full local CI passed all 14 steps on that commit.

## selected-track ripple insertion — 9 October 2026

`InsertTimeRange` opens a gap equal to the copied duration on exactly its
contiguous destination row count, then places the full rectangle including gaps.
Audio crossing the insertion splits with phase/fade continuity; later audio moves
without changing its source mapping. Unselected rows and the global tempo map
stay unchanged. The worker uses the same project-scoped clipboard and prepares
once before one history commit; rejected edits preserve content, history and redo.

Whole MIDI sources keep IDs and loop metadata when every shifted event can be
proven to retain the exact frame translation under constant tempo. Otherwise,
finite occurrences become one-shot fragments with crossing gates retriggered at
the cut. Expansion is bounded and may refuse a densely looped source. Undo keeps
the authored source. No schema change or callback work is introduced.

Native checklist: copy a rectangle with sound and gaps using Cmd-Shift-C;
insert with Cmd-Shift-V at a point or the left boundary of a reverse selection.
Only affected destination tracks should shift, including a clip crossing the
insertion. Compare ordinary Cmd-V, then Undo/Redo. Check browser/note-editor
focus and saved/reopened metadata; sustained-note listening remains separate.

Native review, 9 October: the user confirmed ripple edit works in the fresh
`hex-ripple-insert-20261009.app` review build. Full local CI passed all 14 steps
on clean source `c7ae2d5`; PR #11 is merged. A follow-up request changes keyboard
normal/ripple paste of multiple tracks to retain the copied source tracks and use
only the current horizontal position. Single-track copies still target the
current row. That follow-up is now implemented in a separate feature PR.
Visual time-chunk drags retain cross-track
placement. Listening and measured device timing remain separate gates.

## Option-drag duplication — 9 October 2026

Option selects copy while dragging an Arrangement clip or a time rectangle's
upper-third handle. Command's free-placement override remains independent.
Pressing/releasing Option during the gesture changes the mode and plus cursor;
release uses the current mode. Existing browser sample drags already import copies.
Session cells and track ordering do not yet expose equivalent drag gestures.

Copies replace the destination rectangle, including silence in a time selection,
while retaining source portions outside that replacement. Whole audio/MIDI clip
copies retain authored source/loop metadata and use fresh clip/note IDs; time
copies use the bounded fragment contract above. Copying a time rectangle does not
change the worker clipboard. A drop is one prepared history operation, including
clip copies into newly inserted/appended tracks. Same-origin drops, cancellation
and rejected candidates leave content and redo intact. No persistence schema or
audio callback change is required.

Native checklist after the fresh build: Option-drag audio and MIDI clips on the
same/across tracks and into new tracks; copy a time rectangle containing gaps;
press/release Option while stationary, then release in each mode. Confirm the plus
cursor, source retention and snapped destination. Combine Option with Command,
try an overlapping destination, cancel a drag, and Undo/Redo each successful drop.
Source tests cover fresh audio/MIDI IDs and metadata, destination overwrite,
new-track Undo, time-copy gaps and same-origin redo retention, plus live modifier
changes and source-preserving previews. Project tests, 267 UI tests (2 ignored),
targeted strict Clippy, formatting and source guards passed. The focused PR owns
final clean-commit local CI evidence; the ignored review manifest records the
fresh bundle identity. Native cursor/input acceptance remains open.

## keyboard paste track anchoring — 9 October 2026

The worker clipboard retains the original source track IDs with its bounded
fragment metadata. Both normal and ripple keyboard paste use the source IDs for
multiple rows, or the current target for a single row. Copy and cut share this
policy; successful project opening clears the clipboard and its provenance.
Canonical source-track paste commands validate every row before mutation and
prepare one history operation. Visual copy/move commands keep adjacent-row mapping.

Project and session tests cover copy/cut, replace/ripple, single-row cross-track
placement, inserted intervening rows, missing-source refusal with redo retained,
and visual cross-track copying. Targeted strict Clippy, formatting and architecture
checks pass; final local CI evidence belongs to the focused PR. Native checklist:
copy two tracks, click a different row at a later time, and use Cmd-V then
Cmd-Shift-V; both should affect the original rows. Repeat with a single track and
confirm it pastes onto the clicked row. Undo each successful operation.

Native review, 9 October 2026: the user reported all other issues in the combined
`hex-editing-menu-20261009.app` build look good and authorized merging PRs #12–#16.
Those PRs are merged. This records acceptance of the reviewed editing/menu
behavior, not physical timing measurements or exhaustive listening. Dragging time
selections beyond visible lanes/window edges is a separately reported defect and
the active follow-up.

## captured time gestures and edge scrolling — 9 October 2026

After a valid left-button down, painting a selection or moving/copying its upper
handle stays captured until left-button release or explicit cancellation. Empty
space, the browser and out-of-window coordinates project onto the nearest fresh
visible track lane by y. Exact shared boundaries prefer half-open lane containment
with deterministic ties. Multi-row move targets clamp the whole block to existing
track indices. Transient missing measurements retain the last valid preview.
Horizontal pointer coordinates clamp to the current visible time span, and release
applies its final coordinates/current modifiers before finishing exactly once.

The existing 16 ms UI tick scrolls horizontally when the held pointer is beyond
an Arrangement edge. It preserves scale and clamps to the current project horizon;
no extra tracks or project extension are created by selection alone. Scrolling
uses 1.5 viewport spans per second with at most 50 ms elapsed work per tick, and
reprojects the same stationary pointer. Only timer ticks advance its clock, so
frequent/synthetic pointer events cannot suppress scrolling. Cancelled/stale
revision or generation state, mode changes and modal/project guards prevent pan.
Command snapping and Option-copy refresh without pointer motion. No authoritative
project operation, compilation or PCM work occurs during updates/ticks.

Source validation: 274 UI tests passed (2 ignored), strict UI Clippy, formatting,
architecture, GPUI patch and style guards passed. Tests cover both edges and limits,
interleaved pointer/timer updates, out-of-window release coordinates, nearest-row
projection, whole-block clamping, Command changes and stale/cancelled gestures.
Native checklist: paint and move a range below the last track; drag rapidly into
browser and back; hold beyond left/right edges while zoomed/panned; release outside
the window, then move normally and confirm no continued selection/scrolling.
Also try Command/Option and Escape. Native AppKit delivery remains user acceptance.

Native acceptance, 9 October 2026: the user accepted the captured-selection
review build `hex-time-gesture-20261009.app` and authorized PR #17 to merge.
It is merged as `a250f83`; exact source `25b797e` passed all 14 local CI steps.
Pulse ticket 38 records this completion. This establishes the reviewed interaction
acceptance, not physical input/paint timing measurements.
