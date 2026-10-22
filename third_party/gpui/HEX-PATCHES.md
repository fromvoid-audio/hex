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
explicit grid-literal and bounded accessibility decisions.
view-cache epoch, scene replay and explicit grid-literal decisions.
See scripts/check-gpui-patch.py and run python3 scripts/check-gpui-patch.py from the Hex
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

`rendered_filled_quads` also exposes filled canvas fragments within a debug
element, clipped to the completed scene's content/replay masks. It supports
pointer/worker tests of primitives without inventing per-primitive UI elements.
It remains test-support only and is not native framebuffer readback.

14 October accessibility extension (ticket29/PR188): safe bounded semantic
snapshots/actions and cached prepaint integration. The first native AppKit
candidate is quarantined for an off-UI final-owner hazard; no native AX backend
is enabled at this checkpoint. The reviewed file list and added accessibility.rs are explicitly hashed;
all original baseline hashes, license and prior input/cursor deltas remain.
See third_party/gpui/src/accessibility.rs in the Hex repository for owner/revision,
revocation, asynchronous dispatch, offscreen refusal and native acceptance limits.
The unified diff uses zero context; apply only against the verified archive with
`git apply --unidiff-zero`. Hash verification remains mandatory.

14 October replacement AppKit accessibility boundary: data-only immutable snapshots,
revocable bounded mailbox and UI-only generational native registry in explicitly
listed added platform files. PlatformWindow revokes before core removal; accepted
native close and Drop/dealloc revoke before callbacks/resources. Main-queue wake
contains checked IDs only. No weak/strong MacWindowState is reachable from AX
objects. Scalar queries are snapshot-only; off-UI parent/children return unavailable.
The original unsafe-lifetime candidate remains quarantined. This native source
requires exact Mac compilation/AX teardown/veto/VoiceOver checks; Linux tests do
not establish OS exposure. All baseline identity/hashes and prior patches remain.

14 October native teardown repair: clear semanticBridge/semanticState through the
Objective-C runtime before reclaiming their owning Box. The private helper and
real-ivar reentrant-drop/uncleared-pointer negative control stay inside the existing
Mac accessibility patch. Native execution remains a separate required Mac check.
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

17 October AppKit getter repair: register the SDK's isAccessibilityHidden,
isAccessibilityEnabled and isAccessibilityFocused selectors. A Mac-only real
semantic-object regression verifies enabled/focused snapshot changes and revoked
hidden/unavailable state; the old selectors fail it. All four native fixtures
pass with Hex's macos-blade features. Inspector/VoiceOver acceptance remains open;
see third_party/gpui/src/accessibility.rs. Original archive identity remains unchanged.

17 October clip-border test support: `rendered_border` reads the last matching
completed scene quad with nonzero border widths and returns its color, widths
and ancestor mask. It is confined to the existing test-support context and
changes no production rendering, native behavior, layout or input. Clip selection
regressions use it to verify the actual painted outline and viewport clipping.
Original baseline hashes remain unchanged; the cumulative patch is regenerated
against the hash-verified original test_context.rs.

18 October project Open test support: the existing deterministic test platform now
queues Open picker requests and lets tests complete them with paths or Cancel.
TestAppContext exposes the matching queue/selection methods. This replaces an
unimplemented test-platform leaf; production native picker APIs and behavior are
unchanged. Actual window dispatch, unsaved Save/Don’t Save/Cancel and canonical
save/open events can be tested without touching a user document. Injected picker
results are not evidence of native macOS picker presentation or physical input.

18 October clip-bottom-border test support: `rendered_quad_paint_layers` reads the
completed scene's quad draw order, stable position and masks for all matching
fragments of a debug element, including retained cache replay.
Combined with existing painted border widths, ancestor mask and real separator
bounds, this detects an occluding later row separator. It adds no production
paint/layout/input behavior or native framebuffer claim. Original archive and
source hashes remain verified; the cumulative patch is regenerated.

19 October project picker directory: additive
`App::prompt_for_paths_with_directory(options, Option<&Path>)` delegates through
Platform with ordinary-picker default behavior on other platforms. macOS applies
the optional NSOpenPanel directoryURL before opening; old Open calls retain their
behavior. Save and Open share a filesystem-representation NSURL helper preserving
native bytes and refusing NUL, replacing Save's lossy display conversion. No
security, permission or directory-creation policy changes. TestPlatform records
Open directory and Save directory/suggested-name arguments and supports injected
cancel/error results; these are not native picker acceptance. A test-support Mac
NSURL roundtrip helper shows/configures no panel. Original archive/file hashes,
license and prior deltas remain; the cumulative patch is checked by applying it
to the hash-verified original archive and comparing every patched/added byte.

18 October opt-in bundled-font test support: `TestAppContext::use_native_text_system`
selects the existing macOS `MacTextSystem` before any test windows are created.
Both the App and test-context shaping references receive the same instance.
Existing no-op test contexts remain unchanged. The method rejects replacement
beneath live windows; it creates no OS window, registers no system font and adds
no FFI or unsafe code. EQ tests still require the real embedded Ioskeley family;
this corrects their otherwise impossible native assertion against NoopTextSystem.
Native shaping is not evidence of GPU pixels, audio or physical interaction.

18 October EQ wheel momentum metadata: additive `ScrollWheelEvent::is_momentum`
(default false) retains the platform-reported inertial continuation independently
of physical touch phases. macOS reads the existing `NSEvent::momentumPhase`
inside the existing native input boundary; other platform constructors initialize
false. Raw delta, physical_delta, phase mapping and ordinary content consumers
remain unchanged. Only owner-opted-in parameter graph scrolling uses this bit to
refuse inertial parameter steps/new history gestures after physical Ended. No new
FFI, framework, timer, scroll system or production test-context change. Existing
published baseline hashes verified; unrelated cumulative patch blocks preserved.

19 October EQ terminal/tick metadata: preserve NSEventPhaseCancelled as an
additive TouchPhase::Cancelled. Add optional wheel_ticks and an opt-in physical
helper; retain X11 increments, Wayland discrete/value120 information (including
frames also carrying pixel deltas), and Windows WHEEL_DELTA fractions before
content-line preferences. Raw delta, content scrolling and unknown/Pixels
fallback remain unchanged; macOS tick metadata stays None rather than guessing
an undocumented tick law. Existing constructors/matches are audited together.
Only the opted-in parameter graph uses this metadata; no native framework,
FFI, callback owner, clock, timeout or general scrolling policy is introduced.
Original archive/file hashes and unrelated cumulative patch blocks are retained;
the cumulative patch is regenerated against hash-verified original source and
applied back to verify every changed/added byte. Linux synthetic checks do not
claim macOS/Windows compilation or physical device acceptance.

19 October Wayland v9 per-axis wheel direction: the existing pointer-frame
fold retains raw content accumulation and source/Shift routing. Optional
parameter tick metadata is normalized per original protocol axis before routing
collapse, with frame-local constant-size bookkeeping. Conflicting/unknown axis
direction reports fall back to the old sign for that axis only; other known axes
remain independent. Every Frame drains metadata before any window delivery,
including empty or unfocused frames. The event-wide inversion flag stays false;
physical_delta, the physical tick getter and other platforms are unchanged.
Continuous-only frames do not invent detents. Typed protocol-event unit tests
exercise the production-used fold; they are not compositor/hardware validation.
Original source hashes, license and unrelated cumulative patch blocks remain;
only changed-source provenance hashes are updated and the patch is reapplied
against verified originals to compare every patched/added byte.
