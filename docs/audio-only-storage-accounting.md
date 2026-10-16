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

## concrete stream DATA composition

`AudioStreamStorageLedger` in `hex-stream` borrows the sealed engine ledger and
actual `StreamControl`/`StreamCallbackPages` pairs. There is no generic provider
or trusted-producer mint. Pairing validates actual owner and pair Arc identity.
Repeating the same actual domain charges it once; equal metadata/content from two
independent domains never collapses either allocation. Each explicitly selected
caller CatalogLease contributes its three physical handle fields independently.
The query does not create a lease or clone any owner.

Catalog coverage compares the complete source metadata, including its fingerprint.
It is a coverage fact, not source/catalog owner identity or lifetime custody.
The typed source-domain view retains an unresolved binding for every independent
streamed handle, even if DATA deduplicates or two catalogs contain equal metadata.
No actual source-to-catalog lease exists in the private root API yet.

The concrete factory accepts owned source definitions, not provider buffers or
shared kernels. It creates fresh page/endpoint/queue buffers and fresh per-domain
kernel owners; moved PathBuf/Vec fields remain exclusively owned. Catalog
metadata is a separate inline-value Vec; root streamed metadata is a separately
constructed Arc allocation. Full metadata equality establishes no shared backing.
Thus the whole factory domain reservation is disjoint from sealed root DATA,
selected caller banks and query metadata. No cached lease/component sum is
subtracted to make the combination appear disjoint.

The constructor-collected manifest is disjoint:

| owner | listed DATA |
| --- | --- |
| page endpoint | actual cache, pending-key, source-key and held-return Vec capacities |
| shared page pool | each actual PageBuffer object and final PCM Box slice once, regardless of queue/cache/worker position |
| four rtrb queues | actual public RingBuffer object payloads and separately declared slot backings under the pinned constructor gate |
| fixed shared owners | actual CatalogOwner, stop-flag, pair and verified-catalog object payloads; opaque Arc control blocks stay excluded |
| worker sources | actual Source Vec capacity, including inline reader objects/fixed64KiB scratch; separate actual definition/reader PathBuf capacities and digest Vec capacities |
| kernel owners | actual kernel object payload and weight Vec capacity once per actual shared allocation; one independent kernel handle per source plan |
| worker buffers | actual source scratch, free-page handle, held-page handle and held-failure Vec capacities |
| verified catalog | its independent metadata Vec capacity and actual fixed object payload |
| query metadata | actual domain/lease vector capacities and inline ledger header once |

The worker emits its fixed receipt only after complete verified construction.
Source plans validate actual kernel allocation identity against the captured
cache while those owners coexist. Only scalar totals leave the worker for this report; no worker address can later deduplicate against a new allocation. These
are conservative fixed-domain constructor DATA reservations and unresolved
constructor handle observations, not live worker-field or lifetime receipts.
An unsampled, failed, stopping, finished or otherwise incomplete worker remains
Unknown when observed. Completion/panic racing the query may release storage and
overcount the reservation; it cannot grow this fixed domain or close an obligation.
Borrowing both halves prevents ordinary shutdown/last-owner stop during reporting.
Catalog and join-handle guards remain local to each query; results retain no guard.
The existing loop consumes no further request while a held result is
pending; its held vectors remain within their preallocated capacities. The
complete fixed pool bounds the free/held page-handle vectors. No handoff,
shutdown, thread or callback protocol changed to obtain these observations.

Construction extras contain actual original definition-vector slots, kernel-cache
slots and observed old-cache growth backings, and the largest observed pre-boxing
page Vec capacity once. Definition paths transfer into sources and are not added
again. Kernel tables reserve once and retain that same Vec; no duplicate payload
conversion is invented. The conservative bound adds these disjoint extra
backings to the entire retained DATA domain. It subtracts no cached component or
lease report and does not claim simultaneous process peak.

Queue slot bytes are a **declaration**, not a falsely observed private Vec capacity.
The supported-construction declaration checks rtrb0.3.4 Cargo.lock checksum,
observed registry lib.rs, cache_padded.rs and chunks.rs SHA256, and the supported
compiler/std family. It applies only to the repository's fixed Cargo invocation
and exact required local CI, with the pinned compiler and Cargo.lock. Ambiguous
registry observations, observed configuration/source replacement and unsupported
compiler evidence keep DATA Unknown. A build script cannot establish resolved
source identity for every Cargo invocation: arbitrary custom `--config` or source
replacement is outside this supported class, even if not visible to build.rs.
This is not cryptographic/runtime source attestation or a trusted-producer mint.
The local runner and bundler's fixed argv supply the supported build provenance.

The actual public RingBuffer type supplies payload size/alignment; no mirror
struct is used. The 24 allocation-counter controls exercise all four real message
types at capacities 1,3,6,1024,2048 and8192: exactly two requested allocations,
exact requested bytes, and balanced signed-net destruction. Build-only sha2
checks observed files; architecture policy forbids it as a stream runtime edge.

## explicit outstanding obligations

The exact inherited DATA boundary is original PR177 source
`8d444c06c295e52a8aa0dde275a89ecec6f454e2`, `docs/page-delivery.md` (actual
allocation-capacity reporting), `crates/hex-stream/src/footprint.rs` and README.
Its exclusions remain: allocator rounding/headers, opaque standard-library Arc
control blocks, catalog-channel/thread/OS bookkeeping, stack/thread-local storage,
caller-owned handles and prepared graph/history/export storage. The engine ledger
accounts the explicitly selected graph roots separately; the stream façade does
not silently add those roots twice.

The aggregate explicitly enumerates six opaque-cost obligations per actual domain:
allocator metadata, shared control blocks, catalog channel, worker thread,
operating-system state, and stack/thread-local storage. A missing opaque obligation
invalidates the report. These are **unresolved costs**, not zero-byte allocations.
Typed individual constructor handle observations remain unresolved, as do opaque
channel/thread internals, unregistered caller aliases, every source-domain binding,
root retirement and domain retirement. Unobserved constructor handles add an
explicit Unknown group; reference counts never close or deduplicate handles.

Only `require_listed_data` can succeed for the named DATA-capacity subreport.
`require_complete` remains refused. No whole-process/domain/aggregate storage,
allocator/RSS, lifetime closure, publication or budget-admission claim follows.
Unknown producer/layout/worker state cannot produce a listed bound. Arithmetic
and caller reservation pressure refuse without consuming owners; failed additions
poison their ledger while metadata capacities remain inspectable.

Existing integrated-main caps remain unchanged. Parent-only runtime32MiB/domainN2
terminology does not introduce a new policy here. Aggregate-cost resolution stays
with downstream Pulse2541 / PR236 before publication/admission. Original Pulse24 /
PR177 native, performance and full review acceptance remains open. Exact-source
full local CI and negative-control evidence are recorded with PR233; targeted
checkpoint tests alone are not merge or native acceptance evidence.
