# hex UI iteration 2

Accepted by the user on 8 October 2026. This frozen snapshot is the visual
reference for subsequent native GPUI implementation. Frame revision: hex-live-r66.

## Reference files

- `arrangement.png` and `session.png`: full-size 1900 × 1120 playback concepts.
- `hex.frame.json`: portable editable Frame project, including view states.
- `hex-live-r66.json`: structured geometry and prototype state document.
- `theme-palette.json` and `theme-palette.png`: theme tokens and color board.
- `live-layout-styleguide.json`: compact layout and style constraints.
- `clip-colors.json`: custom color picker with at most 64 user-saved swatches.
- `review.json`: zero reported visual issues; eight prototype scenarios passed.
- `manifest.json`: revision identity and SHA-256 hashes of saved source/artifacts.

## Native implementation handoff

Preserve the compact Arrangement/Session composition, contiguous panels, small
controls, dark dividers and individual darker tag rectangles. Use warm charcoal
backgrounds (#2a2828), surfaces (#2f2d2d), wells (#242222), muted off-white text
(#c3c5c0), primary orange (#f06449), secondary cyan (#00e8df), and tertiary ice
blue (#79aebc). Quiet blue (#8faaba) supports parameters. Strawberry is removed.
Use Yield-style library selection: faint orange fill with a thin orange left marker.
Concept clips use harmonious gray (#454c49); user colors are independent of theme.
All app-owned text remains lowercase through hex-ui's text helper, using bundled
Ioskeley Mono. Preserve existing native capabilities and canonical command/state
boundaries; JS/HTML are design tooling only.

Meters use opaque continuous stereo bars, orange upper regions and red peaks,
with peak markers. Prototype levels are static simulation; the visual thresholds
are not calibrated dB values. Native metering should use real telemetry and a
separately defined scale. Play/Stop visibility is illustrative design behavior.

Routing, sends, device and clip editing controls are visual references, not evidence
of implemented functionality. Do not add decorative native controls that pretend
to work. Native implementation requires a scoped behavior mapping and rendered
window checks; this acceptance does not establish native parity, accessibility,
small-window behavior, audio behavior or user-swatch persistence.

Keep this snapshot unchanged when iterating. Future designs should receive a new
iteration folder; the working files in design/frame may continue evolving.
