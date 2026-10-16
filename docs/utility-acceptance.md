# utility acceptance

Pulse #18 retains its earlier merged linear core, version 2 stateful runtime,
decibel presentation and compact panel. The shared device layout/theme work
#2469–#2474 supplies the current frame and controls. PR #250 closes the remaining
stateful mute defect and adds service-level parameter/history/save-load coverage.

## automated scope

The device tests cover channel selection, polarity, gain, width/M/S, mono,
balance and mute, plus original DC/bass filter responses, partitioning, reset and
history transfer. The session service test edits every Utility parameter through
canonical requests, undoes/redoes each edit, saves and reopens the authored state
with the same track/device identity. These checks establish original Hex behavior;
they do not establish audible or measured Ableton parity.

With filters active, mute suppresses their output after processing as well as
zeroing their matrix input. Histories continue advancing with zero input, so
unmuting does not restore frozen pre-mute histories. Bypass retains the shared
runtime's existing behavior. No state schema or filter response law changes.

## native checklist

Use the optimized stable app delivered through [test-builds.md](test-builds.md).
Keep the build receipt's source revision, version, CI report and included PR
heads with results. Save work and quit normally before replacement, then relaunch
the stable app; disk installation alone does not reload a running process.

1. Add Utility to a regular track and a Return. Confirm readable controls,
   compact input/output columns, theme colors and header-only selection.
2. With asymmetric stereo audio, test stereo/left/right/swap and each polarity
   switch. Left/right disable width/M/S; returning to stereo preserves their values.
3. Drag, type and use keyboard arrows on gain (including silence, 0 and +35 db),
   balance, width and M/S (right-click the dial to switch modes). Confirm focus,
   Escape/cancel, default reset and a single undo/redo per completed gesture.
4. Listen to mono, mute and DC removal. Warm the filters, mute, then unmute:
   muted output must remain silent; inspect audible transitions separately.
5. Enable bass mono at 50/200/500 hz and audition; check low-frequency centering
   and audition output on the regular track and Return. Record clicks or surprises.
6. Save/reopen with nondefault controls; verify values, device order and bypass.
   Exercise stop/play, seek and loops with filters active; record listening results.

Results remain pending until supplied by the user. Shared numeric/gesture source
tests do not prove native focus, physical input, listening or hardware deadlines.
The separately tracked generic knob undo issue #2374 remains visible during
native gesture testing; this PR does not silently claim to repair that issue.

## reference unknowns

The 5 hz DC blocker and fourth-order Linkwitz–Riley bass filters are original Hex
policies. Exact reference magnitude/phase, gain acceleration below -18 db,
smoothing, filter-toggle history and processing order remain unverified. Use the
original signals and predefined alignment/tolerances in
[utility response fixtures](research/utility-audio.md); retain actual reference
captures before claiming comparisons. Broad research #31 is separate from the
0.1 completion scope described in #16.
