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
User owns native restart/folder-picker checks. Deferred: folder removal/reorder and unrelated preferences. Recursive sample indexing is now specified in [sample library](sample-library.md); preview playback is implemented separately.


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
across the indexed WAV sample catalog, supported entries in the current bounded
folder, and the registered native device catalog. Folder/device names match
case-insensitively; indexed samples use the sample library's multi-term path match.
All ignores the Samples source-folder filter and deduplicates original WAV paths
across the catalog and folder, retaining folder WAVs outside indexed roots. Samples, devices and projects remain available
as category filters. Repeating Command-F from search/results replaces the query;
Escape clears the query while retaining its current text or navigation owner;
it never re-enters text editing from results. Category clicks retain the query
and move actual focus to browser navigation, including in Session. Active IME composition is unmarked before selecting the whole
query. Moving the caret or text selection leaves the result viewport unchanged;
only a changed query resets it. Clicking the search field from another control selects the entire query so the
next typed text replaces it. Once already focused, clicks place the caret and
held pointer drags select substrings. A deliberate drag on the initial focus
click overrides Select All from its down-point; release, worker acknowledgments
and repaint never reselect the whole query. Returning from result navigation to
the text field is an explicit text-mode entry even though they share a FocusHandle.
Text selection/caret movement does not move the result viewport. Device and file highlights both require result
focus. Open context/controller menus are dismissed; leaving theme
settings cancels its unsaved preview (an in-flight save must finish first).
A clip-name draft commits using the existing blur behavior before search takes focus.
An unfinished gain numeric draft keeps its focus and value: finish it with Enter
or cancel with Escape before invoking Command-F. Native-device and return-send
numeric drafts pass through the existing finalization barrier: valid values commit
before focus moves; invalid values preserve their draft and focus for repair.
Device rows retain drag-to-track/rail behavior; Enter on an
all-view device displays that instruction rather than importing a file. Clicking a
device keeps browser navigation focus in Session too. The first arrow key selects
the first match; subsequent arrows move through results. Query/category changes
reveal the first row. Accepted background scans preserve the selected file path
or device identity when new files change row offsets.

Project/folder listing remains nonrecursive, capped at 2048 retained entries and
8192 inspected entries. The separate [sample library](sample-library.md) recursively
indexes WAVs in Music and added places with its documented bounds. Filesystem work
stays on background workers. Combined results reuse the cached snapshot until the
query, source, catalog or accepted folder contents change; typing does not scan
folders. Both folder and catalog publication preserve a selected file/device by
identity. Search UI state is transient and does not dirty the project or write
preferences; the derived sample database persists separately.

Native user checklist: press Command-F from Arrangement, Session and a device
control with the browser collapsed; type a sample/project/device name; confirm
filtered results, repeat Command-F to replace the query, clear with Escape, and
drag a matched sample/device to its normal destination. Native macOS interaction
and reference parity remain unverified by cloud source tests.

## Bundled custom Place icons (Pulse #2710 / PR #326)

Right-click an added Place to choose folder, samples, devices, projects,
collection, waveform or search from Hex's existing browser glyph collection.
The same Ioskeley Mono glyphs and shared 22 px picker slot are used expanded and
icon-only; original folder-name hover tooltips and left-click source selection
remain. Icons may be reused. The folder choice clears the assignment. Home and
Music retain their default folder icons. The existing editor context menu owns
focus, pointer occlusion, Up/Down, Enter, Escape and outside-click dismissal;
choosing an icon does not select/import/audition a sample or create project history.

The bundled font's existing provenance is in
`assets/fonts/ioskeley-mono/source.json` (Ioskeley Mono v2.1.0, Normal/Unhinted)
and its `OFL.txt`. No additional artwork, asset package or dependency is introduced.
Native reference gesture parity is not established.

The current browser preference document includes optional `place_icons`, a list
of original native `path` / opaque `id` pairs for additional Places only. Missing
metadata means the folder default. At most 14 unique assignments are accepted;
IDs are nonempty, at most 64 UTF-8 bytes, and contain no control characters.
Duplicate/orphan identities and malformed data are refused without replacing the
file. Unknown bounded IDs are retained across unrelated saves and display the
folder fallback, with one load-time status message. Assignment identity is the
full original native path, independent of order and duplicate basenames; native
non-UTF-8 paths retain the existing lossless encoding. The existing 64 KiB document
and path/count bounds, atomic publication, one in-flight write and latest pending
snapshot apply. Loading/invalid preferences gate the picker; save failures remain
visible and preserve the original settings. Await saved feedback before closing.

Source tests cover actual pointer/keyboard choices, all seven glyphs at normal
and minimum width, focus/escape/outside-click handling, same-name path identity,
unknown fallback, stale removed-Place menus, coalesced saves, malformed-file
preservation, and recreating the editor through its preference load path. These
are source tests; optimized native picker/font rendering and normal app
close/reopen acceptance remain coordinator-owned and open.

Preference error details are limited to 160 characters after the operation prefix;
the sidebar keeps its normal single-line status height with ellipsis and a tooltip.
An explicit choice can retry the same unsaved icon after a write failure is repaired.
Final focused source validation: 19 preferences tests and 71 browser-related UI/
model tests pass; strict affected preferences/UI Clippy and architecture, GPUI,
style, formatting and diff checks pass. This is not prospective full-candidate CI.


## Search text selection (Pulse #2805 / PR #411)

Command-F always focuses/selects all, including when already focused. Mouse
entry selects the current query; subsequent normal clicks, held drags, Shift
selection, Command-A and clipboard operations retain normal text editing. A
focus click tolerates small pointer jitter; an intentional held drag uses the
initial down-point, can return to its anchor and releases safely outside the
field. Selection-only changes do not change the query or prepare sample audio.
Category changes, accepted filesystem work and resizing retain the selection.
Native composition keeps the existing UTF-16 conversion/case-normalization;
Command-F unmarks before replacing the query. Mouse hit positions and keyboard
boundary operations use complete Unicode graphemes (the existing locked
unicode-segmentation 1.13.3 package is now a direct UI dependency). The same
input primitive retains case-preserving clip/track-name mode; first-click Select
All is browser-only. No rendering/timer callback requests Select All.

Rendered test-platform regressions demonstrate original missing focus-click
Select All, missing drag selection and splitting a combining grapheme. This is
new source selection behavior, not proof of resolution or native acceptance for
separate mouse-focus issue #2794 / draft PR #400. Native mouse/clipboard/IME and
shortcut checks remain coordinator-owned; no app replacement occurs in cloud.

## explicit search entry — pulse2820 / PR426

Only the search field itself and Command-F enter query editing. Sources, Places,
folders, disclosures, sample/device rows, blank browser space, toolbar, preview
controls and resize handles use a distinct non-text navigation focus owner.
Down/Up from query results transfer to that owner; returning to the field selects
the query on entry and retains ordinary focused caret/drag selection thereafter.
Selection/highlight state is independent of the actual input handle. Pointer
handlers run in their local browser scope; painting/workers never restore focus.
Close returns focus to the editor. This is separate from the unresolved native
field-click focus-loss report2794 and the future double-click refinement2817.
Optimized native pointer/IME/keyboard acceptance remains coordinator-owned.

## compact picker headings and resize capture — pulse2841 / PR444

At the exact 23 px picker minimum, the section headings show small themed line
marks centered on the existing icon column. Their original single-line text boxes
remain reserved but do not paint; separators overlay those boxes. Collections,
Library, Places and open-settings rows therefore keep every source icon's existing
x/y position and row height. Expanded headings return in the same slots, and full
heading names remain available through hover tooltips. Item names keep progressive
single-line clipping; selectable Place icons and native path identities are unchanged.

Pointer resizing captures the minimum within one existing 4 px divider width.
Once captured, the same drag stays locked until it exceeds two divider widths
above minimum, preventing chatter near the threshold. A new drag starting at
minimum also uses this release band. Outside the band, ordinary pointer widths
remain continuous (subject to GPUI's existing device-pixel paint rounding). Mouse
release retains the captured minimum. The settings gear still expands deliberately
when opening settings, and closing already-open compact settings does not widen it.
This state is transient editor layout; no new persisted format or global width
quantization is introduced. Native physical drag/hover and accessibility inspection
remain coordinator-owned acceptance, separate from headless scene/input tests.
