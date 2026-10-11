# live-set import — ticket 32 design

Status: persistence boundary approved and implemented in source; musical mapping
and genuine-fixture acceptance pending.
Owning workflow: Pulse #32, draft PR #150, branch
`pulse/task-32-g1-d9d724561bea`. This document records the primary's 11 October
2026 scope decision; it does not establish Live compatibility.

## finite scope

Import a minimal user-authored Live set into one new canonical Hex candidate.
Do not export edited Hex projects as Ableton sets. Preserve exact original input
for unchanged-input pass-through and diagnostic inspection. Returning those bytes
never reflects subsequent Hex edits.

The initial musical subset is constant tempo and meter; ordered ordinary audio
and MIDI tracks; names and representable gain, pan and mute; Arrangement placement,
duration and source offset; local audio references; unwarped one-shot audio; and
simple MIDI pitch, onset, duration and velocity. Actual XML fields, versions,
units, defaults and timing laws must be established from user-authored fixtures
and primary reference evidence before implementing mappings.

| input concept | canonical destination | evidence still required |
| --- | --- | --- |
| constant tempo / meter | bounded tempo and meter maps beginning at beat zero | exact fields, units, defaults, constant detection |
| ordered ordinary tracks / names | hybrid track order and original name casing | track hierarchy, identity, ordinary versus special tracks |
| gain / pan / mute | validated linear gain, Hex balance and mute | scaling, domains, pan law; Hex balance is not verified Live pan |
| Arrangement audio placement / duration / offset | project-rate frame regions and asset references | beat/second/frame anchoring, rounding, source offset law |
| local audio references | validated source/prepared media metadata | path resolution, source identity and file-reference structure |
| MIDI notes and finite clip region | unsigned Q32.32 beats and velocity 1..127 | note units, offsets, gates, fractional velocity and silent-note handling |

No XML tag mapping is asserted by this table. In particular, Live pan and MIDI
velocity must not be guessed or silently clamped into a different behavior.
Devices/plugins, automation, complex routing, warping, unsupported looping,
Session behavior and unsupported timing laws require explicit diagnostics and
refusal whenever omission would change sound. Do not insert Hex's default sine
instrument as an unnoticed substitute for a Live instrument. Unknown nonmusical
metadata may be retained without interpreting it.

## bounded persistence extension

Source inspection at `bd0e3c9c19de4cb435ad304c79decd80b27a6075` found no
project-level opaque interchange metadata. Native device envelopes are scoped to
devices and must not be repurposed. `hex-storage` schema 17 is strict.

The primary approved one optional project-owned import provenance envelope with a versioned
source kind (`ableton-live-set`), provenance version 1, adapter version and exact
original file bytes in a shared immutable allocation. Cap original bytes at
1 MiB, decoded XML at 4 MiB, XML nesting at 64 and elements at 100,000. These are
initial safety limits for minimal fixtures, not measured capacity promises.
Reject excess before publication; never truncate provenance. Debug output must
show only kind/version/byte counts, not bytes or private paths. Diagnostic paths
refer to XML structure, not local filesystem paths.

Schema 18 adds a required nullable provenance field, preserving strict
schema-17 decoding and migrating schemas 1–17 to absent provenance. Encode bytes
as a JSON byte array initially to avoid a separate codec dependency, retaining
the existing 8 MiB total document cap on encode and decode. Save may explicitly
refuse an oversized combined document; it must not drop provenance to fit.
Snapshots/history retain the shared envelope, validation checks its bounds and
version, and normal edits do not rewrite or remove it. New empty projects have
none. Future/unknown provenance versions are rejected explicitly in this first
extension. Existing schema-17 readers reject schema 18 rather than discarding data.

The separate compatibility adapter would own gzip/XML interpretation, source
field mapping and diagnostics. `hex-project` owns only the neutral bounded
provenance value and canonical candidate; `hex-storage` persists it without XML
interpretation. The primary approved this extension before persistence code. Schema-18 output
uses compact JSON so maximum-size byte arrays fit within the document ceiling;
serialization stops before output exceeds 8 MiB.

## parsing and publication safety

Read original bytes with a cap before allocation growth. Bound gzip output
independently and reject truncated/corrupt streams and ambiguous extra payloads.
Reject DTD/entity declarations and external entities; parsing performs no network
or external file expansion. Bound tree depth, nodes, attributes/text and diagnostic
count. Unknown metadata remains available through retained original bytes.

Resolve only fixture-evidenced local asset paths against the set directory;
reject remote references and path forms whose semantics are unsupported. Define
traversal/symlink handling explicitly with real fixture evidence rather than
rewriting paths. Decode media through existing bounded hex-media operations and
validate source/prepared counts, canonical IDs, timing and compilation before
publishing. Import failure leaves the current project/history untouched.
Publication uses existing create-new/recoverable storage semantics. No plugin
loading or remote asset retrieval occurs.

## missing evidence and acceptance

No `.als` files were found in the accessible `/workspace` source/environment.
The task branch contains only its Pulse plan beyond the workflow base. Synthetic
XML may test parser safety later, but cannot establish musical field mapping.

Primary must provide or route a Mac fixture task for minimal user-authored sets:
constant tempo/meter, named ordinary tracks with neutral and nonneutral mixer
values, one unwarped local original audio one-shot with placement/offset, and a
finite MIDI clip with known notes. Include authored values, Live build identity,
source/media hashes, ownership/provenance and expected timing. Unsupported-case
variants should cover devices, automation, warp, loops and Session data; avoid
proprietary assets. Unknown harmless metadata can be introduced into an evidenced
minimal document to test preservation without claiming reference semantics.

Required source tests: canonical import values and refusal without publication;
original plain/gzip byte preservation; schema-18 save/reopen/history retention;
strict older-schema migrations; unknown/unsupported cases; malformed XML/gzip,
DTD/entities, limits and asset errors. Run focused tests and full local CI on the
exact final source. Native Live/macOS, reference audio, physical listening and user
acceptance remain independent. No full-fidelity claim follows successful parsing.
