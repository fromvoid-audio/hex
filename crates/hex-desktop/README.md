# hex-desktop

## Purpose

Compose the audio backend and GPUI shell; own executable startup and app-level failure handling.

## Dependencies

hex-audio, hex-ui, hex-midi, hex-mts, and GPUI application lifecycle. The native Session adapter
also uses hex-session's neutral worker sink/source snapshots, hex-engine's
consuming preparation contracts, hex-stream's verified constructor metadata, and hex-media's bounded reader-limit
values (the existing Session/stream workers perform all file decoding).
The nondefault `allocation-probe` test feature substitutes only output-open
completion through the actual native endpoint constructor. It also enables the
existing GPUI UI test support for a rendered browser-action -> real Service ->
CURRENT fixture with caller-owned temporary settings/discovery roots. No
physical app window or device is opened by that fixture. Its dev-only
hex-storage, hex-project, hex-tuning and allocation-counter dependencies exercise real saved-document/previous-save readback, canonical edits,
absolute tuning references
and zero-total/net callback allocations; none is a production desktop edge.
Desktop binds those existing responsibilities; Session gains no audio/stream
edge and Desktop owns no independent scheduler, source worker or editable model.

Desktop keeps the optional MTS publisher and controller workers alive through the application run
loop, injects its handle into UI, and joins them after the native application ends.

## Does not own

Views, rendering algorithms, audio callbacks, file codecs, or an editable project model.

Read ../../AGENTS.md and ../../docs/architecture.md before extending this crate.

Session logging is injected by desktop through `hex-diagnostics`; only that
crate's worker owns log file I/O. See [session diagnostics](../../docs/diagnostics.md).
