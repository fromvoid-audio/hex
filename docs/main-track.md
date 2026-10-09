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
Main-project time selection (ticket 14) and real return routing/rows (13) follow as
separate focused features.

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
