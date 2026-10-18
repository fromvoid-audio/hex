# adaptive arranger grid density

Pulse2694 / canonical PR310 adds adaptive grid choices to the arranger's existing
right-click menus. It does not expose fixed note divisions, triplets or another
musical mode. Insert-track, clip-flavor and track-color actions retain their
order before the density choices. Session and device menus keep their own scope.

The checked labels are `adaptive grid · fine`, `adaptive grid · medium` and
`adaptive grid · coarse`, using 9, 18 and 36 pixel minimum-spacing targets.
Medium is the existing 18 pixel default. Selecting another density immediately
re-resolves the current zoom; later zooming remains adaptive. Existing minimum
subdivision bounds and finite drawable budgets can make presets converge at
extremes. This is a view setting in the existing `SnapGrid`, not a project edit,
history event, persistent preference or schema change.

## one resolution contract

Line painting, alternating four-unit bands, nearest snap and directional edit
steps use `adaptive_grid_step` with the selected density. Disabled snap retains
that density. Toolbar/Command-4 toggle and temporary platform-modifier inversion
restore the selected density; selecting a preset while disabled does not enable
snapping. Consumers must ask `SnapGrid::is_off()` rather than compare only the
medium-density `Off` variant. Existing audio-event-editor grid state remains
independent, and fixed musical divisions remain test fixtures only.

Boundary testing exposed an existing mismatch at a billion-frame viewport:
painting sampled the capped landmark set while snapping could choose a hidden
intermediate line. The shared resolver now coarsens its dyadic step against the
exact visible beat span and a meter-section quota **before** painting would
sample away regular targets. Bands use that same step. At exceptionally dense
meter maps beyond the paint budget, nearest/directional lookup shares the final
bounded painted beat identities. The 256-line and existing meter-map work limits
remain; ordinary neighboring-section snap stays allocation-free. This is bounded
UI resolution, not an engine/DSP or project-time contract change.

The default spacing and ordinary default behavior remain unchanged; pathological
cap cases now refuse hidden targets by resolving the coarser visible grid.

## input and evidence

The existing popup owns pointer/Up/Down/Enter/Escape behavior and restores its
previous focus. Density actions capture project generation and refuse stale
writes. Choosing density changes no selection or project/history command.
Existing clip/color/insertion assertions are retained; the insertion fixture
now clicks the explicit first popup row because a generic row selector is
ambiguous in a multi-row menu.

Pure cases cover all presets, toggle/bypass, ordinary zoom, tempo/meter seams,
widths160/640/1920/8192/100000, maximum spans and260meter points. Rendered GPUI
cases cover right-click, current-choice checks, mouse/keyboard selection,
cancel/focus, disabled/stale refusal, an actually rendered landmark used by a
keyboard edit, and the empty-arranger shortcut toggle.

Original cap regression run `20261018T145351Z-985abcb0` failed at Fine,width160,
length1000000000: snap1536000 was hidden between painted0/3072000. Four other
core tests passed. Repair run `20261018T145712Z-c9719c6c` passed all five then
present core tests. The broader UI run `20261018T145855Z-0ead9625` retained734
passes/two menu-fixture failures/three existing ignored profiles; its strict
Clippy step was not run. The fixtures had clicked a track header instead of a
timeline lane and an ambiguous generic menu row. Run
`20261018T150415Z-e242d1b4` then passed six resolver/eight menu cases, six guards
and strict UI-feature Clippy. A prior pinned-GPUI focus-call compile failure is
also retained. None of these original reports is relabeled for later heads.
Final canonical-head results and all original logs/hashes belong in PR/Pulse.
These focused checks do not replace the merger's actual final-tree full local CI.

## finite coordinator-owned native checks

On the coordinator-selected immutable optimized source, the headless selectors
`adaptive_density` (six tests) and `context_menu` (nine tests, including four new
menu cases) must have positive results with no ignored tests. Use default native
stack and existing disposable fixtures only. This does not require an installed
app launch, backend/input/permission operation or a real library/settings change.

Later, while the user is present in the qualified optimized app:

1. Right-click empty timeline, a track lane and a clip; inspect checked preset,
   existing actions and compact popup layout at supported widths.
2. Choose fine/medium/coarse by mouse and keyboard, then zoom in/out. Verify grid
   and moved/nudged clip/cursor targets agree; undo only the project edits.
3. Toggle snap off/on and temporarily invert it with Command during an edit;
   check the selected density survives. Cancel menus with Escape/outside click
   and confirm previous focus, text shortcuts and selection still work.

Physical UI/paint/audio acceptance and optimized app delivery remain open/root
owned. Cloud work performs no Mac/app operations. No listening, sample-access,
MTS or unrelated suite acceptance is inferred.
