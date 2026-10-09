# hex external GPUI patch

Published gpui 0.2.2, Apache-2.0; upstream files and LICENSE-APACHE retained.
See `hex-provenance.json` for verified crates.io archive checksum and original
file hashes; `hex-patches.diff` records the exact input/cursor delta. Upstream's
published VCS metadata includes dirty=true, so the archive checksum, not a
claim of a clean git commit, identifies this baseline.

Changes: forward right/other mouse drag callbacks on macOS; preserve the
per-event device-direction inversion flag; offer opt-in physical_delta() while
keeping ordinary scroll delta unchanged; initialize the flag false on other
platforms. AppKit access stays inside GPUI's existing native boundary.

This is an excluded external dependency, not a Hex workspace crate. Hex crates
retain unsafe_code=forbid. Do not edit the machine's registry cache, add app FFI,
or grow this patch beyond the documented input/cursor repair. See docs/native-input.md
and run python3 scripts/check-gpui-patch.py from the Hex workspace.

For refresh: obtain a verified published upstream archive; review whether both
fixes are upstream; replace baseline deliberately, reapply only necessary delta,
regenerate provenance/diff and run native-input checks. Never apply formatting
or unrelated refactors to the vendor tree.

9 October cursor extension: original left/right bracket CursorStyle variants,
public AppKit vector cursor drawing retained in two MacPlatformState StrongPtr
fields, native directional fallbacks elsewhere. Test-only cursor state readout
supports deterministic hover assertions; it is not an app control protocol.
License/source headers and original baseline hashes remain unchanged.
