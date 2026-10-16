# lfo pitch assignment — #2383 / pr173

The lfo display labels its four canonical oscillator pitch destinations and
shows the selected letters, including an explicit `selected: none` state.
The amount readout explains Hex's existing maximum depth: 100% produces a
one-octave excursion in either direction. This is an original Hex law, not a
measured Live equivalence. Amount zero and all destinations off preserve tuning.

The A–D buttons use existing descriptor IDs 89–92, generic widget bindings and
session commands. Selection readback follows pending/canonical parameter values;
it is not a second assignment model. IDs, state validation/migration, DSP,
clocks, resource limits and live-history ownership are unchanged. Each control
retains one rendered focus parent: shell controls stay in the shell, while sync,
inactive rate, retrigger and destinations belong to the detail display.

## reference evidence and bounds

On 16 October 2026 the installed bundle metadata reports Live
12.4.6 (2026-09-10_0de5c8fa9a). The official
[Live 12 Operator manual](https://www.ableton.com/en/live-manual/12/live-instrument-reference/#lfo-section)
describes oscillator/filter destination switches, destination intensity, a
second destination chooser and intensity, overall amount, rate/range and
retrigger. This supports individual oscillator assignment as a familiar control
pattern. It does not establish numerical parity.

Direct reference interaction remains open: the running installed Live is playing
an existing user project, so this work does not change that project, transport
or devices to create an experiment. No native/listening/reference acceptance is
claimed. The finite supported implementation is oscillator pitch A–D and the
existing shared amount. Filter mapping, additional destinations, independent
destination amounts and LFO envelope behavior require their own defined DSP/state
contracts; decorative controls for them are not added here. Deferred LFO review
from #2325 remains in #2384.

## verification and user checklist

Focused coverage exercises actual destination clicks, selected readback after
each click, undo/redo, amount editing and project save/reopen.
A rendered regression reproduced the depth caption overlapping the navigation rail
in synced mode at 880 px. Destination captions now share one row with tighter
vertical spacing, retaining the existing widget entities and focus ownership.
Both free/synced modes at 880/1440 px check all captions against the actual rail
and detail boundaries. These are GPUI scene bounds, not native pixel acceptance.
An independent PCM oracle covers all four destinations, ratio/fixed tuning, zero/full amount,
unselected tuning, held/released history transfer and block partitioning.
Existing domain, malformed-state and migration tests remain authoritative.
Arrangement/Session, prepared export and callback allocation fixtures also
exercise destination changes. Production audio code is unchanged, so this UI
change adds no processing hot path; prior timing evidence remains bounded as
described in [operator-lfo.md](operator-lfo.md).

Use the exact candidate's retained full local-CI report and optimized app receipt
for actual check/build results. Automated GPUI scenes do not establish native
pixels, physical device timing or listening acceptance.

1. Open Vector's lfo display. Confirm pitch destinations, selected letters and
   depth explanation remain readable without clipping or rail overlap.
2. Enable lfo and raise amount. With one audible oscillator at a time, toggle
   its corresponding A–D destination; verify only selected pitches modulate.
3. Check zero amount and all destinations off; restore a destination and compare
   free/sync rate and retrigger on/off with sustained and repeated notes.
4. Change assignments and amount during playback and release tails; verify
   responsive feedback, no stuck sound, and consistent Arrangement/Session use.
5. Undo/redo assignment and amount edits, save/reopen the project, and confirm
   the mapping, values and sound return. Report the exact build with results.

Use a disposable project copy. The user owns native interaction/listening review;
no automatic test edits their existing projects or untitled checkpoint.
