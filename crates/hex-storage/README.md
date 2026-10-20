# hex-storage

All new saves use one strict schema33 envelope requiring project tuning,
the complete device-asset manifest (including empty lists), an explicit
`main_devices` list (including an empty Main), and an explicit `placement` field on each audio edit (`null` for ordinary placement). Every device
has one bounded manifest entry with a required nullable A/B comparison and
active/inactive asset bindings. Superseded alpha schemas30/31/32 refuse explicitly; there is
no tuning-only/asset-only compatibility dispatch. Legacy writers and asset-unaware presets explicitly refuse these
references. Storage preserves neutral identities and resolves them through the
project model, without decoding device payloads or loading PCM. See
[device assets](../../docs/device-assets.md).

## Purpose

Versioned native project metadata serialization, create-new saves, and recoverable
replacement of an existing project. Only validated canonical project snapshots
enter or leave this boundary.

The independent `preset` module also owns schema-2 native device-preset envelopes,
bounded opaque payload persistence and create-new publication. It does not change
project schema versions or interpret device codecs. Session provider validation,
ownership and prepared admission govern applying a preset. See
[device presets](../../docs/device-presets.md).

## Dependencies

hex-project for model validation, hex-tuning for validated scale/audition values,
hex-lattice-contract for bounded device state,
serde/serde_json for versioned project JSON,
fs2 0.4.3 for nonblocking control-side advisory file locks (Rust1.85-compatible),
libc 0.2.190 on Unix for nonblocking/no-follow open constants only (no workspace FFI),
and std for bounded file access and same-directory publication.

## Does not own

PCM decoding/streaming, audio callbacks, editing/history, GPUI, Live-set import,
device-specific codecs, plugin hosting or fields not introduced into the model.

Schema16 adds a required nullable generic instrument slot and required nullable
effect-gap position per hybrid track. Schemas1–14 install the explicit sine
default at the front using fresh validated IDs; schema15 instruments migrate to
the front because their format had no position, while schema15 null remains
intentionally silent. Unknown instrument envelopes are preserved, and adding an
instrument type needs no storage branch. See
[instrument slots](../../docs/lattice-instruments.md).

Schema17 requires ordered return-track records and a sparse sends array on each
regular track. Return effect envelopes use the same strict generic device DTO as
regular effects. Schemas1–16 decode with empty return and send collections;
return IDs, device IDs, send references, names, gain bounds, count limits and
allocator high-water are validated by `hex-project`. See
[return tracks](../../docs/return-tracks.md).

Schema 14 adds ordered per-track native device envelopes: stable instance ID,
bypass, namespaced type ID, state version and bounded opaque bytes. Schemas 1–13
migrate to empty chains. Storage preserves unknown types and future device-state
versions without interpreting payloads; registered codecs and playback
availability belong to control-side compilation. Adding a native device type
requires no storage branch. Known document fields remain strict.

Schema 12 adds independently owned Session audio one-shots with asset IDs and
validated half-open project-rate PCM source bounds. Schema 13 makes each track
own both audio and MIDI clip arrays. Schemas 1–12 retain their original strict
readers and migrate into this hybrid model; schema 11 preserves Session MIDI/Stop
cells, and schemas 1–10 retain an empty Session grid. The current schema persists
authoritative tempo and meter points, separate audio and MIDI track content,
MIDI source length/start, retained loop markers and enable state,
independent Arrangement duration, note IDs and Q32.32 timing, plus Session scenes
and sparse typed cells. Session MIDI cells store their own clip ID/name/source
and notes; stop cells persist explicitly, while absent cells remain absent. Scene,
Session clip and note IDs share the project-wide high-water mark and validation
space. Schemas 1–12 keep their original single-content track DTOs and strict
Session tags during decode. Schemas 1–10 migrate to an empty Session grid. The
current document preserves looping audio clip source
phase, the required nullable loop region, track solo and balance, project
master gain, source sample rate/frame count, prepared playback frame count and
conversion version. It also preserves the ID high-water mark, original path/name
casing, track order/gain/mute, complete clip regions and fixed-domain fades.
Strict schema-9 MIDI tracks retain their required `one_shot` tag and migrate each
clip to source length equal to its arrangement length, source start zero, a full
source loop marker and looping disabled. The schema-9 decoder remains strict.
Strict schema-1 loading migrates old frame counts to identity metadata at the
project rate. Schema-1/2 records migrate to soloed=false; schema-1/2/3 records
migrate to no loop region; schema-1/2/3/4 records migrate to center balance and
unity master gain. Schemas 1–5 migrate clips to neutral fades anchored at each
clip's duration. Schemas 1–6 migrate clip source phase to zero. Schemas 1–7
migrate tempo and meter maps to 120 BPM and 4/4. Schema 7 requires the phase
field and validates it against the clip's source interval. Schemas 8 and 9 require both
sorted maps and validates their values and point limits; derived tempo prefixes
are rebuilt from the project sample rate on load. Unknown or missing fields,
conversion versions and document versions are rejected rather than discarded.
Non-UTF-8 paths cannot be stored in this JSON schema and produce an explicit
error. Relative paths are interpreted by callers relative to the project file
directory.

`save_new` never overwrites an existing file. It flushes/syncs a same-directory
exclusive temporary file, then atomically links that complete file to the target
and syncs the directory on Unix. An interrupted preparation can leave a hidden
`.hex-pending` file; a completed target contains the full document. Known
destination collisions (including dangling symlinks) are refused before
temporary-file work; the hard link is still the atomic create-new check.
`save_new_with_token` returns a token for the exact serialized bytes only after
publication cleanup and directory durability succeed; post-publication errors
retain the existing error contract and do not return a token.

`StorageError::PublishedButNotDurable` identifies failures after a complete
create-new destination was published. The destination is retained; cleanup and
directory sync are both attempted. Injected postpublication failure tests
establish this contract, but real power-loss and disk-failure acceptance remain
unverified.

`load_with_token` returns the validated project and an opaque shared `FileToken`
captured from the same bounded read. `FileToken::check_unchanged` compares exact
bytes without writing, preserving original formatting and legacy-schema bytes
instead of comparing a migrated re-encoding. Tokens retain at most 8 MiB and
their debug output exposes only a byte count. Byte-identical replacement is
accepted; formatting-only changes conflict. Oversized reads and missing files
produce errors.

`replace_existing` requires that token and refuses a changed target, target
symlink, or unrecognized recovery sidecar. It prepares a synced new document and
an exact copy of the old document before publishing a target-bound recovery
envelope at the hidden `.<target>.hex-recovery` sidecar. The envelope has strict
magic/version fields, a UTF-8 basename binding and bounded exact prior JSON.
`load_recovery` validates the sidecar and returns both the recovered project and
its exact content token. A later replacement rolls the recognized sidecar
atomically, retaining only the newest prior generation. Unknown, malformed,
oversized, symlink and other-target sidecars are preserved and cause refusal.

Replacement rechecks exact target bytes immediately before same-directory atomic
rename. This is optimistic conflict detection, not a filesystem compare-and-swap;
a noncooperating writer can still race the final check. A successful result
carries the new token and recovery path. If directory synchronization fails
after rename, the result also carries a warning while identifying the published
version. If recovery publication succeeds but project replacement fails, the old
target remains intact and the error identifies the valid duplicate recovery
copy. Basic file permissions are copied to both new files; a permission change
during staging refuses the replacement. std does not preserve ACLs or extended
attributes. Sidecar names must fit a 255-byte
filesystem component; long names that would exceed that limit are refused
before publication.

The bounded project-level import provenance envelope is independent of
native device state: exact original input bytes (1 MiB maximum), source kind and
version, and adapter version. Clones/snapshots/history share immutable evidence;
normal edits retain it unchanged. Debug output excludes contents and private
paths. This is unchanged-input pass-through, not editable Ableton export or
proof of valid/importable Live XML. See [import provenance](../../docs/live-set-import.md).

Schema 18 requires nullable `import_provenance`. Schemas 1–17 migrate to absent
provenance; their strict readers reject the new field. Unknown envelope kinds or
versions, empty input and oversized arrays are rejected. Schema-18 JSON is compact
and retains the 8 MiB document ceiling, including provenance. Serialization stops
at that ceiling; saving never drops evidence to fit. The envelope does not
interpret the source format or provide a musical importer.
Automatic journal checkpoints are independent of previous-save recovery. The
strict journal envelope retains one current snapshot, uses a stable generation
lock to serialize cooperating publication, refuses unknown files, and clears
only the exact owned generation. No untitled history is retained. Native
composition, ownership and tested limits: [project recovery](../../docs/project-recovery.md).

Unsaved projects use that same latest-untitled checkpoint path. Recovery
retains original-document provenance and prepares the complete media and playback
candidate before adoption, without acquiring an overwrite association.

New saves use schema33: required Main devices, audio-edit placement and complete device assets with
active/inactive A/B bindings, project tuning and audition settings, schema28 audio
events, schema27 inherited track colors/per-clip flavors,
schema26 names, required fade-shape metadata and device host gains,
exact provenance, typed audio/MIDI routing, Return feeds, gain points/touch spans
and recording epochs with independent Arrangement clip names. All playback
consumers share the admitted lane model. The
ordinary writer does not select formats based on which fields happen to be empty.

Historical versions18 and19 have two recorded shapes from independent branches.
Decoding selects an envelope by top-level key presence, then applies its strict
reader: main's legacy envelope has neither `import_provenance` nor `routing`;
branch18 has provenance only; branch19 has both. A null or malformed present
field still selects that reader. Inconsistent shapes and unknown/duplicate
fields refuse; decoding never retries a different format after an error.
Schemas20–22 retain explicit readers and their bounded migration contracts.
The explicit schema20/22 writers preserve supported host trims through the shared
device DTO; their output requires this integrated reader for those trims. They
still refuse state they cannot represent instead of discarding it.

See [routing](../../docs/routing.md),
[track gain automation](../../docs/track-gain-automation.md) and
[gain touch metadata](../../docs/gain-touch-spans.md). This persistence integration
owns metadata preservation; audible routing, recording and latency compensation
remain engine/audio/session responsibilities.

Schema24 adds required bounded `audio_feeds` and `midi_routes` arrays to the
complete schema23 envelope. Each audio source is explicitly tagged as Track or
Return; destinations and MIDI endpoints use canonical stable IDs. The strict
reader rejects unknown/duplicate fields, unsupported ports, invalid gain/identity,
cycles and oversized lists before admitting a Project. Schemas1–23 migrate with
empty typed inputs; schema23 retains host trims, provenance, Return feeds and
gain-touch metadata. Explicit schema20/22 writers refuse playable routing rather
than discarding it. Historical schema18/19 shape dispatch remains unchanged.


Schema25 retains the complete routed schema24 envelope and adds required bounded
`audio_clip_state`: exactly one ID-keyed entry for every Arrangement and Session
audio clip, with validated integer `gain_millidb` and required-nullable
`audio_edit`. Session edits must be null. Arrangement state preserves pitch,
beat placement, root/crop, all six warp settings/markers, source gain envelopes
and frozen fades. Missing/duplicate/unknown bindings, malformed state and unknown
fields refuse admission. Historical nested DTOs remain unchanged and strict;
PCM, analysis and caches are not serialized. Explicit schema20/22 writers refuse
nonunity clip gain or any retained edit instead of dropping it.

Unpublished issue27 development schemas20/21 are selected only when both root
`import_provenance` and `routing` keys are absent; exactly one key is invalid.
Their strict reader requires gain on every audio clip, requires `audio_edit` on
every Arrangement clip in21 and forbids it in20. No-audio envelopes are rejected
as ambiguous. Root keys both present select only main's historical20/21 reader,
including when null/malformed; errors never retry another format.

Schema28 preserves the appearance/clip-name envelope plus a strict, complete
audio-event table. Historical schemas keep their explicit readers and implicit
single-event migration; see [audio events](../../docs/audio-events.md).

Historical schema26, adding complete validated Arrangement clip names to
the schema25 musical-audio envelope. Session names retain their existing fields.
Schemas1–25 migrate Arrangement names to `clip`; explicit legacy writers refuse
authored Arrangement names. See [clip names](../../docs/clip-names.md).


Schema29 adds the bounded `fade_shapes` table for every Arrangement audio clip.
Each ID occurs exactly once and carries the lower-handle offsets and integer
curve bends. The existing fade domain remains in its historical DTO or retained
musical root; the table augments that domain before final canonical validation.
Schemas25–28 migrate to edge-aligned linear defaults and reject the new table;
schema29 rejects missing/null, duplicate, orphan, malformed or oversized entries.
Explicit historical encoders refuse shaped envelopes. Named clips retain their
complete schema26 metadata. See [clip fades](../../docs/clip-fades.md).

Schema30 adds required `tuning` with enable state, a validated repeating scale
(3–48 active degrees, all 48 stored cents, root key/frequency/degree and period),
and separate audition waveform/level/envelope settings. Unknown or duplicate
fields, missing/null tuning, invalid values and malformed degree arrays refuse
admission. Schemas1–29 retain strict readers and explicitly migrate to disabled
A440 12-tone equal temperament; they reject the new tuning field. Explicit
schema20/22 writers refuse any nondefault tuning or audition settings rather
than discard them. Local library preset names and MTS-ESP process ownership are
not saved in this envelope. Runtime tuning and audible audition remain separate
engine/audio/session responsibilities.
