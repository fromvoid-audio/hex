# hex-diagnostics

## Purpose

Write bounded diagnostic events to one fresh, timestamped log file per Hex
process session and retain the ten newest Hex session logs. A dedicated worker
owns directory access, file creation, retention, writes and flushes. Producers
submit short messages through a bounded, nonblocking queue.

## Dependencies

The standard library provides the bounded channel, worker lifecycle, file I/O,
retention and error snapshots. This crate has no third-party dependencies.

## Does not own

Application UI, project state, audio processing, callback reporting, log
interpretation, crash upload, or application-specific diagnostic policy. No
filesystem work is performed by `record`; do not call it from an audio callback.

The composition owner should call `SessionLog::start` before launching the UI,
pass clones of its handle to control-side components, then drop the owner after
the UI and other producers have stopped. Dropping the owner closes the worker
even when handle clones remain, drains at most the queue capacity, and joins the
worker. It can wait for file I/O, so it must not be dropped from the UI event
path or audio callback. `shutdown` returns the latest worker-side retention or
write error after the worker joins; `failure` provides a nonblocking snapshot
while the application is running.

Each event is capped at 4096 UTF-8 bytes and truncated at a character boundary.
Newlines and carriage returns are escaped by the worker so an event stays on one
physical log line. The 64-entry queue never blocks producers; full and closed
results are explicit, and full submissions are counted and reported in the log.
The worker flushes each written line. Worker-side I/O failures are exposed as a
best-effort, nonblocking error snapshot.

On macOS the default location is `~/Library/Logs/hex`; other platforms use
`~/.local/state/hex/logs`. Retention recognizes only regular files named
`session-{unix_nanos}-{pid}.log`. Unrelated files and symlinks are preserved;
retention failures are reported through the error snapshot.
