# Selected Arrangement clip movement

Pulse2803 / PR409 captures the explicit stable clip IDs when an already-selected
member starts an actual move drag. It keeps the selected set and one immutable
selection epoch under the existing ClipDrag owner; clicking an unselected header
still follows ordinary replace/add/range selection. Audio and MIDI members share
one picked anchor's snapped delta and relative track offsets. One common lower
bound clamps the entire set at frame zero, instead of clamping members separately.

Audio placement uses canonical move/source mapping. MIDI keeps its authored beat
length and shared immutable notes/expressions; the common frame displacement is
projected through the current tempo map. The picked MIDI anchor retains its exact
chosen beat when no common boundary clamp is needed. A positive canonical MIDI
extent smaller than one integer frame remains in the group; the existing painter
may omit that zero-frame rectangle. No new rendering or model minimum is added.
No new tempo, routing,
track type, PCM render or per-member snap policy is introduced.

All members are projected before any preview is returned. A vertical destination
that cannot fit the complete set refuses the whole move. Multi-member insertion
into a new track is explicitly refused with feedback; existing destination tracks
are required, rather than creating a partial or invented track/routing layout.
The captured group renders every visible member from its original prepared
waveform/note source, and hides moved originals only while a valid preview exists.
Alt retains the existing copy modifier and canonical destination overwrite policy.

Release builds one bounded canonical MoveClipsWithOverwrite command. All moving
originals are protected before unselected destination material is carved by the
existing audio/MIDI overwrite helpers. Final current-format validation and normal
prepared history admission apply once. Invalid/stale/pending work or preparation
failure preserves the whole canonical set, IDs and both history stacks. Escape
and actual GPUI drag-payload retirement cannot leave a late single-member edit.
No parallel pending owner, worker protocol, history routing or callback path is
added. The broader global no-overlap planner in Pulse2822 remains a separate
model task; its ingress audit must include this explicit simultaneous group edit.
This feature does not claim that global rule is complete.

Bounded automated coverage uses actual rendered header selection and move events,
real Service preparation/acks and one Undo/Redo, including a non-primary picked
member, gaps, mixed audio/MIDI across tracks, all-member previews, invalid vertical
fit, real pending/acknowledged independent edits, Escape/capture loss, stationary
Command snap inversion and release's current modifier. Model cases cover either
anchor, common zero-bound, Warp/fades/loop/source/name/flavor and shared MIDI source,
current-format round-trip, invalid IDs, final MAX_CLIPS and prepared refusal.
Model-only fixtures do not claim PCM/listening proof. GPUI payload retirement is
a controlled capture-loss test, not a physical OS window-deactivation experiment.

Root-owned optimized physical acceptance remains open: select clips with gaps on
one and multiple tracks, drag any selected member, confirm all visible previews,
selection borders and stable waveforms move together. Test mixed audio/MIDI,
vertical limits, frame-zero bound, grid/Command, Alt copy, outside drop, Escape,
window/capture loss, one Undo/Redo and save/reopen with unchanged clip content.
Verify no accidental body painting, resize or subset movement. Sole merger owns
final actual candidate full checks/integration/merge; no app operations belong
to this source lane.
