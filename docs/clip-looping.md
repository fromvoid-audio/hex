# clip-local looping

Pulse2843 / PR445 is **in progress**. This is distinct from transport loop-bar
editing (2713) and setting the transport loop from selection (2735).

The requested interaction follows the official Bitwig user-guide “Looping Clips”
section: a bottom-right loop-resize handle enables repetition and changes the
outer extent; the first repeat marker changes the cycle while leaving the outer
extent fixed. See
https://www.bitwig.com/userguide/latest/arranger_clips_and_the_browser_panel/.
No Bitwig modifier policy or additional editing behavior is implied.

## implemented MIDI command checkpoint

`SetMidiClipSourceWithOverwrite` reuses the validation and source mutation of
`SetMidiClipSource`, then calls the existing audio-in-beats and MIDI-interval
overwrite helpers, excluding the incoming stable ID. The control-side Editor
prepares its existing private candidate before one history commit. No new
planner, history owner, source/media duplication, note flattening of the incoming
clip, Engine code or persistent representation is introduced. Existing numeric
`SetMidiClipSource` callers retain their prior behavior. The new ingress must be
included in the global no-overlap2822 audit; this command is not that feature's
completion.

The private test-only scalar planner captures the current MIDI source start and
visible length on first activation. Existing loops retain their loop region;
first-boundary changes preserve the outer duration. Extending a one-shot's
virtual source length can retain trailing silence in the first cycle without
removing authored notes. This planner is **not yet runtime gesture wiring**.

Tests exercise actual canonical commands, occurrence timing, whole-project
snapshot validation, no-op/invalid/prepare refusal, one Undo/Redo, mixed audio/MIDI
carving, final capacity, retained edited-audio roots and late fragment refusal.
A separate real Service test uses one unique RAII temporary WAV/document
folder, without an EditorView, browser/catalog worker or OS audio adapter. It
checks the prepared revision and MIDI preview, exact repeat frames across a
120→240 bpm point, one Undo/Redo, actual SaveNew/Open and the reopened project.
Model-only synthetic media paths in other cases are never opened. These are
model/worker tests, not rendered gesture, PCM/pitch, native input or full feature
acceptance. The original source-command baseline has two genuine failing cases:
it leaves covered neighbors in place and does not run fragment refusal.

## pending protocol and hit planning

PR445 deliberately pins the already-merged PR333/403 dependency once, preserving
its intake history. New gestures must extend the **same** pending-trim typed
command/FIFO/generation/owned-viewport record. Do not introduce another counter,
request key or preview owner. Source/start/track/generation identity and exact
terminal outcome remain authoritative; previews cannot synthesize a commit.

The existing lower fade-out control occupies the literal 8×8 bottom-right
corner. Overlaying a loop control there would hide that accepted trim/fade
interaction. The proposed separate inboard bottom-corner slot preserves this
control and the ordinary edge strip. First-repeat targets exclude the name
header and corner target. Compact-width/height hit priority still needs actual
rendered pointer tests before wiring is called complete; no accepted control
will be silently reassigned.

Looping fill and selection borders will use the same parent clipping and small
bottom-left/right corner radii, based on committed/prospective loop state.
Upper corners and ordinary non-loop clips stay unchanged. Preserve429's complete
bottom outline and separator order. No rounded-style acceptance is claimed from
metadata or property tests.

## remaining model and backend contracts

The model now has the root/Source4-approved optional Copy-only
`AudioViewPlacement { mode, outer_length: AudioDomainClock, cycle_phase }` on
`AudioClipEdit`. The selected `view_start..view_end` stays in the same retained
processing domain. Outer length uses that domain's clock and must be positive;
phase is strictly inside the selected cycle. `None` retains ordinary semantics;
`Some(Once)` remains after Looping off. No additional placement origin is
serialized; the retained domain's preexisting frozen fade metadata is unchanged.

`AudioViewProjection::new(clip, tempo)` borrows the canonical clip and tempo map.
It exposes outer frame bounds, a logical first boundary even beyond the outer
end, an O(1) occurrence count, and checked indexed occurrences. Each occurrence
contains a signed root origin (raw beats or frames), its full projected root
frame length/origin, root-relative crop frames and absolute destination frames.
For beat roots the first origin is `musical_start - view_start - cycle_phase`;
successive origins advance by the selected cycle. Occurrence boundaries project
absolute beats through the actual tempo map, never a repeated rounded frame
period. Zero-quantized or overflowing spans refuse. Frame roots intentionally
retain fixed-frame timing. Consumers still must admit aggregate counts, distinct
root projections, buffers and work before allocation or iteration.

The allocation-free `preview_view_resize` returns Copy scalars.
`ResizeAudioClipView` admits only the operation and recomputes from the current
clip; it does not accept a GUI-prepared clip. Outer changes call the existing
audio and MIDI overwrite helpers in the same private candidate; marker edits
keep occupancy fixed. The new ingress belongs in the global2822 audit. Ordinary
trim/split advance the phase while retaining the cycle/root; explicit movement
changes the origin. Clock conversions and Shift stretch preserve the placement
clock/phase law. The lower-fade path no longer mistakes one root end for the
outer end of a repeated view. These are model tests, not PCM/gesture acceptance.

Strict phase bounds currently refuse a Once left trim/split whose result would
start in the silent tail after its remaining cycle. That state cannot be
represented by phase strictly less than the cycle; it never reveals hidden
source to approximate silence. This finite limitation was returned to root.
The initial marker policy for an existing raw source loop with no view placement
also remains a root coordination item; no new source-to-Warp conversion is
invented. The existing processing root and its raw source loop/phase are kept.

The current-format audio DTO requires a placement field (including explicit
null), rejects unknown fields, and retains Once/Repeat and both clock variants.
Source1's separately frozen A/B wrapper makes format32 current and refuses31;
the disjoint codec patches must preserve that one combined current-format law.
Own codec results before integration do not qualify the future combined32 tree.

Source4 owns Engine preparation/output-range/resource quotes, exact pitch/Warp
root projection and mutable export. No repeated PCM flattening, callback DSP,
new source owner or silent processed-stream fallback is allowed. Model queries
and current-format persistence are not proof that Engine consumes these fields.

The MIDI source now carries `MidiOneShotBoundary::{SourceEnd, LoopEnd}`.
Ordinary sources retain `SourceEnd`; the new loop-capture overwrite command
sets `LoopEnd`. Numeric edits and Looping toggles preserve this policy. The
effective end is the loop end while repeating or bounded Once, otherwise the
full authored source end. Notes remain validated and retained against the full
source length. A bounded Once start must precede its selected end, even with
Looping off. Snapshot, COW/copy and current-format MIDI codecs retain the policy;
missing or unknown current-format policy values refuse explicitly.

`MidiSource::source_playback(duration)` is the shared checked constructor for
Arrangement and Session consumers. It calls
`MidiSourcePlayback::new_with_initial_end`, which independently validates authored
note bounds and clips audible onsets/gates at the selected end. The ordinary
constructor retains its prior behavior. `effective_end()` exposes the same
source-owned law without a compiler-owned mirror. Arrangement's existing clip
query delegates to this constructor. Source4 still must adapt the separate
Session template path and audit both actual compiler paths; model/codec tests
are not proof of that unfinished runtime integration. No notes or controls are
removed.

The existing linear export path explicitly refuses mutable Arrangement MIDI
source loops; prepared preview success does not clear that limitation. Full445
playback/export acceptance must resolve that bounded dependency honestly.

Frontend pending preview/marker/corner gestures, audio loop preparation,
complete Once runtime integration, rendered rounded styling, all-mode PCM/export and optimized
native acceptance remain open. PR445 stays draft/In Progress; the sole merger
owns final prospective-tree full CI/review/integration/merge, and root owns the
serial Mac lane and app delivery. No native/app operation is part of this source
checkpoint.
