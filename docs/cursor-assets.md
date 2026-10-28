# editing macOS cursor artwork

The app-owned resize bracket and loop cursor sources live in
`assets/cursors/resize-left-bracket.svg`, `resize-right-bracket.svg`, and
`loop.svg`. Edit those SVGs directly, keeping the 24 by 24 viewBox. The cursor
hotspot is the center at (12, 12) logical points. The native GPUI cursor owner
rasterizes each vector at 48 by 48 pixels and displays it at 24 by 24 points;
retain a transparent background and enough contrast over light and dark clips.
The vectors reproduce the original Hex cursor geometry previously drawn by the
macOS GPUI patch; they do not contain operating-system artwork.

Rebuild and relaunch hex after editing; artwork is embedded at compile time and
is not live reloaded. Run `cargo test -p hex-ui --test cursor_assets --locked`
to check decoding, transparent background and light/dark strokes at 1× and 2×.
The source check does not verify the macOS pointer position or appearance.

The arrow, hand, crosshair, resize and other OS-provided cursors retain their
native artwork. The invisible cursor is a transparent native cursor, not a
user-editable drawing. Other app icons and font glyphs are not yet represented
by these cursor assets.
