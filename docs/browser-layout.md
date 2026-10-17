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
124 px, leaves at least 120 px for results, and snaps to a 36 px icon rail below
64 px. Drag right to restore labels. In icon mode, collections, sections and folder
places retain name tooltips; folder save/error status remains available by hover.
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

This is a visual shell: tag chips/checkboxes, mini add field, collection entries
and history arrows are disabled placeholders. Labels/tooltips identify pending
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


Sample-result double-click (Pulse2780 / PR386) selects/auditions according to the
automatic audition preference and consumes its second-click bubble. It cannot
import into a track/Session cell or create history. Enter and deliberate drag/drop
remain explicit import routes; device, project and directory double-click retain
their own handlers. Manual waveform/Play/Stop controls are independent.
