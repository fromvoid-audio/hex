# hex-export

## Purpose

Render a compiled linear arrangement or prepared range to stereo PCM16 WAV,
including canonical track/Return capture and separately named stems.
Prepared revisions render through the shared SessionPlayback owner and preserve
the captured revision rather than recompiling a project clone. Prepared mutable
Arrangement export through the legacy entry point refuses nonzero starts.
The controlled entry point replays required instrument history from zero within
the existing bounded preroll limit, with progress and cancellation. Mutable
Arrangement source loops remain refused. Unused Session loop inventory does not
wrap this linear export.

## Dependencies

hex-engine and the Rust standard library, plus pinned rustix 1.1.5 safe filesystem
operations on Linux/macOS for exclusive directory publication and file exchange.
Tests may use canonical
project/lattice contracts and the non-default `instrument-test-support` feature
to verify mutable prepared rendering; this does not add production dependencies.

## Does not own

Project/session editing, job scheduling, progress UI, device I/O, or alternate audio formats.

The output is staged in an exclusive same-directory temporary file, synchronized,
then linked to the requested destination without replacing an existing file.
Explicit overwrite preserves the displaced original at a reported recovery path.
See [the export contract](../../docs/export.md) for scopes, consent, race handling,
platform limits and the remaining acceptance gates.
