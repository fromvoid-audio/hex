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

The existing export prerequisite PR152 at
`e7f9d7125bcaca93374d6a15b55035e082c54659` is integrated with original history.
Selected-track mix and selected stems consume these exact explicit members;
shared Returns remain their separate existing export choice. Time selection
sets only the half-open frame range. Empty selected tracks remain named stems.
The consumer regression covers a separately focused/unselected track and a
project-wide time range without promoting either into selected export sources.

Final exact-source checks and source/base/log identities are recorded in PR179.
Native optimized-build confirmation and manual human review remain unperformed.
No passing gate is registered during the coordinator's readiness/evidence hold;
PR179 stays draft, with export PR152's native/human acceptance still pending.
