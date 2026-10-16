# hex-desktop

## Purpose

Compose the audio backend and GPUI shell; own executable startup and app-level failure handling.

## Dependencies

hex-audio, hex-ui, and GPUI application lifecycle. The native Session adapter
also uses hex-session's neutral worker sink/source snapshots, hex-engine's
consuming preparation contracts, hex-stream's verified constructor metadata, and hex-media's bounded reader-limit
values (the existing Session/stream workers perform all file decoding).
The nondefault `allocation-probe` test feature substitutes only output-open
completion through the actual native endpoint constructor. Its dev-only
hex-project and allocation-counter dependencies exercise real canonical edits
and zero-total/net callback allocations; neither is a production desktop edge.
Desktop binds those existing responsibilities; Session gains no audio/stream
edge and Desktop owns no independent scheduler, source worker or editable model.

## Does not own

Views, rendering algorithms, audio callbacks, file codecs, or an editable project model.

Read ../../AGENTS.md and ../../docs/architecture.md before extending this crate.

Session logging is injected by desktop through `hex-diagnostics`; only that
crate's worker owns log file I/O. See [session diagnostics](../../docs/diagnostics.md).
