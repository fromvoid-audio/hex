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

The existing root mouse-up capture finishes an owned resize through the existing
canonical audio/MIDI drop functions before a browser child can consume release.
It consumes that owned release and retires the GPUI drag. Release over the timeline
still uses its authoritative event coordinate. Generation/revision/clip/asset,
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

These checks establish component behavior, not physical pointer/GPU/audio or
installed-app acceptance. The root-owned native request runs exactly the four
`editor::trim::resize_release_tests::browser_` tests with default native stack.
The sole merger owns final prospective-tree full CI, review and merge; source
readiness remains In Progress until merged. No app operation is part of this work.
