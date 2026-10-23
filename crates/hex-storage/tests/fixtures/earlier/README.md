# original schema30–33 fixture provenance

Each JSON project was emitted by its named historical public encoder and reopened
by that exact source. The original harness asserts complete snapshot equality and
byte-identical encode/decode/encode before publishing the fixture. These are not
current documents with their version number changed.

`provenance.json` records full source identities, fixture and harness SHA256 hashes,
and verified original Rust-source aggregates. An aggregate hashes sorted
repository-relative `.rs` paths, each followed by NUL and its original bytes.
Original sources were exported with `git archive`; only isolated workspace members
and an example harness were added. Original Rust sources were checked against
Git afterward. Full harnesses, per-file hashes and raw build/oracle logs remain in
`artifacts/earlier-migration-evidence/`. No media, native app or device was opened.

The initial five fixtures cover separate tuning/device-assets30, their31 union,
comparison32 and Main33. They contain trimmed/pitched frame and musical audio,
loop splits, fades/source gain, Arrangement/Session MIDI, automation/touch,
Return sends, opaque device payloads and host trims. Tuning, asset references,
comparison states and Main devices appear only where the original writer owned
them. Names and media-path case are intentional.

Additional original writers establish reused-version layouts:

| source | schema | additional evidenced layout |
| --- | --- | --- |
| `a0cedd1d4ef457d431108c26de956c732b9c06e8` | 30 | device assets and nullable sample tempo; nondefault60BPM |
| `1bab7b8473f3725128028bf377e523ad3aff929e` | 31 | combined tuning/assets with sample tempo |
| `5d6c5f73ce9da949d675d97f398511d5e4d65807` | 31 | explicit SourceEnd and LoopEnd MIDI policy variants |
| `4043b1faa78ab6b5ff807ab5e5bd5399783527f7` | 31 | audio placement together with explicit MIDI policy |
| `a9278234a03e90de0a59ebc43d81a0b008b4b03e` | 32 | placement, references-only device manifest, ordinary MIDI |
| `e09aa1ac2c8425c7d59f7a97265f9b5c56528490` | 32 | placement and device comparison |
| `13bd9fa3b3ba852a2f80cc9f2356f99bf8e9d4b1` | 32 | signed fade-projection origin at negative twice-u64 maximum |

Placement follows the original `audio_view` snapshot construction: repeat mode,
12000..36000 root crop,48000-frame outer span and6000-frame cycle phase.
Sample tempo uses original Warp setup before the SetAudioClipSampleBpm command.
The LoopEnd fixture follows the original MIDI toggle commands, retains the note
at10..12 beats despite a2..6 loop/audible boundary, and copies the source into
Session. Its authored notes must survive migration independently of playback.

The same5d6/4043 sources exposed explicit schema20/22 writers using their shared
MIDI DTOs. Four additional fixtures cover both policies in each explicit writer;
the4043 outputs are byte-identical. Each source independently reopens the saved
project and re-encodes it through the same explicit writer. These files require
the foundation envelope, both host trims and the original gain DTO shapes; mixing
their policy fields with inline audio, older trims or later gain seams refuses.
Their complete source/hash/oracle record is `explicit_midi_writers` in the manifest.
