# hex-cli

## Purpose

Provide headless demo generation, WAV/project import offline WAV export, and explicit prior-version project recovery.

## Dependencies

hex-engine, hex-project, hex-media, hex-storage, hex-export, and the Rust standard library.

## Does not own

GPUI, device streams, a second engine, or a second authoritative project model.
Shared PCM16 WAV rendering and create-new publication are owned by hex-export.

Read ../../AGENTS.md and ../../docs/architecture.md before extending this crate.

Project rendering validates original source rate/count, prepares project-rate PCM
with the saved converter contract and validates playback length before opening
the output. Input is capped at 64 MiB / 8,000,000 source frames per file; referenced
prepared PCM is separately capped at 8,000,000 frames. Output never overwrites.

Recover a retained prior generation without replacing either original file:

```sh
cargo run -p hex-cli -- recover-project /path/Session.hex /path/Recovered.hex
```

The target identifies its validated `.Session.hex.hex-recovery` sidecar; the
current target itself may be missing. Recovery writes only to an absent destination,
resolves relative media against the original target directory, preserves path case,
and leaves missing media references intact. It does not decode media, relink assets,
restore editor history, or provide automatic crash recovery. Invalid/unknown backups
and destination collisions are refused.

Saved instrument tracks render the original bounded sine instrument through
hex-engine. MIDI-only projects require no decoded media bank. Export length uses
the compiled audio/MIDI clip and release-tail horizon; unrenderable candidates
fail before creating output. Piano-roll authoring remains a native UI work item.

Import the finite Live-set subset into a new 48 kHz Hex project:

```sh
cargo run -p hex-cli -- import-live-set /path/minimal.als /path/new.hex
```

Nonneutral pan is refused by default. `--accept-hex-balance` explicitly selects
Hex's unverified stereo balance law and emits a diagnostic. Device-free MIDI
retains notes with no instrument, preserving silence. Local matching-rate WAVs
are validated and the complete candidate compiles before create-new publication;
existing outputs survive failure. Exact original ALS bytes persist in schema 18.
See [live-set import](../../docs/live-set-import.md) for the fixture/version field
table, resource bounds, refusal cases and native/listening gaps. No editable ALS
export or broad Live-fidelity claim is made.
