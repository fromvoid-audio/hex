# hex external GPUI patch

Published gpui 0.2.2, Apache-2.0; upstream files and LICENSE-APACHE retained.
See `hex-provenance.json` for verified crates.io archive checksum and original
file hashes; `hex-patches.diff` records the documented input, cursor, and
titlebar-control and explicit grid-literal deltas. Upstream's
published VCS metadata includes dirty=true, so the archive checksum, not a
claim of a clean git commit, identifies this baseline.

Changes: forward right/other mouse drag callbacks on macOS; preserve the
per-event device-direction inversion flag; offer opt-in physical_delta() while
keeping ordinary scroll delta unchanged; initialize the flag false on other
platforms; clear rendered debug selector bounds between test-support frames.
AppKit access stays inside GPUI's existing native boundary.

This is an excluded external dependency, not a Hex workspace crate. Hex crates
retain unsafe_code=forbid. Do not edit the machine's registry cache, add app FFI,
or grow this patch beyond the documented input, cursor, titlebar, test-support,
explicit grid-literal and bounded accessibility decisions.
See docs/native-input.md and run python3 scripts/check-gpui-patch.py from the Hex
workspace.

For refresh: obtain a verified published upstream archive; review which documented
changes are upstream; replace baseline deliberately, reapply only necessary delta,
regenerate provenance/diff and run native-input checks. Never apply formatting
or unrelated refactors to the vendor tree.

9 October cursor extension: original left/right bracket CursorStyle variants,
public AppKit vector cursor drawing retained in two MacPlatformState StrongPtr
fields, native directional fallbacks elsewhere. Test-only cursor state readout
supports deterministic hover assertions; it is not an app control protocol.
License/source headers and original baseline hashes remain unchanged.

9 October custom title controls: `TitlebarOptions::hide_native_window_controls`
opts selected macOS windows out of the three standard AppKit titlebar buttons.
The existing native titlebar, movement and resizing configuration remain in
place; default windows retain native controls. Hex draws its compact replacement
controls in the reserved titlebar strip. No app-side native calls or unsafe code
were added. The source change is confined to `platform.rs` and
`platform/mac/window.rs`; provenance hashes and the cumulative patch were
regenerated from the verified published 0.2.2 archive.

11 October debug selector lifecycle: clear `Frame::debug_bounds` when clearing
the frame under the existing test/test-support cfg. This keeps visual-test
selectors aligned with the most recently rendered frame and does not affect
production layout or input behavior.

11 October compiler compatibility: explicitly type the grid repeat helper's
zero minimum and one-fraction literals as f32. Pinned Taffy 0.9.0's `length`
and `fr` helpers accept `Into<f32>`; this preserves the same values/layout and
removes Rust 1.98.1's float fallback warning without suppressing the lint.

11 October value-drag cursor: cached transparent `CursorStyle::Hidden` on macOS,
arrow fallback elsewhere. Window-scoped frame override persists outside hover
bounds and restores through deactivation/removal/destruction, without NSCursor
hide-count changes. Test-only platform cursor readout verifies restoration after
rendered gesture termination; native appearance/delivery acceptance is separate.

13 October painted-background test support: `VisualTestContext::rendered_background`
reads the last completed scene quad matching an element's debug bounds. It neither
draws another frame nor injects pointer input, so tests can distinguish actual
paint from focus state that has not reached the screen. Test-support only; no
production rendering or native graphics behavior changes.

14 October accessibility extension (ticket29/PR188): safe bounded semantic
snapshots/actions, cached prepaint integration and a native NSAccessibilityElement
backend. The reviewed file list and added accessibility.rs are explicitly hashed;
all original baseline hashes, license and prior input/cursor deltas remain.
See docs/native-accessibility.md in the Hex repository for owner/revision,
revocation, asynchronous dispatch, offscreen refusal and native acceptance limits.
The unified diff uses zero context; apply only against the verified archive with
`git apply --unidiff-zero`. Hash verification remains mandatory.
