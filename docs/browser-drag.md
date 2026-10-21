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
project rate. Native external file dragging remains unsupported. Current checks and user interaction/listening acceptance are tracked in
[Pulse](https://pulse.iocore.dev/).


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


## Create a track on sample drop — implemented; native acceptance pending

Browser WAV drops also support a stable before-track insertion anchor or
append-at-end target. Hovering near a visible row boundary shows a full-width
insertion line and hides both floating and clip-shaped previews. Hovering blank
space below a painted final row, or an empty Arrangement, targets a bottom track.
Only Arrangement timeline/body geometry participates; browser, ruler, mixer and
outside-window space are not import/create targets. Existing track-body drops
continue to import onto that track.

The worker validates the stable anchor/capacity, prepares the media and submits
one canonical insertion/import command with the snapped clip frame start. Track,
asset and clip commit atomically after candidate compilation and share one undo
step. Existing source identities/mixer state and track order must survive history
and save/reopen. Cancel/stale/invalid drops create nothing. No new project schema
field is needed: ordered tracks already persist in schema 13.

The next user-selected correction extends track creation to moving existing
audio/MIDI clips. New autoscroll and native cross-application file dragging remain
deferred. User native
checks will cover between-track line/drop, bottom blank space, empty project,
existing body drop, cancellation and undo/reopen.


Insertion source acceptance: full project/session suites pass; final UI suite
205 passed/one existing ignored, strict affected Clippy, build, guards/fmt/diff
checks pass. Review: `artifacts/hex-track-insert-20261009.app`.

Native checklist: drag a browser WAV to a divider; confirm the full-width line
and no ghost, release and check track order/clip start. Undo/redo once. Drop below
the final track and in a new empty project; each should create one bottom track.
Confirm track-body drops still use that existing track, and Escape/outside drops
create nothing. Save/reopen should retain the inserted order and clip.

The browser floating ghost is now an editor-owned conditional overlay: GPUI's
payload view did not repaint when the shared hover state changed. Root review
also removed screen-origin subtraction because pointer events are already
window-relative. Native moved-window cursor alignment remains part of retest.


Native-feedback correction implemented; user retest pending: divider targets retain line-only feedback;
blank bottom/empty space retains line plus cursor ghost. Insertion x uses actual
left timeline origin with the header excluded on the right. Overlay styling must
not alter workspace height. Existing-clip moves preserve their grab offset and
clip contents through canonical insert+move commands; see clip-drag.md.


Corrected build: `artifacts/hex-track-drag-fix-20261009.app`. Actual rendered
geometry tests use independent painted clip bounds for sample start, and assert
Arrangement origin/size remains constant at bottom hover. Browser and existing
audio/MIDI clips can create tracks; append keeps line+ghost, divider keeps line
only. 209 UI tests pass (one ignored), project/session tests and strict checks/
build pass. User checks: sample drop at noncentral x, ghost in blank append space,
no bottom workspace jump, existing audio/MIDI divider/append moves and Undo.


## overwrite and empty-area feedback — source implemented

Current user-selected work adds a subtle full blank-area drop highlight: an
empty Arrangement has no cursor-y line; append retains the last-track boundary
line and floating ghost. Track dividers remain line-only. Rendered regressions
pass; native acceptance is pending. Existing-track drops use the atomic
overwrite contract in `clip-overwrite.md`, replacing covered portions of both
audio and MIDI victims. Earlier overlap-policy deferrals above describe older
builds; this extension is not yet native-accepted.

209 rendered UI tests pass/one ignored; core/history/persistence/render checks
and strict affected lint/build pass. Review: `artifacts/hex-overwrite-drop-20261009.app`.
Native interaction acceptance remains pending.
