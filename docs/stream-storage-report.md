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
