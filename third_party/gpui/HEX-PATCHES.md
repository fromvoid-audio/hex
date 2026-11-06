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
explicit grid-literal, bounded accessibility and text decoration storage decisions.
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

Live draw order is resolved after best-fit descent, starting at the nearby leaf
and visiting the original sibling subtrees in reverse path order. A high nearby
order can prune older branches sooner. Placement topology, bounds arithmetic and
queried geometry stay unchanged; no placement-path bounds check is introduced,
because floating-point origin + size unions can round an edge inward. Reserved
replay intervals keep their checked admission and skip the live query. A locked-
graph test-support verifier checks insert/reserve/clear/overflow against a linear
scan over deterministic mixed geometry at 1×/2× and a rounded-ancestor case,
alongside the mixed-primitive replay oracle. No balancing, primitive/layer
grouping, native API, allocation or dependency is added.

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

23 October stateless action semantics: the existing bounded accessibility value
contract adds Button, permitting only Focus/Press. AppKit exposes AXButton with no
editable value or value description; value-setting actions refuse. Owner lifetime, revision, snapshot
budgets, revocation and mailbox validation remain unchanged; no new FFI or native
application pointer path. The browser clear action is the first consumer. A real
AppKit semantic-object regression checks role/no-value and action restrictions;
actual Inspector/VoiceOver acceptance remains separate. Original archive hashes
are retained; added-source hashes and cumulative patch are regenerated.

24 October loop cursor: additive `CursorStyle::Loop` uses original circular-arrow
vector artwork, a centered hotspot and cached AppKit ownership alongside existing
bracket cursors. Allocation happens at platform setup, never pointer movement;
headless platforms skip native construction. Linux/Windows retain horizontal
resize fallbacks. No workspace FFI or unsafe code is added. Original published
archive/file hashes and unrelated patch blocks are retained; changed hashes and
the cumulative patch are regenerated from the verified original archive. Native
hover selection is checked headlessly; actual cursor appearance remains a user
interaction acceptance item on the optimized macOS app.

25 October immutable scaled path mesh sharing: Path uses a private owned/shared
vertex store. Mutable logical-pixel construction retains its directly owned Vec,
while scaling keeps original arithmetic/order and publishes a fresh shared
Arc<Vec<_>> array. Cloned scaled Scene paths retain immutable mesh bytes; order,
PathId, mask, color, bounds and contour state remain independent values. Editing
a shared path detaches through Arc::make_mut; normal construction avoids that
atomic work. Existing metal/blade/directx iterators still copy vertex values into
renderer-owned uploads; no borrowed GPU lifetime, native/unsafe call, dependency
or public method change. The root-locked oracle checks clone/original edits,
shared mutation and exact scaled xy/ST/mask bits with retained mixed-alpha/order/
refusal/repeated replay coverage. Full navigation profiles include construction,
sharing and metadata costs, and retain RSS/tails separately from native GPU
acceptance. Published archive/license and unrelated cumulative blocks remain;
changed scene bytes are reproduced by original-archive and offline full guards.

25 October sibling query upper-bound reuse: BoundsTree's private draw-order
query retains both child maxima already loaded for visitation order. After the
first recursive result it visits the second child only if that child's maximum
can improve the result. No new geometric predicate or placement-path pruning,
topology, storage, allocation, reservation or painter-order change. The locked
independent leaf-scan oracle includes higher-child misses, tied maxima and
already-satisfied queries, retaining numeric union/clear/overflow coverage.

### Compact element identities

`GlobalElementId` and Window's active/deferred ID stacks use eight inline
`ElementId` entries instead of 32. The existing SmallVec dynamic spill retains
longer paths without a depth limit or truncation. This reduces inline payloads
moved through Drawable phases and state-map keys. It changes the concrete inline
capacity exposed through GlobalElementId's derived Deref target; callers retain
the same complete sequence, equality/hash/display and push/pop semantics. No
identity interning, shared ownership, lifecycle, dispatch, layout or painter
contract changes. Deep identical-prefix click/focus and held-click repaint tests,
plus keys built in ordinary/forced-dynamic storage, exercise the retained values
and state through the existing root-locked UI test-support graph. Source size
alone is not navigation performance or native acceptance evidence.

### Pointer drag hover state

Window tracks pressed-pointer motion across the shared two-pixel drag threshold
before mouse dispatch. This input state survives consumed move events and cached
view replay, and ends on matching release, button loss, escape or window
inactivation. Drag-state transitions invalidate only views with hover/tooltip paint
dependencies whose painted/current hover can change. Their insertion ranges
replay with cached/deferred paint; group dependencies retain clipped bounds rather
than an external parent's transient hitbox ID. Unrelated clip scenes and ordinary
clicks retain their caches. Div suppresses ordinary/group hover styles and tooltip eligibility
while this state is active. Stored hover callbacks run before mouse dispatch,
independently of propagation, and reconcile at pointer/key terminals. Focus
changes clear drag state without synthesizing hover entry. Callbacks never
mutate view state during paint. Already visible hoverable
tooltips are hidden immediately, and pending tooltip builders do not run during
the drag even when capture swallowed their cancellation event. Typed drag-over styles and
drop handlers keep their existing hit testing and delivery. This is a bounded
pure-Rust input/paint change in the existing dependency; there is no native API,
unsafe Hex code, new UI framework or project/audio contract change.

Window-level state is needed because ordinary Hex capture listeners can stop
propagation without affecting GPUI's style calculation. This state does not grant
exclusive mouse routing, own gesture cancellation or select a gesture cursor;
consumers retain those contracts. Completed-scene regressions check consumed
movement and repeated repaint, tooltip eligibility, release/button-loss/escape/
deactivation recovery, and retained typed drop feedback/delivery. Native pointer
and OS tooltip acceptance remain separate from these headless scene checks.

26 October explicit pointer ownership: Window admits one pressed-button lease
before a control mutates its gesture. Captured mouse input bypasses ordinary/raw
listeners after the initiating press; that press retains gesture-owned ancestor
selection. Keyboard dispatch and the existing typed drag path stay independent.
The owner supplies its mouse handler, cancellation and cursor. Paint records only
a live-held witness, including cached/deferred replay ranges. Missing witnesses,
escape, button loss, deactivation and control-side window removal cancel once
outside paint; normal matching release commits through the owner without cancel.
Cancelled buttons retain bounded inline tombstones until their old release or
next press, preventing trailing actions. Numeric controls use weak entity handlers
and retain their existing typed events, drafts and owner-acknowledgement contracts.
Selection-anchor loss preserves newer text/IME drafts. No native API/FFI or new
unsafe code is added; actual OS interaction acceptance remains separate.

Capture-start input notification also clears framework-only click/drag and active
latches on affected Divs. It uses the existing cached/deferred observer range and
never invokes an action or synthesizes mouse-up. The initiating press's ancestor
selection still runs; an ancestor cannot retain that press and begin a phantom
drag after the exclusive owner releases, including without an intervening repaint.

Pointer-lease Escape sequencing: dispatch the existing keyboard path before
retiring any remaining lease. Gesture key handlers still see held state and keep
their cancellation/propagation guards; a handled Escape cannot accidentally reach
the parent after premature model cancellation. The terminal still runs outside
paint in that same input turn, and idempotent widget cancellation retains one
typed terminal. Other loss/normal release paths and native APIs are unchanged.

Owners may revoke their own lease with `cancel_pointer_capture_for` when a
host refuses Begin after pointer admission. It checks the exact EntityId, leaves
other owners/typed drags untouched, and runs the existing cancellation terminal
immediately outside paint. Refusal therefore retains no input lease while its
trailing release remains suppressed. Splitter sizing snapshots remain host-owned.

The existing macOS cursor owner rasterizes Hex's three embedded SVG sources in
`../../assets/cursors/` using the pinned GPUI SVG dependencies. The 24-point
logical size, centered (12, 12) hotspot, cached native cursor ownership and OS
fallbacks remain in this dependency boundary; the 48-pixel raster is derived
at startup. No new workspace unsafe code or native cursor path is introduced.

Test support may install an application's asset source and inspect completed
monochrome sprite geometry. This is for deterministic SVG consumer tests; it
does not change the native asset cache, create an OS window, or establish
native framebuffer appearance.

30 October text decoration storage: `ShapedLine`, `WrappedLine` and the two
`WindowTextSystem` shaping accumulators keep up to four `DecorationRun`s inline
instead of thirty-two, and spill beyond that to the existing dynamic storage. No
span, text-length or project limit is added; run order, styles, glyph shaping,
wrapping and painting are unchanged. Ordinary labels carry one to three runs, so
each shaped line copies and caches far less inline storage. Only
`src/text_system.rs` and `src/text_system/line.rs` change; provenance hashes and
the cumulative patch were regenerated from the verified published 0.2.2 archive.
Hex's `text_decoration_runs` regression shapes and paints 1/4/5/32/33/80-run
mixed-UTF-8 lines across both inline boundaries.

1 November should-terminate hook (Pulse #4092): the macOS app delegate also
registers `applicationShouldTerminate:`. When the app installed a handler with
`App::on_should_quit`, the delegate runs it and returns NSTerminateLater; the app
answers once with `App::reply_to_quit(bool)`, which sends
`replyToApplicationShouldTerminate:` on a later main run-loop turn so that a YES
reply (and the synchronous `applicationWillTerminate:` shutdown) never runs
inside an App borrow. Without a handler the delegate returns NSTerminateNow, so
upstream behavior is unchanged; replies with no pending request are ignored and
a repeated request while one is pending stays deferred. `App::quit` reaches the
same handler through `terminate:`. New `Platform::on_should_quit`/`reply_to_quit`
methods have no-op defaults, so Linux/Windows sources are untouched. The test
platform records the request and each reply; `TestAppContext::simulate_should_quit`,
`quit_request_pending` and `quit_replies` drive it headlessly (Hex regression:
`crates/hex-widgets/tests/should_quit.rs`). Native code stays in the existing
delegate class inside this excluded dependency; no new framework, FFI owner or
unsafe Hex code. Changes are confined to `src/platform.rs`, `src/app.rs`,
`src/platform/mac/platform.rs`, `src/platform/test/platform.rs` and
`src/app/test_context.rs`; changed-source hashes and their cumulative patch
blocks were regenerated against the hash-verified published 0.2.2 sources.
Dock Quit/logout/restart prompting and AppKit's modal-panel run-loop delivery
during NSTerminateLater remain native user acceptance items.

3 November quit-reply correction: `App::quit` invokes AppKit termination from
the main dispatch queue. During `NSTerminateLater`, AppKit can run a nested
main run loop while that queue remains occupied, so dispatching the reply to
the same queue deadlocks. Schedule a copied Objective-C block in the main run
loop's common modes and wake it instead; this retains the deferred App-borrow
and one-reply contracts. Core Foundation copies the block before returning.
The noninteractive `native_quit_probe` exercises the actual GPUI/AppKit path
for acceptance and cancellation followed by acceptance; run it with
`crates/hex-ui/tests/native_quit_test.py` after building the example. This uses the
existing native dependency boundary and adds no unsafe Hex workspace code.

6 November rotated device titles: the existing monochrome glyph raster cache
and sprite transform support a bottom-to-top quarter turn at a shaped glyph's
baseline. Scene admission and cached replay use the transformed sprite bounds
for clipping and paint order. The app supplies the name and title hitbox, so
normal text layout and pointer handling remain with their existing owners.
Test support exposes the resulting rotated glyph bounds without GPU readback.
