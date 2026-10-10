# hex-export

## Purpose

Render a compiled linear arrangement to stereo PCM16 WAV and publish it as a new file.
Prepared revisions render through the shared SessionPlayback owner and preserve
the captured revision rather than recompiling a project clone. Prepared mutable
Arrangement export is proof-only: start-zero ranges are supported when the
revision otherwise prepares, while nonzero starts and mutable Arrangement
source loops are refused. Unused Session loop inventory does not wrap this linear
export.

## Dependencies

hex-engine and the Rust standard library at runtime. Tests may use canonical
project/lattice contracts and the non-default `instrument-test-support` feature
to verify mutable prepared rendering; this does not add production dependencies.

## Does not own

Project/session editing, job scheduling, progress UI, device I/O, overwrite export, or alternate audio formats.

The output is staged in an exclusive same-directory temporary file, synchronized,
then linked to the requested destination without replacing an existing file.
