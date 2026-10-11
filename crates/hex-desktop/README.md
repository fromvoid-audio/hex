# hex-desktop

## Purpose

Compose the audio backend and GPUI shell; own executable startup and app-level failure handling.

## Dependencies

hex-audio, hex-ui, and GPUI application lifecycle.

## Does not own

Views, rendering algorithms, audio callbacks, file codecs, or an editable project model.

Read ../../AGENTS.md and ../../docs/architecture.md before extending this crate.

Session logging is injected by desktop through `hex-diagnostics`; only that
crate's worker owns log file I/O. See [session diagnostics](../../docs/diagnostics.md).
