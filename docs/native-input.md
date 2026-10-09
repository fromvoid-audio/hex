# native mouse input boundary

Primary architecture decision, 9 October 2026: retain GPUI 0.2.2 and apply a
minimal, pinned external dependency patch. Hex crates continue to forbid unsafe
code. No new app FFI, framework, event monitor or polling workaround is added.

The published package is copied to `third_party/gpui`, excluded from the Hex
workspace, and resolved by `[patch.crates-io]`. Apache-2.0 license and upstream
source headers are retained. This remains the existing native GPUI boundary,
not an exception allowing unsafe Hex code. Exact upstream archive checksum,
original/patched file hashes and the exact input/cursor delta are recorded there. The
published VCS metadata reports dirty=true; the verified crates.io archive
checksum is the baseline identity. Run `python3 scripts/check-gpui-patch.py`.

## defects and repair

The pinned macOS GPUIView registers middle-button down/up but omits
`otherMouseDragged:`. Its event converter already supports NSOtherMouseDragged.
Add that callback (and the adjacent rightMouseDragged counterpart) to the
existing native event handler. Headless UI simulation bypassed this callback
registration, explaining why previous panning tests passed without proving
physical input. The user reported actual middle panning did not work.

ScrollWheelEvent discarded NSEvent's per-event device inversion flag. Preserve
it as `direction_inverted_from_device`, false by default/other platforms, and
provide safe opt-in `physical_delta()`. It reverses inverted axes without
mutating raw `delta`; browser/panel scrolling keeps OS-normal deltas. Only
Arrangement/ruler zoom uses the physical delta: positive Y zooms in, negative Y
zooms out. Read the flag on every event; no preference polling or restart is
needed. No registry cache modifications.

Apple documents preference-based delta inversion and using this flag to
compensate when an interaction should ignore that preference:
https://developer.apple.com/documentation/appkit/nsevent/isdirectioninvertedfromdevice

## acceptance and maintenance

Test Pixels/Lines with the flag true/false, both signs, raw delta preservation,
zoom mapping and browser isolation. Build the macOS target to compile the native
selector/getter boundary. Keep prior capture/release/cancellation tests. Verify
manifest hashes and workspace exclusion; review the exact dependency diff.

The user performs physical wheel and middle-button checks, including Natural
Scrolling on/off and release outside the window. Source/headless tests do not
prove native event delivery or hardware-button mapping. Do not modify their
system settings or operate their active apps during this review.

Upstream refresh must deliberately verify the archive, review whether fixes
are available, reapply only necessary changes, update provenance/diff and run
input checks. Do not expand the vendor patch with unrelated code/formatting.


## original bracket cursors — 9 October 2026 decision

The user accepted full-height edge resizing and requests distinct left/right
bracket cursors resembling Live. Extend the existing excluded GPUI boundary
with original shape rendering and dedicated CursorStyle variants. Retain two
macOS cursor objects on the UI-owned platform, use public NSImage/NSCursor image
and hotspot APIs, and avoid repeated construction on hover. Other platforms
fall back to native directional resize cursors. This grants no unsafe Hex
exception or general vendor expansion; exact files/hashes stay verified.
Physical appearance, scaling and hotspot acceptance are user native checks.
