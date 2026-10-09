# arrangement edit cursor and split

Implemented source contract, 9 October 2026; native acceptance pending.
Click a track lane, including an audio/MIDI clip body, to place a view-only
insertion cursor. It uses the displayed grid; Command temporarily inverts
snapping. Click selection retains the clip body/edge drag contracts. The cursor
is distinct from the audio playhead: placement does not seek, wait for audio
acknowledgment, or require an output device.

A one-device-pixel line now spans only the selected track rows. It switches
between full-opacity white and cyan every second; row dividers paint above it.
An 8px downward marker immediately below the ruler and a matching upward marker
at the arranger bottom remain steady. All share the same frame/viewport mapping
and exclude the track-header column. Horizontal dragging creates a steady
translucent time area, and vertical dragging extends it across tracks. The last
track reached is subtly highlighted as focused. See [time-selection.md](time-selection.md)
for range gestures and canonical editing. The existing timer requests a redraw
only when a zero-width cursor changes color; no callback or per-sample work is
added. Native paint timing is unmeasured.

Command-E splits the clip containing that cursor on its clicked track. A selected
clip wins if multiple clips overlap; otherwise audio precedes MIDI. Empty
positions and exclusive clip boundaries refuse the edit with a useful status.
The cursor survives normal edits/undo and zoom/pan, and is cleared when its track
is removed or the project is replaced. It is transient editor state, not promised
to persist in project metadata. Session splitting is not added.

Audio uses the existing canonical SplitClip command, preserving source phase and
inherited fades. MIDI uses SplitMidiClip with beat conversion on control: left
retains authored source/notes/loop settings, and right materializes the remaining
finite playable note occurrences into a one-shot with fresh IDs. Gates crossing
the cut are retained as carry notes at the right start; the right resets its
source/loop settings. This bounds loop expansion and avoids silent missing tails.
Expansion/clip/note/ID limits fail atomically; successful compiled insertion is
one undo step. Boundary carry notes retrigger, so phase/envelope continuity and
exact Live split parity are not claimed. All original authored data remain on
the left and Undo restores the original clip. No schema change.

Tests cover actual track clicks, free placement, one-pixel extent, viewport
mapping, Command-E audio/MIDI dispatch without audio output, empty/missing
positions, pulse timing, canonical split phases/gates/limits/history, worker
compilation and save/load. User checks: marker placement and pulse appearance;
audio and MIDI click/split/Undo; clip dragging/resizing; sustained-note boundary
behavior and loop-to-one-shot behavior. No automated native input testing.

Original point-cursor verification before the time-selection extension: 757 workspace tests passed, 1 ignored; strict UI and
project/session/storage lint, desktop build and source guards pass. Fresh review
app: `artifacts/hex-edit-cursor-20261009.app`; identity and logs:
`artifacts/edit-cursor-manifest.json`. Native click/pulse/markers, Command-E,
Undo/Redo and continued clip drag/resize checks remain with the user.
