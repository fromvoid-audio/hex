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
