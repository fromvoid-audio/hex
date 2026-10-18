# resize release across panels

Pulse2846 / PR448 fixes a rendered browser-release regression without changing
canonical clip/source/Warp edits or the single pending-trim/FIFO protocol.

Previously GPUI's resize move listener ran even outside its originating element.
Moving a valid audio resize over the browser interpreted browser x as timeline
x and changed an 80,750-frame preview to one frame. Only Arrangement lanes had a
typed drop handler, so browser release then reached root cancellation without
submitting a resize. The real worker retained the original 96,000-frame clip.
Those numbers describe the bounded Linux test fixture, not a physical user build.

Each existing audio/MIDI trim payload now retains its last successfully projected
timeline point alongside the actual pointer. Foreign horizontal coordinates stop
edge-pan and retain the displayed preview; they do not become timeline positions.
Modifier-only updates use the actual point while inside the lane and the retained
point outside it, only while a valid preview remains. Invalid previews cannot
silently resurrect an older valid preview. Temporary preview borrows end before
mutating the preview.

The root mouse-up capture resolves the actual hovered Arrangement track through
current rendered lane bounds/epochs. A different track cancels through canonical
drop validation. Only the browser panel's own hit-tested capture supplies the
explicit browser exception, finishing the owned resize before a browser child can
consume release. Other outside locations retain cancellation. Both paths use the
existing canonical audio/MIDI drop functions and retire the GPUI drag. Release
over the timeline still uses its authoritative event coordinate. Generation/revision/clip/asset,
viewport/lane, focus, Arrangement context, source/minimum limits and queue admission
remain checked. Escape, stale state, foreign focus, view switches and refusals
cancel honestly. Accepted display ownership transfers to the same PendingTrim;
no additional request key, counter, terminal owner or synthetic commit is added.

The focused suite exercises actual rendered handles and picker/audition children,
normal and Shift audio on both edges, MIDI on both edges, fast entry/release,
held browser pan stop, current modifiers including stationary Command changes,
real offline Service acknowledgments and one Undo/Redo. It also retains adjacent
pan, audio-event focus and terminal/FIFO checks. All private browser/sample/recent/
theme fixture paths are bounded before worker startup; no OS audio adapter or
user media is used. A temporary-borrow regression found during implementation
was repaired with its failed test logs retained.

The original outside-cancellation fixture released at (464,214), inside the actual
browser bounds (0,168), size484×890. Its original failure logs remain evidence.
After the coordinator's explicit contract decision, the cancellation fixture now
uses the actual upper rail outside both rendered browser and Arrangement body,
asserting those exclusions before release. Its no-edit/preview-cleanup assertions
remain unchanged. A separate actual audio/MIDI release at the original browser
point verifies commit, worker acknowledgment and one Undo/Redo. Other-track audio
and MIDI refusals are real before→after regressions. Ordinary MIDI admission from
detail/browser focus already passed before repair; explicit edge-down editor focus
acquisition matches Audio and is defensive hardening, not an invented failing case.

These checks establish component behavior, not physical pointer/GPU/audio or
installed-app acceptance. The root-owned native selector
`editor::trim::resize_release_tests::browser_` now runs exactly nine unique tests
with default native stack. All nine use the private bounded constructor before
workers; the older generic clip-drag fixture is not part of this native selector.
The sole merger owns final prospective-tree full CI, review and merge; source
readiness remains In Progress until merged. No app operation is part of this work.
