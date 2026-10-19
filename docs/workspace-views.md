# Arrange and Console views

The main timeline is **arrange view**; the Session clip launcher is **console
view**. The existing compact toolbar icons name their destination in their
lowercase tooltip, and a small adjacent label names the visible view. Internal
`EditorMode::Arrangement`/`Session`, project data and engine terminology retain
their existing identities.

Unmodified Tab toggles these existing views when the workspace or an Arrangement
track header owns focus. Returning focus to the workspace after a switch makes
the next Tab reciprocal when the prior header has left the rendered tree. The
handler bubbles; it is neither a global binding nor a capture listener. Modified
Tab and held-key repeats do not switch. Focused text/rename/search, numeric
controls, device/Operator controls and detail editors retain their own Tab
handling. Modal/popup/draft state and active track-resize/clip/time/loop gestures refuse the
switch even if the workspace still retains focus. Musical typing does not claim
Tab.

Switching uses the existing `set_mode`, without reconstructing the editor or
sending project/transport/history edits. Completed Arrangement time selections
are retained; only an actually active time-selection/move gesture is canceled by
an explicit view switch. Clip membership, Console scene/cell selection,
viewport, Console horizontal position and detail visibility remain owned by the
same editor.

## Bounded source verification

`workspace_view_ui_test` uses a real Service and GPUI test window. It retains a
unique RAII temporary browser/sample/preferences/recent/theme directory **before**
constructor workers start, checks the actual initial browser directory/places,
imports a bounded WAV and adds a real scene. There is no audio adapter or real
user library enumeration. Cases dispatch actual simulated Tab/modified keys and
pointer/rename/numeric input, check the rendered view-label bounds and both
switch directions, retain clip/time/scene/cell/viewport/detail state, and assert
no queued worker request or project/revision change from switching. A controlled
retained-root modal test covers refusal, not a claim of ordinary physical dialog
routing. The unchanged Operator forward/reverse traversal cases remain part of
the focused cross-feature check; their wrap assertions are not weakened.

The original test-only baseline fails because Tab leaves Arrangement active.
An intermediate implementation exposed existing completed-time-selection loss
in `set_mode`; the final switch retains that state. Headless GPUI evidence is not
physical keyboard, IME, accessibility, native display, audio or optimized-app
acceptance. Exact-source full repository qualification belongs to the merger.

The 19 october2026 review regression reproduced Tab switching to Console during
an actual rendered track-height resize. The workspace guard now leaves that
capture active until release; Tab switches normally afterward. A separate actual
header-click → toolbar Console → Tab sequence already returned correctly before
repair. Its initial selection assertion captured state before the header click,
which legitimately changes clip selection; the corrected test captures it after
that click, checks the actual workspace/header focus route and retains the full
project, revision and pending-request assertions. No toolbar handler or selection
policy changed. The original two-failure log is retained as one product failure
and one fixture expectation failure, rather than two product regressions.

The corrected two cases, four original workspace cases and two unchanged Operator
traversal cases pass together: **8 unique tests, zero failed/ignored**. The earlier
six-case source and native receipts remain evidence for their original source;
they do not verify this later resize guard. Strict checks and the immutable
repair packet are recorded with the source handoff. Final prospective-tree full
qualification, merge and user acceptance remain separate.

## Finite native request and user checklist

Root's sole Mac lane may run this exact source once with:

```sh
cargo test -p hex-ui --features ui-test-support --locked --lib -- \
  --nocapture workspace_view_ui_test operator_tab_cycle
```

Expected on the repaired source: **8 unique positive tests**, zero failures/ignored
(6 workspace cases and 2 unchanged Operator traversal cases). The previous exact
six-case native run remains historical; this checklist does not request its
automatic repetition or dispatch a new native job. Preserve the actual case list,
exit status, source/tree, raw log/hash and bounded fixture startup evidence; a
zero-match result does not qualify. This is a targeted command, not full CI.
No app installation, launch, quit, real audio device or user-library operation is
part of it.

On a separately qualified optimized delivery, user-present acceptance remains:
Tab both directions from timeline and track header; verify names, viewport,
clip/time/scene/cell selection and panel visibility after returning; verify Tab
continues local traversal in Operator/numeric controls and never switches while
renaming, searching, composing IME text or using a dialog/popup. Confirm modified
Tab and an active drag, including track-height/header-width resizing, do not
switch, and transport/project history remain
unchanged. Check the compact label and neighboring toolbar controls at supported
small and large window widths. Root coordinates this separately; source passing
or merging does not mark it complete.
