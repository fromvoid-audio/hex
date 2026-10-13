# exact installed-page readiness

This opt-in stream-only unit proves installed/protected pages for an exact
caller-supplied manifest. It does not prove engine demand, native publication,
deadline safety or the full long-project streaming acceptance.

## ownership and identity

Control constructs `WantedPageManifest` from an actual `CatalogLease`, exact
`ReadinessStamp`, and strictly sorted unique `PageExtent` entries. Each extent
is a nonempty half-open range inside one project-rate page and the verified
source's playback bounds. Unknown generations, rate mismatch, duplicate or
unordered pages, invalid extents and checked arithmetic overflow are refused
with the original lease and Vec allocation/capacity intact. Control normalizes
required ranges before construction; validation does not truncate coverage.

The stamp carries request epoch, artifact, project-open generation, editor
revision, device epoch/rate, timeline epoch, performance/Arrangement window,
active revision, route generation and admitted command prefix. Except for numeric
validation, these are opaque caller tokens compared exactly; this unit cannot
verify a token's relationship to an actual prepared graph or current engine
clock. That engine/audio boundary remains unimplemented.

`PageReadiness` consumes a `VerifiedPageEndpoint` on control. It has no Clone
implementation. Submit borrows a control-owned manifest, validates the actual
catalog/pair identity, and copies only finite extents/tokens into preallocated
storage. No catalog/graph owner is cloned, transferred or finally dropped by
submit, service, acknowledge, cancellation or page borrowing. A receipt borrows
the actual catalog; it cannot outlive the owner or coexist with mutable endpoint
use. Its fields are immutable and it is observational, not a playback capability.

## finite service and protection

There are three protection roles: active, candidate and transition. Each role
has one pending intent and one unread receipt slot, not a general graph-domain
limit. Submission reserves that receipt slot before installation. Exact pending
resubmission coalesces; different pending intents, unread receipts and capacity
pressure refuse without changing protection or the epoch high-water mark.
`ReadinessEpochs` never wraps; an exhausted endpoint refuses new intents. The
high-water mark lives in the original endpoint and survives control recovery
and reconstruction of its readiness owner; cancellation cannot reset it.

The caller selects the finite per-role extent limit, no larger than the existing
endpoint cache ceiling (`page_buffers - 1`). Admission checks the exact distinct
page union of old protected sets, pending intents and the new manifest, including
old/new overlap. Shared keys are counted once. It does not evict old protection
to fit a replacement or change any existing component ceiling. Full-cache
replacement can therefore require explicit old-role release or refusal. No
global aggregate memory/graph policy is introduced.

Callback service remains the sole page-request producer and ready-page consumer.
It bounds ready pumping by the existing configuration and request attempts by
the smaller of the supplied limit and finite wanted extent count. Typed request
results distinguish ring acceptance, already-pending coalescing and refusal;
refused work remains wanted and can retry. Submission's union check uses sorted
set binary searches; all scans are bounded by the reserved extent/cache limits.
There are no callback waits, locks, allocations, owner clones or joins in these
new operations. Real worker IO/decoding and explicit control reclamation remain
outside callback service and its allocation probe.

Decoded buffers awaiting the ready consumer are not installation receipts.
`service` first compares the complete current stamp, then pumps ready pages.
Installation checks every required frame extent before any protection changes.
Only after complete validation does it replace that role's protection and expose
`InstalledProtected`. Missing/short pages leave old protection intact and no
receipt. This unit does not render or advance any playback clock/DSP state.

Role protections are independent of legacy pins. `unpin_all` clears only legacy
pins. Releasing/cancelling a role never clears another role. Duplicate arrivals
for a role-protected key return their buffer through the existing bounded return
path and cannot replace the protected backing storage. Safe borrowed-page
lifetimes also prevent endpoint mutation while a page is borrowed.

An unread receipt blocks a new intent for that role. Wrong-catalog/stale
acknowledgments leave it intact. Cancellation names an exact role/request epoch;
a stale cancel does nothing. Cancelling a pending replacement retains that role's
older installed protection; cancelling the older installed epoch releases only
that protection while a newer pending intent retains its reserved receipt.
Queued decoded success arriving after cancellation cannot recreate an intent
or authorize installation. Explicit release uses the same exact-epoch cancel API.

## accounting and recovery

`ReadinessAllocationFootprint` reports the actual returned capacities of all six
wanted/protected Vec allocations, including unused elements. Their bytes are
checked and endpoint-exclusive. `retained_data_bytes` adds them once to the
existing stream reserved-data report. Role bits are already included in actual
cache-slot storage. Caller manifests/graphs and placement of the inline
`PageReadiness` value require separate caller reservations; shared catalog,
queues and page buffers are not charged again. Opaque allocator/Arc/thread/OS
overhead is excluded. This is not process memory/RSS or a sampled peak.

Control-only `into_endpoint` removes role protection, releases metadata storage
and returns the original endpoint. Extra manifest/graph leases still make the
existing shutdown transactionally Busy. Release them on control, then retry
shutdown and use the actual join/pool report. Setup reservation refusal returns
the original endpoint with fixed partial capacity observations; process-fatal
OOM/abort (including error-object allocation) is outside recoverable guarantees.

## remaining boundaries

Tests use exact explicit manifests, real workers, cache failure fixtures and
headless allocation probes. They do not establish engine-derived demand or
concurrent native control delivery. Demand/handoff preview, command-window
barriers, opt-in BUFFERING (silence without advancing the refused chunk), native
publication/provider consumption, callback-stop escrow and lifecycle retirement
are not wired here. Legacy resident and borrowed missing-page behavior remains.
Native streaming publication still refuses and resident import limits remain.

The full issue still requires optimized exact-source native advancing 100-track
mixed-rate playback and edit/seek/Session-launch/fault/listening/reference and
hardware deadline acceptance. Linux checks do not substitute. PR177 remains
draft/BLOCKED under the evidence hold.
