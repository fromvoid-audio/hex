# project tempo input

The project editor's first toolbar control is a compact numeric BPM input,
shared by Arrangement and Session. The old canonical tempo minus/plus buttons
and readout are removed; the meter control remains in the arrangement header.
The separate demonstration/layout-review window is not the project tempo editor.

The field uses the shared `hex-widgets::NumericControl` and canonical
`SetTempoPoint` command. It retains the existing cursor-snapped-quarter tempo-point
semantics, 1..1000 BPM inclusive range and micro-BPM precision (six decimals).
No persistence schema, transport engine, project tuning or MTS behavior changes.
Canonical command/history/timing/storage remain owned by their existing layers.

Click the value to edit. Enter or focus loss commits valid changed text once;
Escape cancels. Keyboard adjustments use the shared numeric control, without
adding a new Tab stop to the existing device traversal. Empty, nonnumeric,
nonfinite and out-of-range text stays invalid
without a canonical edit or clamping. Shared numeric text selection, clipboard,
arrow and accessibility behavior applies. Focused text entry owns the keyboard so
printable transport, track and computer-keyboard shortcuts do not run; project
Undo/Redo does not consume a focused numeric text draft. An invalid unfocused
draft does not suppress workspace shortcuts. A value that rounds to the
current micro-BPM produces no canonical command.

While idle, the field reflects canonical tempo at the cursor's snapped quarter.
An active draft retains its originally displayed tempo point if playback moves
the cursor. It never redirects the write to a later point. A new canonical
revision or project generation retires the old draft. Pending project work
disables editing and cancels a local draft. No drafts, previews or invalid text
reach the project worker.

## source checks and native acceptance

`cargo test -p hex-ui --features ui-test-support --locked project_tempo_`
selects the range/precision test and five rendered input/layout/lifecycle tests.
They exercise actual numeric text events, invalid/empty rejection, cancellation,
text-focus protection, canonical command/micro-BPM, Undo/Redo and beat-to-frame
semantics; layout covers 880/1280px in both modes, plus stale revision/generation
and cursor movement during typing. The blur regression checks valid commit,
invalid/empty refusal and shortcut recovery after returning focus to the editor. These are headless GPUI fixtures. Required
full exact-source local CI remains separate; a focused pass alone cannot merge.
Current-format save/reopen retains existing persistence behavior and remains a
native acceptance item, not an unexecuted automated round-trip claim.

Only the coordinator's serial Mac lane may schedule native work. Reuse unchanged
qualified suites. If needed on the exact qualified source, the bounded new source
selector above is the only tempo-specific native test request; use the existing
target ownership/default-stack/toolchain rules. No installation, launch, app
replacement or new full native/packaging run is requested by this document.

After a coordinator-qualified optimized delivery and with the user present:

1. At supported narrow/wide window sizes in both editor modes, confirm BPM is
   first at the top left, plus/minus are absent, and tuning/play/stop/output
   neighbors remain usable. Confirm text, selection and focus are legible; existing
   device Tab traversal is unchanged.
2. In a disposable project, enter `123.456789`, then `1` and `1000`: each valid
   commit updates canonical tempo once. Undo/Redo restores it. Escape cancels;
   clicking away commits valid changed text once, while invalid/empty/NaN/
   out-of-range text
   cannot apply. While typing, Space, m, z/x, c/v, l and s must not trigger DAW
   actions; editor Undo must not apply a focused text draft. After leaving invalid
   text and focusing the workspace, normal DAW shortcuts must work again.
3. Test at a later cursor quarter in a disposable tempo map. While playing,
   start a draft, let the cursor move, then commit: only the originally captured
   tempo point changes. Confirm metronome/transport follows the canonical map;
   record listening separately from source beat/frame checks.
4. Save to a new scratch path, close normally and reopen on the same executable.
   Verify exact tempo and tempo points persist. Never overwrite an occupied user
   project, discard unsaved data, reset a database or convert an old alpha file.

Native GPU/input, real audio/listening and save/reopen remain OPEN until actually
performed. Prior tuning/MTS physical acceptance gaps are unchanged and are not
qualified or re-requested by this task.
