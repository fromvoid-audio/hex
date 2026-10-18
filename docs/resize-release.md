# resize release continuity

A real Shift edge gesture exposed a release-time display rollback: the held
preview was 377.5 px wide, but release painted the old 314.5 px canonical clip
while its one worker request was pending. The worker then accepted the correct
96000..211215 stretched extent and Warp/source metadata. This proves a temporary
rollback in that reproduction; a permanently lost accepted stretch has not been
reproduced in cloud tests. The user's wider intermittent report still needs
optimized, user-present observation.

Accepted audio trim/stretch and MIDI trim releases now retain one bounded
metadata preview, separate from pointer capture, drag payloads and autoscroll.
The final release recomputation still uses the current pointer and modifiers and
the canonical model helpers. Only an accepted enqueue can retain the preview.
No PCM, Project, model edit or fabricated waveform is held or applied by this UI
state. The existing retained-root/changed-root waveform projection renders it.

The preview matches its exact clip/edge/frame-or-beat/stretch request. A deferred
UI request is not considered submitted. On actual worker submission, it captures
its FIFO completion position, including any earlier typed gesture requests.
Unsolicited events do not consume that position; unrelated earlier completions
do not retire it. Its exact terminal retires it before the actual canonical
snapshot is installed, on success or refusal. Replacement, generation mismatch,
queue refusal or worker disconnection also clears the owned pending state.

Display additionally requires the original stable clip on its source track and
its original asset frame bound. Independent revisions may coexist; a deleted,
moved or changed source owner cannot keep displaying stale geometry. Only the
matching original explicit panned viewport may retain its validated display
horizon until this terminal. Active dragging and its timer end on release; the
pending state never scrolls. A refused/stale release clears its own pointer
capture and leaves canonical material intact. One canonical edit remains one
Undo/Redo, with current-format model/source validation unchanged.

Cloud tests use real Service workers/prepared Engine and GPUI test windows.
They compare actual held/released geometry and canonical results for both Shift
and normal audio edges and both MIDI edges, including source mapping and one
Undo/Redo. Further cases cover Escape, stale generation, close-prompt refusal,
no motion, exact FIFO ownership with an independent loop edit, deletion-induced
worker refusal, deferred submission, and fitted audio/MIDI edge-pan release.
Queue-order tests deliberately control real request order; they are lifecycle
coverage, not a claim that ordinary pointer input creates that order. The
original failures and intermediate assertion/fixture failures remain evidence.

Coordinator-only finite native request: run the five tests in
`editor::trim::resize_release_tests::` and the one new
`editor::drag_autoscroll::resize_pan_tests::fitted_resize_release_keeps_accepted_pan_horizon_until_exact_worker_terminal`.
Expect five plus one positive tests, zero failures/ignored, actual filtered
counts, default native stack and retained raw output/hashes. No unchanged native
edge-pan five or full-suite repeat is requested. The dedicated merger owns final
prospective-tree full qualification and merge.

Physical acceptance remains open: on the actual optimized candidate, repeatedly
resize left/right with Shift held through release, toggle Shift/Command while
stationary, check normal trim and loop/nonloop source bounds, zoomed/fitted edge
pan and held edge, rapid movement/release outside/cancel/focus changes, continuous
correct waveform, final canonical extent/Warp/BPM, one Undo/Redo and save/reopen.
Confirm no pending flicker or final rollback with affected user media. GPU,
physical input/audio/listening and app delivery remain coordinator-owned. Cloud
work does not install, launch, quit or replace the user's app.
