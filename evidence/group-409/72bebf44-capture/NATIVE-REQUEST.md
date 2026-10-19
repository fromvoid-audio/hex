# Coordinator-only finite request — PR409
Frozen source 72bebf44f5c050cc19d106ddd6bcb3b5be27c386; tree ed69c739b4cfa2c1b642013997de43f5fd46c347. No self-dispatch, app/backend, permissions, user-data or unrelated suites. Root may schedule separately; no native PASS claimed.

1. Seven new/changed Project runtime cases (OR-filter semantics, REQUIRE actual seven, no zero-test green):
```
cargo test -p hex-project --locked --lib -- copy_overlap_preserves_ capture_ canonical_subframe_midi_member_is_retained_in_whole_group_move --nocapture
```
The three COPY cases are actual original RED→GREEN; three capture cases cover canonical equivalence, equal-counter different source/COW, retained shell alias refusal/recovery. Changed subframe test compares capture to existing canonical 1Q32 law.

2. Six new/changed actual GPUI cases (REQUIRE actual six):
```
cargo test -p hex-ui --features ui-test-support --locked --lib -- group_outside_lane_clears_all_previews_and_restores_originals group_vertical_autoscroll_invalidates_all_previews_before_repaint group_capture_checks_source_allocation_and_selection_epoch group_capture_profiles_actual_redraw_separately_from_projection group_capture_reenters_valid_lane_and_commits_only_current_projection group_escape_and_actual_gpui_capture_loss_never_commit_any_member --nocapture
```
These use existing bounded private service/fixture setup. Outside/vertical original REDs, source/selection, actual separately-measured full GPUI redraw, invalid→valid current release, shared-payload retirement. Timing diagnostics have NO pass/fail deadline and are not native performance guarantees.

3. Three expected privacy compile-fails:
```
cargo test -p hex-project --locked --doc ClipGroupMovePreview -- --nocapture
```
Unique runtime total13 + privacy3. Stop on real failure/count mismatch. Preserve original evidence; do not silently substitute selectors or repeat old accepted12 as a new claim. Full prospective-tree CI/review belongs to sole merger. Native user interaction/optimized feedback still requires actual acceptance, not these tests alone.
