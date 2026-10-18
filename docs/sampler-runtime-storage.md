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
