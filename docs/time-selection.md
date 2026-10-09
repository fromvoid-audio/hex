# arrangement time selection

Work unit, 9 October 2026. This extends the clicked edit cursor; native acceptance
and measured interaction timing remain pending.

Click empty track space to place a point selection. Drag horizontally to select
a half-open frame interval, and vertically to include contiguous tracks. Reverse
dragging normalizes the interval without changing the endpoint focus: the last
track reached receives a subtle background highlight. One-track selection focuses
that track. Outside an existing time area, clip bodies and resize edges retain their drag priority.
Inside a selected area, its upper third is a move handle, including over clips;
dragging below that handle replaces the selection.
A highlighted area is steady translucent cyan; only a zero-width point selection
smoothly pulses white/cyan with a 1.2-second cycle. Track dividers remain above these overlays. Selection is
transient view state, cleared on project replacement and invalidated track removal.

The selection uses the current adaptive grid and Command snapping override.
Cmd-C copies, Cmd-X cuts, Cmd-V pastes at the selection’s first row/left boundary
(or at a point cursor when no time area is selected), and Delete clears the
area without closing the gap. These shortcuts apply at Arrangement workspace
focus; browser text input and MIDI note editor shortcuts keep their own context.
The upper third of each selected track area is normally invisible as a handle.
Hovering highlights it and shows an open-hand cursor; dragging there moves the
whole area. Below it, dragging creates a new selection, including from inside
the existing selection and over selected clips. These gesture zones own input
inside the area; normal clip dragging resumes outside it.

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
with the same track count and frame duration. It needs enough existing destination
tracks; it does not create tracks implicitly. Source audio fragments preserve
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
both edges of a multi-track time area, system clipboard exchange, range duplication,
insert/delete time with gap movement and Session time-area editing are deferred.
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
invalid/outside drop target, restore the ordinary source display. Keep a submitted
move preview until its worker result arrives, and restore the source on rejection.

`hex-project::preview_clip_fragment` slices only validated audio metadata with
the original clip ID, source mapping and fade domain. It does not mutate the
project or allocate PCM. UI previews borrow the existing waveform cache and
prepared finite MIDI thumbnails; pointer updates do not compile, capture a
clipboard or expand MIDI occurrences. Render work is limited to visible rows and
the corresponding source rows, with the existing MIDI shape budget.

Native checklist: move a mixed audio/MIDI time chunk across rows and earlier/later
in time; check source and destination tails, silent cells and overlapping moves;
leave the valid lanes or cancel and confirm the original display returns; release,
Undo and confirm the preview matches the committed clip positions. Native
smoothness, detailed waveform appearance and listening remain user acceptance.

Native review, 9 October 2026: the user reported that the separately launched
`hex-time-chunk-preview-20261009.app` looks good and works as expected. This records
acceptance of the presented preview behavior, not measured paint/device deadlines
or sustained-note listening. The bundle manifest identifies clean source commit
`4181d68` and its release binary. Full local CI passed all 14 steps on that commit.
