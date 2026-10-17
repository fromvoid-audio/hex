# Audio events inside Arrangement clips

Pulse #2692 / [PR308](https://github.com/PORTALSURFER/hex/pull/308).
This is an original Hex editing contract, inspired by the requested nested audio
workflow. It is not a claim of measured Bitwig or Live compatibility.

An Arrangement audio clip remains the outer container: moving, trimming,
splitting, copying and deleting it retain their existing meaning. Its lower
`events` tab edits a non-destructive sequence inside the retained audio processing
root. The `source` tab retains the original source, phase and warp-marker view.
A legacy or newly imported clip has one implicit event until an edit materializes
its sequence. Silence from an exhausted source remains silence.

Events identify half-open source intervals within that immutable root, rather
than rewriting media. Their coordinates follow its existing clock: frames for
ordinary audio, beats for explicitly musical audio. Snapping alone never enables
warp or converts frame-anchored audio into tempo-following audio. The event grid
uses the project tempo/meter map and the displayed interval, with independent
adaptive on/off state and zoom. The same helper supplies painted landmarks,
snapping and directional nudges. Holding Cmd/platform inverts snap, as in the
Arrangement; Cmd+4 toggles it in the focused editor.

## Edits and interaction

- Click an event to select it; drag its body to move it. Its true edges support
  trimming, including restoration of retained source where bounds allow.
- Drag the ruler, empty time, or Shift-drag the waveform to select time. Drag
  within a selected time range to move its contents and gaps together.
- Cmd+E splits at the cursor, or at both boundaries of a nonempty time range.
- Cmd+C/X/V copy, cut and replace-paste events/time. Delete removes the event or
  selected time; a hole emits silence. Cmd+D duplicates after the selected range.
  Cmd+Shift+V inserts time, refusing if later events would exceed the container.
- Left/Right nudge an event to the adjacent displayed grid landmark, or one frame
  with snap off. Shift+Left/Right adjust its trailing edge. Cmd+A selects all
  visible clip time. Cmd+Plus/Minus/0 zoom/fit; Alt+Left/Right pan the event view.
- Escape cancels the local gesture/selection. Pointer motion previews only;
  releasing inside the waveform commits once. Releasing outside, losing focus,
  changing source revision/project/clip, or changing layout cancels the preview.

A moved or pasted event replaces covered audio: existing intervals are carved
around it, retaining their original source coordinates. A range's holes replace
the covered destination too. Splitting leaves the sample sequence unchanged.
Outer clip cuts retain the complete shared processing/event root and change the
visible crop; editing a child makes its own copy without modifying its sibling.
The container fade and gain apply after event composition, not once per event.
When an outer cut crops through an existing event, the editable view isolates its
visible fragment before an inner edit. Copy/move/trim therefore target that
fragment, while the hidden source intervals and sibling clip stay intact.

The lower editor owns its editing shortcuts while focused. Returning to a track,
Arrangement clip, source tab, device panel or closed detail pane releases that
focus. Clipboard entries retain their source root and generation; this initial
editor accepts paste into their original clip/root, and explicitly refuses stale
or different-source paste. It does not silently reinterpret source offsets.

## Ownership, preparation and saved state

`hex-project::AudioEvents` owns validated, sorted nonoverlapping intervals and a
sequence-local identity allocator. References are scoped to clip/revision;
gestures never retain an unguarded event index. At most 1,024 events exist in one
root. Malformed IDs, overlap, exhausted IDs, overflow, invalid source coordinates
and over-limit edits fail transactionally without changing history.

One typed `EditAudioEvents` command is one undo step. Candidate preparation must
succeed before canonical commit. `hex-engine` composes the retained root on the
worker before applying the outer fade. Temporary composition storage receives a
separate derived-audio budget reservation. Playback consumes ordinary prepared
immutable PCM; it never scans event lists or allocates in the audio callback.
Splits and moves share unchanged media, with source/root identity in cache keys.

Schema27 adds a complete `audio_event_state` table to the existing complete
schema26 envelope. Every Arrangement audio clip appears exactly once, including
implicit sequences. Unknown/duplicate fields, missing owners, duplicate/missing
rows and malformed event bounds are refused. The original JSON token stream is
checked for duplicate keys before adapting the older envelope. Strict schemas
1–26 still migrate to implicit events and retain all previous state. Older
explicit writers refuse event-bearing processing roots rather than dropping them.

Current preparation uses the existing resident transformed-audio path and its
16,777,216-frame root bound. Streamed event edits refuse before history commit,
leaving the previous playable graph intact. Switching an event-bearing root
between frame and musical clocks also refuses explicitly; select its intended
clock before cutting events. Event-specific gain/fades, new media inside a root,
cross-root paste, streamed composition and clock conversion are subsequent work.
Existing clip pitch, source controls and already-musical warp paths remain owned
by their current contracts.

## Verification

Model tests cover interval carving, source continuity, trim, clipboard, limits,
one-step history and retained outer splits. Storage tests cover full round trips,
legacy migration and malformed/incomplete/ambiguous state. Independent PCM tests
compare identity splits, overwrite/gaps, seek/block partitioning, outer splits and
musical/warp identity splits; streamed preparation refusal preserves the project.
Headless GPUI tests exercise actual ruler/drag/key input, independent snap state,
inner-command routing and stale/cancel behavior. They do not establish native
macOS hit testing, rendering, listening or sustained performance.

Native acceptance remains open: import original audio, split/move/trim events,
select/cut/copy/paste time, compare snapped/free moves at multiple zoom levels and
BPMs, undo/redo, move/split the outer clip, save/reopen and listen for correct gaps,
overwrite and retained content. Verify minimum layout, source-tab switching,
focus isolation, cancellation and refusal feedback. User owns native interaction
checks under the repository policy; no app was installed or launched here.
