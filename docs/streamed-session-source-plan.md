# canonical verified source plan

Pulse2544 / PR239 connects the existing Session worker to the native streamed
publication path. The first source/service checkpoint supplies the neutral
preparation described here; desktop composition, nonowning UI clients and native
publication remain unfinished until the rest of that PR is implemented.

`DocumentAudioMode` is selected explicitly for a fresh New/Open before media
preparation. Resident remains the default. `VerifiedStreaming` imports and opens
use `VerifiedWavReader` to verify bounded headers, source format, complete file
fingerprint and page manifests without decoding/resampling full PCM. Playback
frame counts use the canonical checked conversion function. No resident frame
charge is fabricated and the resident duration ceiling is unchanged. Each source
still obeys 64 MiB / 8,000,000 source-frame ceilings. Caller reservations also
bound retained source count, actual path capacity, verification manifest capacity,
header count and read window. Retained history consumes those reservations even
when Undo removes all current clips. Verification/readiness failure never adopts
a partial canonical edit; valid canonical edits do not certify native admission.

Verified source facts contain an immutable resolved path, fingerprint, source and
project rates, source/playback frame counts, conversion version and reader bounds.
They contain no reader, descriptor, page endpoint, catalog or native lease.
Private shared immutable payloads keep Session/history, temporary semantic banks
and read-only snapshots from duplicating verified path backing. Source-count
bounds cover payload count; each plan's bounded map carries its own handles.
The reservation is source metadata, not a process RSS or allocator estimate.
Verification manifest sizes are retained as conservative reservation charges even
though the temporary reader/manifest is dropped before canonical commit.

Relative media resolve against the opened document's retained base; successful
import/open captures that resolved binding. Save As changes the document handle,
not source identity or its base. Ordinary edits and Undo/Redo preserve captured
facts. Reopen/relink verifies a fresh plan rather than silently refreshing bytes.
A checked private semantic source epoch changes for a fresh verified document or
verified binding-set addition. Ordinary edits/history reuse the same epoch.
Semantic keys never grant native catalog identity: the native facade must match
complete facts against its actual retained domain and independently reverify the
expected fingerprint through the worker.

The existing canonical snapshot pairs the exact Project, immutable source plan
and actual worker document generation/editor revision. Source catalog generation,
semantic epoch and document generation are separate. Preview views cannot mint a
canonical snapshot. Semantic compilation uses the existing mixed-source prepared
revision compiler for edit/import/history transactions; it is display/control
preparation, not native constructor custody. Verified-mode `Event.playback` is
absent, including empty and source-free revisions, so it cannot fall back to the
legacy publication lane. The later desktop facade must use fresh consuming
constructor banks and the existing sealed Root/domain/readiness APIs.

Waveform summaries are unavailable for verified-only sources at this checkpoint;
no zero-filled peaks pretend to be measured audio. Resident waveforms remain as
before. Offline export and gain recording explicitly refuse verified-mode
sources. Streamed offline export is deferred to Pulse2650 / PR266; it must not
run the current prepared NoPages renderer. No device eligibility, ownership,
readiness, physical deadline or native UI/listening acceptance is established by
this source plan. The native admitted audio-only/Utility class and original
Pulse24 / PR177 acceptance remain unchanged.

## worker/client endpoint checkpoint

The native worker factory now reserves four constructor-paired terminal
destinations before CPAL exposure: data callback, error callback, UI client and
worker publication. `WorkerPreparedOutput` retains the stream and publication
endpoint. Its one-way `take_client` boundary permanently prevents later Resident
conversion/configuration. The client exposes borrowed/scalar observations and
bounded commands, not a cloneable status Arc, publication queue, graph or stream.
Legacy unsplit `Control` remains compatible with existing Resident callers.

Client close/Drop signals closing and priority Stop, transfers its entire endpoint
Box into the already reserved terminal, then becomes inert. Endpoint/ring/PCM
destruction and joins never occur in that transfer. Actual endpoint destruction
runs on lifecycle control, outside ordinary locks, with Pending recorded before
Drop and sticky first-payload/no-retry failure custody. Callback nonrenderability,
graph/source cleanup and final endpoint resource release are separate: graph
cleanup can complete without UI polling while a client remains live, but the
exclusive output lease cannot release until its actual endpoint destructor has
completed normally. Undelivered clients use the same abandonment path.

The split moves four formerly separate atomic payloads into the measured Status
allocation. A single admission word serializes split gain Begin against worker
publication before either commits ownership or queue state. Callback completion
clears only publication; a matching current gain terminal clears only gain.
Queue failure rolls back its own reservation; stale gain terminals cannot clear a
new take. Legacy unsplit publish-then-Begin ordering remains supported. Verified
constructor mode still refuses gain recording and every owning preview route.

Known DATA covers actual persistent retirement/guard values, both endpoint Boxes,
terminal cell payloads, queue backing capacities and scalar payloads. Opaque Arc
control blocks remain bounded RESOURCE groups, not guessed bytes or scalar
completeness. After the split the auxiliary rings/scalars contribute eleven
paired groups; two additional terminal cells contribute four actual handles,
and the client bundle adds one Status handle. Constructor hooks contribute two
other Status handles. The actual factory visitor and caller reservations retain
these until physical completion; public `require_complete` remains unchanged.

This is an endpoint/lifecycle checkpoint only. Session sink composition,
nonblocking worker open delivery, document-generation replacement and the UI
client integration are still unfinished. Headless controls do not establish
physical CPAL deadlines, OS dispose success, listening or native UI acceptance.

## capture-enabled native boundary

The strict worker factory now constructs the existing spectrum Core on control
before moving it to the analysis worker. An exhaustive owning visitor measures
its two rolling/ordered Boxes, the kernel's four Boxes, actual capture-ring
capacity, stop payload and lifecycle Reaper value. Capture and Reader inline
storage lives in the already measured callback/client Boxes. The std result
channel, stop pair and capture ring are three bounded opaque RESOURCE groups
with six constructor endpoint/handle obligations; the one analysis thread is a
separate bounded lifetime, not an OS/stack/TLS byte estimate.

A private receipt correlates the actual capture buffer, Reader and Reaper. A
same-format unrelated Reader or Reaper cannot cover that factory. Generic dynamic
spectrum workers cannot supply the native receipt. The native path fixes analysis
to its actual device sample rate and refuses a wrong-rate capture before queue
mutation while consuming its sequence/gap. Therefore that constructor cannot
enter the generic worker's old-plus-new-kernel replacement peak. Generic workers
retain their existing rate-change behavior.

The Reader is inside Client before terminal reservation; polling returns only
bounded spectrum values. An independently owned Reader cannot escape the strict
worker factory and survive terminal completion. Lifecycle retains the actual
Reaper through a borrowed once-only join. A worker panic preserves its exact
first payload and marks sticky failure; subsequent advance cannot retry or
release the lease. Even failure during channel construction transfers the Reaper
and exclusive lease into the existing unexposed lifecycle custody. No join runs
on a UI/close/callback path.

Headless controls cover actual fixed-rate worker analysis, wrong-rate refusal,
Reader/Reaper cross-pair refusal, caller DATA reservation refusal, capture-enabled
callback zero total/net allocation and the actual Reader-live lease barrier.
Disabling the native rate guard or dropping the first worker panic payload makes
the focused controls fail. This remains a staged source checkpoint: Session
sink/facade/UI integration, final repository CI and native listening/UI acceptance
are still outstanding.

## Worker connection checkpoint

The existing native lifecycle open lane can now return a four-terminal worker
output to the Session worker. The UI does not receive that output. The neutral
`WorkerAudioSink` constructor negotiates the device rate before canonical Session
startup; an explicit device failure keeps offline editing and saving usable.
Canonical generation/revision notification precedes event-queue backpressure,
and native progress continues while event delivery is full or requests are idle.
Closing Service signals the worker; its adapter close runs there without a UI
join. This source checkpoint provides the connection seam and headless controls;
the desktop adapter, mode selectors and live publication integration remain
outstanding. It is not final repository CI or native listening acceptance.
