# hex-stream

## Purpose

Own verified source catalogs, bounded worker decoding/conversion, fixed-size
page buffers, page request scheduling, cache lookup, and ownership return paths
for streamed Arrangement media.

## Dependencies

Uses `hex-engine` for generation-scoped source keys and borrowed page contracts,
`hex-media` for verified WAV reads and shared sinc kernels, `hex-project` for
canonical asset IDs, and `rtrb` for bounded single-producer/single-consumer queues.

The initial configuration reserves one of its 1,024 page buffers as bounded
staging capacity; the endpoint admits at most 1,023 cached pages so a full cache
cannot starve the worker of a buffer needed to satisfy the next request.

The default catalog admits up to 512 sources, 16 MiB of path spellings, 64 MiB
of page manifests, 32 MiB of conservative resampling-table preparation budget,
and 32 MiB of verified-reader scratch. Its shared source window is capped at
131,072 stereo frames. Request, ready, return and failure rings are bounded
independently; callers can select smaller configurations for tests and jobs.
These are per-runtime limits, not accounting across project history or multiple
live runtimes.

`StreamRuntime` remains the headless compatibility facade. Its consuming
`split()` moves ownership into a `StreamControl` and a `StreamCallbackPages`
endpoint. Both halves retain the same private `CatalogOwner`, which owns the real
worker join handle, stop flag, wake handle and catalog result. A ready control can
create an opaque cloneable `CatalogLease`; it retains the actual owner and the
verified, sorted full metadata membership, including fingerprints. `covers_source`
compares the complete metadata value, and `same_catalog` distinguishes catalogs
even when their generation and keys match. Keep one lease with each prepared graph
that refers to the catalog.

`VerifiedStreamRevision` consumes an `Arc<PreparedRevision>` and a real ready
`CatalogLease`, checking every Arrangement and Session stream binding against
complete verified metadata membership. Validation borrows without allocation or
owner cloning. Failure returns both inputs in `StreamRevisionMismatch`; successful
bindings expose borrowed graph/catalog access and actual catalog identity.
The wrapper has no Clone implementation. Construction, transfer and final
reclamation belong on control; final lease release may stop/wake/join the worker.
This first Pulse24 stage adds no callback endpoint or page-readiness proof and
does not enable native streamed publication or lift resident import limits.

Matching shutdown counts the actual owner references held by the control and page
endpoint. If graph leases remain, it returns recoverable `ShutdownError::Busy`
without changing the pair; `into_parts()` restores both halves for a retry after
the leases are released. A mismatched pair is likewise returned without mutation.
Dropping the control alone does not stop the worker while an endpoint or catalog
lease still owns it. The final catalog owner stops, wakes and joins the worker;
explicit shutdown and final owner/endpoint/lease reclamation belong on a control
thread, never in callback rendering. Pages may be borrowed by rendering, but
dropping their cache is not real-time safe. Failure events stay on the pages
endpoint because consuming one also clears the matching pending request.

`cargo test -p hex-stream --features allocation-probe --test callback_allocations`
measures the split render-facing operations with the pinned dev-only allocation
counter. Worker setup, page preparation, assertions, and shutdown stay outside the
measured closure. This is a headless endpoint probe, not native device acceptance.

`cargo run --release -p hex-stream --example profile_100_long_sources` creates
100 distinct sparse WAV files of 17,000,000 stereo frames each, catalogs their
full logical contents, warms one far-offset page per asset, and renders 100
active tracks through the engine. The run separates source fingerprinting,
catalog verification, page preparation, and warmed rendering. This is a headless
workload profile; it does not establish a device callback deadline or native
project publication support.

## Does not own

Project editing, persistence, UI, CPAL, device callbacks, WAV parsing or hashing.
It does not replace the resident media bank or make streamed playback available
to the native application until the `hex-audio` and project/session publication
paths explicitly own and retain its endpoint/catalog.

`VerifiedPageEndpoint` is a control-constructed, non-cloneable catalog/page
pairing. It checks actual owner and pair identity, exposes borrowed existing
pages, and returns owners through `into_parts` for the existing transactional
shutdown path. It does not establish installed-page readiness or native admission.
Its StreamAllocationFootprint reports actual endpoint-exclusive and shared
transport/catalog reserved capacities with checked retained/construction totals.
The legacy CatalogFootprint remains a payload/subcomponent view, not an additive
heap total. Capacity reporting excludes opaque allocator/Arc/OS/thread machinery
and caller graph storage; it does not establish aggregate admission policy.

`CompactSourceInput` is an opt-in control owner for compact input copying only.
Construction moves the original Vec without allocating; borrowed preflight
checks existing configuration, source/scratch/logical-path limits and complete
key consistency/uniqueness without sorting originals. Fallible exact Vec and
OsString reservations copy paths in original order, preserving raw non-UTF8
representation and metadata. Actual returned slot/path capacities are checked
against explicit caller `CompactCopyReservations`; no default global budget is
introduced. Original capacity, compact capacity and disjoint caller-declared
retained/construction data reservations are charged once in the checked overlap
report. Allocator rounding/headers, stack and opaque process costs are excluded;
this is reserved data capacity, not a process memory cap or observed RSS peak.
An allocation can return excess capacity before it is checked and refused.

Every API-reported validation/reservation/copy refusal leaves original owners
unchanged and recoverable through `into_originals`; partial copies are reclaimed
on control. Recovery after success reclaims the compact copy and returns the
exact original Vec/paths. Process-fatal OOM/abort is outside this guarantee.
Only completed compact-copy reporting is exposed: no worker verification,
poll/readiness states, catalog or runtime escape, or native publication support.
The legacy consuming constructors and their limits are unchanged. Worker
integration, failed-construction capacity observation and an owned reader-path
boundary remain separate work; this copy report does not charge those costs.
These APIs and all final drops belong on control, never in a callback.

`StreamConstructionRequest` is the opt-in control-owned integration of compact
input copying with actual worker catalog verification. Its constructor only
moves existing owners. Callers explicitly reserve a maximum for its reported
construction data scope; no application/global budget is supplied. Originals
remain unchanged on control through Unstarted, Pending, CatalogVerified,
CancellingCleaning and Failed, until successful finish or terminal recovery
transfers them once and marks the request Consumed.

`start` constructs the private endpoint, pool and transport before spawning.
An ownership cell retains compact definitions, pool and worker queues until the
successfully spawned worker takes them once. Setup/spawn refusal reclaims them
on control with actual created-page counts. The worker fallibly copies a distinct
reader path and uses owned media opening; compact definition paths move into
Sources once. `poll`/`poll_timeout` observe actual worker results; a timeout leaves
Pending. Cancellation wins a queued success and prevents finish. No runtime,
page endpoint or catalog lease can escape before `try_finish` receives complete
catalog verification and checks actual component capacities/conservative overlap
against the caller reservation. CatalogVerified is not installed-page readiness.

`try_recover` retains request/original owners while the worker is active. After
actual termination it returns originals with a failure and explicit cleanup:
NotStarted counts, normal full PagePoolReport, incomplete pool accounting, or
JoinFailed with only known queued/held counts. Panic cannot manufacture a worker
inventory or complete pool conservation. Cached-page transfers are not additive
reclamation counts. All methods, recovery and final drops belong on control;
Drop is best effort and can block joining filesystem work. Cooperative cancel
cannot promise an I/O deadline. Fatal OOM/abort is outside recovery guarantees.

Fixed-size ConstructionCapacityReport observations survive API-reported worker
failure, including actual incoming/source/cache/membership slots, path/manifest
capacities, scratch and held buffers. Unique kernel data/weights are charged once,
including reported kernel-construction failures. Incoming definition slots are
construction overlap; transferred compact paths are not charged again. Caller
retained/construction declarations must be disjoint. Construction-only startup/
telemetry objects are reclaimed on control; they do not become playback owners.
The report bounds listed reserved data capacities and conservative construction
overlaps, not simultaneous process peak/RSS. Opaque allocator/Arc/mpsc/thread/OS
costs, stack and caller handles remain excluded. Finalized observations mean a
worker API outcome, not cleanup success; panic observations stay partial and
cannot authorize a finalized bound. Actual excess capacity may exist before
being observed/refused. Legacy consuming APIs and all component ceilings remain.

This is headless catalog construction only. It adds no callback ownership work,
engine/project/storage edits, protected-page prewarm or native streamed
publication. Native endpoint retirement/readiness/admission and sustained
optimized 100-track mixed-rate playback/edit/seek/launch/fault acceptance remain
required before resident limits can be lifted. Full Pulse24 acceptance and HOLD
remain in effect; this source stage is not issue completion.
