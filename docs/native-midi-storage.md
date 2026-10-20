# Canonical MIDI storage observations

Pulse2783 decision21260 approves a narrow borrowed model-owner family. It does
not approve generic Arc provenance or resource registry admission. The current
resource registry gate stays closed pending whole-graph accounting, actual host
custody, construction peak, atomic publication and saturated reaper proof.

`MidiClip`, `SessionMidiClip` and `MidiSourceIdentity` expose lifetime-bound
`MidiSourceStorageObservation` values from their actual owning fields. There is
no public view constructor, raw Arc ingress, owning clone, Weak, mutable access
or persistent address token. Source-allocation equality and physical-field
equality are separate operations. A plain `&MidiSource` supplies neither field
identity nor ownership. Observations cannot survive mutation/movement of their
field. Other live fields can still clone; count closure does not freeze roots.

## Canonical constructor and COW invariant

The sealed family is a maintained **crate-wide invariant**, including crate-visible
clip fields. All `hex-project/src` Rust sources were searched for source fields,
Arc constructors, COW/get_mut and replacement. Supported paths are global sized
`Arc::new(MidiSource)`, clone of an existing canonical field, and sized
`Arc::make_mut`/`get_mut`. No caller-provided Arc, custom allocator, raw pointer,
unsized conversion, cyclic construction, Weak or public Arc replacement was found.

| Owning boundary | Audited paths |
| --- | --- |
| `midi.rs` | placement, overwrite fragments and source edits construct sized Arc; split/COW and public DerefMut use make_mut; identity clones a canonical field |
| `session.rs` | creation, Arrangement-to-Session copies and scene duplication construct sized Arc; cell copy/edits use make_mut; fresh scene copy uses get_mut |
| `snapshot.rs` | both Arrangement and Session restoration validate owned note values and create fresh sized Arc; no caller Arc is deserialized |
| `clip_conversion.rs` | Arrangement and Session audio-to-MIDI create fresh default sized sources |
| `time_range.rs`, `time_range/insert.rs` | materialized paste/shift notes enter fresh sized Arc |
| `clip_copy.rs` | canonical clip clone followed by make_mut for fresh note IDs |
| `lib.rs::midi_source_mut` | Arrangement and Session lookup use make_mut |
| `history.rs` | actual Project snapshots clone canonical Arc fields; borrowed traversal visits current, Undo and Redo without cloning identities |

Unique mutation retains the outer allocation and may replace notes backing.
Shared COW creates a fresh outer body and cloned notes; old identities/history
retain their actual old storage. Public replacement of a borrowed `MidiSource`
value can replace its notes Vec, so capacities are observed anew, never cached
across mutation. Adding a construction path outside this closed family requires
refusal or a new qualified design; observations are not an escape for raw Arcs.

## Native requested-layout qualification

MIDI has a distinct constructor family and environment selector:
`canonical-midi-sized-arc-new-make-mut-v1`, `build_support/midi_gate.rs` and
`HEX_MIDI_ARC_*`. It shares compiler/install/wrapper rejection with the existing
pinned standard-library selector, **not** the PCM family label. It only declares
ordinary native Rust1.94.0, commit
`4a4ef493e3a1488c6e321570238084b38948f6db`, on x86_64 Linux GNU or aarch64 macOS.
Unsupported compiler, target, wrappers, flags, installation or missing evidence
refuse as Unknown; Clippy is intentionally an unsupported-wrapper build.

The independently read primary source is
[pinned alloc/src/sync.rs](https://github.com/rust-lang/rust/blob/4a4ef493e3a1488c6e321570238084b38948f6db/library/alloc/src/sync.rs),
SHA256 `ce15d00d92133cf1f55cd1905d7fe6b4e23e62090dabff6c68fae2d7fe78fe8f`.
`ArcInner` (383) has repr(C,align(2)), two atomic usize fields, then the body.
`Arc::new` (419) allocates `Box<ArcInner<T>>`. `make_mut` (2492) preserves a
unique allocation; its shared branch uses `clone_from_ref_in` (1495),
`UniqueArcUninit::new` (4233), `allocate_for_layout` (2155) and
`arcinner_layout_for_value_layout` (395). Both paths therefore request the
two-atomic header extended by the fixed sized body, padded to alignment. The
Weak branch is not part of the model family and any observed Weak refuses.
`get_mut` creates no allocation. This is a pinned requested-layout declaration,
not allocator RSS, a universal std ABI, native execution or security attestation.

## Selected ledger and unfinished obligations

The ledger uses distinct canonical source and physical-field domains. Each
qualified physical source charges its Arc body (including the embedded Vec
header) once, plus actual notes **capacity** times note element size, including
empty reserved backing. Equal contents in different sources never deduplicate.
Every actual owning field is registered; duplicate direct visits refuse. Cached
prepared-root traversal deduplicates a repeated physical field, not independent
owning aliases. Inline Arc handles are charged only by their enclosing object
visitor; these selected source visitors do not add another header.

Actual project/history and prepared Session/routed/mutable Arrangement identity
fields can be borrowed without synthetic owners. The selected APIs intentionally
exclude enclosing project/history/clip/compiled container storage. They do not
claim to cover an unvisited clipboard, candidate, pending or unreclaimed root.
The eventual whole-root visitor must include all of those actual custodies and
their enclosing capacities. An entirely omitted source can leave remaining
handle counts closed: independent expected-source/allocation oracles are required.
Construction peak must separately account old/new overlap and preparation
temporaries; current observations cannot reconstruct an earlier peak. Actual
callback, publication and reaper tests remain separate obligations.
