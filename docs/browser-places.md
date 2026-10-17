# Persistent browser places

User-requested next-build workflow, 9 October 2026; implemented and source-accepted; native restart acceptance pending.

Added browser folders are application preferences, separate from project schema,
editing history and audio state. A dedicated hex-preferences boundary owns
versioned/bounded serialization and recoverable file publication; hex-library
continues to own listings and UI owns presentation/navigation. Only added folder
places are persisted in this unit. Default home/music places are not duplicated.

Restore original paths/order/casing on startup; retain unavailable/disconnected
folders and report listing failures when selected rather than forgetting them.
Bound count, path lengths and file size, validate schema and duplicates, and
preserve malformed existing settings rather than silently replacing them.
Saving errors must be visible. Non-UTF8 paths must roundtrip losslessly or be
explicitly refused, never silently changed. Use a stable machine-user config
location shared by review bundle identifiers.

Load/save runs outside UI interaction and callback work; bound and order/coalesce
writes so an old snapshot cannot overwrite a newer addition. Tests use isolated
temporary settings files, never the user's real preferences. No project dirty
state or undo step for adding a browser place.

Acceptance: add folder, wait for acknowledged save, restart app, same place
present and usable. Test serialization/defaults/duplicates/limits, missing folder
retention, malformed/unwritable failures and background load/save coordination.
User owns native restart/folder-picker checks. Deferred: folder removal/reorder,
indexing, preview playback and unrelated preferences.


Implementation: schema-1 browser JSON, at most 14 additional places alongside
Music/Home defaults, 64 KiB document limit and bounded native path payloads.
UTF-8 paths and native Unix bytes/Windows UTF-16 units roundtrip; foreign native
encodings are explicitly refused. Same-directory create-new temporary publication
uses sync and rename. Save revalidates existing settings before replacement,
including corruption after startup. UI load/save runs on the background executor,
with one write in flight and one latest pending snapshot.

The browser displays saving/saved/error feedback. Wait for “browser places saved”
before restarting during the native acceptance check. Test settings paths are
isolated from the real machine configuration.


Checks: 8 preferences tests and 197 UI tests pass, one existing UI test ignored;
strict affected Clippy, desktop build, architecture/style guards, fmt and diff
checks pass. Review bundle: `artifacts/hex-browser-places-20261009.app`.

User acceptance: add a folder, wait for saved feedback, close/reopen this review
build and confirm the folder remains and opens. Retest audio/MIDI dragging,
cross-track preview and second pickup after the edit completes. Running earlier
apps and unsaved projects are preserved. No native acceptance claim yet.

## Global browser search

Command-F from the editor opens the browser, chooses the compact `all` category,
focuses its search field and selects the previous query. Typing filters immediately
by case-insensitive name across supported entries in the current folder and the
registered native device catalog. Samples, devices and projects remain available
as category filters. Repeating Command-F from search/results replaces the query;
Escape clears it. Open context/controller menus are dismissed; leaving theme
settings cancels its unsaved preview (an in-flight save must finish first).
Device rows retain drag-to-track/rail behavior; Enter on an
all-view device displays that instruction rather than importing a file.

The existing library listing remains nonrecursive and capped at 2048 enumerated
entries. This is not an index of every subfolder or saved place. Folder scans stay
on the background worker; typing only filters cached results. This UI state is
transient and does not dirty the project or write preferences.

Native user checklist: press Command-F from Arrangement, Session and a device
control with the browser collapsed; type a sample/project/device name; confirm
filtered results, repeat Command-F to replace the query, clear with Escape, and
drag a matched sample/device to its normal destination. Native macOS interaction
and reference parity remain unverified by cloud source tests.
