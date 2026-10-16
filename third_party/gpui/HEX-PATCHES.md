# hex external GPUI patch

Published gpui 0.2.2, Apache-2.0; upstream files and LICENSE-APACHE retained.
See `hex-provenance.json` for verified crates.io archive checksum and original
file hashes; `hex-patches.diff` records the documented input, cursor, and
titlebar-control, view-cache epoch, scene replay and explicit grid-literal deltas. Upstream's
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
view-cache epoch, scene replay and explicit grid-literal decisions.
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

16 October cached debug selectors: under the same test/test-support cfg, retain
each frame's selector insertion history and include its range in `PaintIndex`.
Cached paint replay restores only that range, preserving nested replay and
duplicate-selector paint order while frame clearing removes absent selectors.
Only `src/window.rs` changes; no production runtime, public API, native or unsafe
boundary changes. Hex's real cached-view regression covers nested reuse without
extra renders, duplicate overwrite order, movement and removal. The existing
audio-trim regression locates and drags the cached handle without disabling cache.

16 October view-cache epoch: `AnyView::cached_with_epoch(style, u64)` includes an
opaque caller epoch in the existing view-cache key. A changed epoch rebuilds in
the current draw while retaining the entity/global element identity and pending
input state. Existing `cached(style)` uses epoch zero, preserving its behavior.
Epoch changes do not schedule a draw or enqueue notifications; the owner does
that. Default construction and weak upgrades start at zero; cloning and failed
downcasts retain the supplied epoch. The change is confined to `src/view.rs`,
with no native, unsafe, scheduling, event-delivery or scene-replay implementation
change. Hex tests cover same-draw rebuilding, subsequent reuse, and preserved
mouse-down capture through a same-project immutable snapshot refresh.

16 October ordered scene replay: `Scene::replay` may reserve one conservative
bounds-tree interval for an already ordered cached subtree. Eligibility requires
no current external layer, no old external layer at range entry, a balanced
contained layer stream, finite nonnegative geometry and a finite union of clipped
primitive/layer bounds. Checked global order arithmetic, including one following
live order of headroom, precedes any mutation. Source order differences and ties
are translated without collapsing them; typed buffers and paint operations share
the original append logic and paths receive fresh IDs. Contained Start/End
operations remain in the stream. Refusal uses the unchanged per-operation replay.
No whole-row layer shortcut, native calls, unsafe code or new dependency is added.

The implementation is confined to `src/scene.rs` and `src/bounds_tree.rs`;
one test-support-only `TestAppContext` verifier in the already patched
`src/app/test_context.rs` reaches deterministic private fixtures using Hex's
existing locked graph. It avoids GPUI's separate, unresolved development-only
test dependencies. The oracle compares actual legacy/fast batch painter
sequences in every elementary overlap region for six allocatable primitive
kinds, translucent payloads, mixed-kind ties, 1×/2× geometry, nested layers,
multiple ranges, future overlays, eight repeated cached generations and legacy
fallback. It checks stable spans, fresh PathIds and mutation-free refusal for
invalid layer contexts/ranges/geometry and near-order-overflow reservations.
macOS native image-buffer Surface allocation is outside this fixture; its
unchanged shared append branch remains compiled, without a fake native handle.

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

16 October rounded-layout test support: `rendered_background` skips transparent
border-only quad fills, which `Style::paint` emits after the filled surface.
`rendered_surface_geometry` exposes the last filled matching quad's corner radii
and ancestor mask in scaled pixels. Both read only the completed test scene;
no production paint, native API, cursor or input behavior changes. Hex widget
regressions cover opaque fills followed by same-bounds border-only quads, rounded
surface geometry, oversized content masks and hit exclusion at constrained sizes.


16 October disposable profiler input: safe public `Window::dispatch_input`
forwards to the existing `dispatch_event` and discards its private metadata.
The result type stays private. No native handler, FFI, scheduling, OS-event
injection or default behavior changes. This primary-approved adapter lets the
explicit native timing window exercise the regular mouse dispatch path.
