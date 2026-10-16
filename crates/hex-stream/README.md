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

The owning runtime unit fixture verifies shutdown with an undrained one-page
ready queue and a second page actually held by the worker. Test-only channel
barriers delay the real worker after catalog preparation; publication events,
not a fixed sleep, establish the required queue state before shutdown. A delayed
worker negative control reproduces the old 30 ms assumption yielding zero queued
pages while still joining and reclaiming the complete pool. The hooks are absent
from production builds; shutdown, queue bounds and callback behavior are unchanged.

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

## Borrowed DATA inventory

The control-only `AudioStreamStorageLedger` composes the engine's sealed borrowed
root ledger with actual control/pages pairs and explicitly borrowed caller leases.
It captures actual constructor capacities, counts an entire actual domain once,
and preserves separate owning-handle, source-binding and retirement obligations.
Only the named listed DATA contract can be complete; excluded opaque costs remain
explicit obligations and `require_complete` refuses. No generic page provider can
mint this report, and no publication/lifecycle capability follows. Queries use the
existing control mutexes; never call them from a callback.

Queue slot DATA declarations cover only the fixed repository Cargo invocation
with pinned compiler/Cargo.lock, observed source-file checks and exact required
CI/allocation probes. Build-only sha2 checks those observed files; build.rs cannot
attest resolved sources for arbitrary custom Cargo `--config`/source replacement.
Unsupported observations stay Unknown. Ordinary runtime behavior and all
component limits remain unchanged. See the exhaustive
[DATA/obligation manifest](../../docs/audio-only-storage-accounting.md).

## borrowed persistent pair disposal

`PreparedStreamDisposal` is the control-side alternative used by native candidate
custody. Preparation reserves collection storage before the exact original pair
moves. Busy external leases leave the pair untouched; join occurs outside the
worker mutex, retaining the first worker panic payload and endpoints on failure.
Normal completion requires conservation of every fixed pool page before
destruction. Failed disposal stays incomplete and is never retried. This does
not change the legacy consuming shutdown API or certify native quiescence,
complete cost or admission. See [never-installed disposal](../../docs/never-installed-audio-disposal.md).


## closed constructor domain custody

`ConstructorStreamDomain` consumes the real control/pages pair only after
constructor DATA and excess caller-handle checks. It retains its actual source
lease and disposal capsule without exposing a lease or render endpoint. Listed
DATA and constructor-bounded opaque resource lifetimes are separate: this never
assigns zero bytes to std/channel/thread/OS internals or upgrades the public
borrowed ledger's completeness. Normal worker join, pool conservation and final
endpoint/channel/metadata destruction precede its report. The native host keeps
charges until report consumption and must enforce its global two-domain limit.
Native readiness is connected only through the opted-in audio constructor path. See
[constructor publication](../../docs/audio-candidate-publication.md).

## sealed readiness and endpoint loan (PR237 checkpoint)

A constructor domain may additionally reserve a finite extent capacity for each
of Active, Candidate and Transition. A checked requested-layout lower bound refuses insufficient caller DATA
allowances before allocating disposal/readiness backing. Six actual Vec capacities
and the internal readiness catalog lease's three handles are then charged before
consuming the original pair. Its move-only `ConstructorPageLoan` carries the one actual request producer
and callback endpoint; it cannot clone another producer or export raw endpoint
ownership. The original control/catalog/disposal owner remains on control, and
normal disposal cannot stop or join until the exact loan returns. Returning a
wrong pair preserves both owners. Inline capsule storage belongs to its host;
its backing reservation remains charged through the report barrier.

Decoded buffers are not readiness. Bounded service installs complete requested
extents, validates actual valid-frame lengths, atomically replaces only its own
protection role, and publishes one reserved unread receipt per role. A receipt
must be acknowledged against its exact stamp/extents before borrowing protected
samples or replacing it. The borrowed view exposes only listed protected page
keys and cannot schedule missing pages; it is still not an engine-demand
certificate. Other roles survive overlapping replacement/cancellation;
a protected PCM page cannot be replaced by a duplicate shorter decode. The union
of protected/outstanding pages must fit the actual existing cache, with no pool
cap increase. Stamps are correlation data, not proof of engine demand or native
publication. Audio must bind these operations to its real constructor and engine
window before enabling streamed installation; PR237 connects that opted-in path.
An exclusive borrowed promotion token validates all involved roles before commit,
then moves the actual protected Vec backing to Active without allocation or a
fallible second-domain commit. Bounded single failure consumption releases the
actual failed pending key for retry; the native owner retains its diagnostic.
Legacy borrowed pages retain their established miss behavior.
