# Split browser visual composition

17 October 2026: the native GPUI browser follows the user-supplied Live screenshots:
search and navigation slots at the top, collections/library/places in a left
column, tag groups above results, selected-item tags and preview below results,
and an optional tag editor column to the right. The browser starts at 480 px and
retains its outer resize/collapse behavior. Opening the tag editor adds a column
to the right of that base width, preserving the picker, results and preview widths;
closing it returns that space to Arrangement/Session. The existing workspace
minimum applies to the base browser, with the editor borrowing additional workspace
space while open. Workspace/drop geometry includes the extra column.

The picker has an independent 4 px drag divider. Its session-only width starts at
124 px, leaves at least 120 px for results, and resizes continuously to a 23 px
minimum: the existing 22 px browser-control hit/centering slot plus the sidebar's
existing 1 px right border. Collection, Library and Place glyphs share that fixed
leading slot at every width. Labels hide below 64 px without moving the icon column;
drag right to restore them. Full-row hit targets remain at least 22 px wide/high.
In icon mode, collections, sections and folder places retain name tooltips; folder
save/error status remains available by hover.
Section settings expand the picker when opened. All paint uses the existing semantic
theme and captions use the bundled typography/lowercase display boundary.

Panel visibility and tag-group disclosure work. Section settings allow hiding
categories (at least one remains visible) and cycling six icons for visual review.
Those settings are session-only. Existing folder places still persist through
hex-preferences; their add/folder-picker/status behavior remains available.
Sample selection shows a real waveform from the existing bounded background
preview cache. Below the tag strip, a single 22 px row contains automatic
audition (the `a` toggle, initially on), manual Play/Stop, and a clipped miniature
waveform. Stereo channel extrema share one neutral envelope without cancelling
opposite-phase peaks. Loading/errors remain bounded single-line placeholders.
Clicking or moving Up/Down to a new sample auditions it when automatic audition
is on; manual Play and Right remain independent. Repeating navigation at a list
boundary cannot retrigger the same selection. Search/category/source resets stop
obsolete requests. The controls preserve result focus and never start project
transport. Stop invalidates decode and queued/active preview PCM; actual callback
acknowledgements drive playing/finished state, with repaint only on transitions.
There is no preview loop control or automatic replay. Native audio/visual
acceptance is separate from headless layout and deterministic PCM tests.
No fabricated waveform or sample tags are used. File import, project opening,
device/sample dragging and current search semantics retain their existing paths.

Tag chips/checkboxes, the mini add field and history arrows are disabled placeholders.
Collection names can select their own empty source; membership and editing remain
explicitly pending. Labels/tooltips identify pending
functionality. Disclosure, filtering-panel visibility and opening/closing the
full editor are interactive; tag assignment and persistence are not implemented.

Pulse owns the remaining plans (Hex 0.1):

- #2664: waveform metadata, transport/playhead and preview interactions.
- #2665: shared bounded tag taxonomy, item identity and persistent assignments.
- #2666: top tag filtering and combined query semantics; depends on #2665.
- #2667: mini/full editors, suggestions, mixed states and atomic writes; depends on #2665.
- #2669: bounded back/forward navigation history and restoration.
- #2672: collections, icon picker and sidebar preference persistence; depends on #2665.

The visual implementation is #2678 / PR294. Global Command-F work remains #2659.
These follow-ups are planning records, not implemented features.

The Samples item now uses the [indexed sample library](sample-library.md): its
results combine recursively discovered WAVs across configured roots, with source
filters and refresh in place of folder breadcrumbs. The top search, sidebar,
tag placeholders and bottom preview retain this composition. Projects continues
using folder navigation.

## Native visual/input checklist

User-owned macOS acceptance remains pending. Review the actual native window:

1. Search, switch samples/devices/projects, open places and navigate folders;
   confirm results and bottom item identity change together.
2. Hide/show tag filters, expand groups, open/close the right tag editor and use
   the mini strip's editor button. Verify independently scrolling regions.
3. Select and audition WAV files, including an unreadable file; confirm real
   waveform/loading/error states and preserve right-arrow audition and dragging.
4. Drag the picker's divider left to icons only; hover collection, library and
   place icons for names, select a section/place, and drag right to restore labels.
   Resize the outer browser at narrow/wide window sizes, collapse/reopen it, and
   verify results stay usable. Open the editor from both top and mini-strip controls:
   results/preview must retain their width, with workspace shrinking to its right.
   Close the editor and verify the space returns; test resizing while it is open.
5. Toggle section visibility and cycle icons; confirm the final category cannot
   be hidden and clearly distinguish session settings from persistent places.

Automated GPUI geometry/input checks verify the split and toggles; they do not
establish native GPU pixels, macOS glyph rendering, accessibility or listening.


Preview-cache ownership (Pulse2778 / PR384): composing visible result rows only
reads existing metadata and cannot insert or evict cache entries. Actual preview
requests own the bounded16-entry cache; the selected waveform and in-flight/latest
pending requests are protected from eviction. Cached failures are stable on hover;
deliberate drag preparation retains its existing refresh path. This removes a
render/evict/reprepare/notify cycle that could flash preparing and repaint the
whole editor. Headless input checks retain canonical project identity, surrounding
lane geometry/background and selected metadata across hover/selection. Native
temporal paint acceptance remains separate; no generic GPUI patch is inferred.


Browser waveform clicks (Pulse2779 / PR385) are manual audition, independent of
automatic result-selection audition. The hit is normalized to finite clipped
bounds; background decoding resolves it once to an absolute frame in the actual
output-rate preview PCM, avoiding stale display metadata/rate conversions. Exact
end is valid and finishes silently; invalid/zero-width geometry cannot dispatch.
Publication moves the existing PCM owner with a starting cursor; there is no
slicing/copying, new stream or callback seek command. Generation cancellation
covers repeated clicks, selection and Stop. A one-pixel position marker follows
actual callback frame acknowledgements; pointer moves cannot start playback.
Progress invalidation occurs only when the phase or visible pixel changes. The
lower clip-editor waveform retains its own interaction contracts.


Sample-result double-click (Pulse2780 / PR386) selects/auditions according to the
automatic audition preference and consumes its second-click bubble. It cannot
import into a track/Session cell or create history. Enter and deliberate drag/drop
remain explicit import routes; device, project and directory double-click retain
their own handlers. Manual waveform/Play/Stop controls are independent.

## Single active source — Pulse2727 / PR340

One session source value selects a Library category, an original-path Place root,
or one of the existing collection placeholders. Library, Collection and Place
highlights and result dispatch read that same value; keyboard result focus and
hover do not create additional selected sources. Choosing a source retains the
text query, resets its result viewport/selection, invalidates older folder requests
and cancels obsolete audition. No tag assignment, collection storage/history or
shared-library connection is added; collection results explicitly say assignments
pending instead of showing another source's contents.

A Place owns its root even while navigating a child folder; only that Place is
highlighted. Without a query it shows its bounded supported folder contents.
With a query it combines indexed WAV matches within that root and supported
current-folder matches, with original-path deduplication. Samples remains the
all-roots indexed catalog; its folder chips select the corresponding Place rather
than retaining an independent filter. Command-F chooses All and preserves indexed
global sample/device/folder search. Adding a root from Samples still indexes it
without changing the active source. Plain folder snapshots are cached and invalidated
on accepted publication, so unchanged pointer/repaint activity does not rebuild rows.

A selected unavailable Place stays selected with an explicit error and no stale
indexed results; it does not silently substitute Home. If a selected Place is
removed from the place model, fall back explicitly to Samples and prune derived
catalog records outside the remaining roots before rescan. Added places keep their
existing bounded persistence; no user files are changed by source selection.
Inline hierarchy, collections/tag editing and navigation history remain their
separate owning issues. Native normal/icon picker, focus and offline-source
acceptance is coordinator-owned and pending.

## picker alignment follow-up — pulse2706 / PR322

Headless GPUI geometry reproduced the old 63-to-36 px snap and a 1 px glyph-center
offset from adjacent browser controls. The shared slot centers the actual glyph
layout boxes, retains row heights and removes the width snap. Tests exercise all
three source-row types at minimum, transition and normal widths, plus full-row
clicks, hover without selection/audition/preparation, search focus, keyboard Down,
selected paint and drag lifecycle. Tooltip names/handlers remain unchanged by source
inspection; their physical native display and optical glyph appearance remain
coordinator-owned optimized acceptance, alongside smooth native divider dragging.
No tree indentation, results, preview, persisted preferences or divider feedback
behavior is implemented by this follow-up.
