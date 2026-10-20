# paired project exports

Pulse2783 decision21741 selects the existing restricted `SharedMetadata<Project>`
facade, exported as `hex_session::SharedProject`. Public raw Arc<Project> APIs
could not supply a closed canonical constructor invariant. This adds no owner,
cache, registry or provenance family and no engine dependency in the model.

Canonical ViewState, CanonicalAudioSourceSnapshot and normal/tuning/device
preview and gesture producers now consume fresh owned Project values once at
the existing paired constructor. Handle clones retain its receipt. Consumers
borrow immutable Project values or clone the whole paired handle. No raw Arc,
Weak, adoption, mutation or detachable receipt escapes. `shares_owner_with` is
allocation equality for cache/correlation only; `PartialEq` still compares
contents as the previous Arc consumers did, without declaring identity.

Outer header/body and each actual paired field are observed separately. A source
snapshot's field aliases the canonical view's Project allocation. Independent
canonical views and previews allocate independent outer bodies even when values
compare equal. Repeated fields do not substitute for missing exported aliases.
Actual exclusive model buffers, canonical MIDI and native-state children use
existing typed traversals; repeated visits to one body deduplicate the SAME
physical MIDI fields, while a separately cloned Project adds real child aliases.
Inline Undo Project values belong to their history buffer rather than this Arc
body charge. Source/loop/waveform geometry and UI gesture behavior are unchanged.

`visit_shared_project_selected_storage_on_control` is selected storage only.
It refuses import-provenance and edited-audio shared domains pending their own
qualification; an outer wrapper does not make those nested allocations known.
This is a prospective closed-graph restriction, not a new processing behavior:
referenced registry admission remains closed. No compatibility reader or
historical-alpha conversion is required. Existing projects/media are untouched.

Fresh Project cloning still allocates owned model buffers and shares canonical
children. Neither the facade nor final capacity observations recover old/new,
input/history/temporary construction peak. Complete live root inventory/custody,
atomic stale/cancel/refusal publication and saturated control/reaper retirement
remain mandatory. These selected exports do not qualify enclosing ViewState,
source-snapshot/preview/renderer owners or the full graph by themselves.

New oracles cover actual canonical view/snapshot plus two real preview producers,
independent equal outer bodies, omitted outer alias and whole-body totals,
exact/minus-one retained budgets, unknown nested ownership, old exports after
Session retirement, and nested actual MIDI/state-field deduplication. Compile-fail
docs reject raw adoption and mutation. An existing audio allocation harness probes
128 repeated paired-body/model/MIDI/state inspections after ledger reservation;
its result must be executed on a supported constructor build before claiming
zero allocation/free. No native visual/listening or playable Sampler claim follows.

## Canonical source snapshot parent

`service::SharedCanonicalAudioSource` carries the same existing restricted
`SharedMetadata` pairing for the actual fresh `CanonicalAudioSourceSnapshot`.
The worker constructs it once; event, native desired-source and preparation
copies clone the paired handle. No raw `Arc` adoption or mutable owner accessor
is added. Project/context capture, source selection and native publication
behavior remain unchanged.

`visit_canonical_source_selected_storage_on_control` visits that actual outer
field/body, its actual paired Project field, source-map capacity and resident
PCM fields. Repeated outer aliases do not invent additional embedded Project
fields. Independent snapshots retain separate outer allocations even when they
share a Project. Verified-file facts remain unsupported by this selected visitor;
its known outer body never qualifies an unseen child.

This is selected inventory, not a complete exported-view/root visitor. Preview
owners, enclosing records, every other live owner, old/new/temporary construction
peak, guards, atomic publication and retirement remain separate obligations.
Fresh snapshot construction must be covered by the candidate construction quote
before publication; post-construction retained observations cannot establish its
historical peak.

## Prepared gain-preview child

`SharedGainAutomationPreview` uses the same existing immutable paired facade.
Canonical and preview view producers consume their newly prepared owned preview
once; exports clone the paired handle. There is no raw Arc upgrade, extra gain
cache, PCM copy or mutable shared accessor. Typed consumer changes preserve gain
curve query and editing behavior.

The selected gain visitor charges the qualified outer body/header once, every
physical shared field, actual track-entry capacity, each actual knot backing and
both prepared touch backings. Inline Vec headers belong to their enclosing body
or track entries. Empty reserved capacity stays charged after clear. Fixed knot,
mix, span and step fields are exhaustively inspected; new owned fields require
explicit traversal. Independent equal-content previews retain distinct backing
charges. This neither detects entirely unvisited owners nor establishes enclosing
view completeness, construction peak, frozen custody or atomic publication.


## Prepared MIDI-preview child

`SharedMidiArrangementPreview` carries the existing private prepared-revision
`OwnedShared` allocation and receipt together through a narrow crate-private
bridge. The actual prepared owner, its extraction machinery and retirement path
remain unchanged. Exports add handles to that same outer allocation, not copied
preview bodies or adopted raw Arcs. The older prepared owner has no stored cache
version; its exports report no cache identity rather than deriving one from an
address or adding another version map.

Explicit fresh empty previews allocate the qualified sized-empty-array origin.
General defaults and raw/untracked owners remain Unknown, including through the
bridge. Empty input Vec disposal occurs on control before that fresh allocation;
this is not a proof of the earlier input/preparation peak. No legacy conversion
or compatibility reader is added.

Selected traversal charges outer header/body, each physical exported handle,
the actual private shared clip-slice header/body and every sample Vec capacity,
including empty reserved samples. Slice-entry inline Vec headers are charged in
the slice body. Repeated outer exports do not duplicate embedded clip fields;
separate equal allocations remain separate. The actual prepared owner is still
a distinct live field: exported handles cannot stand in for it. Missing it must
fail selected handle closure. Fixed clip/sample leaves are exhaustively matched.

The new runtime oracles require a supported constructor build. They cover actual
prepared allocation identity before producer retirement, omitted prepared/export
aliases, repeated-visit deduplication, exact/minus-one retained budgets, reserved
empty samples, independent empty headers and raw/default/unsupported refusal.
Their post-construction observations do not establish whole-root inventory,
construction peak, guarded publication, final retirement or usable Sampler UI.

## Actual retained snapshot records

The selected record visitor now traverses both actual record Vec backings and
EVERY occupied paired ViewState: current and retired records, readers released
but not cleaned, unpainted records and cleanup-latched owners still present.
Each view visits its canonical source, actual Project field, waveform maps and
children, MIDI preview, gain preview and document-path backing. Scalar/atomic
record and view fields are exhaustively matched. No synthetic read-ID owner or
released-reader count substitutes for an actual retained object.

`SnapshotFamilyQueries::visit_selected_storage_on_control` borrows these actual
records while the existing family guard remains held. The same ledger must also
join the actual holder/table, other families, prepared/history/pending/native
owners and all other live roots before publication. The method supplies no
whole-root witness or construction quote, and guard release is not a seal.
Cleanup still takes its existing try-write route and cannot release a still-read
snapshot; durable ACK alone releases no owning bytes.

Document PathBuf inline headers belong to ViewState. On the supported pinned
Rust targets, its actual capacity is byte backing: audited Rust commit
`4a4ef493e3a1488c6e321570238084b38948f6db`, `library/std/src/path.rs:1789`
delegates to OsString; `library/std/src/ffi/os_str.rs:350` delegates to its Buf;
`library/std/src/sys/os_str/mod.rs` selects bytes on Linux/macOS, and
`library/std/src/sys/os_str/bytes.rs:19,133` stores/returns the actual Vec<u8>
capacity. The visitor charges capacity, including empty reserved paths, using
the existing Vec backing contract. Other targets/unsupported pinned constructor
builds refuse. No encoded-length estimate, raw address, allocator usable-size or
historical peak is inferred. These primary source files were read at that exact
commit; this does not constitute runtime or platform acceptance by itself.

New runtime oracles cover two actual independent canonical records and omission
totals, exact/minus-one selected retained budgets, an unadopted pending source,
released old readers before actual cleanup, guarded family retirement, reserved
empty path capacity/dedup and known-outer/unknown-MIDI refusal. Final capacities
and the ledger's own construction metric cannot recover earlier Project clones,
input Vecs, old/new/temporary construction overlap or saturated global reaping.
Registry admission and production UI capability replacement remain closed/open
work respectively until those separate obligations qualify.

The next Desktop oracle uses the actual registered persistent family, actual
sealed job Box and scoped Session records together. It joins the actual legacy
Event child fields inside the same nested guard scope and measures 128 repeated
inspections after ledger reservation. The legacy raw Event parent and remaining
worker/prepared owners are explicitly unqualified; omitted physical aliases must
refuse selected closure. A known record cannot silently qualify that production
path. The oracle awaits native compilation/execution; no allocation/runtime
result follows from source inspection. Its test serialization uses the existing
two-slot table unchanged, with actual control retirement before releasing the
fixture gate. No backend or GUI is opened.

## All-family guard boundary (primary22146)

The first implementation acquires every configured actual outer slot in index
order, including vacant slots. A vacancy is protected against reservation or
registration during inspection. The pre-reserved guard Vec checks actual returned
capacity against its component scratch allowance before acquiring any guards;
failure releases only inspection guards and preserves sources/custody. Inline
headers, backing capacity, sealed Box bodies, inner lists/records and all other
roots remain separate charges within the existing numerical limits.

Slot views expose borrowed actual job/phase/incarnation/failure/reservation facts.
Reserved, failed, poisoned, panic-bearing or consumed states cannot qualify intact
first-graph custody. A failed-step job may remain inspectable diagnostically;
consumed final-Drop reservation is not an inspectable body or freed bytes. Active
phase/type dispatch does not qualify payload children. Checked opaque incarnation
observations are correlation, never reusable publication permits.

The narrow engine control payload borrow exists only to acquire the concrete
Session inner inventory guard. It exposes no mutation or Box, but generic T can
contain cloneable owners; it must never enter UI capability dispatch or count as
provider completeness. Inner guards expose selected traversal and scalar queries,
not records/ViewState/owning children. They stay on their acquiring thread and
cannot outlive their actual payload/outer guard. Inner shared guards freeze
structural owners, not read-release/derivation or independent abandonment atomics.
Durable ACK remains available, and existing cancellation publication linearization
is still required.

New component oracles cover three actual fresh scalar jobs plus a protected
vacant slot, all-slot writer refusal, actual guard/table/Box bytes, scratch
exact/minus-one, stale incarnation, partial-acquisition contention/poison custody,
reserved/failed/panic/consumed refusal, and an actual Session inner guard allowing
ACK/abandon while refusing insertion without source loss. They await runtime
execution; scalar protocol fixtures are not three fully admitted Sampler graphs.
Compile-fail tests cover actual payload/inner guard lifetime, acquiring-thread
restriction and borrowed-slot/drop order.

The non-static inner guard Vec cannot use the current Any/'static Vec ledger
observer. This exact typing limitation is primary follow-up22152. No proxy byte
claim, static cast, self-referential aggregate or alternate observer has been
implemented pending its narrow resolution. The production all-family adapter,
complete joint peak, cancellation-safe native publication and full Sampler
admission remain open. The established order stays all outers, all inners, ledger,
then the existing short native barrier last, with guards held throughout.
