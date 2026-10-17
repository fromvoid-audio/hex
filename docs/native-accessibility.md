# native accessibility boundary

## scope and ownership

The 14 October 2026 coordinator decision for ticket29/PR188 permits a narrow
semantic-control API and AppKit implementation in the excluded GPUI0.2.2 patch.
Hex crates retain `unsafe_code = "forbid"`. This is not permission for app FFI,
a new UI framework, an upstream identity change or unreviewed vendor edits.
The original archive checksum/file hashes remain authoritative; new source and
changed files are explicitly enumerated and hashed by `check-gpui-patch.py`.

`AccessibilityDescriptor` describes an owner-lifetime ID, acknowledged revision,
name, value/range/choices, availability, focus and separate draft/pending status.
`Div::accessibility` supplies actual clipped layout bounds and a UI-only action
handler. Nodes follow GPUI's cached prepaint ranges, not a parallel render tree.
Cached views rebuild after a refused semantic snapshot, allowing recovery when
invalid owners disappear. Native element release occurs outside the snapshot lock.
A window admits at most4096nodes,4096bytes per text,128choices per selector and
two MiB aggregate semantic text. Invalid/duplicate/oversized snapshots refuse as
a whole. These are accessibility admission bounds, not project/audio limits.

`AccessibilityBinding` supplies acknowledged device-panel values and a weak-owner
admission guard. Numeric values use the existing control's canonical formatter,
range and step. The guard rechecks project generation/revision, current control
ownership and pending/gesture barriers. Widget-local draft/drag and an immediately
submitted AX transaction also refuse another edit. Press/set/step uses ordinary
control events, existing worker preparation and history; focus alone does not edit.
A queued action is not an accepted canonical edit. Pending/refused values are not
published as acknowledged values. A future native value-change notification must require an actual acknowledged
revision/value change; local draft status is separate.

## native backend ownership and acceptance boundary

The original AppKit candidate remains quarantined in published history/evidence.
It upgraded weak MacWindowState on arbitrary AX query threads, permitting an off-UI
final drop of GPUI/GPU state. Do not restore that implementation.

The replacement backend is source-enabled in the excluded dependency, with macOS compilation and native fixture results recorded below; real-client
acceptance remains pending. `platform/accessibility_bridge.rs`
is a data-only transport: immutable Arc snapshots, owned scalar/text/frame data,
actual-owner Weak<()> leases, a64-request mailbox and checked window/registration
epochs. Its last drop cannot release App/Window/renderer/input-handler state.
Native element ivars contain only that transport and owner IDs/leases. There are
no window pointers, window weak references, executors, callbacks, Rc or GPUI refs
reachable from an element. Snapshot queries do not render, mount, call the worker
or synchronously dispatch. A query already holding data may linearize before
revocation; later queries and actions refuse.

A separate main-thread-only registry holds UI callbacks and the native cache.
It has one entry per real window, no closed-window tombstones, at most4096cached
controls and non-reusable checked registration epochs. It is structurally !Send.
The existing `dispatch_async_f` main queue receives only a boxed checked window
key. No ForegroundExecutor/AsyncApp or window reference crosses threads. On UI,
requests recheck owner lease, revision, action domain and exact choice inventory,
then invoke the existing accessibility callback/AnyWindowHandle update outside
all registry and snapshot borrows. The existing widget/project guards revalidate
again. A callback is restored only into the same still-live registration; reentrant
close cannot recreate an entry. Queue acceptance never claims a canonical edit.
Value notifications require changed acknowledged revision and value, not enqueue
or pending success. The64request bound includes owned choice inventories (up to
128bounded labels per request); no work occurs on the audio callback.

Revocation starts `Window::remove_window`, accepted native close before its
callback, and idempotent Drop/deallocation fallbacks. Vetoed should-close does not
revoke. Revocation makes the snapshot unavailable and clears queued requests
before detaching the registry. Native cache/callback release occurs on UI outside
registry borrows. UI cache entries retain no widget and native elements retain no
parent view, preventing a parent/window cycle. A retained native element remains
data-only after window teardown. Repeated off-UI fallback revocation schedules
cleanup only on its first transition.

Native object-tree parent/children lookup is UI-only. Off-UI tree requests return
unavailable rather than dispatch synchronously or touch window/native-cache
owners. Scalar/name/value/range/frame queries remain data-only on any thread.
Actual Accessibility Inspector/VoiceOver must establish whether that restriction
is sufficient for real clients. If not, a separate reviewed object-tree lifetime
contract is required; a Send wrapper or weak window upgrade is not a solution.
Unmounted nodes have no fabricated frame and editing them refuses; scroll/mount
navigation remains a separate gap.

The shared transport tests cover concurrent retained snapshot revocation, mailbox
capacity/wake/refusal, stale epochs, nested-close restoration refusal, UI owner
versus background data destruction and reentrant data-drop lock availability.
A compile-negative control rejects moving Registry<()> to another thread. Native
owner/inventory/domain tests are included under the Mac module and execute on macOS; Linux cannot run them. These checks do not prove actual AppKit teardown or veto handling.

## coverage and remaining native acceptance

The source builds mounted NumericControl/Toggle/Selector device-panel
semantic descriptors and validates source-level actions through existing controls.
The replacement AppKit hooks compile on macOS and pass the native fixtures below;
actual Inspector/VoiceOver exposure remains unverified.
Unmounted controls have no fabricated frame and stale source requests refuse. A complete offscreen logical navigation/scroll-to-mount path
is not implemented; the explicit refusal remains an accessibility coverage gap.
Other specialized controls/graphs, complete context hierarchy and physical-input/
accepted-Main telemetry remain separate work. This does not complete ticket29.

Linux source tests establish descriptor/routing/refusal/history behavior only.
The primary must compile the exact frozen source on macOS and inspect its optimized
artifact with Accessibility Inspector/VoiceOver: names/roles/units/ranges/choices,
focus-only navigation, one accepted edit/Undo, busy and rejected edits, concurrent
queries/actions, closure with retained AX objects, owner/generation replacement,
scroll/unmount/remount identity and actual bounds at960x600/resized/HiDPI windows.
Check SDK selector/role behavior, notification timing and native parent lifetime.
Native choice setters must refuse oversized, embedded-null or ambiguous strings;
exercise invalid-snapshot recovery and nested queries during retirement.
No AppKit exposure, VoiceOver, native readability or frame-time pass is claimed by
headless fixtures. Native failures block readiness; do not replace them with
synthetic AX success.

## clear-before-drop teardown repair

Independent review of4308ce5 found that detach/dealloc reconstructed their owning
Box while the native ivar still exposed its pointer. Callback/cache destruction
could reenter teardown or queries before that pointer became dangling. This is a
source-proven hazard; no actual AppKit crash is claimed. Both owning ivars now
transfer through one private take-and-clear helper: the Objective-C runtime clears
the non-object void* ivar before Box::from_raw, revoke or Drop. A missing/uncleared
slot refuses transfer. The runtime setter avoids manufacturing a mutable Rust
Object reference from a shared reference; no public native API/lifecycle changes.

A third Mac-only regression uses real NSObject ivar storage for both owning names.
Its old-order negative control observes the still-exposed pointer before returning
ownership without destruction. The fixed path's destructor performs nested actual
bridge/element readers and another take: readers see unavailable, second take
refuses and the two owners drop exactly once. This fixture does not open a window
or exercise actual AppKit close; its macOS result is recorded below.
Parent4308 full Linux CI is historical for the corrected head. Retain the original
quarantined backend and default-stack failure evidence; do not relabel either.

## cached-view integration — 16 october 2026

PR188 also includes merged dense-selection cache PR264. Cached prepaint reuse
requires both a matching caller epoch and a valid prior accessibility snapshot;
neither condition replaces the other. Existing cached-accessibility recovery
and clip-scene epoch/painter-order regressions exercise both paths. Test-only
selector insertion history and semantic prepaint ranges remain independent.
The combined vendor patch is regenerated from the verified original0.2.2 archive
and checked by applying it to that original source and comparing every declared
patched/added file hash. These checks do not execute AppKit or satisfy the three
Mac lifecycle tests, native compilation or AX/VoiceOver acceptance.

## macOS compilation and selector regression — 17 october 2026

Local continuation of PR188 compiled the AppKit backend on macOS26.6.2 arm64
with rustc1.94.0. SDK inspection found the boolean properties use the getters
`isAccessibilityHidden`, `isAccessibilityEnabled` and `isAccessibilityFocused`.
The prior `accessibility…` selectors left AppKit reading superclass defaults.
A real `GPUISemanticControl` regression queries all three getters against an
enabled/focused snapshot, a disabled/unfocused update and revoked transport.
Before the selector repair it returned `(false,false,false)` instead of
`(true,true,false)`; after repair all four Mac tests pass, including the three
previously unrun owner/inventory, clear-before-drop and data-only graph tests.

Run with the same macOS renderer features as Hex:

```sh
cargo test --locked --manifest-path third_party/gpui/Cargo.toml \
  --no-default-features --features font-kit,macos-blade,test-support \
  --lib platform::mac::window::native_accessibility::tests -- --nocapture
```

The standalone default feature command requires the unavailable Apple `metal`
compiler on this Command Line Tools installation. The explicit Hex feature set
avoids changing the renderer or installing tools. The initial failing regression
also ran with GPUI's runtime-shader feature and the same native backend.
Retained logs are under `artifacts/native-29/`; exact final CI, source and optimized
build identities are recorded on PR188 and in the installed test-build receipt.
These fixtures create Objective-C objects, not a window or VoiceOver session.
Inspector/VoiceOver tree navigation, popup choice behavior, notification timing,
close/veto and actual input/paint acceptance remain the finite native checklist.
