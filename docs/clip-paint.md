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
preview is view state. Release dispatches one `PlaceMidiClipWithOverwrite` command,
so creation and cutting use the normal worker, persistence and undo/redo path. Clip headers
and visible edge trims keep their selection/move/resize gestures. Clip bodies
accept ordinary single-click time selection. Painting cuts existing audio and MIDI within the new half-open span, reusing the
canonical hybrid overwrite helpers. Interior painting yields the original before,
the new empty MIDI clip, and the original after, with no full-length underlay.
Partial/exact-boundary overlap retains only material outside the painted span;
touching clips are unchanged. Retained audio preserves immutable assets, loop
source positions, gain and anchored fades; MIDI uses existing finite-fragment
note rules. The explicit painting command leaves ordinary additive
`PlaceMidiClip` callers unchanged. A refused candidate preserves the original
project and history. This supersedes the old additive-paint contract for
Pulse2718/PR334 at the user's explicit request.

Escape, pointer-button loss, focus/window loss, window resize, release outside
the original track, project replacement/revision change, pending edits, or a
changed viewport/snap setting cancel the gesture without creating a clip.
Main/Return rows, headers and the browser are not paint targets. Autoscroll and
audio-source generation are outside this gesture's scope.

Automated checks cover the captured second press, growing preview, absence of
edits before release, canonical placement/undo/redo, cancellation, stale project
identity, clip-header rejection, painting over audio/MIDI bodies, minimum length
and free pointer precision. Pulse2718 adds three-adjacent-fragment geometry,
independent rendered header selection, audio source/loop/fade continuity, MIDI
content, boundary overlap and atomic refusal/Undo/Redo cases. Exact-source test
results are recorded in PR334/Pulse2718; added tests alone are not a pass claim.
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

4. On a disposable long audio clip, paint wholly inside its body: exactly three
   adjacent clips, with no full-length original underneath. Select and move each
   header independently. One Undo restores the long clip; Redo restores the split.
5. Repeat on a long MIDI clip with notes across the cut, and with looping audio.
   Retained audio timing/source windows and MIDI content should agree before and
   after the painted interval. Paint across a left/right edge, exactly over the
   original and touching its end; only the overlap is replaced.

Native checks require a separately qualified optimized candidate coordinated by
the single Mac owner. No app installation, physical listening/input, waveform
GPU deadline or complete Live-parity acceptance is implied by source checks.

At the combined audio/MIDI clip limit, painting first retires MIDI clips fully
covered by the painted interval on the private candidate. Those victims only
free capacity. Audio and remaining MIDI fragmentation then retain their existing
admission checks, followed by the new painted clip's ordinary admission. This
admits a valid final hybrid count without allowing a temporary helper count to
refuse it, raising the cap, or publishing a partial cut. An invalid final count
or failed preparation preserves the original Project, ID cursor and Undo/Redo.
The capacity regression covers the exact limit with one audio split/two covered
MIDI victims, over-limit final refusal, and post-edit preparation failure.
