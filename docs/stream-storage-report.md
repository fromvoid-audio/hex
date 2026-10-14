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
what the sink retains. This initial unit always has Unknown Arc overhead.

The borrowed Playback header is inline in its caller's envelope, not necessarily
boxed. Callers charge their actual envelope/Box. Session and renderer runtime
Boxes report their pointee data once; child Vecs report only backing capacities.
Inline tempo-period/onset/span/tail arrays need no separate heap charge.
Current traversal reports Arrangement root/track/clip containers, resident PCM,
clip metadata Boxes, streamed metadata, processor slices/meters, prepared roots,
Session runtime Vec capacities/retained contexts and renderer Vec capacities.
Unexpanded gain/instrument, inventory/preview/revision metadata,
Return-tail, typing, mutable and lattice-internal owners explicitly remain
Unknown. Existing DSP-twin declarations are separate from these incomplete
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
this unit. Until that audit/declaration is approved, Arc overhead remains
Unknown and complete admission refuses. Opaque providers likewise require
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
reports `requested_alignment_verified=false`. Separate independently reviewed
alignment observation and actual Mac target checks remain pending. All production
Arc costs remain Unknown; no whole admission/root wiring is enabled.
