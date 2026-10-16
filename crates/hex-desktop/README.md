# hex-desktop

## Purpose

Compose the audio backend and GPUI shell; own executable startup and app-level failure handling.

## Dependencies

hex-audio, hex-ui, hex-midi, and GPUI application lifecycle. Desktop keeps the
optional controller worker alive through the application run loop and injects
its handle into UI; it joins only after the native application ends.

## Does not own

Views, rendering algorithms, audio callbacks, file codecs, or an editable project model.

Read ../../AGENTS.md and ../../docs/architecture.md before extending this crate.

Session logging is injected by desktop through `hex-diagnostics`; only that
crate's worker owns log file I/O. See [session diagnostics](../../docs/diagnostics.md).
