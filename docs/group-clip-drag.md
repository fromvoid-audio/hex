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

## review return: protected copies and invalid preview retirement

Review found that copy-mode destination carving could erase a selected original.
Three independent Audio/MIDI/mixed canonical regressions reproduced that loss.
Group copies now keep selected originals physically present and exclude their
stable IDs in both existing overwrite victim paths. Single-clip wrappers retain
their original exclusion and frame/beat conversions. Unselected destination
material is still carved; final capacity, prepared admission and one Undo/Redo
remain canonical. Undo restores content while retaining the allocator high-water.

Actual GPUI pointer tests also reproduced populated group previews after leaving
all lanes and after a real vertical UniformList scroll invalidated row geometry.
Every unsuccessful candidate and no-target/insertion lookup now invalidates the
whole group projection; vertical scroll clears it before repaint alongside the
primary destination. Valid planning repopulates it. The painter and original
visibility therefore agree without giving outside release commit authority.

At the first copy/preview checkpoint, repeated preview construction remained
open. Its measured bounded allocation/scan costs and the subsequent accepted
capture contract are recorded in Pulse2803. The implementation below addresses
that separate finding without changing canonical commit authority.
Original22bc source/Mac12 receipts, new red/green checks, merger final combined
full CI and physical optimized acceptance are separate evidence. This bounded
repair is not a claim that every PR409 review finding is closed or merge-ready.

## drag-local immutable-source capture

`ClipGroupMovePreview` captures one checked selection once from the current
`Arc<Project>`. Its fields and original indices are private; it retains only a
Weak source witness, checked original coordinates/indices, and reusable selected
metadata. Before every update or indexed original read it compares that witness
with the actual current allocation. Project has no public interior mutation;
Weak prevents `get_mut`, and `make_mut` dissociates the source allocation. Equal
IDs/revision/content on a different allocation are insufficient. This is UI
correlation, never native provenance or authority to commit cached clips.

Projection always starts from captured original coordinates, including shared
zero clamping, exact MIDI-anchor beats and positive subframe lengths. Musical
placement shells detach once while retained processing domains stay shared.
Updates retain storage when invalid but publish no visible members until every
member succeeds. A caller-retained clone of a musical preview shell causes a
controlled invalidation/refusal, rather than a hidden pointer-path COW allocation;
normal renderer consumers borrow previews. Dropping the alias permits recovery.

Pointer, modifier and autoscroll changes reuse that storage. Existing generation,
revision, selection epoch, pending state, lane geometry and active-drag checks
remain. Context retirement removes the capture inside the shared Rc payload,
including Stop, Escape, root MouseUp, project replacement and superseding drags.
Outside/vertical invalidation hides all previews and restores original visibility
without dropping/recloning reusable metadata. Rendering borrows original audio
through checked captured indices, and anchor freshness avoids a source-track
clip scan. Actual MouseUp still submits IDs/anchor/current position to the fresh
canonical planner and its original prepared transaction; cached metadata never
becomes a commit payload.

Focused tests compare the old canonical projection with mixed/Warp/tempo capture
for either anchor, A→B→A, invalid→valid, zero bounds, subframe MIDI, replacement,
reorder/delete and metadata-alias refusal. GPUI tests cover same-source new views,
equal-counter source replacement, selection change, retained payload retirement,
outside/vertical reentry and actual redraw separately from projection. Instrumented
standalone update probes report zero metadata/backing allocations and frees for
frame and musical selections up to16,384. Full GPUI redraw includes unrelated
view work and is measured separately. These are debug source measurements, not
optimized/native deadline, audio, delivery or physical interaction acceptance.
Final exact-source receipts and immutable originals are in Pulse2803.

## complete batch planning, copy completion and passive victim paint

The returned PR409 capacity finding is handled by a private final-state batch
plan, rather than successive one-clip overwrite/install operations. It unions
half-open destination cuts per row in each victim's coordinate domain. Audio
uses frame cuts and the existing typed split/source/fade law; MIDI preserves the
existing audio-to-beat lower-bound fence and exact beat cuts. Each original
victim is planned once. Disjoint cuts retain the intervening gaps. The first
surviving piece keeps the victim's ID; later audio pieces and materialized MIDI
pieces receive fresh IDs. A retained MIDI prefix keeps its authored source;
other pieces use finite one-shot carried notes as the single-interval wrapper
does. The extracted single-interval helpers keep their existing behavior.

Final clip and note counts include Session clips and sources, even though Session
cells are never overwrite victims. Copied MIDI notes and all retained/materialized
notes count toward the final limit. Occurrence enumeration has a separate checked
bound, including occurrences discarded by a cut; full removal needs no expansion.
The plan reserves complete row replacement storage and IDs before installation.
All fragment births precede command-copy births. Old Arrangement rows are cleared
before complete replacements are installed, inside the existing isolated candidate
and one preparation/history commit. Refusal preserves the complete prior project,
ID high-water and history. No extra rollback clone or raised capacity is used.

`Created::Clips { first, last }` reports the copied ClipId endpoints. Actual
Arrangement clip births in that range are exactly the command copies; NoteId
holes are allowed. The UI filters real committed clips, never enumerates integers.
A private scalar intent is armed only after accepted enqueue and receives its FIFO
ordinal only after actual Service submission, including deferred/barrier queues.
Earlier completions cannot select copies. After the corresponding response installs
canonical state, generation, source revision, selection epoch/primary and current
editor context must still agree. A changed selection/document cannot be stolen;
refusal, closure and replacement retire intent. Ancillary error text does not hide
a real committed result. The resulting selected group supports the next drag.

Group landing paint now shows unselected overwrite victims after all ghosts.
Both copy and move exclude every selected original. Each visible row collects
only scalar interval endpoints, sorts once and sweeps union intersections; no
member-by-victim Cartesian product or persistent cache. There are at most two
events per member/victim and two paint-local Vecs. These allocate during paint,
not capture projection/pointer metadata updates or audio callbacks. A synthetic
maximum-model sweep measured 32,768 events, 524,288 event-capacity bytes and
131,072 output-capacity bytes in the debug source check; this is not a native
frame-deadline guarantee. Separate actual full GPUI redraw measurements include
unrelated layout/waveform work. The accent has no input handlers and is clipped
to the visible row; outside invalidation/reentry retains the existing contract.

Focused verification covers the three retained review failures, legal final-MAX
cross-row frees, Session-inclusive MAX/MAX+1 clips and notes, disjoint Audio/MIDI
cuts and held-note carry, ID exhaustion, enumeration versus discarded occurrences,
preparation rollback, real mixed Track release/Service copy selection with NoteId
holes, one Undo/Redo, subsequent dragging, deferred FIFO/refusal/stale selection,
ancillary errors, passive highlights and outside/reentry. Source receipts remain
separate from sole-merger prospective full qualification and root-owned optimized
physical/listening acceptance. Root's finite native follow-up is: select mixed
clips on two rows, Alt-drag over a third victim, confirm protected originals and
victim accent, release, drag the selected copies again, then Undo/Redo both edits;
also check Escape/outside reentry and unchanged waveform continuity. No app build,
install or native interaction was performed by this source task.
