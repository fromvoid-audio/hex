# hex-export

## Purpose

Render a compiled linear arrangement to stereo PCM16 WAV and publish it as a new file.

## Dependencies

hex-engine and the Rust standard library.

## Does not own

Project/session editing, job scheduling, progress UI, device I/O, overwrite export, or alternate audio formats.

The output is staged in an exclusive same-directory temporary file, synchronized,
then linked to the requested destination without replacing an existing file.
