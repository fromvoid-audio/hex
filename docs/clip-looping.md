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

Root accepted the bounded AudioClipEdit view direction: retain the original
processing root/cycle, add Copy-only outer-span, phase, Once/Repeat and processing
origin relationship. Source4 owns the exact Engine preparation/output-range and
resource quote contract; Source6 must obtain its scalar units/API before freezing
saved fields. Beat-clock audio must project absolute cycle endpoints through
the tempo map and retain pitch. A fixed frame modulus is valid only for the
intentional frame-clock case. No repeated PCM flattening, callback DSP, new
source owner or silent processed-stream fallback is allowed.

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
removed. The audio placement representation remains pending implementation.

The existing linear export path explicitly refuses mutable Arrangement MIDI
source loops; prepared preview success does not clear that limitation. Full445
playback/export acceptance must resolve that bounded dependency honestly.

Frontend preview/marker/corner gestures, audio loop representation/preparation,
complete Once runtime integration, rendered rounded styling, all-mode PCM/export and optimized
native acceptance remain open. PR445 stays draft/In Progress; the sole merger
owns final prospective-tree full CI/review/integration/merge, and root owns the
serial Mac lane and app delivery. No native/app operation is part of this source
checkpoint.
