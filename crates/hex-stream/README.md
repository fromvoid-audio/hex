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
Complete capacity reporting remains pending narrow media reporting APIs; current
CatalogFootprint is not a complete heap or aggregate admission report.
