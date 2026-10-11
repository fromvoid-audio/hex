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

## custom window controls — 9 October 2026 boundary decision

Pulse ticket 11 and the user's explicit custom-traffic-light request authorize
this focused extension of the existing excluded GPUI dependency. Hex workspace
crates continue to forbid unsafe code. Add only an opt-in titlebar flag, default
false, which hides the standard macOS close/minimize/zoom buttons using public
AppKit APIs. Retain the native titled, movable and resizable transparent window;
reapply hiding through the existing traffic-light layout/transition path. Keep
all default/non-opt-in behavior and the generated demo's native controls.

App circles use current safe GPUI minimize/fullscreen/zoom APIs. Custom Close
must invoke the editor's `request_window_close` and remove the window only when
it returns true; asynchronous Cancel/Discard continues to own dirty/pending work.
The same close guard remains installed for OS close requests. Bootstrap has no
editable project and follows the existing clean-window close lifecycle.

Retain native titlebar dragging and system double-click behavior. Do not add an
app FFI bridge, event monitor, manual position loop or synthetic release. GPUI's
current `start_window_move` is a no-op on macOS. Apple's separate
[performDrag API](https://developer.apple.com/documentation/appkit/nswindow/performdrag%28with%3A%29?language=objc)
may omit mouse-up delivery; adding it would require an additional input-lifecycle
design and is outside this change.

The source scope is GPUI `platform.rs` and `platform/mac/window.rs`, already in
the pinned patch set. Preserve baseline checksums and license notices, regenerate
the cumulative diff and patched hashes, and retain the source guard. Native
button hit-testing, dragging, resizing, minimizing, fullscreen transitions and
Cancel/Discard remain user acceptance; source tests cannot prove AppKit behavior.

## rendered debug bounds — 11 October 2026

GPUI's test-support `debug_bounds` map is populated while painting the next
frame. Clear it with that frame so selectors absent from the newly rendered UI
cannot expose bounds left over from an earlier frame. Production behavior is
unchanged because the map and clear are both gated by GPUI's existing test/test-
support cfg. Hex's device-panel UI test covers a rendered device removal and
asserts that both its action selector and device-frame selector disappear.

## grid helper compiler compatibility — 11 October 2026

Pulse2318 is a bounded child of dependency-warning follow-up2143. The pinned
Taffy0.9.0 `style_helpers::{length, fr}` inputs are `Into<f32>`. Explicit f32
suffixes preserve GPUI's zero minimum/one-fraction grid-repeat values, removing
Rust1.98.1's float-literal fallback warning without changing layout or adding
lint suppression. The cumulative vendor diff and patched hash include only this
two-literal source change; published baseline hashes/license remain unchanged.
The block/proc-macro-error future-compatibility notices remain open in2143.
Validation: the provenance guard, full exact-source local CI and a fresh release
build cover this maintenance change; it introduces no new native interaction.

## value-drag invisible cursor — 11 October 2026

Pulse2340 authorizes a focused safe-widget/excluded-GPUI cursor extension.
`CursorStyle::Hidden` selects a cached one-pixel transparent AppKit cursor during
an active shared numeric value drag. It does not use NSCursor hide/unhide or
cursor warping. The owning Window retains a restoration guard only while the
rendered override exists and the window is active. Arrow restoration applies
outside hovered bounds, at deactivation/removal and on guard destruction.
An inactive window cannot install the override. Other platforms use arrow until
separately validated. `None` retains its upstream hide-until-movement semantics.

The narrow native addition remains in platform/mac/platform.rs, with the enum,
window lifecycle and non-macOS match exhaustiveness in existing patched files.
Test-platform cursor readout is confined to test support. Regenerated cumulative
diff/hashes preserve the verified upstream baseline and license. Rendered cursor
and lifecycle tests pass; physical macOS appearance/event delivery remain required
before merging PR155. See device-controls.md for the exact native checklist.
