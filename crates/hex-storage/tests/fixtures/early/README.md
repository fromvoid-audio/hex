# early saved-project fixture provenance

The repository's initial source, `89674b362cc2234cdc359fdd353618c707878848`,
contains the public schema13 encoder and frozen strict readers for 1–12. These
forms of evidence are deliberately distinct:

- `schema1-reader` through `schema8-reader` follow the original
  `legacy_schemas_migrate_and_schema_twelve_roundtrips_complete_state` construction:
  start with the actual schema13 wire shape, retain only that version's original DTO
  fields, and validate with that exact original reader. Values exercise nonidentity
  source conversion from 2 onward, solo from 3, enabled loop from 4, balance/master
  from 5, nonzero fades from 6, retained source phase from 7, and tempo/meter from 8.
  Schema1 retains its original identity-frame contract. No absent writer is claimed.
- `schema9-reader` through `schema12-reader` are the original literal test inputs
  from `schema_nine_literal_migrates_midi_source_to_strict_one_shot_defaults`,
  `schema_ten_migrates_to_an_empty_session_grid`,
  `literal_schema_eleven_migrates_session_midi_and_stop_cells`, and
  `literal_schema_twelve_migrates_session_audio_cell_and_asset_window`.
- Each `schemaN-oracle13` is independently emitted by the original 89674b36 public
  encoder after its original reader opens `schemaN-reader`. The original source
  asserts normalization encode/decode equality. Current tests compare complete
  migrated projects against these oracles rather than duplicating defaults.
- All other JSON documents are emitted by each named original public encoder.
  The only explicit older writer used is the actual `encode_schema22` on
  60c88034, whose source also has a newer default writer. The original source
  asserts complete snapshot equality after reopen and byte-identical re-encode.

Original source crates were exported by `git archive` into isolated workspaces.
Only the top-level workspace member list and an example harness were added;
original Rust sources were compared byte-for-byte with their Git blobs afterward.
`provenance.json` pins every full source identity, fixture/harness hashes, source
aggregate hash, command, and original validation. The source aggregate hashes
sorted repository-relative `.rs` paths, each followed by NUL and its exact bytes.
Harnesses, reduced workspace manifests and raw logs remain in
`artifacts/early-migration-evidence/`; the shared historical target is
`artifacts/migration-evidence/target`. No native app or audio device was launched.

Where supported, public commands create Arrangement and Session MIDI notes,
opaque effect/instrument/Return payloads `[0,255,42]`, host trims, nonunity gain,
musical source/pitch/gain edits, automation, touch overlays, names/appearance,
events and fade curves. Sources 60c88034 and 3c7c786b explicitly refuse nonempty
recorded touch spans, so their valid files retain the complete empty-touch
inventory; later public sources supply nonempty touch evidence. The schema29 sample-BPM
fixture enables original Warp before setting 87 BPM, as its public contract requires.
Empty schema20/21 documents verify that complete inline-audio envelopes remain identifiable
without clips. Existing `../lineages` fixtures supply main19 and foundation19–22
with nontrivial provenance, Return routing, automation and host gains.

The adapter preserves saved semantics, not later capabilities absent from those
writers. Legacy default sine instruments use the original later migration contract:
Pre-schema15 tracks receive one new version1 sine state with empty payload, IDs allocated
from saved `next_id` in track order and instrument position 0; explicit silence in schema15+
is retained. Old gain points have no explicit incoming seam and old touch origins
start at saved placement. Old musical audio has no placement/projection/offsets.
Current incompatible features are refused by retained explicit older writers;
unsupported or mixed historical fields are never silently discarded.

`schema20-session-audio-b9274930` and `schema21-session-audio-a9da374e` separately
exercise the inline Session-audio gain path: `Session Audio Case`, source window
17..143 and-9350millidB survive relocation into the complete current audio-state
inventory. Both original DTOs save gain only for Session audio;21's Arrangement
musical/pitch edit does not create Session processing metadata. Original snapshot
and byte roundtrips and unchanged-source hashes are in `session_audio_writers`
in the manifest.

| fixture | source | wire lineage |
| --- | --- | --- |
| `schema13-hybrid-89674b36.json` | `89674b362cc2234cdc359fdd353618c707878848` | hybrid |
| `schema14-effects-9efed7e1.json` | `9efed7e1c22934c21d756700d40baf0bf8d27602` | effects |
| `schema15-instrument-a5de1baa.json` | `a5de1baab3264b9b5fa90fe073fbe8aa1ee37b5f` | instrument |
| `schema16-position-65410c68.json` | `65410c68807e37d2be15f64a21f996cca3a3d7b7` | position |
| `schema17-returns-583bfdc1.json` | `583bfdc15f12c140c0733b6d44547c02aeb0e959` | returns |
| `schema18-provenance-173482a3.json` | `173482a373d8938ad5820f3774bb013470a30776` | provenance |
| `schema18-output-dc6c050a.json` | `dc6c050a611f5c446adf8172250a32e509309367` | output |
| `schema20-inline-gain-b9274930.json` | `b92749302883e971d899add3aeb3788f0d11ebc6` | inline-gain |
| `schema21-inline-edit-a9da374e.json` | `a9da374e2babc5a9540bd540b34bbad6ff6a3a95` | inline-edit |
| `schema22-touch-60c88034.json` | `60c8803427b3f61cec8c46fd96db724d785a856c` | touch |
| `schema23-two-trim-c3076a24.json` | `c3076a2418b6bb6ac5cad3ee927a919996d2ba0b` | two-trim |
| `schema23-combined-3c7c786b.json` | `3c7c786b5c3841f7c2d28feec434dc3284dd7aed` | combined |
| `schema24-routes-f7c73dd0.json` | `f7c73dd0a6b54fa79bc28706815fccd22989d4ed` | routes |
| `schema25-audio-e9736fce.json` | `e9736fcee2cb33db294bdc0f0db3420eb7aeaa4f` | audio |
| `schema26-names-9f381a6f.json` | `9f381a6fec1d51095588172a522f7aaa425bf5c6` | names |
| `schema26-events-309d41ae.json` | `309d41ae541d70a3dfbf2669e1ff2454cfe5ddfb` | events |
| `schema27-appearance-e409a8f2.json` | `e409a8f2a44ff3e1ea993ba21bcd9216a1e32364` | appearance |
| `schema27-named-events-d8922776.json` | `d8922776fbff142e0484bbf782ad9559a0cea142` | named-events |
| `schema27-named-fades-0298bf88.json` | `0298bf88e39414509fb0bddd99e04187e76e0a60` | named-fades |
| `schema28-appearance-events-819a4cc4.json` | `819a4cc485ffd6a3037730a5e34f34566986b484` | appearance-events |
| `schema28-appearance-fades-e50ffea3.json` | `e50ffea38895cbdfe0ac9fc9b89110fabbe2417e` | appearance-fades |
| `schema29-combined-caed5306.json` | `caed5306deb782b0e537ea4cf22c40a5ac5662b3` | combined |
| `schema29-sample-bpm-a0cedd1d.json` | `a0cedd1d4ef457d431108c26de956c732b9c06e8` | sample-bpm |
| `schema20-empty-b9274930.json` | `b92749302883e971d899add3aeb3788f0d11ebc6` | empty inline audio |
| `schema21-empty-a9da374e.json` | `a9da374e2babc5a9540bd540b34bbad6ff6a3a95` | empty inline audio |
