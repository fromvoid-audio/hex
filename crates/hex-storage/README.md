# hex-storage

## Purpose

Versioned native project metadata serialization, create-new saves, and recoverable
replacement of an existing project. Only validated canonical project snapshots
enter or leave this boundary.

## Dependencies

hex-project for model validation, serde/serde_json for versioned project JSON,
and std for bounded file access and same-directory publication.

## Does not own

PCM decoding/streaming, audio callbacks, editing/history, GPUI, Live-set import,
plugin state or any fields that have not yet been introduced into the model.

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
