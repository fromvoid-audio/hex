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
remain. Icons may be reused. Default Icon clears the assignment; folder is also
an explicit bundled choice. Home and
Music retain their default folder icons. The existing editor context menu owns
focus, pointer occlusion, arrow navigation, Enter, Escape and outside-click dismissal;
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

## row-anchored icon menu — pulse2826 / PR430

The added Place row's right-click menu anchors to the row's lower-left corner
(clamped inside the window), regardless of the pointer's position in the row.
Default Icon resets the assignment above a compact four-column grid of the seven
existing bundled glyphs. Current choice has a persistent theme accent outline;
hover and keyboard selection have separate feedback, and each glyph has its
original named tooltip. Left/Right move one choice, Up/Down move grid rows (or
Default), Enter chooses, Escape/outside-click dismiss and restore prior focus.
Assignments still capture the clicked native path, never the active row index or
previously selected source. No preview/import/query requests are emitted.

This follows the supplied screenshot's reset/grid composition using Hex's own
assets. Additional Places are customizable; built-in Home/Music, Library categories
and Collections do not have a persisted custom-icon identity in the current
model. No new all-source identity/persistence format is claimed here. Existing
Place context actions are icon choices; no Rename/Remove/Show in Finder commands
are invented. Source input tests cover two same-named paths, reset, expanded/compact
rows, anchor geometry, actual mouse and keyboard choices, focus/dismissal and
actual preference-save/editor-reopen. Physical native appearance, accessibility
announcement and app close/reopen acceptance remain coordinator-owned and open.

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

## Built-in source menus and reversible sidebar removal — Pulse2844 / PR446

Home and Music use the existing row-anchored bundled icon grid and Default Icon
reset. Their saved identities are `home` and `music`, independent of the resolved
native Home path. Unknown icon IDs survive saves and show the default folder
with a bounded diagnostic. Custom Place assignments remain keyed by original
native paths; paths, casing and same-name identities are unchanged.

Remove on Home, Music, Samples or a custom Place hides that sidebar entry only.
The source paths remain in Places and the indexed catalog roots, and no files,
project media or history entries are deleted. Removing the active source stops
its preview and selects visible All. Existing source settings expose checkboxes
to restore every hidden Place and Samples; All remains reachable with Command-F.
Visibility and built-in icon overrides use the existing bounded browser document
and background latest-snapshot save lane. Malformed settings remain untouched.
Other section icon cycling and the broader collections work in2672 remain separate.

All is the first Library row in both expanded and compact modes, retaining stable
internal section IDs. The existing accepted Command-F handler is reused unchanged:
it reveals All, preserves/selects the query, finalizes valid edits and respects
invalid numeric drafts and IME handling. It adds no recursive indexing.

Source tests exercise actual built-in right-click/grid/reset, compact glyph
centers, actual Remove/restore controls, preference reopen and preserved WAV bytes,
catalog roots/entries and request queues. Native optimized appearance, user menu
navigation and actual app close/reopen remain root-owned acceptance gaps.
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

## repeated text double-click selection — pulse2817 / PR423

First query focus click and Command-F still select all. In focused editable text,
a double-click on a full selection collapses to the clicked caret; the next
selects all again. Partial/collapsed selections double-click to select all. The
local gesture remembers selection before the first click, so its ordinary caret
placement cannot erase the toggle's starting state. Native 1/2 and rapid 3/4 click
pairs share that policy; no timer/global mouse hook is added. Single clicks,
Shift selection, held drag and release keep their usual semantics. Keyboard,
text/IME replacement and focus entry invalidate stale gesture snapshots; worker
completion/paint does not. Selection alone changes no query, preview or project.

Shared name fields preserve original Unicode/raw paths and the presentation-only
lowercase mapping; clipboard/IME ranges remain source UTF-16. Already edited
numeric text uses the same toggle without reformatting its draft or emitting a
value edit. Knob reset and nonediting numeric adjustment remain separate gestures.
Rendered tests exercise actual down/up click counts, partial/drag/release,
filesystem worker/tick/resize updates, Unicode composition and empty text. Native
physical mouse/IME/accessibility acceptance remains coordinator-owned and open.

## selected-track sample input — pulse2821 / PR427

Enter on a selected sample in browser navigation inserts into the selected regular
Arrangement TrackId at the current edit/playback cursor. It refuses visibly when
no valid track is selected, the Arrangement is busy, or Session is active; it never
falls back to a hovered, default or new track. Enter while editing the query,
including IME composition, does not insert a sample. Explicit drag/drop remains
independent, and result double-click still does not import.

The compact grid icon in the browser toolbar toggles quantized sample input. It
starts off for each editor and survives browser close/reopen and source changes.
When enabled, input snaps to the **nearest** active Arrangement grid line using
the same tempo, meter and adaptive zoom resolver as Arrangement snapping. A disabled
grid or stale/unavailable timeline geometry causes a visible refusal. With input
quantization off, the original frame position is retained.

Invocation captures the native sample path, selected TrackId, resolved frame,
document generation and revision before worker preparation. Selection/cursor/grid
changes cannot retarget that request. The existing bounded audio batch controller
handles cancellation, stale contexts, errors and one canonical overwrite/history
commit; decode and graph preparation stay off the UI/audio callback. Same-track
source fragments remain intact outside the inserted interval, and one Undo restores
the preceding clip content (ID allocation remains monotonic). This Enter route
reuses the canonical overwrite entrypoint; global no-overlap coverage across other
entrypoints remains issue2822/PR428. Optimized native input/audio acceptance remains
separate from headless input and real-worker checks.
### invalidated text double-click pairs

A text/keyboard/IME update, Shift selection or drag invalidates the saved first
click. If the platform subsequently sends an even click count, the shared policy
returns no toggle and the field uses its normal caret/selection path. It never
substitutes current selection for the discarded snapshot. Fresh rapid1/2 and3/4
pairs still toggle from their own pre-gesture state. No text/query/value event,
formatter, numeric knob reset or geometry policy changes.

New bounded actual-caller regressions cover keyboard, text, IME, Shift and drag
invalidation followed by click2, then a fresh3/4 pair. The shared pure policy
covers both prior/current full-selection states. Original browser/name/numeric
and physical input evidence remains source-bound; native click-count/IME and
clipboard acceptance is separate from headless source checks.

The new PR423 toggle fixture now precreates one existing test Environment and
its samples/WAV before Service/browser startup, then injects that same Arc into
the editor. Actual browser/home/root, all four preference paths and the catalog
path correlate with that owner. A real catalog-save gate proves the same root
survives view/caller retirement, saves successfully, then cleans up without
recreation. No import or new PendingSetup is needed for this browser-only fixture;
no production input policy changes. Original query/IME/double-pair/value-event
assertions remain byte-identical. Older fixtures are not converted by this fix.

### keyboard row-action eligibility

Left/Right operate only on a highlighted browser result. Clicking a source/Place
or pressing Escape retains navigation focus but clears the highlight; subsequent
Right cannot audition an implicit first sample or expand an implicit folder, and
Left cannot collapse one. Up/Down still enter and navigate results through the
existing selection path. A highlighted sample's Right audition and highlighted
folder Left/Right remain available, as do explicit row clicks and manual preview.
The browser key context and search/Command-F text ownership are unchanged.

Pulse2858/PR460 promotes the verified five failures from both main2683942b and
original423 into normal rendered regressions. They use actual routed keys,
private filesystem workers and canonical Service Init; refused row actions leave
canonical metadata/history and project requests unchanged. Selected-row controls
and Up/Down-to-Right plus Command-F typing remain positive cases. This is source
coverage, not physical keyboard/audio-output or native delivery acceptance.

Quantized sample input refuses every disabled Arrangement grid, including a
remembered fine, medium or coarse density. Disabling input quantization keeps
the exact invocation frame even when the Arrangement grid is disabled. Active
density presets continue to use the same nearest-grid resolver. Integration
controls exercise actual Enter, no queued import/history mutation on refusal,
real worker commits and Undo, retaining the original six caller cases.

Picking a previously hidden known Home, Music or custom folder restores its
visibility through the existing source-settings setter before selecting it.
If loading or read-only preferences refuse restoration, the picker leaves the
current visible source selected. Cancel/error remains a no-op on source visibility;
Samples keeps its existing picker-selection convention. Paths, icons, media and
catalog roots remain intact. Actual injected picker-result tests cover restore,
preferences reopen and refusals; native panel behavior remains separate.
