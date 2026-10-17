# Arrangement clip painting

Double-click empty space in a regular Arrangement track, keeping the second
press held. Drag right to preview the clip's length, then release in that track
to create an empty MIDI clip. All regular tracks support MIDI content. A double
click without movement creates one snap-grid cell, or one quarter-note beat
with snapping off. Dragging left keeps that minimum preview length.

Start/end positions use the canonical tempo map and current snap setting;
Command temporarily toggles snapping as in other Arrangement gestures. The
preview is view state. Release dispatches one existing `PlaceMidiClip` command,
so creation uses the normal worker, persistence and undo/redo path. Existing
content is never a paint start target; other clip selection/dragging and ordinary
single-click time selection keep their existing gestures. A painted span may
extend across existing content under the existing non-destructive placement
contract; painting does not delete that content.

Escape, pointer-button loss, focus/window loss, window resize, release outside
the original track, project replacement/revision change, pending edits, or a
changed viewport/snap setting cancel the gesture without creating a clip.
Main/Return rows, headers and the browser are not paint targets. Autoscroll and
audio-source generation are outside this gesture's scope.

Automated checks cover the captured second press, growing preview, absence of
edits before release, canonical placement/undo/redo, cancellation, stale project
identity, existing-content rejection, minimum length and free pointer precision.
Native interaction remains pending. User checklist:

1. Double-click empty track space, hold the second click, drag right and release:
   the created MIDI clip should match the preview. Undo and redo once.
2. Repeat with snapping off and with Command held. Try a double click without
   moving, and drag back left to confirm a positive minimum length.
3. Cancel with Escape or release over another track/header; no clip should appear.
   Single-click selection and existing clip movement should still work.
