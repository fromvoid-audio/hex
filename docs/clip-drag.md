# Arrangement clip dragging

User-selected update, 9 October 2026: implemented; native user acceptance pending.

Audio and MIDI clips move within or between hybrid tracks through one
metadata-only native gesture, started from the shaded upper-third clip header
with an open-hand cursor. Clicking the header selects the clip, including while
worker edits are pending or when leaving another clip's inline name editor. The lower two
thirds behaves as empty track space for time selection and double-click painting;
narrow full-height edges still trim. Audio start/duration remain project-rate frames;
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


## New-track move extension — implemented; native retest pending

MoveClipToNewTrack and MoveMidiClipToNewTrack create a default hybrid track
before a stable track ID or append it, then move the original clip as one
validated candidate. Source track remains, even if emptied. Clip/asset/note IDs
and source metadata are preserved; only destination track and clip time change.
Audio remains frame-anchored; MIDI start is beat-anchored and length is retained.
One undo restores the move and removes the created track. Existing ordered-track
persistence requires no schema change.

UI dividers show only an insertion line. Empty/bottom append space shows the end
line plus a cursor ghost. Both browser samples and existing clips will use the
same destination geometry, with original grab offset retained for existing clips.
The timeline excludes the right-hand header; pointer x is never shifted by an
assumed left header. Floating overlays remain absolute, never layout participants.
User native checks will confirm existing audio/MIDI insertion/append/undo, sample
placement under the mouse, and stationary workspace geometry at the bottom.


Source acceptance: actual per-lane release routes divider targets into the
new-track command; an active-gesture identity guard avoids duplicate parent/child
handling. Rendered tests cover audio/MIDI divider and append targets, captured
grab offset, regular moves and repeat pickup. Compiled Session editing covers
undo/redo/save/reopen and ID preservation. 209 UI tests pass/one ignored;
project/session checks and strict affected lint/build/guards/fmt/diff checks pass.
Review: `artifacts/hex-track-drag-fix-20261009.app`. Native retest remains open.


## overwrite and empty-area feedback — source implemented

Current user-selected work adds a subtle full blank-area drop highlight: an
empty Arrangement has no cursor-y line; append retains the last-track boundary
line and floating ghost. Track dividers remain line-only. Rendered regressions
pass; native acceptance is pending. Existing-track drops use the atomic
overwrite contract in `clip-overwrite.md`, replacing covered portions of both
audio and MIDI victims. Earlier overlap-policy deferrals above describe older
builds; this extension is not yet native-accepted.

209 rendered UI tests pass/one ignored; core/history/persistence/render checks
and strict affected lint/build pass. Review: `artifacts/hex-overwrite-drop-20261009.app`.
Native interaction acceptance remains pending.

## held-clip intersection feedback — 17 october 2026

The existing-track landing preview highlights its occupied intersections with the
editable theme secondary accent (bright turquoise by default). Audio and MIDI
victims share the same feedback, including browser sample landing previews.
Only the intersection inside the visible viewport is tinted; gaps and exact
edge touches stay clear. A move excludes its original clip; Option-copy includes
it, with stationary modifier changes refreshing the highlight. Overlapping
victims are merged before painting so tint intensity stays uniform. Release,
cancellation and invalid/stale targets remove the transient feedback with the
existing preview lifetime. No project/history or overwrite semantics change.

Source regressions cover mixed audio/MIDI intersection geometry, gaps, clipped
spans, exact touches, merged victims, source exclusion/copy toggles and release
cleanup. Native visual acceptance remains pending: hold an audio or MIDI clip
over another clip, inspect partial/full overlap and gaps, toggle Option over its
original position, then cancel/drop and check that the highlight disappears.


## Full-content held previews — Pulse2890 / PR492

Internal landing, group, append-row and floating previews compose the same
read-only clip title, audio body and MIDI note painters as ordinary clips.
Prepared audio peaks retain their actual original root/offset; edited audio never
falls back to an asset thumbnail. MIDI uses the accepted prepared note samples
with the existing per-row shape budget. Titles are passive text, never another
instance of a live rename input. The theme selection outline identifies the
clone while retaining opaque readable source content.

A destination may imply a different future tempo/warp projection. Until that
candidate is compiled, the held image deliberately shows the original ready
source crop, translated and fitted into destination geometry. A checked mapping
of the destination's visible fraction crops the original source window; it does
not claim newly rendered destination audio. No pointer-motion decode, DSP,
project clone, new PCM owner or drag command is introduced. Escape, stale-state
refusal, capture and release retain the existing gesture lifecycle. Floating
multi-selection still uses the existing anchor-card behavior; valid lane group
previews show each selected member, without adding a new off-lane drag mode.

The owning actual GPUI checks observe MIDI quads, colored title quads, borders,
waveform paint bounds/masks and the exact accepted processed waveform Arc/offset.
They cover trimmed/faded/pitched audio, looped Beats-warped audio, landing and
floating content, and mixed audio/MIDI group move/copy presentation. Project and
pending command counts remain unchanged while held; Escape removes the preview.
An original missing-note regression and a later opacity-only mismatch are kept
as separate evidence. The latter already painted the target note. A third test
fixture failure required dispatching the actual window modifier change for Alt;
it did not establish a product copy/capture defect.

These are component/painter observations, not native GPU pixels or listening.
Final prospective-tree qualification and merge belong to the sole merger.
User-present optimized acceptance remains open: drag audio/MIDI and mixed groups
through lane, browser and append positions; inspect source details, clipped edges,
small/header-only rows, zoom and tempo changes; move/copy/drop, Escape and Undo.
Future rounded loop-bottom styling remains a separate feature and must preserve
this shared body clipping; no unmerged loop implementation is imported here.
