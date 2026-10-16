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
and lifecycle tests pass. On 11 October 2026 the user reported native cursor
appearance/restoration, typing and Undo passed and requested merging PR155.
See device-controls.md for the supplied native checklist.

## rounded device layout scene evidence — 16 october 2026

The existing GPUI test-support patch now skips transparent border-only fills in
`rendered_background` and exposes `rendered_surface_geometry` (filled quad corner
radii and ancestor mask, in scaled pixels). This corrects background inspection
when `Style::paint` appends an outline quad after content. Only
`src/app/test_context.rs` changes; published archive/license/original hashes stay
pinned, and patched hash/cumulative diff are refreshed. Rendered widget tests
exercise filled surfaces with structural borders and oversized content. Scene
geometry and hit masks are evidence of GPUI containment; GPU antialiasing,
physical display pixels and native visual acceptance remain separate.


## disposable native profiler input adapter — 16 october 2026

Primary boundary decision for issue #14's supplementary timing harness: add a
safe public `Window::dispatch_input(PlatformInput, &mut App) -> ()` adapter. The
existing public `dispatch_event` returns private propagation metadata and cannot
be called from Hex. The adapter forwards to exactly that existing dispatch path
and discards the private return value; `DispatchEventResult` remains private.
There is no native-handler, FFI, scheduling or OS input injection change and no
unsafe Hex code. Only the explicit disposable profiler calls the adapter; normal
editing and event delivery are unchanged. Provenance and cumulative diff retain
the verified 0.2.2 archive and existing patch scope. The optimized native build
and source guards compile/check this boundary; measured scene paint/submission
observations do not establish physical input or physical display latency.

## cached scene debug-selector replay — 16 october 2026

Primary-approved narrow test-support decision for #2648 / PR264: preserve debug
selector bounds when `AnyView.cached` replays paint. `Frame` retains an
insertion-history map under the existing test/test-support cfg; `PaintIndex`
records its offsets and `reuse_paint` restores only the cached segment. This
keeps nested reuse and duplicate overwrite order correct without restoring
unrelated removed selectors. Existing per-frame clearing remains effective.
The patch is limited to safe Rust metadata in `src/window.rs`; normal builds
have no added field, operation, public API, native access or input behavior.
Published archive identity and original hashes stay pinned; the cumulative
patch and patched window hash are regenerated from that verified archive.

A rendered nested-cache test proves retained selectors across genuine scene
reuse with unchanged child render counts, current duplicate overwrite order,
moved child bounds and selector removal. The previously failing audio-trim
fixture now finds its cached handle and completes its ordinary drag with cache
reuse enabled. This test metadata is not physical display or native OS evidence.

## stable cached-view invalidation — 16 october 2026

Primary approved one additional safe GPUI boundary for #2648 after a rendered
regression proved that replacing the cached child identity on a same-revision
read-only snapshot refresh lost an otherwise-current clip mouse-down. The same
gesture passed with the original direct builder. `AnyView::cached_with_epoch`
adds an opaque `u64` caller epoch to the existing cache key so changed content
rebuilds in the current draw without replacing view/element identity. Existing
`cached` callers use zero and keep their behavior. Changing the epoch does not
schedule a frame; Hex notifies the live editor owner. No dirty notification is
queued from child emission, and no native/unsafe/event-delivery or bulk replay
code changes.

Hex retains a child's identity until its track is removed or the project
generation changes. New immutable snapshots update every retained child to the
current Arc once and prune removed track IDs; old offscreen snapshots are not
retained. Keys and transient fresh-render states advance the epoch. Tests prove
current-frame selected color/viewport geometry, next-frame reuse and preserved
mouse-down/drag capture across a same-revision snapshot refresh. Original archive
hashes remain pinned; only the documented `src/view.rs` addition extends the
provenance guard's approved file scope.

## ordered cached-scene replay — 16 october 2026

Primary approved the private GPUI scene/bounds-tree optimization for #2648 only
after the optimized headless fixed-fixture run measured zero clip rebuilds yet
spent most of its frame replaying cached clip paint. A validated cached range can
reserve one conservative union bound/order interval, then retain its internal
order differences, mixed-kind ties and original operation stream. It does not
place an entire track into a shared paint layer or change live clip geometry.
The original replay loop is the fallback for old/current external layer contexts,
unbalanced contained layers, invalid geometry, empty visible content or checked
order reservation refusal. Refusal makes no scene/tree mutation. Reservation
leaves room for one following live order; it does not claim arbitrary future
order-exhaustion protection for the legacy renderer.

`scene.rs` and `bounds_tree.rs` are the only added production patch files.
The already patched `TestAppContext` gains one test-support-only verification
entry point returning `Result<(), String>`; deterministic std-only fixtures
exercise private GPUI code through Hex's root lockfile, without adding GPUI dev
dependencies or testing another dependency graph. The retained original replay
is the oracle for payload/layer streams, PathIds and actual batch painter order
over every elementary overlap cell, including alpha/ties, 1×/2× bounds, multiple
cached ranges, fresh live overlays and eight cached generations with stable
order spans. Invalid layer/range/geometry and overflow cases verify no mutation.
No unsafe/native/API-to-renderer data change is introduced. The macOS Surface
branch keeps the original shared append logic; native image-buffer rendering
has no dedicated fixture here. Pinned archive/original hashes are preserved and
the cumulative diff/hash guard includes the explicitly approved files.

Source verification does not establish a speedup or native frame budget. The
next optimized headless replay comparison and canonical native 360-sample run
remain required; their distinct measurement limits remain unchanged.
