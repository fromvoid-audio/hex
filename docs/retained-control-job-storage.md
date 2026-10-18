# retained control job outer storage

Pulse2783 decision21945 moves only the sealed fresh job holder and its neutral
step outcome into Engine. Audio still owns reservation, incarnations, per-family
guards, readiness, persistent service custody and cleanup phases. The sole holder
constructor consumes an owned T into Box::new(TypedJob<T,E>) and unsizes that same
Box. Its private trait has exactly one implementation. No raw Box/Arc adoption,
owner extraction, clone, mutable payload accessor or replacement constructor is
provided. Typed payload dispatch is not payload provenance or whole admission.

The separately named `fresh-retained-control-job-sized-box-new-v1` gate selects
pinned Rust1.94.0 commit4a4ef493e3a1488c6e321570238084b38948f6db and the ordinary
native supported installations. It does not inherit the Sampler constructor
family. Build output saves the observed compiler/installation/selection decision.
Unsupported or ambiguous wrappers/flags/targets remain Unknown. This is requested
layout qualification, never allocator attestation, RSS or usable allocation size.

Read-only primary source inspected at the pinned commit:

- [boxed.rs](https://github.com/rust-lang/rust/blob/4a4ef493e3a1488c6e321570238084b38948f6db/library/alloc/src/boxed.rs):239–265 documents the box_new intrinsic and Box::new;2254 has the Box unsizing implementation. SHA25625d54ba919797977110ff0e5b4c4c4cec15c42ecfae3e37f6d8ed0c6fc13fe65.
- [alloc.rs](https://github.com/rust-lang/rust/blob/4a4ef493e3a1488c6e321570238084b38948f6db/library/alloc/src/alloc.rs):484–493 passes the supplied size/alignment layout to Global.allocate. SHA25603fac86ce9f1ec183926f7ba9c5e8d8c8f3c90eeea79686de04f34ed67099585.

The visitor borrows the actual stored holder and reads Layout::for_value from its
private Box pointee. It charges exactly that TypedJob body (inline T, function
pointer, padding). There is no Box header or allocated vtable. Holder/Option/fat
pointer storage belongs to the actual parent/table backing once. A repeated visit
to the same physical job deduplicates; independently allocated equal jobs remain
distinct. The ledger borrow prevents moving/replacing/destroying the field while
its observations are live. No pointer/address identity is exported as a lease.

Only this outer allocation is qualified. Payload backings, shared owners and all
other roots must be traversed by the concrete Session adapter. A preexisting raw
Arc child remains Unknown; fresh outer construction cannot upgrade it. Body limit
refusal precedes taking the pending source. Its body component quote does not
cover input backing, predecessor jobs, child temporaries or the complete old/new
construction peak; those remain the joint preparation obligation under unchanged
caps. The first usable Sampler graph remains closed until that proof is complete.

Failed steps keep the actual holder available to control accounting without
reopening UI reads. Consumed final-Drop failures cannot be inspected or retried;
the exact slot and its body-component reservation remain unresolved/charged,
alongside separately required child/family reservations. The scalar reservation is
not a claim about physically freed/retained bytes. Only actual successful final
Drop permits vacancy and clears that component reservation. Panic payload storage
and partial children never become known merely because the outer holder was known.

## Nonowning transport route

The fresh job constructor mints a checked, never-reused opaque route before
consuming its source. The route stays in the holder; the Box payload layout is
unchanged. Identity exhaustion preserves the pending source. Holder/token fields
are inline parent charges, not new allocations or a payload qualification.

A route hint is stored beside each actual per-family guard in the same
capacity-accounted table backing. Registration installs the actual holder under
its writer before publishing the hint. Successful disposal clears the hint with
the vacancy transition. Failed/consumed records remain charged. Lookup checks the
actual holder route, active phase and concrete payload under the guard; a stale
hint cannot dispatch to a replacement. Neither routing nor downcast establishes
resource provenance.

The Session read capability contains only that route, one private reserved read
stamp and static operations. The private Desktop adapter revalidates the actual
registered Session family and exact read before binding. There is no Clone;
derivation reserves a distinct read and can refuse. Scoped continuations are
stack-local and expose only `SnapshotRead`. Moves/discarded Events do not enqueue
an owner or require lifecycle readiness. Canonical/device-preview/tuning-preview
read slots drop independently with the Event, including unpainted returns.

Durable release takes the selected outer shared guard and then the records
shared guard. It can wait for a current writer; it is not wait-free and must not
run under a corresponding same-thread writer. It takes no native barrier or
service-wide callback lock. Audited production release/report operations return
errors without panic; a failed ACK is reported and does not claim retirement,
release reservation bytes, or retry an owning destructor. Generic public hooks
remain unqualified and cannot certify arbitrary callbacks or children.

This transport slice does not yet replace the legacy owning Event/ViewState and
UI frame/canvas fields, wire worker publication to a fully inventoried admitted
root, quote the full joint old/new/temporary peak, or establish atomic publication
and saturated whole-root retirement. Those paths remain closed for referenced
registry processing. Protocol tests using actual Events/registered service hooks
are not CPAL, GUI or app-delivery acceptance.
