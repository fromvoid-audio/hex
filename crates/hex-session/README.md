# hex-session

## Purpose

Own the canonical editable project together with bounded decoded media and
cached waveform summaries. It is the control-side application controller for a
thin GPUI layer: validated project commands, prepared audio import,
undo/redo, read-only media views, and playback compilation all pass through one
session.

The control worker owns a project-scoped time-selection clipboard. Copy is
metadata-only; cut adopts the captured clipboard after prepared compilation;
paste compiles before history commit. Successful project opening clears it.

## Dependencies

`hex-live-set` prepares bounded Arrangement import candidates on the worker;
no current project is replaced before canonical/media compilation succeeds.

`hex-project` owns canonical project state and history. `hex-media` decodes WAV
files into validated audio, converts source rates and supplies cached peak summaries. `hex-engine`
compiles the canonical snapshot into offline playback. The crate uses the
standard library for its bounded resident media map and dedicated worker.
`hex-lattice-contract` supplies typed native device and parameter identifiers,
values and opaque state. Add-device and parameter requests resolve definitions
through the engine; the worker patches its latest canonical state and compiles
before history commits. Discrete requests each create a history step; continuous
gesture grouping/coalescing remains a separate dependency.
`hex-storage` owns schema serialization and durable file operations; the session
worker coordinates open, create-new Save As, and token-checked rolling-recovery
Save Current with media validation. Opened/saved document paths and exact file
tokens live on the worker; relative media continue resolving from the directory
where the current model was opened. `hex-export` owns linear
PCM16 rendering and create-new WAV publication; a separate worker handles export
without holding the canonical editing worker during rendering.
Export jobs retain the exact captured `Arc<PreparedRevision>` and pass it to the
shared prepared renderer; the worker does not reduce it to an Arrangement
project or recompile it.
`Request::RestorePlayback { generation }` creates a paused playback handoff from
the latest successfully compiled revision without changing project state. Its
event echoes the generation in `restore_complete`, including when recompilation
fails, so the UI can reject stale recovery attempts.

`ArrangementSeekWorker` is a separate control thread with an explicit 8 MiB
stack reservation for bounded runtime construction and Arrangement preroll.
Each Service reserves one such stack; this is distinct from its candidate
runtime storage ceiling. The worker does not queue behind edits, and its one-running,
one-replaceable-pending, one-ready-result mailbox does not block submission when
the result slot is occupied. A newer request cancels running work and replaces
pending work. The unread result moves into a worker-owned retirement slot under
the short mailbox lock; the worker drops it after unlocking. If that slot is
occupied, the stale ready result stays in its own fixed slot; the worker drains
both before starting pending work. Replaced pending request metadata and its
prepared-revision `Arc` are dropped by the submitter after unlocking. A
latency-sensitive caller must retain its audible/current prepared-revision
owners across superseding or invalidating requests until worker ownership
drains; only unread runtime `Playback` destruction is guaranteed to happen on
the worker.
Explicit invalidation cancels and clears all admitted work. The worker returns
the exact prepared graph, request/generation/revision identity, destination,
replay budget and desired play state with the candidate. The candidate is not
published to audio; callers still need the current project/route/Stop admission
fence. Teardown signals cancellation and detaches without joining the caller;
the worker drains its owned pending item and runtime slots before exit. Pending
request metadata and prepared-revision handles displaced by submit/invalidate
may be released on the caller after unlocking.
`ArrangementSeekMode::LinearArrangement` (the default) preserves the
export-style Arrangement-only runtime. `ArrangementSeekMode::Interactive`
constructs a normal Session runtime before replaying Arrangement history, so it
keeps Session route support and can inherit into a normal compiled successor.
The request and result carry the selected mode. Both modes refuse enabled
Arrangement transport loops. Wrapping a candidate does not prove its target
position or publication readiness; the caller still validates current
project/route/Stop identity. Normally the lane owns one candidate runtime; its
two fixed retirement slots bound defensive stale-state retirement to at most
two preroll runtime budgets, with no new preparation until both drain.

Tests use `serde_json` to check saved document version/provenance structurally.

## Does not own

Serialization formats, durable-write primitives, audio devices or callbacks,
UI state, plugin hosting, or another project model. WAV
decoding, rate conversion and waveform preparation must happen on a control worker before an
import is committed; they never run on the audio callback.

Undo/redo stores project metadata only. Decoded audio and summaries remain in
the session bank so a restored reference can compile without reloading media.
The caller-provided resident-frame ceiling and the canonical asset-count limit
bound this retained bank. Exceeding either limit rejects an import rather than
silently discarding cached media.

The service has 32 request slots and two event slots. GPUI submits/polls without
waiting; the worker decodes, edits, compiles, loads and saves. Project replacement
validates and compiles a candidate before committing it. Save failures preserve
the prior document association; successful Save As adopts the exact published
byte token, and replacement adopts its new token even when reporting a post-commit
durability warning. Saving does not change the model revision or compiled graph.
Each worker `Event` marks processed Save As/Save Current requests with
`save_complete`, including failures, so the UI can release an in-flight save
without guessing from the status text.
Endpoint teardown closes
the worker channels without joining on the UI thread.

Each accepted candidate is prepared as one `PreparedRevision` before project,
history, resident-media or document-open state is committed. It owns the exact
Arrangement graph, MIDI preview and validated Session inventory used by the
worker. Edit, import, undo, redo, initial load, open and restore reuse that
artifact. The service prepares the optional Session playback kernel on its
worker. If only Session timing/runtime preparation is unavailable, it returns
the exact artifact with an explicit capability reason and keeps Arrangement
playback available; canonical compile or inventory validation errors still
reject the transaction. The legacy command/preview return types remain
available and derive their outputs from the same full prepared transaction;
they clone the compiled Arrangement value to preserve their historical owned
return types.

The controller owns one shared sine table for its lifetime and retains it across
project replacement. Session template reuse additionally requires unchanged
owning MIDI-source identity and exact timing/rate compatibility. These prepared
structural work envelopes are not runtime Session mixing or physical audio
deadline guarantees.

Prepared imports preserve original source rate/count while PCM, waveform and clip
intervals use project-rate frames. Worker open prepares one asset at a time and
validates both source provenance and playback length before replacing the session.
Each source is capped at 64 MiB / 8,000,000 frames; resident converted PCM has a
separate 8,000,000-frame desktop ceiling, including assets retained for undo.
Opening a saved project still requires its rate to match the active device.

`service::prepare_audio_preview` is a read-only browser-drag preparation helper.
It uses the same bounded WAV decode and project-rate resampler, then returns only
the converted frame count, rate and a 48-bin waveform summary; decoded PCM is
dropped before returning. The UI calls it through GPUI's background executor,
keeps at most one preparation active and coalesces pending requests. It does not
submit a project edit or reserve project history. A browser sample drop onto an
existing Arrangement track uses `Request::ImportToTrackWithOverwrite`, which
reopens and validates the source, cuts overlapping audio/MIDI clips, and commits
one undoable import. Other non-drop imports retain `Request::ImportToTrack`.
Browser summaries
refresh on each drag start, but the source is not pinned while the pointer is
held; a file modified during that interval can differ from the displayed
preview. Import still reopens and validates the current source.

For a browser sample drop in an Arrangement gap or empty project, the UI submits
`Request::ImportToNewTrack` with a stable before-track anchor (or `None` for the
end) and snapped frame start. The worker validates the anchor, track/asset
limits and resident budget before decoding, then commits the prepared track,
asset and clip together. A removed anchor, canceled request or failed compile
leaves the canonical project unchanged. Existing-track sample drops use
`Request::ImportToTrackWithOverwrite`; new-track drops have no destination
clips to cut.

The canonical project supports hybrid audio/MIDI tracks, MIDI source offsets,
retained loop markers, finite arrangement lengths, note editing and schema-13
persistence. Session audio
imports create independently owned frame-anchored one-shots in empty audio cells;
occupied targets, missing media and resident-budget failures preserve canonical
state and resident playback. Original name/path casing is retained. Instrument
candidates compile through the original bounded sine instrument, shared by
native playback and offline export. Expanded loop occurrences are counted
project-wide before allocation and candidates that exceed occurrence, voice or
onset budgets fail before replacing the working session; admitted edits return
the prepared graph. Native creation and piano-roll commands use this same service.

Instrument edits prepare the exact candidate graph before committing canonical
history. Preparation failures preserve IDs, history/redo and worker revision;
success returns that graph and its matching immutable MIDI Arrangement preview
without a second compile. Native callback publication carries both monotonic
Session performance time and the loop-wrapped Arrangement cursor. The service
does not own audio callback state or activate Session UI controls.
`ViewState.midi_preview` is an Arc-backed index keyed by
clip ID and shares the state revision; save/path-only events reuse the same Arc.
Native export captures its compiled audio/MIDI clip and release-tail horizon.
Prepared linear WAV export uses that captured revision directly; enabled mutable
Arrangement instruments currently require a zero range start, and mutable
Arrangement source loops are explicitly refused.

`tests/session_workflow.rs` exercises the public worker contract with original
Session MIDI clips, an explicit Stop cell and an Arrangement MIDI companion.
Clear-cell/scene deletion and undo/redo preserve stable IDs; Save As/Save Current
and a fresh worker reopen preserve the complete canonical project and prepared
inventory. Explicit relaunch renders match across irregular block partitions,
including loop and one-shot ends. Canceled quantized launches remain canceled
past their original deadline. These are headless persistence/runtime checks;
native dialogs, physical output and reference parity are separate acceptance.

`tests/scene_editing.rs` covers worker-driven scene duplication and stable-ID
reordering, prepared inventory order, undo/redo, Save As and fresh-worker reopen.
It explicitly launches original and duplicate cells and checks identical PCM
across different render partitions; runtime route state is created by those
launch commands rather than persisted in the project.

Native startup opts into `Service::with_recovery`. Committed canonical revisions
are checkpointed on the worker before acknowledgment; failure preserves working
edits and reports the recovery gap. `OpenWith` admits explicit journals, previous
saves or path-only missing-media relinks only after complete decode/compilation.
Recovery opens dirty without an overwrite association; ordinary relinked Open
retains its original exact-byte document token. Untitled recovery retains only
the latest project, with no history. Contracts, source tests and native gaps:
[project recovery](../../docs/project-recovery.md).


## Gain take coordination

Arming prepares a transient raw-fader revision without changing canonical history
or its cached compiled project. Delivered audio receipts normalize into bounded
half-open spans using exact content/performance positions and loop passes. A
revision-stamped splice batch rechecks generation, revision and lane epoch, then
commits the complete take as one Undo/Redo edit. Failed/stale/cancelled takes leave
the original lane intact. Audio owns application and receipt delivery; the UI
owns only drafts and gestures. See [gain recording](../../docs/gain-touch-spans.md).
