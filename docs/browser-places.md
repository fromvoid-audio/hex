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

17 October 2026: places now occupy the split browser's left navigation column;
the same background persistence and add-folder paths remain in use. The new
section/icon settings are transient and do not modify this preferences schema.
See [browser composition](browser-layout.md) and Pulse #2672 for their persistence.

## Global browser search

Command-F from the editor opens the browser, chooses the `all` library section,
focuses its search field and selects the previous query. Typing filters immediately
by case-insensitive name across supported entries in the current folder and the
registered native device catalog. Samples, devices and projects remain available
as category filters. Repeating Command-F from search/results replaces the query;
Escape clears it and restores text mode even if the query was already empty.
Category clicks retain search focus, including in Session. Active IME composition is unmarked before selecting the whole
query. Moving the caret or text selection leaves the result viewport unchanged;
only a changed query resets it. Clicking the search field restores caret editing
without moving the result viewport. Device and file highlights both require result
focus. Open context/controller menus are dismissed; leaving theme
settings cancels its unsaved preview (an in-flight save must finish first).
A clip-name draft commits using the existing blur behavior before search takes focus.
An unfinished gain numeric draft keeps its focus and value: finish it with Enter
or cancel with Escape before invoking Command-F. Native-device and return-send
numeric drafts pass through the existing finalization barrier: valid values commit
before focus moves; invalid values preserve their draft and focus for repair.
Device rows retain drag-to-track/rail behavior; Enter on an
all-view device displays that instruction rather than importing a file. Clicking a
device keeps browser keyboard focus in Session too. The first arrow key selects
the first match; subsequent arrows move through results. Query/category changes
reveal the first row. Accepted background scans preserve the selected file path
or device identity when new files change row offsets.

The existing library listing remains nonrecursive and capped at 2048 enumerated
entries. This is not an index of every subfolder or saved place. Folder scans stay
on the background worker; typing only filters cached results. This UI state is
transient and does not dirty the project or write preferences.

Native user checklist: press Command-F from Arrangement, Session and a device
control with the browser collapsed; type a sample/project/device name; confirm
filtered results, repeat Command-F to replace the query, clear with Escape, and
drag a matched sample/device to its normal destination. Native macOS interaction
and reference parity remain unverified by cloud source tests.
