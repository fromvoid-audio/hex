# arrangement overview

Implemented source, 9 October 2026; native visual acceptance pending.

A compact 24-pixel map sits above the Arrangement ruler. It always fits the
project timeline and includes every track and both audio and MIDI clips. A
128-column by 16-row occupancy raster aggregates dense tracks; audio, MIDI and
mixed occupancy use distinct colors. MIDI positions follow the canonical tempo
map. The viewport outline shows the visible time range; clicking centers that
range at the selected project position, clamped at timeline limits.

The thumbnail is cached by project generation, view revision, timeline horizon
and track count. Panning reuses the cache; edits rebuild it. Drawing merges
adjacent occupied cells and has bounded raster work. This avoids scanning the
entire project on each pan or paint, but edit-time cache construction still
visits project clips. Native paint timing and large-project responsiveness are
not yet measured. Empty projects retain the compact strip and ruler alignment.

No project schema, history, audio scheduling or transport behavior changes.
The bottom scrollbar request was cancelled; no handle is added.

## native review

1. Check the map above the ruler with audio and MIDI on several tracks. Clips
   near the project end must remain visible while the main timeline is zoomed in.
2. Zoom and pan: the thumbnail stays fully zoomed out while its viewport outline
   moves. Click near the beginning, middle and end; navigation clamps cleanly.
3. Add, move and remove clips/tracks: the thumbnail should update immediately.
   Check a dense project and a project with no tracks.

Source tests cover last-track inclusion in a 100-track model, tempo-mapped MIDI,
mixed occupancy, cache reuse/invalidation, bounded raster and rendered click
geometry. User inspection owns the actual native appearance and interaction
verdict. See arrangement-navigation.md and native-input.md for related checks.

Verification: 228 UI tests pass, one existing ignored. Strict UI Clippy, desktop
build, formatting, architecture/UI-style/GPUI-patch guards and diff checks pass.
The final band candidate budget is bounded even when conversions yield no
painted bands. Existing upstream dependency compiler warnings remain; no Hex
warning or check failure. Native interaction/appearance verdict remains open.
