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
Unexpanded Return-feed, gain/instrument, inventory/preview/revision metadata,
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
