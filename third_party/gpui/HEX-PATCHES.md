# hex external GPUI patch

Published gpui 0.2.2, Apache-2.0; upstream files and LICENSE-APACHE retained.
See `hex-provenance.json` for verified crates.io archive checksum and original
file hashes; `hex-patches.diff` records the documented input, cursor, and
titlebar-control delta. Upstream's
published VCS metadata includes dirty=true, so the archive checksum, not a
claim of a clean git commit, identifies this baseline.

Changes: forward right/other mouse drag callbacks on macOS; preserve the
per-event device-direction inversion flag; offer opt-in physical_delta() while
keeping ordinary scroll delta unchanged; initialize the flag false on other
platforms. AppKit access stays inside GPUI's existing native boundary.

This is an excluded external dependency, not a Hex workspace crate. Hex crates
retain unsafe_code=forbid. Do not edit the machine's registry cache, add app FFI,
or grow this patch beyond the documented input, cursor, and titlebar decisions.
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
