# Arrangement clip painting

Double-click empty space or the lower two thirds of an audio/MIDI clip in a
regular Arrangement track, keeping the second press held. Drag left or right to
preview the clip's length, then release in that track to create an empty MIDI
clip. All regular tracks support MIDI content. A double
click without movement creates one snap-grid cell, or one quarter-note beat
with snapping off. The original double-click position stays anchored: dragging
left moves the clip start earlier, and crossing the anchor switches direction.

Start/end positions use the canonical tempo map and current snap setting;
Command temporarily toggles snapping as in other Arrangement gestures. The
preview is view state. Release dispatches one existing `PlaceMidiClip` command,
so creation uses the normal worker, persistence and undo/redo path. Clip headers
and visible edge trims keep their selection/move/resize gestures. Clip bodies
accept ordinary single-click time selection. A painted span may
extend across existing content under the existing non-destructive placement
contract; painting does not delete that content.

Escape, pointer-button loss, focus/window loss, window resize, release outside
the original track, project replacement/revision change, pending edits, or a
changed viewport/snap setting cancel the gesture without creating a clip.
Main/Return rows, headers and the browser are not paint targets. Autoscroll and
audio-source generation are outside this gesture's scope.

Automated checks cover the captured second press, growing preview, absence of
edits before release, canonical placement/undo/redo, cancellation, stale project
identity, clip-header rejection, painting over audio/MIDI bodies, minimum length
and free pointer precision.
The user confirmed rightward painting on empty tracks works on 17 October 2026.
Painting through clip bodies, leftward painting and direction reversal still
need native acceptance. User checklist:

1. Double-click empty track space, hold the second click, drag in either direction
   and release: the created MIDI clip should match the preview. Undo and redo once.
2. Repeat with snapping off and with Command held. Try a double click without
   moving. Cross back over the initial press position during a drag: the preview
   should switch sides while keeping that position as its anchor.
3. Cancel with Escape or release over another track/header; no clip should appear.
   Single-click selection and existing clip movement should still work.
