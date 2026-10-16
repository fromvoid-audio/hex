# canonical verified source plan

Pulse2544 / PR239 connects the existing Session worker to the native streamed
publication path. Desktop now composes the neutral source plan, worker-owned
native preparation and nonowning UI Client described below. Exact final repository
verification and native UI/listening acceptance are recorded separately. Earlier
checkpoint sections retain their historical scope and do not override this
connected contract.

`DocumentAudioMode` is selected explicitly for a fresh New/Open before media
preparation. Resident remains the default. `VerifiedStreaming` imports and opens
use `VerifiedWavReader` to verify bounded headers, source format, complete file
fingerprint and page manifests without decoding/resampling full PCM. Playback
frame counts use the canonical checked conversion function. No resident frame
charge is fabricated and the resident duration ceiling is unchanged. Each source
still obeys 64 MiB / 8,000,000 source-frame ceilings. Caller reservations also
bound retained source count, actual path capacity, verification manifest capacity,
header count and read window. Native reader windows use the existing stream
source-scratch frame allowance, not the 4096-frame output page size. Downsampling
48 kHz to 44.1 kHz requires up to 4530 source frames including the resampler halo.
The worker still validates this source window against both reader and actual
scratch bounds before catalog readiness; file/source/page/runtime caps remain
unchanged. Retained history consumes those reservations even
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

## connected desktop and editor

New/Open selects the mode before decoding. A Verified new-track import removes
only its freshly generated default instrument in the same validated canonical
transaction; batch import remains one Undo step with stable IDs. Existing-track
instruments and all Resident creation behavior are preserved. Unsupported edited
instrument/MIDI/DSP/typing graphs remain editable/saveable and explicitly refused
for native audio; they never fall back to legacy publication.

The existing Session worker owns the Desktop sink. It binds complete captured
source facts to actual sealed Root domains, prepares fresh consuming banks in one
persistent runtime slot, and stages from that same slot. One active, one queued
successor and one preparing runtime justify three cells and their archive/report
reservations. Existing canonical cardinality limits produce conservative global
row ceilings; each request reserves scratch for its smaller captured Project.
Actual constructor capacities, aliases, DATA/resource closure and final seal
remain authoritative. These reservations are not exact costs or a process memory
cap: 32 MiB mutable runtime (including native State/workspace), 32 MiB page-pool
PCM per actual domain, immutable graph/source metadata and opaque kernel/resource
scopes remain distinct. No arbitrary 192 MiB/eight-graph allowance is introduced.

All revisions after Verified opt-in, including resident-only and empty successors,
use captured context, typed custody, command-prefix ACKs and normal N2 admission.
An empty required-source set still checks the actual sealed/nonretiring domain;
it may reuse an actual covering domain, never a stale token. Same-document Undo
can reuse a retained superset; a new document generation needs a new output/domain
identity even when paths are identical. Source-bank construction starts only
after the Root runtime cell and source pair cell are reserved. Cancelled, refused,
failed and archived owners remain charged through normal destruction and report
consumption. Ordinary domain/queue pressure reports busy without vetoing an
already valid canonical edit. A permanent refusal stays refused while its
ordinary cleanup retires. Incomplete ownership cleanup retains its lease and
reports save/restart hex, rather than promising an impossible in-process reopen.

Generation notification is independent of output readiness. The worker relinquishes
old output/publication before event backpressure; the editor closes the old Client
on that notification before awaiting the new Client. Explicit restart results are
request-correlated; automatic replacement is not a second restart. Output failure
leaves canonical editing/saving usable. Service Drop signals without joining; the
existing worker/lifecycle paths progress cleanup without UI collection.

The UI owns no native stream/output or Verified candidate. Its Client supplies
borrowed status, scalar command tokens, direct priority Stop and bounded Pause. Resident
compatibility alone retains existing preroll/PendingPlayback and one nonblocking
forwarding cell; this is not a universal rewrite of Resident graph ownership.
Verified seek/launch uses PR238 only after the exact current graph is actually
applied. Preview/recording/typing ownership never crosses the sealed Verified lane.

Submitted, installed and applied are separate observations. A bounded SeqCst
status tuple is written only by the actual eligible install before its ACK;
legacy install clears it. It binds full captured source context, actual output
identity and audio revision. Bounded reads may return unavailable on contention
without erasing the UI's prior observation. This preserves truthful last-audible A
when Submitted(A) is coalesced and newer canonical B refuses. Only an exact current
context/client match can reconstruct nonowning current graph/seek metadata or
advance restoration. A current unpublished refusal stays rejected in the footer;
an actual later ACK wins. This status reconciliation adds no owning graph history or event queue.

The nondefault Desktop fixture substitutes only output-open completion, using
actual callback/endpoint guards and the normal completed-handle retirement path.
It runs production Session verification, domain preparation, native publication,
worker decode and callback rendering. This proves headless behavior, not physical
CPAL stop/disposal success, device deadlines, UI interaction or listening.
Waveform peaks for verified sources remain explicitly unavailable; offline streamed
export remains Pulse2650 / PR266. Original Pulse24/PR177 and native100-track/edit
acceptance in PR240/241 remain open.

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

The Resident compatibility adapter has one constructor-reserved request/result
cell, not an owning candidate queue. Nonblocking Client submission preserves the
original Playback on synchronous refusal. Worker publication reports its real
submitted revision separately from both the request token and callback applied
revision; no mutex spans publication or reclamation. The cell's actual inline
payload and two shared handles are added to native constructor accounting.
Verified configuration requires a virgin cell and refuses owning Playback
admission for its entire exposed lifetime. Desktop/UI use of this cell is still
being connected; the native headless controls establish only this boundary.

## Pending verified pair checkpoint

A new pending pair can quote bounded constructor DATA and resource counts before
allocation. It reserves the existing endpoint and exact-pair disposal backings
before starting the source worker, removing the former fallible endpoint setup
and inline failure join after spawn. Complete catalog publication, actual byte
capacities and closed handle associations are still required before sealing.
The same cleanup backing moves into the sealed domain; cancellation or changed
file refusal conserves the original pool through normal join and destruction.
An actual worker panic retains its first payload and remaining custody. The
quote is a conservative allowance, not scalar completeness or page authority.
The native Root now reserves one of its existing two domain cells before this
factory runs. Starting, verification, sealing and cleanup retain that same
incarnation and its conservative DATA/resource allowance. Close during startup
cannot take the cell or drop a successful live result. Actual catalog refusal
enters normal lifecycle cleanup; first-panic failure stays charged. A sealed
endpoint enters the existing publication ring only after reserving its actual
entry and checking the terminal barrier; late closure retains it in Root custody
instead. Full queue pressure leaves the pending owner untouched. Pool/metadata
closure does not free the cell until the scalar report is consumed. All new
inline slots and execution destinations are included by the existing actual Root
size visitor; opaque synchronization/worker costs remain resource obligations.
A pending unsealed domain cannot satisfy a new complete native claim. Desktop/UI
composition remains outstanding at this source checkpoint.
