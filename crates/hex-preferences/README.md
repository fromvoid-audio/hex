# hex-preferences

## Purpose

Persist small, versioned application preferences independent of any one project:
user-added browser folders, recently opened project paths and four RGB theme roles.

## Dependencies

serde and serde_json encode the bounded versioned document; std handles native
path representations, bounded reads, and atomic same-directory publication.

## Does not own

Project documents or history, browser listings, UI layout/navigation, audio
state, plugin state, or synchronous filesystem work on the UI thread.

Browser places are retained in order, including folders that are currently
missing. Recently opened projects use a separate document and retain missing
paths so users can see and diagnose moved projects.
UTF-8 paths use portable strings; non-UTF-8 paths use native Unix bytes or
Windows UTF-16 code units and produce an explicit error on another platform.
Each document is capped at 64 KiB, 4 KiB per path and 8 KiB of combined native
path data. Browser places are capped at 14; recent projects are capped at 10,
most recent first. Saves use a synced temporary file in the destination
directory and atomically rename it into place. Each feature uses its own file so
independent background saves cannot overwrite one another.

Theme colors use a separate versioned `theme.json` document. Channels are bounded
eight-bit values; contrast and live preview belong to the UI. Malformed/future
settings are preserved and refused rather than overwritten.

Browser Places also retain optional icon IDs keyed by exact native Place paths.
At most one bounded, nonempty, control-free ID (64 UTF-8 bytes) is accepted per
additional Place. Unknown IDs roundtrip unchanged; glyph inventory, defaults and
fallback presentation belong to the UI. Orphan/duplicate icon records are refused.

Browser preferences also contain at most three stable built-in source records
(`home`, `music`, `samples`) with visibility and optional bounded icon IDs, plus
at most fourteen hidden additional native paths referencing existing Places.
Duplicate/unknown built-in identities and orphan hidden paths are refused.
Sidebar visibility never removes a source path or touches its contents.
