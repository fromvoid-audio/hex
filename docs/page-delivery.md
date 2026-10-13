# Bounded streamed playback delivery

## Next concrete outcome

Exercise an actual verified media worker, fixed page ownership queues and engine
rendering against borrowed pages. Preserve resident rendering while adding a
streamed source alternative. This is the next implementation dependency after
shared kernels and VerifiedWavReader; no native streamed playback path exists yet.

The first headless worker/renderer slice is source accepted. It is not a native
playback capability. The resident distinct-source baseline is captured
in artifacts/stream-delivery-resident-before-profile.log with source copies,
compiler and executable identity in the matching manifest. Preserve that
baseline while changing clip layout and the borrowed-render interface.

The first source test must traverse real request/ready/return queues, read and
convert a non-project-rate WAV, and render Arrangement PCM matching resident v1.
Then exercise 100 independent long sources with one active clip per track. That
fixture proves its stated workload; arbitrary overlap density, device chains,
Session readiness, native input and sustained deadlines remain full-goal gates.

## Responsibilities and interfaces

Add hex-stream when its worker/cache implementation is ready, with a README and
workspace policy in the same change. It owns source catalogs, verified descriptor
lifetimes, explicit rate-pair kernel reuse, scheduling, fixed page buffers and
request/ready/return ownership. Depend on hex-media and hex-engine (and canonical
project IDs when needed). Keep codecs and hashing in hex-media. Extend the policy
to allow rtrb in hex-stream and the hex-audio -> hex-stream edge; CPAL stays in
hex-audio. Do not create an empty protocol crate before implementation.

hex-engine keeps no stream/media dependency. Add Copy runtime source keys and
validated project-rate source metadata for streamed clips. Borrow a narrow page
view at render time, ideally contiguous spans from a requested source/frame
position, so a page lookup happens per span rather than a linear cache search
per sample. Preserve resident APIs and canonical mixing/clip order, fades,
source-phase, loop and master-gain semantics. Compile streamed source references
through an explicit media-resolution path while leaving existing media banks
usable. No queues, file handles or fingerprint calculation belong in engine.

hex-audio later supplies the callback-owned page view, receives pages with bounded
work, coalesces Copy requests, and retires pages/catalog leases off callback using
the existing publication/reclamation discipline. Root desktop/session composition
must inject endpoints; GPUI does not implement disk or cache logic.

Runtime keys include project generation and AssetId. Descriptor metadata also
carries the captured content fingerprint, conversion version and rates. Catalog
bindings are immutable within a generation; relink/replacement needs a fresh
binding/generation. Successful Open creates a new generation even if AssetId(1)
is reused. Undo within a project retains needed descriptors. Export retains its
original generation. Identity is not a lowercase path string or AssetId alone.

## Initial budgets and ownership

Start with 4,096 stereo frames/page and 1,024 fixed buffers: 32 MiB of PCM payload,
excluding queues, lookup metadata, source manifests, kernels and worker scratch.
All capacities are configured/admitted on control side. Account for digest-vector
capacity, unique shared kernels, waveform summaries, all live catalogs/history,
active/candidate/retired revisions and exports in aggregate budgets.

A 256-frame block on 100 single-clip tracks can touch 200 pages at adverse source
boundaries. One page of lookahead adds 100. Retaining an old and prepared new
100-track window can require 600 pages with lookahead; a 320-page pool would not
cover that transition. The 1,024-page starting pool provides headroom for this
specific case. It is not blanket admission for 256 tracks with arbitrary overlap.
Derive unique page demand from actual active clips, source offsets and loop seams;
refuse/defer preparations that cannot fit without starving active audio.

Allocate buffers, cache lookup storage and queue slots before callback use.
Transfer Box<PageBuffer> ownership rather than constructing PCM Arcs in the
callback. Request packets contain Copy keys and page indices. Ready packets own
one existing pool buffer. Evictions return that buffer to the worker. Full queues
retain buffers in bounded held slots; never allocate, wait, lock, log or finally
destroy a buffer/source on callback. Stale completions return through the pool
and cannot enter another generation's cache. Bound queue draining and request
work per block; lookup must not scan every cache slot for each audio sample.

One worker initially owns readers/converters and free buffers. Coalesce duplicate
page requests, prioritize imminent pages ahead of lookahead and use request
generations for cancellation. Typed source-change/cancel/decode failures travel
back to control; never publish partial or unverified pages. Validate complete
source signal constraints before admitting a streamed project, including distant
nonfinite floats, using bounded preparation work.

## Readiness and misses

Prepare and pin mandatory first-block and lookahead pages before publishing a
streamed graph. Keep the paused playhead warmed so prepared Play requires no
deliberate disk/compile wait. Candidate edits cannot evict mandatory active pages
while warming their own window. Initial graph/page installation and command
consumption ordering must be tested explicitly.

Cold Seek and Session launch use asynchronous prewarming before command admission,
with pending UI state and the existing audible state retained until ready. Never
accept a cold start and silently lose its transient. Loop seams need prepared
pages before enabling streamed loops. This is the intended integration contract;
those native paths are not implemented yet. Measure request-to-first-audible time
and keep physical storage/device latency distinct from UI responsiveness.

A genuine cache miss contributes zero for the missing source span, keeps the
musical clock moving and increments bounded/coalesced miss telemetry. Preserve
other valid track contributions; do not substitute stale samples or block. This
is a recovery path, not successful warmed-playback acceptance.

## Lifecycle and acceptance

Source catalogs/workers outlive every active, candidate, deferred, retired and
export graph using their generation. Stop new requests, stop audio, drain page
and publication ownership, then join/reclaim sources off callback. Repeated
startup/shutdown, request cancellation and failed Open must preserve prior usable
state. Detached worker lifetime is insufficient for this contract.

First headless checks: exact converted PCM across page/source boundaries and
irregular blocks; missing pages zero plus one coalesced request; queue saturation;
stale generations; changed source; cancellation; all pool buffers accounted for;
shutdown and final ownership off callback. Use real worker IO and engine page
rendering. Run a 100-independent-source fixture and report preparation, queue
latency, render timing, footprint and misses separately. Integrate hex-audio only
after these pass, with allocation/retirement probes. Then wire native import/Open,
publication, history, persisted identity, Session/seek/loop readiness and export.

## Current acceptance checklist

- [x] The real worker opens a verified non-project-rate source, reuses a shared
  conversion kernel, fills only preallocated pages and publishes complete PCM.
- [x] The same engine Renderer matches resident v1 bits across page/source ends,
  source phase, trim, loops, fades, irregular blocks, mix order and master gain.
- [x] Provider-less playback explicitly rejects streamed content; resident
  callers, paused rendering and overflow rollback retain their contracts.
- [x] Cold misses preserve valid tracks and clock progress, coalesce requests
  and expose bounded telemetry. Pressure and stale completions lose no buffers.
- [x] Source mutation, distant nonfinite samples, cancellation and worker errors
  produce typed failure; shutdown drains and joins with every buffer accounted.
- [x] A real 100-independent-long-source fixture reports preparation separately
  from warmed render timing, pool/manifest/kernel footprint and actual misses.
- [x] Engine/media/stream tests, targeted strict Clippy, desktop build and policy
  checks pass from a stable source checkpoint; inspect the resulting interfaces.
- [ ] Follow with canonical compilation, callback allocation/retirement and
  native project lifecycle/readiness integration. The first headless slice
  alone does not complete streaming or the full performance requirement.

Headless acceptance is now evidenced by root scalar and real-worker debug/release
PCM checks, resource/stale/mutation/shutdown checks, eleven stream tests including
queue pressure/turnover, and the scoped 100-track 17M-frame profile. The first
seven checklist lines above are satisfied for the documented headless fixtures;
native callback, catalogue/history/export-wide budgets and publication readiness
remain the final open line. See artifacts/stream-delivery-final-manifest.json for
source, executable and log identity, and performance.md for reporting limits.

## Canonical integration dependencies confirmed in source

`hex-engine/src/compile.rs` currently resolves every Arrangement asset from a
`BTreeMap<AssetId, Audio>`, validates rate/playback count, and constructs each
clip in canonical order. Extend that one control-side compiler with explicit
resident/streamed resolution rather than duplicate MIDI, solo, preview or
mixing behavior in a second compiler.

`PreparedRevision::prepare` builds both Arrangement and Session inventory from
the same resident bank. `PreparedSessionAudioClip` currently retains Audio and
compares its PCM identity for phase handoff. `hex-session::Session` also retains
resident assets and exposes that bank for revisions. An Arrangement-only page
renderer cannot make those native paths streamed by itself. Extend these source
contracts together before lifting native import limits; preserve source identity,
history/recovery leases and fresh project-generation semantics. Until then,
provider-less prepared/native APIs must continue to refuse unsupported streamed
content rather than admit a revision that later plays silence.

## Next finite compiler outcome

Add control-side resident/streamed source resolution to that one canonical
compiler, with existing resident entry points unchanged. A clearly named
Arrangement-only function is the next independently useful bridge; it must not
create a second project model or claim to prepare Session/native playback.
Validate every referenced streamed source's AssetId and expected nonzero open
generation, original rate/count, project rate/playback count and conversion
version against the canonical asset. Unused media entries remain permitted,
matching the current resident API and retained-history use. Never fabricate
resident frames for a streamed binding.

Acceptance: resident API/PCM and MIDI preview parity, mixed resident/streamed
canonical mix/solo/loop/fade/phase behavior, each metadata/generation mismatch
rejected before rendering, and an actual worker-backed canonical Arrangement
matching resident PCM across page/source boundaries. Keep service/native
PreparedRevision on its current explicit resident path until Session is extended.

Then split control lifecycle from callback pages. StreamRuntime's Drop joins a
worker, so the whole object cannot enter the audio callback. One worker/catalog
and pool must serve normal revisions of an opened generation; no 32 MiB pool per
edit. Retain source leases through existing audio graph retirement/reclaim. A
separate bounded prewarm request/ack path must respect SPSC producer ownership;
control cannot race the callback on the current request producer or page cache.
Extend PreparedRevision/Session inventory and one-shot route rendering using
the same source resolver before native streaming publication.

Persisted source identity is a further required decision: current canonical
assets save path/rates/counts/version but no expected fingerprint. Reopen scans
capture current bytes and cannot prove an earlier file remained unchanged. Add
an explicit schema/sidecar identity contract and legacy migration before claiming
save→reopen source-integrity validation. No such guarantee follows from this slice.

Canonical Arrangement resolver acceptance — 9 October: `PreparedAudioSource`
provides one resident/streamed source bank; existing resident APIs borrow their
original maps without copies. The shared compiler and preview helper preserve
canonical clip/MIDI behavior. Five focused tests and full engine regressions/
Clippy pass. Root actual-worker debug/release matches 60,452 PCM frames against
resident wrappers/frozen v1 conversion, including three explicit loop-boundary
seeks, identical MIDI previews, zero warmed misses and eight-buffer reclamation.
Renderer remains linear; automatic streamed transport-loop scheduling is open.
Session/audio allocation regressions and desktop build pass, but their native
path remains resident. Control/callback ownership split is the next finite unit.

## Control/page split implementation contract

Keep the headless Runtime facade, but expose separately owned control and page
halves. Control owns worker join/wake, cancellation and catalog readiness.
Pages own preallocated cache/coalescing state and bounded request/ready/return/
failure queue ends. Failure draining stays with pages because consuming a
failure clears its pending request and enables retry; later telemetry must copy
the bounded failure record rather than share the consumer or mutate the cache
from control. Pages have no worker Thread, mpsc receiver, descriptor or join API.
Their ordinary destruction still frees cached storage: reclaim them on control
after callback quiescence. This is an operational ownership boundary, not a
claim that arbitrary endpoint Drop is realtime-safe.

Explicit shutdown consumes the matching pair. A private token identifies the
runtime, independently of AssetId/generation collisions. A mismatched pair must
return both owned halves unchanged for correct subsequent shutdown. A matched
shutdown returns cache ownership, cancels/wakes/joins on control, drains ready
and held returns, and counts actual terminal boxes exactly once. Best-effort
control Drop may cancel the worker; descriptors therefore survive only while
control remains alive. Do not invent a metadata lease that cannot prevent worker
shutdown. Keep that owner alive until referencing revisions/endpoints retire;
dynamic catalog install/release and graph leases remain later work.

The callback page endpoint remains the sole request-ring producer. A future
prewarm path needs a separate bounded control→worker queue and readiness acknowledgment,
with bounded scheduling/coalescing and callback ownership of ready-page install.
This stage adds no concurrent producer, native CPAL endpoint or Session streaming.
Acceptance is split-half actual-worker PCM, identical facade behavior, wrong-pair
recovery, pressured exact pool accounting, and allocation/deallocation probes for
page operations. Setup, worker-thread IO and shutdown stay outside probe scopes.

Split acceptance — 9 October: the implemented facade and separate control/pages
API pass eight unit, one allocation integration and six worker integration tests
in debug/release, strict Clippy and format/policy/style guards. Root real-worker
split tests match 60,452 PCM frames/frozen v1 pages and identical previews; eight
buffers are accounted after join. Two actual catalogs with identical file/AssetId/
generation keys reject wrong pairing without stats/cache mutation, then correctly
reclaim 16 buffers. Endpoint probes cover successful queue requests, failure
retry, ready install/eviction/full return ring/held returns and pins without
caller-thread allocation/deallocation, with alloc/free sensitivity calibration.
Control-side mismatch boxing and all setup/shutdown occur outside those probes.

This does not prove native endpoint lifetime, arbitrary endpoint destruction,
dynamic generation release, prewarm readiness, automatic streamed loop scheduling
or sustained device deadlines. Continue with Session source resolution/rendering
and the remaining native ownership/readiness dependencies.

### Real catalog ownership accepted; implementation paused — 9 October

StreamControl and callback pages now retain the actual shared catalog owner.
Ready catalogs provide opaque CatalogLease handles with verified sorted full
metadata membership; fingerprints/generations must match and separate runtime
identities remain distinct. Control Drop no longer cancels a worker retained by
pages/leases. Matching shutdown refuses outstanding leases before mutation and
returns both halves intact; dropping leases permits retry and exact pool
accounting. Final owner release stops/wakes/joins off rendering.

Debug/release stream suites pass nine unit, one allocation integration and nine
worker integration tests. Strict Clippy, workspace formatting and architecture/
UI style guards pass. Independent debug/release checks match 110,758 Session PCM
frames, preserve cache/stats through Busy recovery, reclaim eight buffers, reject
wrong full metadata and distinguish identical-key catalogs; an uncached real page
read succeeds after controller Drop. Private test observation confirms final lease
release joins on the named control reclamation thread after endpoint/controller
release. Provenance: `artifacts/stream-catalog-lease-acceptance-manifest.json`.

This is headless ownership acceptance. Native graph/export lease attachment,
callback endpoint retirement, device-fault teardown and non-UI final reclamation
remain required; prewarm installed-page acknowledgment and sustained native
streaming are also open. Ordinary endpoint/lease final Drop is not realtime-safe.
The user requested a pause after this unit to redirect work toward quicker
practical usefulness. No further implementation or prewarm/native admission
work was started; preserve the full roadmap for that discussion.

## Graph/catalog binding stage — Pulse24 / draft PR177, 13 October 2026

`hex-stream::VerifiedStreamRevision` pairs the exact immutable prepared graph
with the actual verified catalog owner. Its control-side constructor consumes
both owners, borrows every streamed source and checks complete catalog membership:
generation/asset key, fingerprint, source/project rates, frame counts and
conversion version. Unused catalog sources are allowed. Failure returns both
inputs unchanged through `StreamRevisionMismatch::into_parts`; no partial owner
installation or worker mutation occurs. The first mismatched source key is
reported. Distinct catalogs remain distinguishable even when metadata/keys and
the graph Arc are identical. The wrapper intentionally has no Clone implementation.

The engine exposes `PreparedRevision::streamed_sources()` as a borrowed iterator:
compiled Arrangement track/clip order first, then Session `audio_clips()` inventory
order. Repeated source references remain separate; resident audio and MIDI are
omitted. Iteration allocates nothing, clones no Arc and exposes no mutable or
private clip state. Engine retains no dependency on stream.

Graph owners may retain separate old/candidate bindings to one catalog. Existing
Busy shutdown refuses while either lease remains and returns the control/pages
pair unchanged. Releasing the wrappers on control permits the existing exact
pool-accounting shutdown. If the wrapper holds the final catalog reference after
control/pages retire, its final release stops/wakes/joins on its control reclamation
thread. Borrowing a graph or catalog is not authorization to construct, clone or
finally drop owners on a callback. No native callback integration is added here.

Focused coverage includes wrong generation/fingerprint/source and project rates/
frame counts/conversion contracts, Session-only mismatches, mixed streamed and
resident Arrangement/Session enumeration, borrowed metadata remaining valid after
external preparation handles retire, unchanged Arc counts, distinct same-key
catalogs, independent old/candidate graphs, transactional Busy recovery and final
wrapper-owned worker reclamation. The existing real-worker Session allocation
probe now retains a verified binding and checks allocation-free metadata iteration;
construction/reclamation remain outside its measured render scopes.

This is the first bounded source stage of the full long-project streaming ticket,
not completion of Pulse24. Native publication still explicitly refuses streams;
resident import limits are unchanged. Endpoint publication/retirement,
control-to-worker prewarm with installed protected-page acknowledgment, complete
coexisting-catalog budgets, sustained advancing 100-track mixed-rate native
playback and edit/seek/launch/fault/user acceptance remain unimplemented/unverified.
PR177 stays draft/in progress. Its complete eight-criterion verification gate,
including manual human_review and both native criteria, must remain intact.

## Typed catalog/page pairing checkpoint — pulse24 / draft177

`VerifiedPageEndpoint` consumes the existing `StreamCallbackPages` and verified
`CatalogLease` on control. Validation checks both actual private catalog owner
and pair identities before endpoint/worker mutation; identical metadata/source
keys do not establish identity. The wrapper has no Clone implementation. Its
borrowed page accessor reuses the existing engine-neutral MediaPages endpoint;
it neither installs protected pages nor authorizes native publication.

Mismatch retains both inputs; its owning error boxes the large page endpoint
on control, following the existing shutdown error convention. Successful pairing
and borrowed endpoint operations allocate/free nothing and create no Arc owners.
Construction/error recovery and final disposal are control-only operations.
`into_parts` returns pages and lease intact. Release that lease and graph leases
on control before calling the unchanged `StreamControl::shutdown(pages)`.
Remaining leases cause transactional Busy refusal with the original control/pages
pair; no new shutdown method silently releases owners before a refusal.

Tests independently alter owner and pair identity, reject distinct catalogs with
identical keys, recover owners, render actual borrowed worker PCM, preserve cached
pages through repeated Busy refusals, and reclaim/join with exact buffer accounting
on a named control thread. The allocation probe covers successful construction
and borrowed use, not the existing whole native callback/shared-context handoff.

Allocation-capacity reporting is completed in the subsequent checkpoint below.
No provisional 192 MiB aggregate admission or eight-graph policy is implemented.
Native streaming refusal, resident/component limits and full acceptance remain.


## Actual allocation-capacity reporting — pulse24 / draft177

`VerifiedPageEndpoint::allocation_footprint()` borrows the pairing's complete
`StreamAllocationFootprint` without locks, allocation, ownership clones or
mutation. `StreamRuntime::allocation_footprint()` preserves the headless facade
and returns a control-side snapshot after catalog verification, using the existing
catalog mutex but creating no lease. Endpoint and catalog leases also expose their
respective component reports. Queries do not authorize playback/admission.

The report distinguishes three disjoint retained allocation classes:

- Endpoint-exclusive cache, pending-key, source-key and held-return vector
  capacities, including unused slots. Cache entries charge pointers, not another
  copy of their page payloads.
- Shared transport: four queue slot allocations and queue object data, catalog
  owner/stop-flag data, all PageBuffer object data and PCM box payloads. These are
  charged once per actual catalog/pool, regardless of queue halves, lease count
  or whether a page is free, ready, cached, returning or held.
- Worker/catalog: Source vector slots (including inline reader objects and their
  fixed verification scratch), separately owned definition/reader path capacities,
  reader digest manifests, one shared kernel object/table per reused rate pair,
  independent catalog-membership metadata slots/object, source scratch and worker
  free/held-page/held-failure vector capacities. Legacy CatalogFootprint's scratch,
  manifest and table fields are subcomponents, not additional additive charges.

Construction-only definition and kernel-cache vector capacities are reported
separately. The checked construction bound adds the definition slots and twice
kernel-cache capacity (old/new allocations can coexist during Vec growth), and
replaces retained kernel table bytes with the existing conservative table
reservation, and adds the largest observed pre-boxing page Vec allocation once
to bound possible shrink overlap. It deliberately bounds potentially overlapping lifetimes; it is not
an observed peak. Checked products/sums/subtraction return None on overflow or
an inconsistent kernel reservation; no saturation or wrapping is budget evidence.

The narrow media APIs expose only the reader's private path capacity and kernel
object data size. They reveal no private ownership or mutable state and allocate
or clone nothing. Per-component ceilings and their existing enforcement semantics
remain unchanged; the report does not silently make logical path limits into
capacity limits or impose aggregate/graph-domain policy.

Capacity bytes exclude allocator rounding/headers, opaque standard-library Arc
control blocks, catalog-channel/thread/OS bookkeeping, stack/thread-local storage,
caller-owned handles and prepared graph/history/export storage. Inline objects
inside vector slots are already counted. This is the complete owned data-capacity
report for the listed stream resources, not process RSS or an exact allocator bill.
A future project-wide ledger must enumerate all live catalog identities, count
shared resources once, and account separately for excluded resources before
choosing an aggregate admission cap. 192 MiB/eight graphs remain unapproved.

The real two-source, one-shared-kernel Linux fixture uses four pages, 8,192 source
scratch frames and intentionally over-reserved definition paths/vector slots.
It reports endpoint 712 bytes, transport 133,537 bytes and worker/catalog
733,172 bytes: 867,421 retained bytes, with a 14,000,445-byte conservative
construction bound. Definition path capacity is 2,142 bytes versus 94 logical
bytes; reader path capacity is 94, manifests 64, and one kernel table is 532,480
bytes. These platform/fixture-specific values are retained in test logs, not
portable size constants or a native100-track admission proof. Tests check actual
capacities, shared-kernel deduplication, inline scratch classification, unused
capacity, checked overflow and report stability through extra control leases
and real queue/cache page movement. The allocation probe measures report queries
and checked totals without allocation/free; no whole native callback clone claim.

Native consumption, combined pages/observer rendering, shared-context handoff,
installed-page prewarm/readiness, aggregate admission and full optimized native
playback/edit/seek/launch/fault acceptance remain outstanding. PR177 stays draft
and Pulse24 blocked under the readiness/evidence hold.

## exact-manifest stream endpoint readiness

PR177's opt-in `PageReadiness` unit reserves bounded active/candidate/transition
intent/protection storage and one sticky receipt slot per role before installing
pages. Borrowed control-owned manifests retain actual catalog/pair identity and
carry exact lifecycle/window tokens. Real decoded pages in the ready queue do
not authorize playback: every required half-open frame extent must first be
installed and role-protected transactionally. Missing/short pages, wrong actual
catalogs, stale windows, cancellation and receipt/capacity pressure cannot yield
a current receipt or clear other protection. Protected duplicate pages return
without replacing backing storage; legacy unpin leaves role protection intact.

The callback endpoint remains the sole request producer and ready consumer.
New operations use preallocated metadata and do not clone owners or introduce
callback allocation, final drop or join. Actual readiness Vec capacities are
reported separately and added once to reserved stream data with checked math;
caller manifest/graph storage and opaque overhead remain excluded. Control
recovery retains the existing Busy refusal and actual shutdown/pool reporting.
See [the stream contract](../crates/hex-stream/READINESS.md) for precise ordering,
backpressure, role cancellation, numeric limits and recovery guarantees.

This proves exact supplied manifests only. Engine-derived demand/handoff,
native intent delivery, BUFFERING without refused-chunk advancement, native
publication/consumption and callback-stop escrow remain unimplemented. Existing
native streaming refusal and resident limits are preserved. Full optimized
native 100-track mixed-rate/edit/seek/launch/fault acceptance remains pending;
PR177 remains draft and Pulse24 blocked under the unchanged hold.

## headless domain escrow and reported cleanup

The stream-owned domain escrow consumes actual control/readiness/retained-lease
inputs only after borrowed identity validation and all fallible reservations.
Its non-cloneable callback batch retains full-ring refused owners in their
original slots; normal SPSC returns cannot steal a separately reserved terminal
batch slot. Reaper validation rejects duplicate/wrong-owner payloads with owning
recovery. Observer abandonment cannot abandon the independently held control root.

Whole-domain cleanup checks known physical references before releasing any
lease, role protection or metadata. External graph/manifest owners leave it Busy
and unchanged for retry. Reported shutdown records actual known reclamation,
unknown worker inventory as None after panic, and observed pool imbalance.
Completion requires terminal return, a drained normal ring and actual successful
join/conservation reports. Capacity accounting charges each actual catalog once,
records original input/output construction overlap and excludes opaque/caller
costs; it is a reservation ledger, not current process memory or admission policy.

See [the headless escrow contract](../crates/hex-stream/DOMAIN_ESCROW.md). A mock
lifecycle harness proves ordering/retention only; actual CPAL closure disposal,
device-lease/reopen and durable quarantine wiring are not implemented. No audio,
engine, service or native streaming publication behavior changes in this unit.
The full native acceptance and readiness/evidence hold remain outstanding.
