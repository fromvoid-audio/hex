# editing browser place icons

The seven browser place icons live in `assets/icons/browser-place/`. Their file
names correspond to the stable preference IDs `folder`, `samples`, `devices`,
`projects`, `collection`, `waveform`, and `search`. The place rows, icon grid,
browser section choices, and sample and project result rows use this same
embedded catalog. Unknown saved IDs
keep their stored value and display the folder fallback until changed.

Edit the SVG for the desired ID, keeping a 16 by 16 viewBox and transparent
background. The renderer uses the vector as a mask in the current theme text
color; use shape opacity rather than relying on SVG colors for contrast. Rebuild
and relaunch hex to see edits; the artwork is compiled in, not live reloaded.
Run `cargo test -p hex-ui --test browser_place_assets --locked` to check the
catalog and 1×/2× raster output. Native appearance and interaction still need a
visual check in the optimized app.

Hosts embedding hex-ui install `hex_ui::assets::HexAssets` with GPUI's
`Application::with_assets` before opening an editor. Editor entry points refuse
to open when the expected embedded catalog is missing or altered.

These original Hex vectors replace only the browser place catalog. Other app
icons and font glyphs are not yet covered; system-owned cursor artwork remains
outside this library. See [cursor artwork](cursor-assets.md) for the custom
macOS cursor sources.

The editor transport playback controls use `assets/icons/transport/play.svg`,
`pause.svg`, and `stop.svg`. These use the same 16 by 16 mask and rebuild
workflow. Their current text color follows the action surface, including hover;
other transport glyphs have not yet moved to the vector library.

Browser folder disclosure arrows use `assets/icons/browser-disclosure/` for
collapsed and expanded folders. Loading and error indicators remain text so
those states stay distinct from the artwork.

The split, undo, and redo transport actions use `split.svg`, `undo.svg`, and
`redo.svg` in the same transport asset directory. Their hit targets and disabled
state continue to be owned by the transport controls.

The transport add track, add MIDI clip and remove track artwork is in
`add-track.svg`, `add-midi-clip.svg` and `remove-track.svg` beside these icons.
