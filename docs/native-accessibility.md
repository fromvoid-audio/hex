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
published as acknowledged values. Value-change notifications require an actual
acknowledged revision/value change; local draft status is separate.

## AppKit lifetime and threads

A `GPUISemanticControl` subclasses `NSAccessibilityElement`. Each native object
owns only a boxed weak window token and stable ID; cached objects also have weak
control-owner leases. Visual unmount retains native identity while the real
control exists. Owner removal expires the lease; window teardown permanently
revokes dispatch before releasing controls and native objects. External retained
native objects subsequently report unavailable and refuse actions. No strong
native/widget/window cycle is introduced.

Native queries read complete published owned snapshots under a short native-state
lock. Screen frames are calculated on the UI thread during publication; queries
never render/mount, call project workers/audio or synchronously dispatch to UI.
Notifications occur after publication and after releasing the state lock, so nested
queries can inspect the coherent snapshot. Names, roles, values and bounds are
actual metadata/layout, not debug selectors or tooltips.

Actions enter a64-request bounded native queue; only owned typed requests and a
weak token cross threads. A main-queue trampoline releases the state lock before
calling GPUI, checks current revision/availability, and GPUI/widget/owner guards
revalidate again. Teardown prevents a running trampoline from restoring dispatch.
Queue overflow refuses; no callback/audio wait or unbounded main-thread drain.

## coverage and remaining native acceptance

This checkpoint exposes mounted NumericControl/Toggle/Selector device-panel
controls, with slider/checkbox/popup semantics and existing validated actions.
Unmounted controls have no fabricated frame and cannot be edited through stale
native references. A complete offscreen logical navigation/scroll-to-mount path
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
No AppKit exposure, VoiceOver, native readability or frame-time pass is claimed by
headless fixtures. Native failures block readiness; do not replace them with
synthetic AX success.
