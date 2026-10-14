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

## native backend blocker and required next contract

The first AppKit candidate is quarantined and is **not included** in this source
checkpoint. Its off-UI queries upgraded a weak token to the full MacWindowState.
Concurrent closure could make an AX query the final strong owner and destroy
GPUI/GPU state off the UI thread. Linux source/CI passes do not validate that
ownership path. The candidate patch is preserved in the worker's evidence; do not
restore it without resolving this blocker and native verification.

The next backend must use a separate data-only, Send+Sync semantic snapshot whose
last drop cannot destroy any GPUI/window/renderer/input-handler state. Native
objects hold revocable weak references only to that data and actual-owner leases.
Queries read owned scalar/text/frame data without rendering, mounting, project or
audio calls, or synchronous UI dispatch. Off-UI queries must never upgrade the
full MacWindowState or retain a GPUI host NSView. UI-only parent lookup and action
dispatch need an opaque generation token and a UI-owned weak registry; only typed
requests/data cross threads. Teardown revokes the token and detaches dispatch
before GPUI release; retained native objects then become unavailable.

The proposed native queue remains bounded to64requests, with main-thread owner,
revision, payload and availability revalidation, never a canonical-success claim
at enqueue. Object release/notifications must occur outside snapshot locks.
Actual AppKit role/value/action selectors, parent lifetime, concurrent queries,
closure and native exposure still require the primary's exact-source Mac gate.
No native class, native query/action hook or native notification is enabled by
this checkpoint. PlatformWindow's publication/action hooks are currently unwired.

## coverage and remaining native acceptance

This checkpoint builds mounted NumericControl/Toggle/Selector device-panel
semantic descriptors and validates source-level actions through existing controls.
It does not yet expose those controls to the OS accessibility tree.
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
