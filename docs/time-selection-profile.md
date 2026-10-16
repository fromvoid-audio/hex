# Native time-selection profiling

Issue #14 retains its canonical merged PR #31. This supplementary tooling measures
native input dispatch to selection paint, edit feedback and UI-observed completion
separately for identical hybrid content on one versus 100 tracks. It does not
change editing behavior or establish physical display/audio deadlines.

```sh
cargo run --release --locked -p hex-session --example time_selection_fixture -- /absolute/fixture-dir
cargo build --release -p hex-desktop --locked
target/release/hex-desktop --profile-time-selection /absolute/fixture-dir > raw.csv
```

The generator refuses existing fixture files. Both sizes contain 50 audio plus
50 MIDI clips per track, eight notes per MIDI clip, one shared 48000 Hz stereo WAV,
1.5-beat clips on two-beat steps, and gaps. There are 100/10000 total clips and
400/40000 authored notes. The fitted lane selection spans 21.3%–61.9%; the platform
modifier disables snap, exercising partial boundaries. Paste/insert use an untimed
copy and an actual main-lane cursor click at 71%.

The disposable native GPUI window embeds the real EditorView and normal session
worker. Audio is offline. All three preferences paths are under a newly created
`native-profile-PID` directory beside the fixtures; there is no recovery journal,
user project open, user preference write, audio stream, OS input injection or
control of another app. CLI filesystem preflight happens before the UI loop.
Only explicit in-process window input and normal bound actions are dispatched.
Setup, open/decode/compile, viewport fitting and validation are untimed.

One/100-track cases alternate for each operation and repetition. Each named copy,
cut, replace-paste and ripple-insert case has five warmups plus 40 measured samples
(360 rows total; filter `warmup=false` for the 320 measured rows). Dispatch begins
after varied 7–17 ms timer delays rather than exclusively at a frame callback.
The harness waits for an active native window and settled, painted lane geometry;
activation loss during a probe, a no-op/unadmitted request, an unexpected status
or project revision, an editor error or a 30-second phase timeout fails the run.

CSV durations are microseconds from the corresponding dispatch start:

- `selection_first_paint_us`: actual main-lane mouse dispatch to the native paint
  observer after the real editor child, with a nonempty project-scoped selection.
  All selected track IDs are validated after timing.
- `operation_first_paint_us`: bound-action dispatch to the first native paint
  showing a proven queued control request or its correct completed result.
- `operation_ui_completion_paint_us`: dispatch to paint after the worker event
  has been consumed, no requests remain, and the expected status/revision appears.
  This includes normal 16 ms UI worker polling; it is not a raw worker-finish clock.
- Each corresponding `*_following_frame_us` value uses a callback registered
  **from that paint** and observed at the following frame boundary, after the
  observed scene's submission. It is a separate observation bound, not physical
  pixels, compositor completion, GPU completion or physical audio deadlines.

`project_revision` is the editable project's counter, not a Git revision. Exact
Git/build/executable/toolkit provenance belongs in separately retained run metadata.
Rows are collected in memory and formatted outside timed paint callbacks. A one-slot
nonblocking final-result channel hands the report to a dedicated desktop writer
thread. That thread writes and flushes stdout/stderr, then acknowledges through a
second one-slot channel. Only successful publication permits native app quit;
report/publication failures exit nonzero. Publication has a 30-second bound and
performs no file or console I/O on the UI thread. macOS native termination does not
return from `Application::run`, so post-loop output is deliberately avoided.
Successful rows explicitly have `failures=0`; the first failure aborts with a
nonzero exit, `failures=1`, completed count and available partial CSV on stderr.
Closing the disposable window early publishes a failed partial report. External
application termination before publication exits nonzero and may prevent output;
it cannot count as a successful measurement. Phase timeouts include activation,
pending request, lane geometry/epoch, paint and probe state diagnostics. Native execution
and full final-source local CI remain separate evidence; a compiling harness does
not establish any timing result. Record actual numbers without silently expanding
this measurement PR into optimization work.
