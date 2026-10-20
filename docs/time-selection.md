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

Time-range paste replaces the **entire destination rectangle**, including copied silence,
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

## whole-clip keyboard clipboard — pulse2770 / PR378

Click a clip header, optionally extend the clip selection, then press Command-C
to capture complete clips. A deliberate header click clears an unrelated time
area; body time-selection gestures retain the rectangle contract above. Copying
without a time area uses selected whole clips. Cut still requires a time area.

Click elsewhere on a track to place the edit cursor, then Command-V pastes at
that time even though the source selection is now cleared. The worker retains
the immutable payload independently of view selection and OS text clipboard.
Repeated paste remains available. Successful whole-clip or time-range copy
replaces this one project-scoped payload; refused copy preserves its predecessor.
Project replacement clears it as before. Text inputs retain their own keys.

Whole clips preserve full source/loop/fade and note metadata, relative start
times and gaps. Every paste allocates fresh clip/note IDs and commits one atomic
Undo operation. Existing canonical placement, overlap and ripple operations
remain in use: pasted extents replace old destination content, while gaps are
untouched and valid overlapping copied members do not erase each other. A single
copied source track follows the clicked target; copies from multiple source
tracks retain their original stable TrackIds and use the new horizontal origin,
for both normal and ripple paste. No track creation or cross-track remapping is
introduced. Missing assets/tracks, invalid bounds and exhausted limits refuse
the complete edit; no PCM or file conversion occurs during copy.

The rendered regression reproduces an unrelated old range pasting 12,000 frames
instead of the selected 48,000-frame clip, then passes after the routing repair.
Single/multiple clips exercise real worker copy/paste, destination clicks,
relative gaps, fresh IDs, one Undo/Redo, repeated paste, text focus and disposable
save/reopen. Model tests cover retained content after source removal, mixed
audio/MIDI overlaps and domains, stable multi-track normal/ripple ownership and
atomic overflow refusal. Exact final required checks are recorded in Pulse/PR.

Native acceptance remains with the coordinator's serial Mac lane on a future
approved optimized candidate containing PR378: copy one clip then several clips,
click a different time/track and paste, repeat, compare full content/gaps, Undo/
Redo, and confirm text-input copy/paste does not replace the Arrangement payload.
Use a disposable project for save/reopen. Record the actual source/build and a
short interaction capture; this does not require saving/quitting or replacing
the user's current running app. No native acceptance is claimed here.

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

Command-D in the focused Arrangement duplicates a positive time selection at its
exclusive end, using the same atomic `CopyTimeRangeTo` edit as Option-drag time
copy. This is normal overwrite paste, not ripple insertion: destination contents
inside the copied rectangle are replaced, while copied gaps and partial source
windows follow the existing time-chunk rules. Source rows are the existing
contiguous time-selection rectangle; the main-track proxy scopes all Arrangement
tracks. Audio and MIDI fragments are supported; unsupported automation transforms
refuse through the canonical edit. The clipboard is unchanged.

After successful acknowledgement the selected rectangle advances to the copy,
so another press repeats it. Pending edits must finish before another duplicate;
overflow and invalid ranges make no partial edit. MIDI/detail/session and text or
numeric focus retain their own commands. Without an active Arrangement range,
the existing clip/scene duplication behavior remains.

## duplicate selection ownership (pulse2880 / pr482)

Successful Command-D and Option-drag copies retain the destination time rectangle
through the worker acknowledgement and subsequent cursor/repaint ticks. A valid
completed range deliberately has no primary clip; ordinary stale-clip
reconciliation must not replace that choice with the first original clip.
Unselected point cursors, explicit whole-clip creation/selection and generic
stale-track fallback keep their existing behavior.

The pending copy's selection intent applies only while its original source range,
project scope, cursor generation and captured selection-intent epoch still own
that selection. Actual pointer range/cursor choices, deliberate track/clip
selection and range cancellation advance a checked view-local epoch. Successful
Console cell/scene clicks and keyboard navigation advance it at their accepted
user-input boundary; shared selection helpers and internal worker ACKs do not.
Tab alone, refused pending-edit clicks and navigation without a target keep it.
Returning A→B→A to identical coordinates cannot revive an old ACK's selection side effect.
Worker acknowledgements and late paint/cursor ticks do not advance the epoch,
so unchanged-intent copies continue chaining from each destination. Exhaustion
permanently invalidates the epoch for that editor instance and refuses new range
move/copy commands before admission; it never wraps. A newer pointer range or
Escape supersedes that view intent. An already accepted copy
still commits normally and remains one Undo operation; this does not cancel or
roll back the canonical command. Escape before submitting an Option-drag preview
queues no edit and preserves the previous selection.

Actual input/Service regressions reproduce both original primary-clip reselection
and a late copy acknowledgement stealing a newer range on another track.
Additional delayed-ACK keyboard and Option-drag regressions return via actual
pointer input to the identical source rectangle before consuming the real Service
ACK, and verify the committed destination survives while the later A stays
selected. Delayed Arrange keyboard/Option-drag copies also preserve a newer
Console MIDI choice made by actual arrow navigation; Tab-only, refused-click
and no-navigation-target controls retain unchanged intent. A near-exhaustion
fixture exercises actual later selection, ACK invalidation and subsequent copy
refusal without wrapping. Coverage
includes three consecutive Command-D copies, Option-drag followed by Command-D,
single/multiple rows, partial audio and MIDI with a trailing gap, exact asset/
source-offset preservation, fresh IDs, rendered destination bounds, later frames,
Undo/Redo and cancellation. Fixtures use one precreated private environment for
media and constructor paths before workers; no fabricated canonical event or
acceptance is used. Pointer rounding oracles derive from the accepted frame range,
not a guessed pixel-to-frame equality.

Finite coordinator/user follow-up in a qualified build: repeat Command-D three
times over a partial mixed-track range with gaps; Option-drag it and immediately
duplicate again; inspect the stable destination range after worker completion,
then Undo/Redo. Select another range and return to the original range while a
copy is pending; verify Escape before/after submission preserves the documented
distinction. While a copy is pending, Tab to Console and use arrows to select a
MIDI cell; its selection should survive the copy ACK. Native paint,
installed-build and physical interaction acceptance remain separate from headless
source checks. Existing historical PR367 and its evidence are preserved.
