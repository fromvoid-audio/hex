# control-only playback storage report

`Playback::visit_storage_on_control` is an opt-in borrowed inspection skeleton.
Call it only while exclusively owning playback on control, before publication.
It is not called by production callbacks and does not inspect active mutable
runtime from another thread. It never allocates/clones owners; callers provide
bounded charged control scratch and may refuse a visit when it fills.

Items separate measured reserved container/pointee data, existing provider
fixed-storage declarations, shared actual owners, declared owner overhead and
Unknown. A measured pointee is not a claim about its internal heap. OS and
allocator bookkeeping remain outside this data-accounting contract. No RSS or
exact allocator total is claimed. A summary can be complete only within this
stated contract; any Unknown makes `require_complete` refuse independently of
what the sink retains. General Arc overhead remains Unknown; only the narrowly gated PCM producer below may emit a declaration.

The borrowed Playback header is inline in its caller's envelope, not necessarily
boxed. Callers charge their actual envelope/Box. Session and renderer runtime
Boxes report their pointee data once; child Vecs report only backing capacities.
Inline tempo-period/onset/span/tail arrays need no separate heap charge.
Current traversal reports Arrangement root/track/clip containers, resident PCM,
clip metadata Boxes, streamed metadata, processor slices/meters, prepared roots,
Session runtime Vec capacities/retained contexts and renderer Vec capacities.
Gain/instrument/provider/state dependencies, unapproved Arc overhead and
lattice-internal owners explicitly remain Unknown. Ordinary owned buffers
expanded in the sections below do not complete those enclosing owners. Existing DSP-twin declarations are separate from these incomplete
nested reports. No provider is assumed complete by default, and no provider
trait or lattice admission contract changes in this unit.

Shared refs compare actual Arc allocations, not content, asset keys or addresses
saved beyond a lifetime. They borrow roots and cannot outlive them. Repeated
visits are not an aggregate sum: sinks must independently deduplicate shared
allocations and their nested allocations. Exclusive items belong to the caller's
current Playback root. No persistent receipt/registry is established here.
Before publication integration, control must anchor immutable roots and retain
conservative old/new owner unions through tails and actual retirement reports.
It must not regenerate a report by traversing callback-mutated storage.

## arc inventory and unresolved declaration

Visited categories are Arrangement, PreparedRevision, Session inventory, MIDI
preview, resident PCM slices, streamed metadata, processor-instance slices,
device meters, shared context tables and retained SharedContextOwner. Payload
size/alignment can be inspected safely; Arc allocation-header size is not a
public std contract. Nested Arc categories still covered by Unknown include
state payloads, note banks/templates, sine tables, provider definitions and
context objects. Typing Weak owner bookkeeping is explicitly Unknown too.

No guessed header constant is introduced. A possible future declaration needs
the exact pinned Rust alloc source on every supported target: audit ArcInner's
reference counters, representation, DST alignment/padding, allocation layout,
and Weak-only lifetime. Derive an alignment-aware conservative bound from that
implementation and validate supported target/stdlib provenance; treat it as an
assumption-bound declaration, not measured allocator bytes. The installed cloud
toolchain lacks rust-src, and no native target layout evidence is supplied by
this unit. The narrowly approved version-bound PCM producer below does not generalize to
other Arc families; generic Arc costs remain Unknown and complete admission refuses. Opaque providers likewise require
exhaustive audited fixed-storage and nested-owner reporting; size_of_val cannot
stand in for it.

## preserved limits and next seam

No limits change. The existing 32MiB preroll estimate excludes immutable graphs
and keeps its existing conservative checks. Shared-context payload/provider
limits and stream per-component caps are unchanged. Domain N2 is a simultaneous
domain-count refusal policy, not a graph/project memory cap. An eventual complete
report feeds the existing caller construction/prepared reservations, charging
original inputs, staging, runtime/graph/shared roots, worker construction and
old/new retirement overlap without subtracting unreclaimed owners. This skeleton
is not wired to those reservations and is not complete admission evidence.

Root publication/reuse, opted-in resident-only/new-generation empty catalogs,
tokens and preroll remain separate unfinished work. Linux/container tests do
not establish native CoreAudio/100-track/deadline/listening acceptance.

## completed engine-owned return-feed family

The Return-feed DAG now exhaustively visits its order Vec, outer outgoing
Vec<Vec<Feed>>, and every inner Feed Vec. The enclosing graph pointee already
contains the plan and its two Vec headers; outer reserved slots contain inner
Vec headers. Only backing capacities are added, including empty overreserved
inner buffers and unused outer slots. Feed leaves are exhaustively matched as
scalar-only; adding fields to plan or Feed requires reviewing the owning module.
Nested containers use the actual graph owner plus distinct component/index
locations, allowing bounded caller scratch to deduplicate repeated graph visits
without collapsing equal-content allocations belonging to different graphs.
There are no independent shared owners or provider declarations inside this
family. This completes only its owned data traversal, not its containing Arc
or Playback. Arc overhead and other families still remain Unknown. No admission,
publication, clock, routing, DSP or limit behavior changes.

## private pinned arc declaration validation

`hex-audio/tests/support/declared_arc_layout.rs` is PRIVATE test support, not a
product API or admission producer. Its explicit passed compiler/target proof
checks the runner-selected rustc verbose observation against audited Rust commit
`4a4ef493e3a1488c6e321570238084b38948f6db` and the running native test target
(`x86_64-unknown-linux-gnu` or `aarch64-apple-darwin`). This is not a build-time
product compiler gate; the workspace rust-version minimum does not pin it.
Source assumption: the official pinned `library/alloc/src/sync.rs` ArcInner
layout audited by worker2 has two AtomicUsize counters before the payload:
<https://github.com/rust-lang/rust/blob/4a4ef493e3a1488c6e321570238084b38948f6db/library/alloc/src/sync.rs>.

The checked declaration is `Layout::array::<AtomicUsize>(2)?.extend(payload)?.0`
then `pad_to_align`. Sized payloads use `Layout::new`, slices checked logical
length and element layout, and strings UTF8 byte length. It is an assumption-bound
requested layout, not a stable private std ABI or RSS/allocator metadata measure.
No header constant is guessed. Overalignment/ZST padding is retained.
Arc<Vec/String> pointee headers and their separate backing capacities are not
collapsed. Known allocating constructor paths capture layout/source/target while
strong owners exist and preserve that scalar receipt through Weak-only lifetime;
no dead DST is accessed. Weak::new carries no allocation declaration. Arbitrary
existing/raw/custom-allocator/ambiguous default-empty owners remain Unknown;
zero length does not prove allocating provenance. Constructors are test support,
not a production fallible original-owner recovery or publication API.

`arc_requested_layout` runs under the EXISTING allocation-probe feature and
pinned allocation-counter0.8.1, without dependency/registry changes or new unsafe
code. It verifies requested SIZE/counts and actual reclamation, including empty
allocating paths versus untracked shared defaults, clones, weak-only ownership,
overreserved source Vecs, replacement and separate Vec/String storage. Alignment
is DECLARED only: this counter never observes Layout::align. Every receipt still
reports `requested_alignment_verified=false`. Requested-alignment observer results and native helper runs must be combined
and reviewed separately; this private helper unit supplies no such evidence. The private helper never enables a producer. The narrow PCM declaration below
has its own build gate; all other Arc costs remain Unknown and no whole admission/root wiring is enabled.

## completed session midi source-template family

Ongoing Session source templates now report their four owned buffers: intro and
repeat SourceEdge Vecs, and intro and repeat SourceSpan Vecs. Exhaustive matching
covers the template, scalar DensityEnvelope and both scalar leaf records. The
source-template Arc pointee already charges its headers and inline proof fields;
only actual backing capacities are added, including overreserved empty repeat
buffers. Each component has a unique location under the ACTUAL template Arc,
so reused templates can be deduplicated independently of inventory/graph roots;
equal-content templates with different owners remain separate charges.

The existing public Playback visitor reaches these buffers through a private
Session-inventory traversal. The later ordinary-container unit below covers inventory names and vectors,
instrument-template buffers and note-bank entries. Their unapproved Arc
and opaque provider/state dependencies still remain Unknown. Source Arc owner overhead
independently remains Unknown. The Session tempo family is now reported through
the reviewed owning hex-project accessor described below; point lengths are
never substituted for allocation capacities. Instrument/note-bank Arc and provider storage, and retirement receipts,
remain incomplete. No provider/Arc/whole-Playback completeness claim,
callback traversal, public trait redesign, cap/root/preroll change is made.

On any visitor error, callers MUST discard the partial sink: no returned complete
summary exists and partially accumulated items are not an admission receipt.
Tests cover all four partial-refusal points, overflow, actual overreserved
capacities, no repeated headers/density charge, per-container actual-owner dedup
versus independent equal sources, and the real Playback/Session consumer seam.

## completed session tempo backing family

`TempoMap::owned_allocation_layout` returns an immutable scalar layout descriptor
from the owning hex-project module: actual point/prefix capacities, privately
computed element sizes, and checked backing-byte totals. It exposes no mutable
map, point/key, pointer or vector and does not change validation, numerical
conversion, capacities, policies or caps. The two Vecs have scalar elements and
no nested/shared ownership. Zero-length spare buffers in private accounting
fixtures still have a charge; such fixtures do not bypass nonempty valid-map
construction. Multiplication and combined-total overflow return None.

The control Session visitor now adds both backing allocations under the actual
Session inventory Arc owner, independently of source-template owners. The
inventory pointee already includes PreparedTempoMap/TempoMap/Vec headers and
periodic scalar metadata, so those are not charged again. Repeated prepared and
runtime references to the same inventory need dedup by actual owner AND charge
location; exclusive items always remain scoped to the inspected Playback root.
All other inventory/provider/Arc/retirement unknowns independently prevent
complete admission. Any failed partial sink must be discarded. There is no
callback traversal, root publication, preroll or Arc completeness activation.

## version-bound declared pcm arc overhead

Only `Audio::new` validated nonempty Vec-to-Arc PCM origin now caches a private
checked requested-layout receipt while the actual strong slice is live. Audio
clones preserve that receipt and the actual allocation; no new Arc clone occurs
in reporting. Only this known family may emit separate OwnerOverheadDeclared
bytes. Payload data remains measured separately; the receipt's inline fields
increase actual parent pointee/Clip stride and are not charged again. Raw,
default-empty, untracked future origin or missing/mismatched receipt stays Unknown.
There is no PCM Weak API; claims must remain retained through actual graph/tail
retirement. Future Weak coverage requires separate approval and lifetime proof.

The std-only engine build script actually invokes Cargo-selected RUSTC `-Vv` and
`--print sysroot`, checks pinned release1.94.0/commit4a4ef493, native HOST==TARGET
on the two supported targets, and ordinary official install component metadata.
Wrapper/ambiguous flags/custom sysroot/build-std inputs, missing metadata,
malformed/failed commands or failed evidence write select Unknown without
breaking legacy Audio construction. Relevant compiler/target/wrapper/flag inputs
and installation metadata are Cargo rerun triggers. Deterministic build evidence
archives command/compiler/sysroot paths, stdout/stderr/exit diagnostics and the
decision; no flag values or environment dump. Build-script output overrides
caller "verified" environment text. It is NOT a security attestation or detection
of disguised compiler/stdlib tampering; unsupported known configurations refuse.

Formula uses checked Layout::array<AtomicUsize>(2).extend(payload).pad_to_align
under this version-bound declaration, never a guessed fixed header. This is the
Layout requested from the allocator by audited std. Physical allocator rounding,
bookkeeping and RSS are outside scope, including custom global allocator physical
behavior; no final allocator policy or compiler hash infrastructure is added.
All other Arc/provider/retirement Unknowns remain independent. No whole Playback
completeness, caller reservation/root publication, cap or preroll is enabled.

## ordinary prepared/session owned-container coverage

The control-only traversal now covers the following additional backing storage.
This is owned-container coverage, not completeness of their opaque/shared
children. Vec capacity and String byte capacity are charged, never logical len;
containing headers and reserved element slots stay in their parent charge.

| enclosing allocation | backing allocations and charge location |
| --- | --- |
| actual Session inventory Arc pointee | scenes, cells, scene-cell index/range vectors, tracks, MIDI/audio clips and mutable-factory vector; every scene/track/MIDI/audio name String indexed within that inventory |
| actual shared context-table Arc pointee | owner-handle and ID-index vectors; nested context owners retain separate actual Arc identity and explicit opaque-provider Unknown; cached payload declarations are not duplicated as measured data |
| actual PreparedRevision Arc pointee | mutable-track descriptor vector, each effect descriptor vector and Arrangement MIDI-source descriptor vector |
| actual Session MIDI-instrument-template Arc | intro/repeat voice vectors and intro/repeat interval-node vectors; actual nested note-bank Arc separately owns its entry vector, while context/provider and all unapproved Arc overhead costs remain Unknown |
| actual nested MIDI-preview clip-slice Arc | each preview sample vector; the slice pointee charges clip headers, separately from the outer preview Arc |
| exclusively control-owned Session runtime | each active/tail mutable occurrence primary/scratch vector, distinguished by active/tail role and slot |
| each active/tail TypingPerformance Box | its inline arm's effect-topology vector; Box and topology locations distinguish active slot0 from tail slots1 onward |
| inline Return-effect tail bank in Session runtime | reserved Option<ReturnEffectTail> slot vector, including vacant/spare slots; each occupied chain's existing declaration plus opaque-owner Unknown |

Existing Session route, release, suppression, continuation, context-slot,
mutable-event and runtime-chain vectors remain reported. Typing palettes,
voices, command/receipt/event arrays, Return scratch and other fixed arrays are
inline, so no extra heap charge is added. The actual containing Box/pointee or
reserved element stride includes those arrays and any scalar receipt fields.
The Return tail chain itself is inline in its slot; lattice-owned entries,
configuration, meters and opaque processor storage still require their own
contract and remain Unknown. NativeDeviceState/source-identity internals,
inventory sine/context owners and instrument/note-bank Arc overhead, typing note/context/state
providers and Weak-only lifetimes likewise remain explicitly Unknown. This
unit neither infers those costs from size_of_val nor applies the approved PCM
origin declaration to another Arc family. All unsupported costs still refuse
complete admission independently of any measured items the sink retains.

DSP-chain declaration/Unknown locations now distinguish Session track0,
Return1, track-tail2, Return-tail3, renderer track4 and renderer Return5, followed
by the actual vector slot index. Previously repeated chain locations could not
distinguish distinct exclusively owned chains. Shared preview sample locations
use their actual clip-slice allocation identity, so two wrapper clones alias
that allocation while equal-content independent previews do not. Shared
inventory/revision containers similarly use their actual enclosing owners.
Exclusive locations remain scoped to the inspected Playback root.

Tests exercise real spare capacities (including empty vectors), name byte
capacities, nested header exclusion, actual shared allocation alias versus
independent equal data, unchanged strong counts, distinct typing/tail/chain
locations, retained mutable ledger ownership, and every bounded partial-sink
refusal. Checked multiplication refuses before emission; every partial sink on
any error must be discarded. The visitor adds no public provider contract,
callback traversal, allocation, ownership clone, runtime mutation or limit
change. Retirement receipts, caller-reservation/root-publication wiring and
preroll remain unfinished. No full Playback or native acceptance is claimed.

## tracked prepared metadata and explicit owned seam

Private `OwnedShared<T>` pairs an actual Arc with a checked version-bound
requested-overhead receipt. Only audited owned sized and nonempty Vec-to-slice
constructors capture it. Untracked Arc intake never upgrades; ambiguous empty
slices/defaults stay Unknown. The same pinned compiler/target/build gate as PCM
is reused, without broadening that gate's trust claim. A scalar stores constructor
origin and optional checked overhead; pointee data size is read only while the
paired strong owner is live. It is not a Weak-only reclamation receipt. Metadata
fields are included in actual enclosing pointee/element sizes, not added twice.
No persistent address/registry, custom unsafe, mutable exposure or callback
reporting/ownership operation is introduced.

Audited constructor uses in this unit are Project/PreparedRevision owned roots,
prepared Arrangement/Session/preview roots, Session source/instrument templates,
note banks, nonempty preview clip slices and context tables. Cached control
reuse carries the same actual allocation/receipt. State/device/sine/provider and
trait-object Arcs remain untracked/Unknown. Requested-layout declarations never
imply complete retirement: `arc.retirement_receipt` is explicitly Unknown until
actual old/new/tail ownership union and cleanup receipts are implemented. The
legacy note-bank payload/index/Arc-handle guards and exact numeric ceilings are
unchanged; receipt bytes belong to actual containing graph/Vec charges, not a
silently enlarged bank cap. The guard is not total graph/process memory.

`OwnedPreparedRevision` exposes only a restricted view: sample rate, streamed
flag/bindings, seek-history requirement, track IDs, scene ID/name leaves and
borrowed scalar MIDI-preview samples. It has no public Arc/Weak, Deref, AsRef or
raw PreparedRevision/Project/Inventory/Preview access. `clone_on_control` shares
the paired root explicitly. `Session::compile_owned_prepared_revision` is opt-in
and uses the same compile-value validation before allocation as legacy methods;
no production default is switched. Unsupported compiler layouts remain Unknown
without rejecting otherwise valid preparation. Fatal allocation OOM/abort stays
outside API-reported recovery, as in the legacy control constructors.

Owned Playback returns None from legacy `prepared_revision`: that means raw Arc
unavailable, not unprepared. `prepared_origin` distinguishes Arrangement-only,
legacy Arc and owned metadata; `owned_metadata`, source-fact borrowing and
`same_prepared_owner` expose no graph owner. Fallible runtime preparation borrows
the owner and completes all fallible work before cloning the root into the
finished runtime; API refusal returns the original wrapper/receipt. The Session
candidate recovering API returns a control-allocated recovery Box on failure,
with the same inner owner; this carrier is not a complete construction-budget
claim. No callback error is newly boxed.

Production caller audit: Session candidate construction now borrows source facts,
service stream classification reads the actual streamed flag, UI retains owned
pending Playback without legacy publication and reports unsupported preparation.
Both legacy audio publication routes and the lifecycle preparation facade refuse
owned origin before acceptance and return their input unchanged; callback stream
fault handling also refuses a malformed owned legacy binding. Engine preroll's
raw getter is confined to existing legacy Session-runtime paths. Other repository
getter occurrences are tests. Legacy getter behavior remains unchanged.

`VerifiedOwnedStreamRevision` is a control-only typed verifier retaining the
owned root plus actual CatalogLease. It validates every borrowed Arrangement and
Session binding and compares actual root/catalog identities; mismatch returns
both owners unchanged. It exposes restricted metadata/boolean identity or moves
owners back on control, never a raw graph Arc. This is NOT decoded/installed-page
readiness, domain admission, publication or native acceptance. Legacy verified
wrappers remain supported. The native typed bridge remains gated: root/caller
reservations, borrowed staging integration, old/new/tail receipt union, opted-in
resident/empty-catalog domain lifecycle and preroll are still unfinished. No
cap/default/publication policy changes or native100-track claims are made.

## caller-reserved anchored claims (control only)

`ClaimReservation` supplies bounded item/anchor counts and existing caller
prepared/construction byte reservations; it is not a new project, track or
process limit. `PlaybackClaims::try_prepare_on_control` borrows the runtime
exclusively while its original Box remains retained. It fallibly reserves
persistent items/anchors and temporary visitor scratch, records actual returned
capacities (including reclaimed partial reserves), and checks arithmetic and
reservation overlap. Container reservations may allocate before their returned
capacities can be checked; this is not a physical allocator peak cap.

Complete admission refuses every Unknown independently of sink behavior. No
retirement Unknown is waived. API refusal returns the identical original Box
and reports observed scratch/container capacities and the separate control
error carrier. Process-fatal allocation OOM/abort is excluded. The inline claim
header is identified and included once; caller parent declarations must exclude
that overlap. Other caller retained/construction declarations must be disjoint.

Persistent shared claims hold actual typed strong owners, not borrowed addresses
or content keys. Allocation plus charge location plus equal charge deduplicates
within a root and across a control-only old/new union; exclusive charges belong
to each retained Playback Box. Constructor declarations do not infer external
alias or Weak retirement. Unsupported anchor families refuse. All capture,
traversal, cloning and reclamation in this unit occur on control.

`PublicationClaims` is only an owning control pair with the actual typed graph/
catalog verifier. Mismatch returns both owners. It cannot publish, render, mark
pages installed, or produce an ACK. No default production path uses it. Native
old/new/tail retirement producers and bounded lifecycle union slots remain to
be wired: there is deliberately no automatic claim-release operation. ACK or
predecessor Box return cannot retire charges. Conservative retention keeps the
entire predecessor set through the last inherited effect/mutable/typing tail
and retained/retiring context; Busy/panic/missing/incomplete receipts must keep
that set in existing custody. The anchor test demonstrates retained allocation
and unchanged whole charges across actual alias returns; it is not proof of the
unimplemented native transition producer.

The narrow lattice `PreparedStorageDeclaration` defaults to Unknown. Audited
`ExclusiveOnly` covers the provider pointee and all exclusively owned nested
heap, never shared/Weak/opaque owners or runtime twins. Built-in sine owns only
sample_rate; its shared lookup table is a static inline OnceLock/fixed array,
not a hidden heap allocation. Its prepared-provider report remains separate
from graph/runtime completeness. FM4/operator/external/mutable/shared-context
providers remain Unknown. Empty runtime chains report actual entry capacity
and private element size only when entries are actually empty; nonempty
zero-DSP chains still refuse opaque owner completeness. Existing numeric bank,
context, runtime, stream and N2 domain policies are unchanged. Root publication,
retirement receipt wiring, preroll and full native acceptance remain incomplete.

## sealed split and bounded lifecycle storage

The consuming claim split returns `ClaimedPlaybackOwner` and immutable
`AnchoredPlaybackClaims`. Both have private constructors; the owner exposes no
raw Box, Deref, mutable Playback borrow or replacement path. Reunion compares the
existing unique checked Session runtime identity AND actual prepared-root owner.
Two runtimes sharing a prepared root cannot reattach each other's claim half.
The original Box allocation survives split/reunion/refusal. Successful splitting
allocates/clones nothing; owning control refusal carriers use explicit fatal
OOM/abort exclusions. The maximum inline split metadata is charged before moves.
Archived item access borrows the actual anchored receipts, not active runtime.

The opt-in audio root reserves fixed slot and scalar-record Vec capacities from
caller item reservations, reports actual capacities and checked byte arithmetic,
and retains them in the existing bridge/lifecycle root. No default graph cap or
global owner registry is added. A private checked process-local root identity
allocator holds only one AtomicU64; root identity plus checked slot incarnation
rejects stale/recreated-root associations. Association/receipt constructors are
private; tokens are move-only scalars and hold no graph/catalog owner.

Fixed storage reservation is NOT admission. Full union/catalog/association
accounting remains Incomplete and refuses before owners/path moves; current
Unknown reports are never admitted. In particular, the retained verifier's
CatalogLease can itself cause Busy shutdown. Before callback wiring, control
must define actual lease release (while retaining graph charges) and/or proved
coverage by a surviving same-catalog slot, followed by truthful cleanup.
Preliminary graph projection is not a complete/deduplicated union admission.

The closed receipt predicate requires returned custody, completion of all five
inherited-owner families, accounted catalog cleanup and required quiescence;
incomplete conservation stays retained. ACK, silence, voice completion, Pause,
Stop, Seek and predecessor Box return do not clear charges. No real callback
retirement producer is installed here. Protocol-model tests exercise missing,
duplicate/stale facts and allocation/drop pressure; they do not prove native
receipt production. Actual transfers, including dormant owners and the
ValidatedContextTransfer association preflight/commit, remain to be wired.

Lifecycle tick takes the claim manager outside the actual media-root mutex;
enabled-but-absent is Busy and cannot be cleared by a concurrent tick. Only empty
or unstaged/no-owner storage may be disposed during close; occupied/incomplete
slots retain custody. The actual-root test observes control-thread disposal
outside that mutex. The existing native publisher still refuses owned origin;
sealed render/handoff delegation is the next engine seam, not public raw mutable
access. No typed production publication, preroll or whole24/native claim exists.

Closed lifecycle roots reject publication preflight and staging before slot
reservation or owner moves; staging refusal returns all sealed inputs. Receipt
input requires Accepted phase and currently refuses every fact as Incomplete:
accepted custody/incarnation alone cannot prove a final transfer boundary that
prevents later dormant owner transfers from invalidating completed roles. No
catalog cleanup receipt or role fact is manufactured by this infrastructure.

## opt-in catalog custody reservations

The reaper can preallocate caller-bounded late-lease records and a distinct lease
Vec, reporting actual backing capacities/checked bytes. ControlSlot inline
metadata remains charged in existing escrow slot size. These declarations are
data capacities, not RSS/allocator bookkeeping or a new aggregate memory cap.
Existing legacy constructors reserve zero late cells; the N2 domain policy and
per-worker limits are unchanged. Original source/path owners are never touched.
Configuration refusal leaves domain ownership and accounting unchanged.

Move-only reservation/awaiter/receipt handles carry the existing actual reaper
seal, domain-slot incarnation and checked reservation nonce. Existing seal
clones happen only on control; no new allocation/Arc layout assertion is made.
Exact cancellation is explicit; abandoned handles keep bounded occupied rows.
Records/leases and their charges stay in existing durable lifecycle custody.
Pending reservations block domain return/reaping and rollback; configured
storage must be explicitly released after exact cancellation before legacy
rollback can recover its original inputs. Release/reconfigure preserves nonce
history. Actual domain reuse advances incarnation, refusing overflow before
input owners move.

Low-level stream commit validates actual catalog/control pair and reservation
before moving a lease into its preallocated destination. Typed verifier commit
returns the restricted graph root, with no clone/allocation/drop after the move.
It proves physical custody only, not graph-owner retirement. Actual shutdown
releases known custody leases on control and retains its actual join/pool report.
Only successful accounted shutdown yields a private sealed cleanup receipt;
receipt consumption returns the actual report, and outstanding proof rows block
report consumption/slot reuse. Busy, missing/panicked/incomplete reports never
produce completion. Legacy normal/terminal callback return transports unchanged.

Audio binds a reservation to its private claim root/incarnation and unique
runtime identity in the already allocated claim slot. Closing/rollback cannot
discard it; exact cancellation restores the original token on refusal. Current
full claim admission still refuses, so this binding has no enabled publication
producer. The post-disposal API consumes PublicationAnchors, never reconstructs
a disposed PlaybackClaims, and currently returns original anchors plus token as
Incomplete. Its eventual success owns anchored graph claims, restricted prepared
root and stream cleanup waiter. The authoritative control-disposal/final-transfer
boundary and claim-bound cleanup-fact ingestion are not implemented; no fake
proof is supplied. Entire graph/cached catalog charges remain retained. No
callback forwarding, preroll, provider expansion or whole24/native claim.

The standalone control reaper tests call shutdown outside any media-root mutex.
This does not establish native integration: existing NativeStreamRoot::tick
still calls legacy reclaim_receiver while holding that mutex. Before enabling
the new claim path, lifecycle must take actual cleanup custody into persistent
control-owned storage with a Busy marker, perform teardown outside the mutex,
and restore/retain all reports and owners on uncertainty. This unit neither
changes that legacy ordering nor claims native unlocked cleanup is complete.
Successful physical lease-move tests observe zero allocation/free and unchanged
actual catalog-owner counts with an allocating positive control; they are not
whole-callback, hard-real-time or native acceptance evidence.

## partial native outside-lock cleanup custody

NativeRootOwner now preanchors ordinary state and a companion CONTROL-only
cleanup mutex in the same allocation. Actual inline capsule/receiver/panic slot
and existing identity-handle sizes are charged in root storage before caller
path preparation. The existing bridge tag anchors exact root identity; checked
extraction incarnation and vacant destinations guard reconciliation. No new
reaper executor, global registry, domain cap, callback lock or provider API.

Tick and restricted report consumption move actual bridge/preparation/return
receiver into persistent custody before releasing ordinary state. Only a borrow
crosses catch_unwind. Graph/container/catalog disposal and shutdown/join run
outside the ordinary mutex; failed/panicked/mismatched custody stays faulted with
actual reports and the first cleanup payload, and destructive retry is refused.
Closing is sticky through reconciliation. UI publication/preparation/status
refuses Busy while custody is away, preserving original input/path allocations.
Headless exclusive bridge reclaim remains unchanged. The idle terminal-disposal
permit holds only control custody, with ordinary state unlocked; native lifecycle
acquires it before taking coupled terminal owners and retains terminal custody
on Busy/fault. Backend guards/rendering never acquire this mutex.

This checkpoint is NOT complete native lifecycle custody. Existing
retire_prepared_output still owns Retirement on its stack and calls drop(owner)
outside the borrowed cleanup-capsule catch. Actual NativeCallbackOwner's retained
panic payload can itself panic in Drop. A real-owner negative control confirms
that failure is outside the capsule latch; in production it can unwind the
stack-owned Retirement/lease. Before claiming completion, the lifecycle executor
needs persistent bounded Retirement/terminal-disposal custody through a borrowed
catch, an incomplete disposal tombstone and retained destructor-panic payload.
Do not reconstruct a partially destroyed Box or invent cleanup/role completion.
This further ownership contract is escalated before implementation. Full claim
admission/disposal-role proofs remain refused, with no callback publication,
provider expansion, preroll, native100-track or hardware performance claim.

## partial persistent native retirement execution

The preceding checkpoint's actual-owner negative control remains as historical
failure evidence. The existing lifecycle worker now keeps one persistent
Retirement execution slot. It borrows that slot across unwind catches rather
than capturing its sole owner. Occupied execution/quarantine stops consumption
of another queued retirement; reconciliation never replaces retained custody.
This adds no executor, registry, queue or device capacity. Inline execution,
control-root anchor and cleanup-permit storage are included in native additional
storage reporting, not represented as allocator/RSS or complete admission.

The coupled terminal boxes occupy separate persistent fields. Before either
box is destroyed, NativeCallbackOwner's original first panic allocation moves
into a third field. Its destructor cannot run while callback destruction
unwinds. The original callback payload and the first teardown-unwind payload
have separate records. Each stream/box/payload enters disposal-pending BEFORE
its owner is taken; completion is recorded only after destruction returns.
A panic leaves an irreversible incomplete tombstone, remaining owners, actual
payload records, media root and exclusive lease retained. No Box reconstruction,
payload overwrite, repeated destruction or cleanup success is inferred.

The idle cleanup permit stays outside the borrowed catch and remains held until
failure is durably recorded and the ordinary root is latched incomplete. Stream
destruction is also inside this borrowed boundary: successful pause proves the
stop prerequisite, not destructor completion. Root cleanup runs only after the
permit ends, outside the ordinary mutex. An already-faulted retirement does not
retry destructive work. This does not grant a force-stop policy, backend drop
thread guarantee or hard-real-time bound.

Tests inject failures on the actual data/error boxed-owner destructors and
original callback payload, observe original owner identities and control thread,
retain the real exclusive lease/root across failure, and assert no repeat or
payload replacement. A separate generic stream-destructor test exercises the
pending tombstone and a bounded queue model exercises occupied-slot pressure;
neither claims real CoreAudio failure behavior. Test-only destructor probes add
no production callback work. The original naked terminal-disposal negative
control still demonstrates why local owning tuple/catch custody is insufficient.

This correction remains partial source evidence. Physical terminal disposal is
not inherited graph-role retirement or a claim-release receipt. Complete claim
admission, authoritative disposal/final-transfer proofs, publication wiring,
preroll and sustained native100-track/mixed-rate/edit/seek/launch/fault acceptance
remain unfinished. No Unknown, cap or provider declaration is weakened.
