# borrowed audio-only DATA and unresolved obligations

Pulse2538 / canonical PR233 follows the private root constructors in PR232. It
adds no native publication, retirement receipt, lifetime closure, runtime cap,
source/catalog lease, callback work or owner transfer.

## engine checkpoint

`AudioRootStorageLedger` borrows actual persistent `AudioPreparationCustody`
values and optional caller `OwnedAudioInputs`. Its DATA rows are sealed. Shared
DATA deduplicates only by actual borrowed Arc owner, charge location and equal
constructor declaration. Exclusive containers are anchored by their actual
immutably borrowed owning field, with their actual capacity and element stride.
Equal PCM or metadata values never establish identity. Each physical owning
handle retains its own anchored field and existing handle ID; reporting clones
nothing. Refused additions invalidate the partial ledger. Roots and original
inputs remain in their existing custody and follow existing normal cleanup.

The named root DATA subreport is distinct from unresolved handles and retirement.
`require_known_storage` only checks this selected DATA subreport.
`require_complete` always refuses with `RetirementReceiptUnknown` after any
DATA uncertainty is ruled out. This type provides no publication capability.

The ownership manifest is exhaustive at each owning module:

- Input banks: source and alias vector capacities, independent original/alias
  handles, and constructor-paired resident Arc payload/header plus separate PCM
  Vec capacity, or independent streamed metadata Arc allocation.
- Arrangement source staging, clip rows/boxes and private Track targets: actual
  source fields, paired declaration/handle destinations, clip/send/gain buffers,
  zero-capacity touch storage, fixed meter and empty-chain allocations. Final
  Track fields are checked against the same closed audio-only class.
- Return rows and actual buses: empty-chain owners, Return-feed plan and staging
  row/order/outgoing/incoming/visited buffers, including each nested feed Vec.
- Session source/cache/name rows and structure: all scene/track/cell/index/range
  capacities and String backings, actual source handles at their present field.
- Tempo and meter: both actual point/prefix allocations, persistent copy buffers,
  canonical intermediary and final prepared/final Session owning locations.
- Session assembly: all destination and paired-source buffers, pending names,
  unused MIDI/mutable vectors checked empty, and actual final inventory vectors
  and names. Original source associations are revalidated against the bank.
- Fixed shared sine/context allocations: one DATA charge per actual owner and
  every independent owning handle. Nested analytic-context owners/spares refuse.
- Preview and revision: the actual empty preview allocation, four root shared
  allocations and four handles, validated empty mutable descriptors, and actual
  children at their staged or final location.
- Custody and ledger metadata: inline fields once, actual vector capacities and
  disjoint recorded old-growth construction overlap. Track growth already
  includes gain-copy growth; it is not added a second time. These are conservative
  capacity bounds, not a sampled simultaneous process peak.

Opaque panic payloads, pending source destruction without a complete binding and
caller scalar byte claims without disjoint-owner evidence remain Unknown. Known
rows are not a total for those states. Duplicate selected custodies, inconsistent
charges, raw constructors, unexpected Weak observers and invalid associations
refuse. Stack placement, allocator rounding and RSS are not established by an
inline DATA charge. No public mutable graph/Arc/Weak owner escapes.

## remaining stream composition

The next checkpoint composes this sealed engine inventory in `hex-stream`, which
already owns actual control/pages/catalog identity. Catalog membership validates
full metadata including fingerprints; equal metadata is not a lease or owner
identity. The stream producer must capture actual listed DATA capacities and
retain opaque-cost, handle, source-binding and retirement obligations separately.
A missing/unsupported constructor or unsampled component remains Unknown.

The exact parent DATA boundary is original PR177 source
`8d444c06c295e52a8aa0dde275a89ecec6f454e2`, `docs/page-delivery.md` (actual
allocation-capacity reporting), `crates/hex-stream/src/footprint.rs` and its README.
It excludes allocator rounding/headers, opaque std Arc control blocks,
catalog-channel/thread/OS bookkeeping, stack/thread-local storage and caller
handles from the listed stream DATA report. These exclusions must be explicit
unresolved obligations in the aggregate; they are not free storage. No complete
whole-process/domain/aggregate storage or admission claim follows.

Existing integrated-main caps remain unchanged. Parent-only runtime32MiB/domainN2
terminology does not introduce a new policy here. Aggregate-cost resolution stays
with downstream Pulse2541 / PR236 before publication/admission. Original Pulse24 /
PR177 native, performance and full review acceptance remains open.
