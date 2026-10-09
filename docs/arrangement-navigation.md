# arrangement grid and pointer navigation

Implemented source, 9 October 2026; native acceptance pending.

The Arrangement grid uses musical time and adapts its visible subdivisions to
horizontal zoom. Draw only visible, bounded grid content behind clips, aligned
with the ruler and excluding right-hand track headers. Use subtle subdivisions
and stronger musical boundaries so clips and waveforms remain readable. Cover
empty Arrangement as well as existing tracks. The later snapping extension
in `grid-snapping.md` shares this visual spacing and enables adaptive snapping
by default, with Command temporarily inverting it.

Mouse wheel over Arrangement zooms horizontally around the pointer's timeline
position, within the existing viewport limits. Browser, device and piano-roll
wheel behavior remains owned by those panels. Middle-button drag pans time
horizontally and the virtualized track list vertically. Keep capture through
release outside the workspace, and clear gesture state on cancellation or
invalid project state. No authoritative project/history or audio changes.

## native review

1. Verify the grid is visible with no tracks and behind audio/MIDI clips. Zoom
   out and in: density should adapt without crowded lines or a blank timeline.
2. Wheel over a recognizable clip edge: that timeline position should remain
   under the pointer while scale changes. Try the left/right viewport limits.
   Wheel over the browser should keep its normal behavior.
3. With enough tracks to scroll, middle-button drag in all directions. Both
   axes should pan, stop at their limits and release cleanly outside Arrangement.
4. Confirm normal clip selection, drag/drop and edge trims still work; grid and
   navigation must not change the project or create Undo steps.

Source and rendered tests establish deterministic geometry/event behavior;
physical mouse/trackpad feel remains a native user check.

## verification and review

216 UI tests pass, one existing ignored; strict UI Clippy, desktop build,
architecture/style guards, fmt and diff checks pass. New tests cover grid density
and bounded invalid cases, empty-grid/ruler alignment, nonzero ruler origin and
pointer anchoring, one-line wheel detents in both directions, browser/horizontal
input isolation, small/clamped wheel preventing row scroll, middle-pan exact
frame mapping/vertical limits/held outside movement/release. Mode changes and
Escape cancel pan. Existing stale-lane drag regressions now use middle pan
because Arrangement wheel is reserved for zoom.

Grid work is bounded to 256 visible lines, computed once for the Arrangement
background. Wheel pixel movement accumulates in 80-pixel steps with at most four
steps per event; line input maps one line to one step. Zoom steps scale by 4/5
inward or 5/4 outward within the current timeline horizon. This does not change
project data, snap settings, schemas or audio. No measured native paint/input
timing or physical-device feel verdict is claimed.

Review: `artifacts/hex-arrangement-navigation-20261009.app`; source/binary hashes
in `artifacts/arrangement-navigation-manifest.json`, full UI evidence in
`artifacts/arrangement-navigation-ui-tests.log`. Prior drag/drop milestone is
pushed at `b873931930fef57f49b60cfc5005faa406b555f2`. Navigation source remains
uncommitted for review. Existing running apps/unsaved projects were preserved.

Latest extension: prospective appended-row clip previews replace floating
mouse ghosts over eligible blank Arrangement destinations; see
`grid-snapping.md`. The navigation build above predates this extension.

Snapping extension is now source-tested (220 UI tests/one ignored) in
`artifacts/hex-grid-snapping-20261009.app`; native review remains pending.


The Arrangement overview now fits all tracks and audio/MIDI clips above the
ruler, with cached compact drawing, viewport outline and click navigation.
See [arrangement-overview.md](arrangement-overview.md). Command preserves grid visibility; alternating
four-unit bands and the native input repair are in this review milestone.
Native visual and physical-input acceptance remain pending.
