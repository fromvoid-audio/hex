# session diagnostics

Each desktop launch creates a new `session-{unix_nanos}-{pid}.log` in
`~/Library/Logs/hex` on macOS. Retention keeps the newest ten owned regular log
files, including the current session; unrelated files and symlinks are preserved.
There is no upload or network logger. Other platforms use `~/.local/state/hex/logs`.

The compact debug button at the far right, beside the version, opens the current
session log in the system text editor (`open -t` on macOS; the default file
application on Linux). It appears in the editor, startup and demo headers.
The log path comes from the injected session handle; opening does not scan or
read the log on the UI thread. Launching runs on a background executor, repeated
clicks are ignored while it is pending, and logging/launch errors show a dialog.

`hex-diagnostics` owns file creation, pruning, writes and per-line flush on a
worker. Desktop starts the service before GPUI, injects its producer handle,
and joins it after the application lifecycle exits. A 64-entry queue accepts
control-side messages of at most 4096 UTF-8 bytes. Full queues are explicit and
counted; UI retries changed status/error state on a later tick. These are
best-effort diagnostics, not durable project storage or a complete command trace.
Filesystem failure is surfaced in the editor and at shutdown; it does not abort
project editing or replace the original playback error.

The log contains startup identity and timestamped changed editor status/error
messages. Audio faults retain the first render cause, failed block starting
frame and applied audio revision until a new graph is successfully admitted.
Nonfinite output is identified separately from an engine error. Engine render
wrappers preserve MIDI, timing, arrangement and mutable-instrument causes;
admission/retry classifications and Stop behavior are unchanged.

Callback publication uses a preallocated 256-byte UTF-8 atomic record. It does
not format, allocate, log, write files or lock. Control reads use three bounded
snapshot attempts and allocate the readable message outside the callback. Tests
cover first-fault retention, recovery, UTF-8 truncation and zero total/net
callback allocation for the publication/Stop path. This does not prove physical
native device deadlines or identify the reported Operator stop by itself.

For Pulse 2305, open the diagnostic build, play Operator notes, then change the
controls that caused playback to stop. Send the newest log in the directory
above, together with the affected saved project if available. Native reproduction
and the underlying stop fix remain open; this change supplies evidence.
