# Explicit track selection — pulse2387 / pr179

One editor-owned selection stores canonical regular TrackIds independently of
focused clip/device ownership and Arrangement time ranges. Track-name clicks
replace the selection; Shift-click selects the inclusive canonical range from
the anchor; Cmd/platform-click toggles membership; platform+Shift adds a range.
This follows the existing platform additive-action modifier convention (for
example additive solo), while range selection is a documented Hex contract,
not an assertion of measured Live parity. Empty tracks are valid members.
Selected name highlights and the selected-count status persist after release.
Keyboard track navigation selects one track. Individual device/track controls
retain their existing focused-owner semantics.

Compatible topology changes retain existing IDs without selecting inserted
tracks; removed IDs are pruned and do not silently rejoin on Undo. Opening a
project clears the old selection. The consumer accessor returns only explicit
members in canonical order; a time highlight never adds selected export sources.

Partial implementation: selected-source/stem export wiring requires the existing
PR152 prerequisite, which is not in this branch's current-main base. That
cross-lane integration awaits coordinator direction. Rendered range/toggle,
empty-track, topology and exact-members tests pass. Full final-source CI,
export-consumer verification, native optimized-build confirmation and manual
human review remain required. No passing gate is registered during the
coordinator's readiness/evidence hold; PR179 stays draft.
