# arrangement overwrite drops

Implemented source contract, 9 October 2026; native acceptance pending. Dropping an Arrangement clip or browser WAV
on an existing hybrid track replaces the occupied interval. The incoming clip
keeps its identity and contents. All intersecting destination audio and MIDI
clips lose the covered region; uncovered left/right pieces remain. Intervals
are half-open, so exact touches are allowed. A moved clip excludes itself.

This is a drop-specific edit, not a migration of saved overlapping projects.
Generic placement/move APIs and the renderer's existing summing policy remain
unchanged. Session cells are outside this contract.

## fragment ownership and timing

Full coverage removes a victim. Edge coverage trims it. Interior coverage splits
it into two pieces. One piece retains its original clip ID; an additional piece
gets a fresh ID. Audio fragments use the canonical split/trim source phase and
fixed fade domain, preserving the uncovered audio samples.

The MIDI left fragment retains its source and note IDs, with a shorter finite
clip duration. The right fragment materializes the original finite playback's
retained note occurrences into a one-shot source: source start zero, source
length equal to the fragment duration, full loop brace, looping disabled.
Pitch, channel and velocity are retained; projected notes receive fresh globally
unique IDs. A right-only fragment may retain the victim's clip ID. A silent
tail is an empty one-shot source.

Notes held across the cut end on the left and retrigger at the right boundary
for their retained duration. This is an intentional edit discontinuity, not a
claim of identical synthesizer phase or PCM. Looped right fragments become
finite one-shot note sequences. Undo restores the original source and looping.

MIDI-on-MIDI cuts use exact beats. Audio-on-MIDI uses control-side tempo mapping:
the left endpoint is the greatest beat whose rounded frame is no later than
the incoming start; the right endpoint is the first beat whose frame is no
earlier than the incoming end. Guard lower-bound sentinels and arithmetic.
Collapsed/zero-frame fragments are omitted. Small rounding gaps are allowed;
new frame overlap is not. MIDI-on-audio uses the incoming mapped frame interval.

## atomicity and limits

Plan/count MIDI occurrences before enumeration. Refuse expansion beyond the
canonical note limit (or an explicitly documented tighter limit). Preflight may
conservatively count all occurrences of a victim that needs a right fragment,
including those outside that fragment, and refuse dense loops before expansion.
Clip counts,
fresh IDs, media admission and compiled graph limits apply to the whole
candidate. No callback conversion, allocation or unbounded loop expansion.

Import/move and victim carving form one validated candidate, compiled before
one history commit. Any validation, expansion, ID or compilation failure leaves
the prior project, media, allocator, history and applied audio state intact.
Undo/redo and save/load must preserve the complete result.

## acceptance

Cover audio/MIDI/browser drops, both victim types, same/cross-track moves,
full/edge/interior coverage, exact touching, multiple victims, phase/fades,
looped and boundary-held MIDI notes, silent tails, limits and failed candidates.
Verify one Undo and persistence. Rendered tests cover drop routing and previews;
native interaction and listening acceptance belong to the user.

## native review checklist

1. With no tracks, drag a browser sample into Arrangement: the entire blank
   section highlights, a ghost follows the pointer, and no horizontal line
   follows pointer height. With tracks, blank space below them highlights and
   the fixed last-track line remains.
2. Drop a sample into the middle of a longer audio clip: the sample occupies
   the destination and the original has left/right remnants. Try an edge and
   full coverage. Touching clips should not be cut.
3. Move existing audio and MIDI clips over other clips, on the same and another
   track. Try audio over MIDI and MIDI over audio. The dropped clip keeps its
   contents; underlying covered material is removed.
4. Check a held MIDI note and looping MIDI victim: the right tail retriggers
   at its boundary and becomes a finite one-shot clip. Undo should restore the
   original notes/loop settings.
5. Undo once, redo once, save/reopen, and drag the resulting fragments again.
   Also confirm new-track divider/append drops retain their working behavior.

Native review is pending. Do not infer listening or physical-device deadline
acceptance from source tests.

## source verification and review build

209 rendered UI tests pass (one existing ignored). Independent full project and
session suites pass; engine clip-fade render comparisons pass 5/5. Strict
affected Clippy, desktop build, formatting, architecture/style and diff checks
pass. New tests cover multiple victims, exact touches, held MIDI retrigger, mixed
clip kinds, loop expansion refusal, clip-cap rollback and full-cover import at
the clip limit, one-step history and compiled save/reopen.

Review app: `artifacts/hex-overwrite-drop-20261009.app`. Evidence is in
`artifacts/overwrite-drop-core-tests.log`, `overwrite-drop-audio-split-tests.log`
and `overwrite-drop-manifest.json`. Existing running bundles were preserved.
No commit/push. Native interaction/listening review remains open.
