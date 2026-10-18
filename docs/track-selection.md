# Explicit track selection — pulse2387 / pr179

One editor-owned selection stores canonical regular TrackIds independently of
focused clip/device ownership and Arrangement time ranges. Track-name clicks
replace the selection; Shift-click selects the inclusive canonical range from
the anchor; Cmd/platform-click toggles membership; platform+Shift adds a range.
This follows the existing platform additive-action modifier convention (for
example additive solo), while range selection is a documented Hex contract,
not an assertion of measured Live parity. Empty tracks are valid members.
Selected name highlights and the selected-count status persist after release.
Keyboard track navigation selects one track. Clicking a clip on an unselected
track selects that owner; a clip within the selected group retains the group.
Individual device/track controls
retain their existing focused-owner semantics.

Compatible topology changes retain existing IDs without selecting inserted
tracks; removed IDs are pruned and do not silently rejoin on Undo. Opening a
project clears the old selection. The consumer accessor returns only explicit
members in canonical order; a time highlight never adds selected export sources.

The merged export prerequisite PR152 is integrated with original history.
Selected-track mix and selected stems consume these exact explicit members;
shared Returns remain their separate existing export choice. Time selection
sets only the half-open frame range. Empty selected tracks remain named stems.
The consumer regression covers a separately focused/unselected track and a
project-wide time range without promoting either into selected export sources.

Final exact-source checks and source/base/log identities are recorded in PR179.
Native optimized-build confirmation, physical modifier/input testing, listening
and Live reference comparison remain open. Source merge does not complete those
manual acceptance requirements or export PR152's native acceptance.

## Header Backspace — pulse2721 / pr337

Backspace with an actually focused, selected Arrangement track header removes
all explicitly selected ordinary tracks in canonical order. A focused header
that has been toggled out of the selection does nothing. Main/Return headers do
not acquire regular track deletion ownership. Clip, MIDI, Session, browser,
rename and numeric focus retain their existing delete/text behavior.

The bounded `RemoveTracks` canonical command preflights all IDs, then reuses
`RemoveTrack` cleanup for track contents/devices, incoming/outgoing audio and
MIDI routes, and Session cells. One normal validated history command makes the
selection one Undo/Redo step; preparation/refusal leaves the project intact.
There is no alternate batch-history or routing validator. Delivered canonical
state prunes removed selection IDs and header focus, including when Undo later
restores the tracks; restored tracks do not silently rejoin the selection.

Rendered key/worker regressions cover single/multiple headers, full snapshot
Undo/Redo, numeric editing, rename/browser text, clip deletion and an empty
explicit selection. Model tests cover reference cleanup, survivor preservation,
invalid/stale/duplicate IDs and retained redo. Optimized native physical input
acceptance is still open; source tests do not establish it.

The toolbar's existing remove control keeps its single focused-track semantics,
including when that header has been toggled out of the explicit selection.
Toolbar `RemoveTrack` and header-key `RemoveHeaderTracks` actions are distinct:
toolbar behavior must not depend on whether a click transfers or retains
header focus and must not inherit the header-key no-op or multi-selection
contract. The rendered real-worker regression clicks
the actual toolbar after both toggled-out and multi-header selection, preserves
header Backspace's no-op, and checks single-track removal plus one Undo/Redo.
Ordinary GPUI clicks transfer focus in the cloud test; a separate controlled
focus-retention case proves the action contract without claiming native focus
reproduction.
Source repair/final integration qualification receipts remain separate from the
original09c/native4 evidence in Pulse2721/PR337.
