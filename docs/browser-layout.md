# Split browser visual composition

17 October 2026: the native GPUI browser follows the user-supplied Live screenshots:
search and navigation slots at the top, collections/library/places in a left
column, tag groups above results, selected-item tags and preview below results,
and an optional tag editor column to the right. The browser starts at 480 px and
retains its existing resize/collapse behavior and workspace minimum. At narrow
widths the sidebar becomes an icon rail. All paint uses the existing semantic
theme and captions use the bundled typography/lowercase display boundary.

Panel visibility and tag-group disclosure work. Section settings allow hiding
categories (at least one remains visible) and cycling six icons for visual review.
Those settings are session-only. Existing folder places still persist through
hex-preferences; their add/folder-picker/status behavior remains available.
Sample selection shows a real waveform from the existing bounded background
preview cache, with loading/error/unsupported states and an audition control.
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
4. Resize to the narrowest/widest browser widths, collapse/reopen the browser,
   and check result widths, text clipping and the retained timeline workspace.
5. Toggle section visibility and cycle icons; confirm the final category cannot
   be hidden and clearly distinguish session settings from persistent places.

Automated GPUI geometry/input checks verify the split and toggles; they do not
establish native GPU pixels, macOS glyph rendering, accessibility or listening.
