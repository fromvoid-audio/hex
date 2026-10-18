# Sampler runtime storage boundary

Pulse2783 decision21536 approves the closed Sampler constructor route, not
qualification from trait inspection. This is one component of the still-open
whole supported graph proof; registry resource processing remains refused.

The actual `PreparedTrackRuntimeFactory::Sampler` branch constructs a fresh
pair through the concrete `PreparedSamplerRuntime` factory. A private engine
`RuntimePair` retains that origin inseparably with its lattice pair. Generic
construction, even with the same concrete factory or a wrapper whose `as_any`
impersonates it, remains Unknown. No setter, mutable pair escape or conversion
can relabel an existing generic pair. Session and checkpoint construction use
the actual source enum; moves into retained tails preserve the same owner.
History copying changes scalar state, never the destination's constructor route.
Keyboard factory preparation remains a separate unfinished integration path.

Lattice exposes neutral lifetime-bound observations of its actual primary and
scratch Box fields. They report pointee `Layout`, physical field equality and
allocation equality without raw addresses, mutable references, owner clones or
caller constructors. A compile-fail fixture rejects use across pair mutation.
Observation alone proves neither provenance nor custody. Commit may swap the
two Boxes; every observation must be borrowed fresh after mutation.

Before a concrete Sampler pair becomes an engine track runtime, both fresh
twins now run their read-only zero-frame source preflight against the actual
retained configuration. Missing or mismatched source identity/rate/layout/window
refuses construction, before occurrence buffers or an exposed host exist.
Generic provider construction remains opaque. A regression first reproduced
acceptance without the required source, then checks both source refusals and
unchanged independent PCM playback from a separately accepted host. This binds
the constructor's source requirements; it does not certify configuration custody,
all later moves or the whole graph's storage/publication.

## Requested layout and accounting

The separate `sampler-runtime-sized-box-new-v1` gate uses pinned Rust1.94.0
commit `4a4ef493e3a1488c6e321570238084b38948f6db`, ordinary native x86_64 Linux GNU
or aarch64 macOS. Compiler/install/flags/wrapper/cross-target or evidence failures
remain Unknown. Environmental selection is shared; the PCM/MIDI constructor
family labels are not runtime Box evidence.

Independently read primary sources at that commit:

- [alloc/src/boxed.rs](https://github.com/rust-lang/rust/blob/4a4ef493e3a1488c6e321570238084b38948f6db/library/alloc/src/boxed.rs#L239), SHA256 `25d54ba919797977110ff0e5b4c4c4cec15c42ecfae3e37f6d8ed0c6fc13fe65`: sized `Box::new` calls intrinsic `box_new`, documented to call `exchange_malloc` and move the value.
- [alloc/src/alloc.rs](https://github.com/rust-lang/rust/blob/4a4ef493e3a1488c6e321570238084b38948f6db/library/alloc/src/alloc.rs#L487), SHA256 `03fac86ce9f1ec183926f7ba9c5e8d8c8f3c90eeea79686de04f34ed67099585`: `exchange_malloc` passes the requested size/alignment to `Global.allocate`.

The actual private Sampler constructor is sized `Box::new(SamplerRuntime)`.
Exhaustive typed destructuring audits its prepared scalar table and fixed
optional ActiveVoice array, including compilation of the empty-array case.
New owned fields require an audit update. Runtime declaration bytes, type IDs,
configuration equality and `as_any` do not establish origin.

Qualified storage charges two distinct nonzero bodies using their observed
layouts, checked against the closed concrete layout. Inline fat pointers and
origin are charged once by the enclosing host/Vec; no guessed Arc header or
heap vtable charge is added. Actual occurrence Vec capacities and retained
resource configuration owners are separate. Repeated visitation deduplicates
the same actual fields. Whole-body omission needs an independent expected
allocation/byte oracle because Box owners have no Arc count closure.

The complete mutable-runtime visitor charges its embedded resource configuration
through the actual containing runtime Vec/body, then visits the configuration's
shared lease and source children. It does not charge that inline field again as
a standalone root. The separate resource-only visitor still charges its actual
standalone field. An independent comparison of these two views reproduced a
24-byte overcharge and now checks the actual Session body, parent Vec capacities,
two concrete Box layouts and occurrence Vec capacities, plus exact/minus-one
limits. Both views remain selected inventories: unvisited Session/revision/source
owners prevent complete closure.

This is requested allocation accounting, not RSS or a universal Rust ABI.
Surrounding graph/model/history/pending/unreclaimed owners, root custody,
old/new/preparation construction peak, stale/cancel atomic publication and
actual saturated control/reaper final-drop remain mandatory and unfinished.
Selected closure or successful runtime construction opens no registry gate.

## Closed compiler ingress preparation

Primary decision22890 permits a private closed reuse of the complete compiler
into existing revision custody, with a conservative construction allowance
reserved before any new owner. This ingress is not implemented or qualified yet.
The Session instrument-track lookup now uses one exactly requested Vec followed
by a single allocation-free unstable key sort in the existing `SortedEntries`
container, replacing its temporary tree map. Its actual backing is capacity
visible; no node-size estimate or whole compiler peak is inferred. Canonical
track insertion tests preserve per-clip instrument identity and actual unchanged
source/template reuse when track order differs from key order. Other compiler
scratch, transform failure custody, root installation, inspection and cleanup
still need the closed-entry reservation/audit.

Transform transaction unwinding now latches failure without cleanup, allocation
or a nested catch in Drop. Actual pending entries and their budget tokens stay
in that preparation; subsequent direct or compiler transaction entry refuses
without invoking the new closure. Even an abandoned empty pending transaction
cannot qualify the original-only storage view. Ordinary returned errors still
settle their pending entries and permit the existing healthy retry behavior.
Tests cover empty and real 64,000-byte pending derived PCM, retained charge,
repeated non-entry and explicit control teardown. This is failure-state handling,
not a whole compiler peak bound or permission for automatic input recovery.
Nested active compiler entry has the same abnormal-exit guard. The public outer
transaction rechecks the sticky failure after its closure returns and before
taking/committing pending entries. A panic swallowed inside that closure still
forces outer refusal. The nested-catch regression exercises actual pending PCM
both with and without repeated re-entry attempts; no outer success or cache
commit is inferred from a caller returning `Ok` after a nested fault.

The complete Arrangement compiler now reserves a single per-track audio clip
Vec from the canonical clip count before compiling its first child. It pushes
only successfully compiled present clips in the same order, removing the
temporary `Vec<Option<Clip>>` and flatten/collect overlap. Transformed waveform
sidecars and ordinary error cleanup keep their existing behavior. This makes
one backing explicit; it is not the missing whole retained/scratch/overlap quote.

Routed Session MIDI counting likewise uses a single reserved scalar Vec, bulk
sorted once through the existing `SortedEntries`, rather than a temporary tree
map. Every canonical track has one row, including empty source tracks; chained
fanout and weighted template/note limits keep the existing behavior. Its actual
capacity is visible and no tree-node layout estimate is needed. The Arrangement
compiler now constructs its legacy transform fallback only when no preparation
was supplied; the closed caller's supplied preparation no longer causes an
unused cancellation-owner allocation. Routing/PCM/source-isolation and exact
fanout-limit regressions cover the lookup change, and the existing transform
suite covers both legacy and supplied preparation behavior. These helper bounds
still do not supply the complete preconstruction quote or private custody entry.

Arrangement MIDI onset-density, voice-density edges and interval-tree nodes now
reserve their scalar backings fallibly and explicitly before population. Edge
count multiplication is checked. Onset scratch is destroyed before the voice
edge reservation; stable span/event ordering, half-open event windows and voice
limits are unchanged. Ordinary allocation refusal returns through the existing
transaction rather than adding a panic path. The existing boundary/density and
real Sampler revision-host tests cover this change; allocator requested capacity
still needs validation inside the eventual whole construction allowance.

The two MIDI stable sorts now use allocation-free unstable sorting with the
same complete keys. This preserves order because those keys are unique:
canonical clip/note IDs distinguish simultaneous notes, and the source iterator
uses iteration0 only for the initial occurrence and distinct positive indices
for repeats. A per-track span key includes start/clip/note/iteration; a global
edge key additionally includes kind and track order. Coincident on/off edges
therefore remain distinct, with off before on. This removes sort heap scratch
without dropping ordering fields or modifying musical timing. The full MIDI
integration suite covers independent tempo/loop/gate/preview PCM, interval
queries, voice limits and the exact maximum edge count.

Track sends, compensation feed rows and MIDI-route metadata also reserve one
backing from their actual canonical counts before filling. Existing ordering,
gain resolution, missing-target refusal and signal-graph preparation stay in
place. Private canonical routing validation now uses the separately checked
borrowed scalar graph helper; public untrusted snapshot validation is unchanged.
These helper bounds still do not provide the aggregate private-entry allowance.

Session MIDI row traversal now visits actual direct and routed row backings,
name capacities, canonical MIDI identities, copied native state fields and
paired source/instrument templates. Supplied custody roles reach both row sets;
the candidate seal keeps its explicit Current role. Nonempty construction and
complete-root qualification remain refused until teardown and the other owners
are covered.

Before moving an aggregate into teardown fields, Session assembly now refuses
opaque context, mutable-factory and history-retarget destructors. It checks
each retained instrument template's own context table as well as the current
table: an older cached template can still own an older provider. This check
does not clone, release or replace any owner. Tests preserve the actual opaque
factory, an independently retained opaque context and a paired Sampler context;
the old-template negative still refuses with an empty current table. This is
only an opaque-destructor preflight, not certification of concrete child
destructors, original-source final-drop custody or the whole construction peak.
The existing Session capsule now also retains direct/routed MIDI rows, scalar
source-routing pairs and concrete mutable factories during retirement. A popped
row or factory first moves into a persistent pending field; inspection visits
that actual field before the later explicit drop. Pending-drop flags refuse
retry after abnormal exit. Row/factory Vec capacities remain charged throughout
retirement; only their empty backings are released on control at final clearing,
leaving vacant destinations for later preparation. Aggregate transfer/restore
still moves the same owned inventory, without cloning its children.

The selected retirement regression runs both direct and routed Sampler paths
with the original resource bank retained. It checks actual current/Undo/Redo
aliases, independent backing capacities, every pending destination and omitted
pending-field refusal through each cleanup step. A further regression keeps the
actual fresh complete-compiler meter and tempo maps, or uses the existing real
copy-custody transfers, across both direct and routed paths. Cleanup returns
transferred tempo ownership to its existing destination; fresh owned concrete
maps retire on control without inventing a copy transfer. Meter and timing
pending markers precede destruction and refuse retry after abnormal exit; the
parent retains the panic and failed custody. Concrete maps contain scalar Vec
elements. Copy-custody phase chooses the cleanup destination, not allocation
provenance. Project map pointers and independent musical-time conversions remain
unchanged. This covers those selected map cleanup paths, not a fresh compiler
constructor or preconstruction quote. Fresh complete-compiler owner installation,
aggregate preconstruction reservation, full native union and publication remain
required. Public constructor validation still refuses MIDI graphs; this selected
retirement capability does not enable the usable Sampler host.

Mutable revision descriptors now have actual-field traversal too: outer rows,
effect/source Vec capacities, instrument and effect canonical state fields and
Arrangement MIDI source identities. Inline handles belong to their enclosing
row backing once. Equal descriptors from two real compiler invocations retain
independent backings and require every physical alias; neither equal content nor
a descriptor ID substitutes for an owner. The revision capacity report includes
these observed backings, not a preconstruction estimate of them.

After all four revision/child values have returned to existing destinations,
the existing control cleanup retires descriptor state/source aliases and empty
backings. A pending flag precedes destruction and refuses retry/inspection after
abnormal exit; the parent retains its existing failed custody. The descriptor
types contain canonical state/MIDI metadata, not provider destructors or PCM.
The regression moves actual descriptor output from two Mono/Stereo compilations,
keeps real editor history, includes bypassed effects and spare capacities, and
checks independent six-backing totals, omitted-owner refusal, repeated visits,
exact/minus-one budgets and each normal control retirement. Other products are
retired with the original bank held. This is selected descriptor inventory and
cleanup, not the fresh complete compiler entry or aggregate peak proof.

Descriptor construction validates every track/order/source/count before row
allocation, then reserves all real nested effect/source buffers before cloning
the first canonical owner. Filling uses the same immutable project and selector;
there is no extra plan/cache allocation. This changes allocation order, not the
required aggregate preconstruction allowance or the set of retained aliases.
A routed-source regression checks two independent descriptor destinations with
the same actual MIDI source, preserving track IDs and source identity. Complete
compiler ingress and its old/new/temporary peak reservation remain open.

Analytic note-bank key scratch is now reserved once from checked authored-note
counts before filling. Session reuses its finished clip-count Vec for per-track
key counts and retires the previous-count scratch before allocating key buffers;
it still counts direct and routed notes against the same limit. Mutable factories
do not build unused analytic-key lists. Silent/bypassed analytic paths and actual
previous-bank reuse retain their existing behavior. The scratch regression checks
zero, duplicate-heavy and maximum-note inputs, stable pointer/capacity through
fill and deduplication, invalid-note refusal and actual bank reuse/PCM. This
bounds one helper's requested growth; the aggregate entry must still reserve its
backing and all other live owners/scratch before construction.

Signal-topology preparation also participates in the complete compiler's
temporary cost, including the zero-latency path that returns no retained graph.
Its typed-key tree set/map are replaced by explicit reserved scalar Vecs. Node
and edge counts are checked before builder allocation; insertion refuses an
exhausted backing instead of implicitly growing. Per-track/Return nested
buffers and owner ordering are reserved from actual counts too. Unstable sorts
use complete identity/ordinal keys, preserving the previous tree ordering and
stable parallel-edge ties without a sort workspace. Existing latency, topology
and graph limits still apply; no compensation or typed route is skipped.

Tests compare against the historical tree oracle at graph limits, preserve a
parallel-edge latency plan under shuffled enumeration, reject duplicates and
invalid endpoints, and compare counts against an actual compiled send/Return/
typed-feed graph. Present default generators count even without MIDI clips.
Existing delayed PCM/partition/handoff tests and actual original-PCM Sampler
preparation still exercise the changed path. This exposes bounded scalar
construction backings; it does not qualify a raw SignalGraph Arc, complete
compiler peak, nonempty Arrangement cleanup or native publication.

Fresh Arrangement tempo and MIDI-route metadata now have destinations in the
existing root assembly. Inventory visits the actual TempoMap point/prefix Vecs
and routing Vec capacity in either the aggregate or staged location, including
empty reserved routing backing. Restore moves these same owned values after
whole-destination preflight; it does not qualify a raw shared compiler result.
Capacity reports include these backing bytes independently of root containers.

Control retirement marks metadata disposal pending before taking the map and
routing backing. Normal completion releases them; abnormal exit leaves pending
and the existing parent failed custody, refusing inspection, reservation and
retry. The new regression uses actual fresh compiler metadata, checks independent
capacity totals, exact/minus-one budgets, backing identity, unchanged authored
timing and caught disposal failure. Other compiler products remain separately
rooted in this selected test. Nonempty track/factory teardown, aggregate
before-first-owner reservation/private ingress, complete native conservation and
publication remain open; the public MIDI constructor still refuses admission.

The approved paired-child compiler branch reuses the existing musical Track
layout rather than allocating a legacy Track and replacing its children. Scalar
mix/ordering validation is factored from field assembly; the latter only moves
supplied clips, instrument, chain and meter. The existing legacy compiler calls
this same layout. This helper does not qualify its inputs or grant a storage
permit: the private branch must retain actual birth declarations and aliases in
its pre-reserved custody destinations before exposing any owner.

The existing concrete Utility custody constructor now accepts typed Track or
Return ownership and retains that tag with each actual paired child declaration.
Validation rejects a mismatched owner even if device ID/type and layout match.
Empty, bypassed, memoryless and stateful constructor tests retain original
configuration gains and whole-owner moves. This factor alone supplies no
complete compiler selection or admission. The private Return continuation below
adds its concrete inventory/retirement; nonempty Arrangement instrument rows,
aggregate preconstruction quote and native admission remain unfinished. No public compiler output or raw
Arc is adopted, and no second chain/configuration is created to qualify the first.

Private Return assembly now retains its concrete Utility custody alongside the
actual chain. The same paired constructors create each configuration, optional
filter and meter once with the real Return owner ID; original declarations
survive reserved, installed and moved-root locations. Capacity and ledger
traversal include nested payload/header/metadata backing and handle obligations.
Only actual asset-free Utility children are eligible at this internal seam.

Restore returns the same chain/child fields to existing destinations. Control
cleanup validates before extraction, marks disposal pending before destruction,
and clears child birth records only after normal child retirement. Pending
observation/retry refuses; the existing parent retains panic/failed custody.
The selected tests compare the actual public compiler's effect-chain PCM and
partitioned filter history, preserve gains/bypass/owner tags and physical child
pointers, exercise omitted aliases/exact-minus-one and constructor error/panic,
and keep complete-retirement certification explicitly refused. Low-level fixed
handle visitors must run once inside the ledger transaction; a duplicate visit
refuses and invalidates partial totals rather than declaring another owner.

This does not open the public constructor: its nonempty Return refusal remains
covered by the new test. The private complete compiler selection and aggregate
before-first-owner allowance, nonempty Arrangement instrument rows, full native
conservation/publication and the functional Sampler workflow still need work.


Private Arrangement rows now have fixed destinations for the actual instrument
and canonical native-state fields. Their existing paired note bank/context
children and scalar span/tree Vec capacities are traversed in both installed
and returned locations. The enclosing row backing charges the inline instrument
body once; a parent-aware child visitor avoids an extra standalone-body charge.
The state observer joins the existing ledger transaction without starting a
nested transaction. Reservation, construction and restore require vacant
instrument destinations before any owner movement.

Control retirement preflights the actual context-table destructor surface and
all existing meter/chain births before extracting the Track. Instrument and
state move directly into the persistent row; each disposal marks pending before
removing its owner. An abnormal exit remains pending, blocks observation/retry,
and is retained by the existing parent failed-custody path. Tests exercise
direct and routed loaded Sampler fields, spare span/node capacities, physical
backing preservation, independent omitted-alias refusal, exact/minus-one budgets,
unchanged inline charge across extraction and the actual parent panic boundary.
The fixture extracts selected instrument fields from a complete compiler output;
its unrelated legacy meters/chains are not adopted or declared qualified.
This extends selected traversal/retirement only: the public Sampler constructor
refusal, aggregate before-first-owner allowance, private complete compiler entry,
callback conservation and final native publication remain open.


The existing production signal-topology constructor now creates its one shared
body through `OwnedShared`, retaining that original receipt in the private
Arrangement field and the real renderer/Session runtime graph fields. No raw
Arc is adopted and no second topology is allocated for qualification. The
scalar musical topology/latency algorithm, zero-latency fast path and runtime
budgets stay unchanged. This concerns immutable topology; mutable delay rings,
runtime scratch, callback transitions and complete native conservation remain
separate obligations.

Root metadata has an existing-custody destination for this paired topology.
Traversal covers its header/body and actual latency-plan, typed-key, owner-order,
Track/Return plan and nested device/send/feed Vec capacities. The enclosing
parent charges the inline handle once. Distinct physical fields join closure;
repeated visits do not add handles and shared descendants are visited once.
Restore moves the actual owner to this destination. Control disposal includes
it under the existing pending-before-destruction marker and refuses untracked
origins before extraction. The selected oracle exercises actual compiler and
renderer fields, independent missing aliases, exact/minus-one, normal control
retirement, stable allocation identity and unchanged raw-ingress refusal. This
removes one retained child obstacle; it does not supply the aggregate quote or
private complete-compiler ingress and does not open Sampler admission.


Concrete Utility child preparation now computes a private requested-layout
allowance before reserving Entry backing or creating its first child. It uses
the codec's shared current-payload constant and existing paired layout helpers
for payload/configuration/meter/filter/outer-chain bodies and headers. The
optional filter is conservatively reserved even for a memoryless or bypassed
state. Existing empty Entry capacity stays charged; growth includes the old
backing alongside the replacement, and construction overlap also includes the
Vec-to-slice input and one codec input buffer. These are scope ceilings and
conservative temporary allowances, not observed simultaneous allocations or a
unique whole-root budget.

The constructor rejects unexpected backing capacity and verifies actual retained
children against that allowance. Invalid authored state refuses before any
Entry/child mutation. Tests cover fresh/growing/reused/reserved-empty backing,
filtered and bypassed combinations, source handle counts, host gains, actual
concrete child prices and unchanged destination on refusal. This private quote
is available to the aggregate compiler calculation; it does not itself reserve
the required global old/current/history/candidate union or authorize ingress,
registration or publication. The enclosing same-ceiling preconstruction and
actual final ledger checks remain mandatory.

The Session original-resource producer now borrows a private immutable projection
of the actual media field. The existing public producer delegates to the same
algorithm; it does not clone Session or its editor, create another bank family,
or change source validation, cancellation, capacities or native PCM aliases.
PreparedDeviceSample can use this disjoint borrow beside the actual mutable
editor prepared-transition callback, validating the callback's source Project
and exact candidate assignment against the pending decoded sample.

The focused test runs original Mono and Stereo at 24 kHz against a 48 kHz
project without a frame-copy derivative. It deliberately refuses after resource
preparation and checks the complete current Project and Undo/Redo collections
are unchanged, media is unpublished, and the original PCM pointer and samples
survive bank and producer retirement through the existing lease. Its test-only
bank destination is not production custody. This factors the required borrowed
producer seam only: the sealed candidate/request binding, persistent bank
installation, aggregate preconstruction reservation, displaced-history retirement
and final model/media/audio publication frontier remain required and closed.

The actual AudioPreparationCustody now has one private original-resource bank
field. Moving that custodian through the existing draft into the final candidate
moves this field too. Its embedded visitor charges the original paired body,
catalog and PCM once, includes every actual bank/lease alias, and skips repeated
visits of the same physical bank field. The enclosing custody body charges the
inline bank header. Original-bank recovery requires this resource field to be
normally retired first.

Control cleanup retires the bank after runtime, prepared roots and child
custodians. A precompiler vacant custodian can also retire its unpublished bank.
The destruction marker is set before take and only cleared on normal return;
unresolved state refuses observation and recovery, and abnormal exit latches the
existing first-panic/FailedIncomplete state without retry. Tests cover original
Mono/Stereo aliases, independent standalone-vs-parented bytes, omitted handles,
exact/minus-one budgets, actual draft-to-final-owner movement and normal control
retirement while a real source lease remains readable. A separate injected
unresolved marker test proves refusal only, not a final-drop-panic or saturated
host reaper certificate.

These tests inject the private field after preparing an existing audio-only
fixture. Public construction still refuses nonempty resource custody. No bank
setter, extractor, owning install parameter, producer callback or snapshot bank
was exposed. The sealed producer/install operation, before-first-owner aggregate
allowance, actual pending request binding and atomic publication remain open.
