# Browser WAV dragging

User-selected workflow, 9 October 2026; implemented; user confirms the drag workflow works (9 October).

Dragging a browser WAV begins with a cursor-attached floating clip. Inside a
valid Arrangement lane it switches to the exact snapped landing preview; outside
lanes it becomes floating again. Show only one preview at a time. Ordinary
Arrangement audio/MIDI drags should use the same conditional free/locked behavior.

Source paths are retained verbatim. Waveform/duration preparation is bounded and
background-only through the media/session contract. Pending metadata is indicated
truthfully; no exact-duration claim from a guessed placeholder. Only a valid
release imports through the canonical existing-track worker path, with one undo
step. No mutations while dragging. Stale project/listing/viewport/snap, busy
state, invalid target, conversion failure, Escape or outside release must leave
project/history unchanged. Preserve existing clip move units and limits.

Bound source preview requests/caches and keep them independent of authoritative
project-edit pending state. Preview file failures are visible without producing
a project asset. Actual import revalidates the file and handles errors normally.

GPUI 0.2.2 supports these previews in-window. It does not expose outbound native
macOS file dragging; cross-application file drops are not supported in this
implementation and require a separately documented native integration. Internal
GPUI drag payloads are not OS file drags.

Acceptance: rendered pointer browser pickup, floating/locked transitions, exact
target import request, cancellation/stale guards and regular audio/MIDI move
regressions; background preparation bounds/duration and failure tests. User owns
native pointer/listening checks. Broader roadmap remains paused.


Implementation checkpoint: hex-session exposes narrow read-only
prepare_audio_preview, using import decode/resample bounds and a 48-bin waveform
summary; no PCM enters UI. GPUI background work uses a capped path/rate metadata
cache and one in-flight/latest-pending preparation request, independent of the
project-edit queue. During initial preparation a lane start marker is shown
without claiming a full width; exact bounds arrive with valid duration metadata.
Drag start refreshes source metadata to avoid stale cached file duration.

Rendered checks cover floating→locked→floating, exact import request and off-lane
cancel; a source-rate conversion regression compares preview duration at the
project rate. Native external file dragging remains unsupported. Final checks
and user interaction/listening acceptance are tracked in todo.md.


Final checks: 200 UI tests pass, one existing ignored; preferences/session
suites, strict affected Clippy, desktop build, fmt, architecture/style guards
and diff checks pass. Root also checked strict UI Clippy with ui-test-support.
Review bundle: `artifacts/hex-browser-drag-20261009.app`.

User checklist: drag a WAV out of the browser into empty space, then onto a
track, back out and onto another track. Confirm one floating/locked preview at
a time and that release places it at the shown start. Undo once. Escape or
release outside tracks should import nothing. Repeat free/locked transitions
with an existing audio or MIDI clip. Native sound/interaction remain pending.

A file modified externally during a held drag can differ from its prepared
preview; import reopens and validates it. Source pinning for a held drag is not
implemented. Outbound native file dragging remains a separate boundary task.

User accepted the browser drag build on 9 October and authorized source commit/push. External OS dragging and held-source pinning remain deferred.
