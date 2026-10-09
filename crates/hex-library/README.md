# hex-library

## Purpose

Provide bounded, deterministic filesystem listings for the native media browser.

## Dependencies

Uses only the Rust standard library. The UI owns navigation and presentation; this crate owns directory enumeration, supported-file filtering, ordering, and listing limits.

## Does not own

Does not own GPUI, audio playback, project state, recursive indexing, media decoding, preview playback, or persistent library places.
