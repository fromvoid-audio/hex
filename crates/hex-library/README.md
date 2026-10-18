# hex-library

## Purpose

Provide bounded, deterministic filesystem listings and a persistent recursive sample catalog for the native media browser.

## Dependencies

Uses only the Rust standard library. The UI owns navigation and presentation; this crate owns directory enumeration, supported-file filtering, ordering, listing limits, recursive WAV discovery, and atomic publication of a versioned derived sample database. All filesystem operations run on control workers. See [sample library](../../docs/sample-library.md) for scan/cache bounds and reconciliation.

## Does not own

Does not own GPUI, audio playback, project state, media decoding, preview playback, or persistent library places.
