# pinned main track

Pulse 12 owns the always-visible Arrangement main row. It is a fixed 44 px sibling
below the regular scrolling tracks and their trailing append/drop region, above
the shared clip/device detail panel. Its timeline grid uses the regular viewport;
its 176 px header aligns with regular track headers. Canonical master gain controls
move into this row; Session retains its existing master control strip.

Main is not a regular TrackId or clip/sample destination. Arrangement bounds
remain the upper regular region. The outer drag surface spans both regions so
crossing/releasing over main updates and cancels previews safely instead of
leaving stale regular drop targets. No project schema or audio routing changes.
Main-project time selection (ticket 14) is implemented in a separate focused
change under verification. Real return routing/rows (13) remain a separate feature.

Source checks pass: 304 UI tests with two existing ignores, including empty/short/
dense minimum 880×540 geometry, canonical master commands in Arrangement/Session,
clip/sample cancellation over main and regular append/import regression paths;
strict UI Clippy, architecture/style/GPUI guards, formatting and diff review.
Full clean-commit CI and native acceptance remain separate gates.

For the combined native build, verify the row stays at the bottom while scrolling
many tracks, including with the detail panel opened; the last regular track is
reachable. At minimum window size, check browser width, timeline/header alignment
and master controls. Drop a browser sample into trailing empty regular space,
then try a drop on main: only the regular drop should create a track. Existing
clip and captured time-selection drags should keep working across boundaries.

## project time selection

Pulse 14 treats main as an editing proxy over all canonical project tracks. A
point click places the cursor across those tracks; dragging paints one steady
time rectangle, preserving gaps. The upper-third hover handle moves that whole
rectangle horizontally; Option copies it. The lower region paints a new range.
Copy/cut/replace-paste and ripple insertion reuse the existing bounded worker
commands, immutable media and one Undo, without turning main into a TrackId.
An empty project has no valid proxy range. Topology changes reconcile the scope
with the current project; future canonical returns join it when implemented.

Command-E on the main cursor dispatches `SplitTracksAtFrame`. The project
validates a nonempty unique track set, converts the cursor frame to musical time
once, and splits strictly crossing audio/MIDI clips on a private candidate. A
later failure leaves the complete original intact, including direct Project
application; touching clip boundaries is not a split. One Editor command means
one history entry. Ordinary single-clip splitting retains its existing policy.

Focused source checks cover mixed audio/MIDI atomic splitting and one Undo on
100 tracks. They do not establish native input-to-paint latency or physical
audio deadlines. After storage recovery, all four project split tests, seven main-row UI tests
and eighteen edit-cursor tests pass. Empty/stale topology and explicit move-scope
retention regressions pass, along with strict project/UI Clippy and source guards.
Full local CI passed all 14 steps on clean `373e8c2` in 276.455 s, with
unchanged source digest and `stale=false`. Evidence:
`artifacts/local-ci/20261009T182718Z-b28879d0/summary.json`.
[PR31](https://github.com/PORTALSURFER/hex/pull/31) is source-complete; native
interaction and dense 1/100-track feedback/completion measurements remain open. In the
combined build, select time on main, copy/cut/paste and ripple-paste over gaps,
move through the upper-third handle, Option-copy, and split a main cursor across
audio and MIDI. Check Undo, selection after track deletion, and empty projects.
