# Arrangement clip dragging

User-selected update, 9 October 2026: implemented; native user acceptance pending.

Audio and MIDI clips move within or between hybrid tracks through one
metadata-only native gesture. Audio start/duration remain project-rate frames;
MIDI start/length remain canonical beats. The UI preview resolves a typed
destination and its rendered frame bounds using the canonical tempo map.
Release uses the same destination resolver and dispatches one validated
MoveClip or MoveMidiClip command. No authoritative project edit during drag.

Inside a valid lane, only the landing preview is drawn in the target track at
the exact snapped destination. The next user-selected browser-drag update also
restores a cursor-attached floating preview outside lanes. It respects clip duration, viewport clipping and the
original mouse-down grab offset. No cursor-following floating ghost while a valid track landing preview is shown. A preview
is feedback about the proposed destination, not evidence of an applied edit.

Cancellation and invalid destinations must leave project/history untouched:
Escape, release outside a track lane, stale project generation/revision or
viewport/snap settings, missing source/destination, pending project work, and
invalid conversions/overflow. Preview cleanup must follow GPUI drag lifetime
without leaving stale hover graphics. Unchanged drops create no history step.

Deferred: new autoscroll, multi-clip moves, duplicate-on-drag, overlap-policy
changes and browser-to-timeline dragging. Edge trims and piano-roll note gestures
retain their existing contracts.

The preview is a compact translucent cyan landing rectangle; the original clip
stays visible until the edit is accepted. Source dragging shares one gesture
implementation, with typed positions and source-unit grab offsets. Current-paint generation
eligibility prevents offscreen rows from remaining eligible drag targets.
The GPUI floating element is zero-sized.

Targeted tests cover audio snap/free candidates, MIDI start/length across tempo
changes, command selection, original grab anchoring, lane boundaries and stale
source/project/viewport state. These are source/headless checks, not native
pointer acceptance. User checklist:

1. Drag an audio clip within its track and to another track, with snapping on/off;
   the landing rectangle should match the released position. Undo/redo once.
2. Repeat with MIDI, grabbing near the middle; confirm selection still works,
   notes/beat length survive and the clip can move to a track containing audio.
3. Move over a header/browser or outside lanes: no landing rectangle. Escape
   or release outside should leave the project unchanged.
4. Zoom/pan so a clip is partially cropped, then drag; confirm its grabbed
   position remains steady and landing agrees with release.

The user owns native checks while present. A separate review bundle preserves
running apps/unsaved work. Broader roadmap work remains paused.


Review build: `artifacts/hex-clip-drag-20261009.app`. UI suite: 192 passed, one
existing ignored. After preserving the snap-off exact-frame fast path, focused
musical-time 8/8 and drag 11/11 pass; strict UI Clippy, desktop build, fmt and
architecture/style guards pass. Native gesture acceptance remains pending.


Native report follow-up: per-batch UniformList pruning erased valid cross-row
previews. That pruning is removed; lane eligibility now uses the owner's current
paint generation. Rendered GPUI tests cover audio pickup/preview/MoveClip dispatch,
MIDI cross-row preview/MoveMidiClip dispatch, and second pickup after a simulated
matching worker completion. A rendered scroll regression excludes stale lane
bounds. The fixture completion is simulated, so native worker/device and the
reported post-move pickup still need the user's retest.
