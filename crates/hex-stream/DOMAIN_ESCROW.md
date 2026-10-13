# headless endpoint-domain escrow and media cleanup

This unit owns actual stream control/readiness/lease inputs, finite endpoint
returns and truthful cleanup observations. It does not own audio publications,
CPAL stop/disposal, device leases or application recovery policy.

## setup and identity

`MediaDomainInput` moves a checked positive `DomainId`, actual `StreamControl`,
`PageReadiness` and original retained-lease Vec. `DomainIds` refuses exhaustion;
IDs are not catalog identities. `domain_escrow` first borrows all inputs to check
actual owner/pair, every retained lease, duplicate IDs and duplicate actual
catalogs. One physical endpoint/SPSC pair belongs to each catalog owner; multiple
leases charge that catalog once. Separate catalogs with identical source keys
remain independent owners and are charged independently.

Control reserves all control/callback slot vectors before moving any input.
API-reported reservation/validation failure returns the original input Vec,
capacity and all original owners/lease Vecs. Fixed partial capacity observations
are retained. Unobserved domain-data totals are None. RingBuffer, Box and Arc
allocation has no recoverable API; process-fatal OOM/abort, including owning-error
storage, is explicitly excluded. No callback setup allocation is permitted.

The outputs are a non-cloneable boxed `CallbackDomainBatch`, an independent
`TerminalBatchSender`, and the control-owned `MediaDomainReaper`. The caller
chooses normal-return capacity from 1 through the physically registered domain
count (1 for zero domains). This is a transport bound, not a global graph policy.
Existing stream/cache/catalog component ceilings remain unchanged.

## return paths

The callback batch owns preallocated optional endpoint slots and the sole normal
SPSC return producer. `readiness_mut` borrows its existing endpoint. Successful
`try_return_domain` moves that endpoint out exactly once. A full ring restores
the same owner to the same slot and reports Pending; observed consumer
disconnection refuses before consuming the endpoint. Subsequent domain return
cannot duplicate an owner. Return methods allocate/free/clone/join nothing.

Normal return does not consume the separately reserved terminal slot. Control
or a lifecycle harness explicitly calls `try_return_stopped_batch` after all use
and borrows of the batch have ended. Actual seal identity prevents cross-escrow
transfer; refusal returns the same Box. Successful transfer is once-only.
The terminal producer is not stored in its own queued batch, avoiding an Arc/ring
ownership cycle. The reaper retains the terminal batch's storage until control
reclamation, and can still receive earlier normal returns.

`poll_one` accepts at most one payload. It checks index/ID, actual pairing and
duplicate state before consuming an endpoint. Terminal validation checks the
whole batch before moving any slot. Invalid/duplicate payloads are owning
control-side errors and cannot overwrite existing owners. Exceptional returned
error objects belong to the caller's error-storage reservation, not the normal
callback allocation ledger.

Keep the actual control root alive independently from observation waiters until
terminal return and cleanup. `MediaCleanupObserver` owns only a phase byte; its
abandonment cannot cancel cleanup. All root/batch/error final disposal belongs
on control. The disconnected-queue tests deliberately dispose roots on control;
they do not authorize abandoning a live native retirement root. Reaper methods
do not spawn a hidden worker or promise autonomous completion without a driver.

## whole-domain cleanup

`try_reap_domain` validates owner/pair and retained leases before any mutation.
Known references are exactly control + pages + the endpoint's internal catalog
lease + the physically supplied retained leases. No public Weak or clonable
control/pages are exposed. External graph/manifest references therefore produce
Busy before unpinning, clearing metadata, releasing leases or requesting stop.
The complete domain remains available for retry, with original protection and
reserved capacities. No graph retirement is inferred from an ID or counter.

At exactly known references, cleanup releases known leases on control, recovers
the endpoint and invokes `shutdown_reported`. An unexpected post-commit refusal
is retained by the root, marks failure and never claims rollback/completion.
The reported API preserves owning wrong-pair/Busy errors; terminal results keep
known cached-return transfers, ready/held reclamation and actual returned worker
inventory. Cache transfers are not added again to reclaimed counts. Worker panic
or a missing handle leaves unknown inventory/conservation as None. Imbalance
retains the actual observed counts and conservation=false. Legacy shutdown keeps
its signature and WorkerJoinFailed/PoolAccounting error mapping.

Successful cleanup requires an actual joined worker and exact pool conservation.
Root Complete additionally requires terminal return, an empty normal-return ring
and successful reports for every registered domain. Partial/incomplete reports
produce FailedIncomplete; timeout or interest abandonment is not completion.
Join runs only on control and can wait for IO cancellation without a deadline.

## allocation and acceptance boundaries

`DomainEscrowFootprint` is a setup-time reservation ledger, not current live
process memory. It records actual returned slot/vector/ring capacities, queue
objects, the boxed batch object, one observer byte and each initial domain's
reserved stream/readiness/retained-lease data once. Outcome metadata is included
in control-slot size. Construction adds original input-Vec capacity overlap with
new output storage. Cleanup can release domain data; the reservation ledger
does not assert it remains allocated. Independently overlapping source preparation
and caller graph/manifest/error storage require caller reservations. Allocator/
Arc/thread/OS overhead and opaque graph costs are excluded. This is not RSS,
a sampled peak or a configured aggregate admission policy.

Headless tests use actual workers/pages and inject setup reserves, wrong owners,
full/disconnected returns, duplicates, external leases, worker panic and pool
imbalance. Callback moves are allocation/free probed. A lifecycle fixture checks
failed stop retaining root/lease, disposal before terminal transfer, Busy retention
and actual cleanup before a mock reopen permission. It makes no CoreAudio claim.

Native integration still needs a confirmed-stop closure carrier, durable lifecycle
and quarantine root retention, device-lease/reopen policy, and separate spectrum
and media cleanup. This unit does not change stream.pause, audio/service/engine
fields, native publication or import limits. Engine-demand/handoff preview,
BUFFERING consumption and optimized native advancing100 mixed-rate/edit/seek/
launch/fault/listening/reference/deadline acceptance remain pending. HOLD intact.
