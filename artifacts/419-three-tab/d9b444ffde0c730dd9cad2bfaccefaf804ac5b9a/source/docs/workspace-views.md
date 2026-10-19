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
handling. Modal/popup/draft state, an owned project-Open intent, and active
track-resize/clip-paint/clip/time/loop/ruler gestures refuse the
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

A further actual held-double-click paint regression reproduced Tab switching to
Console while the paint capture and preview were active. One additional existing
guard now preserves that capture and its exact rendered preview. Normal release
still submits one canonical paint command with one model Undo/Redo; Escape then
release submits no edit. Tab switches normally after either capture ends. This
case runs both outcomes and preserves all earlier assertions: **9 unique focused
tests pass, zero failed/ignored** (7 workspace and 2 unchanged Operator cases).
The real pointer/key routing and canonical command application are automated
component evidence; they do not claim a real worker acknowledgement or physical
mouse/keyboard acceptance. The failing baseline is retained separately.

### 19 october2026 — Open lifetime and ruler capture review

Two additional rendered regressions failed against the exact integration source:
Tab switched to Console during a selected-project confirmation and while the
left ruler scrub was held. The synthetic `unused.hex` pending-path fixture had
missed the real ownership transition: rendering consumes `pending_open_path`
into the existing `project_open` intent before Open completes. The guard now
checks that owner's `busy()` state as well as the pending path. The retained-root
modal fixture uses the real picker lifetime; an additional test creates a valid
disposable project, selects it, verifies the actual pending confirmation and
consumed path, and checks unchanged project/revision before canceling it.

A read-only scrub activity query protects the existing gesture, including a
released final target until its established admission/refusal retires it. No seek
or Open lifecycle changed. Actual ruler mouse routing uses the existing offline
seek-ack gate; both release with its normal unsupported-output refusal and Escape
retire the target, after which Tab switches normally. No OS audio device opens.
All **11 unique focused cases pass, zero failed/ignored**: nine workspace cases
and the two unchanged Operator cases. The two baseline failures and earlier
integration failure remain separate evidence; the unrelated header-width failure
belongs to its source owner. Final combined qualification and physical acceptance
remain open.

## Finite native request and user checklist

Root's sole Mac lane may run this exact source once with:

```sh
cargo test -p hex-ui --features ui-test-support --locked --lib -- \
  --nocapture workspace_view_ui_test operator_tab_cycle
```

Expected on the repaired source: **11 unique positive tests**, zero failures/ignored
(9 workspace cases and 2 unchanged Operator traversal cases). The previous exact
six-case native run remains historical; this checklist does not request its
automatic repetition or dispatch a new native job. Preserve the actual case list,
exit status, source/tree, raw log/hash and bounded fixture startup evidence; a
zero-match result does not qualify. This is a targeted command, not full CI.
No app installation, launch, quit, real audio device or user-library operation is
part of it.

If Root requests only the newly changed review cases, the bounded command is:

```sh
cargo test -p hex-ui --features ui-test-support --locked --lib -- --nocapture \
  workspace_review_tab_preserves_selected_open_confirmation_until_cancel \
  workspace_review_tab_preserves_held_ruler_until_release_or_cancel
```

Expect exactly **2 unique tests, zero ignored**; this does not request automatic
repetition of the unchanged earlier cases or dispatch native work.

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

Also hold a double-click body-paint drag and press Tab: keep Arrange and the paint
preview. Release to paint once, or Escape then release to cancel, then verify Tab
switches normally. This checklist does not dispatch a native job or create a new
verification gate.

### Auxiliary capture and device-menu review return

The next source-specific review reproduced Tab switching during four actual
rendered interactions: a held clip press before the drag threshold, an open
device-add menu, a project-spectrum resize and device-chain navigation. The
existing workspace refusal now checks those owners too. Private read-only
queries expose only whether each existing owner is active; capture, focus,
worker, model and history policies are unchanged.

The pre-drag guard also exposed a pre-existing ordinary-click cleanup gap:
the clip's click handler consumed mouse-up before its separate release handler
cleared the recorded press. The click handler now clears only its captured
project generation, clip and non-trim edge before selecting the clip. A delayed
click cannot clear a different generation, clip or trim-edge press. There is no
blanket pointer reset or vendor change.

New rendered cases exercise body/trim press and release, ordinary click without
Tab, delayed keyed click, menu entry from root and real header keyboard focus,
spectrum release/Escape, and overview/middle-button device navigation. The add
button's actual click focuses the existing tracked root parent through GPUI;
it does not preserve header focus as initially assumed. Menu tests establish
both starting focus contexts, then assert the real root route after clicking.
They preserve project/revision and queue state, retain each capture through Tab,
and allow switching again after the existing release/cancel/close path.
Headless pointer/key evidence is not physical/native acceptance. The immutable
handoff retains all original review failures and the separate fixture focus
assumption failures. Final prospective-tree qualification remains merger-owned.
