# PR310 visible-section remainder return

Canonical source4f3a39fec474ee2b2fd52633e4e8b0d8950daea3,
tree3921506fb977d0eaa77273dabba522ffaba4aa71, parentfc0d57f092e4e8c1a8b59d7812d657e13be04903.
Same pulse/task-2694-g1-4bb74a101848 / PR310. No main integration/history rewrite.
Only musical_time.rs::adaptive_grid_step, its private density test, owning docs.
No Source5 Sampler, Source6 loop/trim, browser, menu wiring, Engine or Session edit.

ACTUAL BEFORE
Unchanged fc0d resolver plus the new regression (before-regression-only.patch):
128 sections/all3densities/start0=>256; pans1/3000=>255. Then129/Fine/start0=>129,
assertion expected256, terminal cargo101/0PASS1FAIL0ignored. Raw retained.
This is paint-list identity evidence, not physical pixel/native acceptance.

REPAIR
Compute quotient plus visible-section-order remainder before dyadic coarsening;
129 sections get127 two-slot quotas and2 one-slot quotas, not129 one-slot quotas.
Same256 cap, same frame-edge marker reservation, no extra allocations/history
scan/cache/publicAPI/constructor or grid redesign. Existing candidate sampling
uses the same quotient/remainder order. Nonempty visible sections guarantee a
positive divisor as before. Larger maps retain existing final painted snapping.

AFTER
Focused1PASS/all18 combinations:128+129 ×3density ×3pan. At origin256; either
pan255. Actual lines sorted, current nearest snap remains painted, directional
next equals next painted line, temporary bypass exact. No assertions removed.
Final clean candidate checks/counts/commands/status and raw logs are recorded in
candidate-checks.json; those supersede no historical receipt. The earlier
fc0d27resolver+9menu focused successes and all original failures remain distinct.

REVIEW/HISTORY
Root/source review return2694note26558 and prior25073 read fully. Current
PRRT_kwDOVB_C-s6ttAjf/comment4308101352 reproduced then repaired. Existing pan,
dense candidate sampling, offscreen no-paint-list, marker-origin and quantized
edge tests remain. Source owner does not self-resolve review threads/claim
approval. Merger must assess exact returned source plus actual qualified-main
composition; inherited unrelated-feature/trim findings are not repaired here.
Prior evidence archive464726B/SHA0efed81834d30d7158b7bda9d6e0a6b34cbf1eae80fdafc19244ed64d7d1beaf
at evidence refe3598e9905b436e1d89b9b902aa3d194087e1a5b remains historical,
not newly executed/relabelled. No hostedCI/selfmerge/native/app actions.

NEXT
Sole merger owns source review/dependency/scope integration/exact combined-tree
full localCI/merge. Pulse InProgress until actual merge. Root alone owns serial
native/optimized delivery and user-present visual/menu/input acceptance. Existing
user path after actual delivery: ArrangeView right-click empty timeline/track
lane -> adaptive grid fine/medium/coarse with current-choice checkmark.

FINAL CANDIDATE RECEIPTS
28resolver+9actual GPUI menu/input tests:37PASS/0failed/0ignored; strictUI feature/all-targetClippy +five source guards+format PASS. Nine focused commands total, all exit0, source unchanged. SummarySHA256 72afecf9587bbaaeb88948842eac91ade1a727554403f540a1fd47275546ad47. No full prospectiveCI/native/approval claim.
