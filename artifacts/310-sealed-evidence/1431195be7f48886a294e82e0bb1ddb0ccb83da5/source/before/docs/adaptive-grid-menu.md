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

## bounded fixture correction

The original c7 focused receipts remain historical. Source review then found
that the new rendered cases inherited an ambient browser constructor through
`clip_drag_ui_test::editor_window`; those tests were not safe native candidates.
After PR363 actually merged at `9900da4e`, this branch integrated that exact
related base once and replaced only the private grid-menu window factory. Each
case now retains its own disposable browser/preferences/cache directory and
uses the merged pre-discovery test constructor. No constructor, production
browser or shared action wiring was duplicated or changed. The earlier evidence
archive's native checklist is superseded by this bounded-fixture requirement.

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

## returned meter-origin quota repair

A later independent review found a separate early quota omission:100 long visible
meter sections reserved100 marker slots even though their origins were also the
regular section landmarks. The actual frozen83138-derived regression produced
only100 painted lines with a256-line budget. The repair shares those identities;
only frame-quantized viewport-edge markers outside the regular beat span need
extra reserved slots. The existing binary searches bound that check to two edge
origins, with no cache, allocation or whole-history scan added to snapping.

Bounded tests cover100 sections/all three densities/three pan phases, enough
remaining regular landmarks, paint/snap/directional matching and temporary bypass;
separate one-frame rounding cases keep both edge markers reachable. All earlier
phase-independent count, dense256 sampling and offscreen-history no-paint-list
regressions remain. The9 actual menu/input/discoverability cases are unchanged.
Original red/green receipts and earlier immutable archives retain their source
identities. Final candidate integration/full20 and inherited333 qualification
remain the dedicated merger's responsibility; no old report qualifies another
tree. The production menu is still available only after actual merge/delivery;
no installed-app or native visual/input acceptance is inferred.

## returned visible-section remainder repair

The next review boundary compared128 and129 long visible sections. Applying the
same floored quota to every section painted256 lines for128 sections but only129
for129 sections; later candidate sampling could not restore discarded targets.
The resolver now distributes the available quota's remainder in visible-section
order before dyadic coarsening, matching candidate allocation. The256-line cap
and exceptional frame-quantized edge-marker reservation are unchanged.

A focused regression compares both section counts across all presets and three
pan phases, checks the actual painted identities, snap and directional targets,
and temporary bypass. Existing pan stability, dense-map sampling, offscreen
history and edge-marker cases remain. This source repair does not qualify
physical menu interaction, delivery, inherited feature ancestry or the merger's
final combined tree; earlier source-bound receipts remain historical.

## returned mixed-section capacity repair

The all-long128/129 regression above did not cover unused short-section slots.
On unchanged4f3a39fe,128 one-quarter sections plus one8192-quarter section painted
only129 lines when the long section was last, or130 when first. Both keep enough
natural landmarks for256. The exact18-case mixed test retains all presets, three
pan phases, actual painted identities, matching nearest/directional snap and
modifier bypass. Its original count failures remain separate evidence.

The resolver now computes each visible section's natural demand from the unchanged
base adaptive step and the same conservative visible span. Max-min capped sharing
assigns the largest level whose capped demand sum fits, then residual slots to
unsatisfied sections in their existing order. Short sections consume only their
natural demand; long sections receive the unused capacity before dyadic coarsening.
Paint selection uses the same rule and emits every coarsened landmark when all fit.
The existing over-budget section sampler, edge-marker reservation and256-line cap
are unchanged. Arbitrary alignment/dyadic spans need not use every available slot.

Counts saturate at the small budget-plus-one bound; raw beat spans use widened
integer arithmetic. The inclusive level range0..=256 requires at most nine probes,
plus one final sum, not eight probes. Resolution scans only binary-bounded visible
sections and adds no heap storage, persistent cache or historical scan. The
existing offscreen-history test verifies that snap/nudge do not build paint lists;
this counter is not an allocator-wide byte measurement. Independent quota boundary
checks compare zero/one/huge demands against one-slot-per-round allocation.

The mixed regression reports actual debug paint/snap/directional elapsed work
separately from no-new-heap source inspection and paint-list counters. It calculates
each snap once before checking membership; the first after-run's redundant
per-line recalculations were stopped and preserved as a test-efficiency correction,
not a product failure. Timing is build/workload-specific and establishes no native
or optimized UI deadline. Source checks and required prospective-tree CI remain
separate, as do actual user menu/grid acceptance and delivery.
