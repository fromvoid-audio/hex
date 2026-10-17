# native panel host bindings

`hex-ui/src/editor/device_panel/bindings.rs` owns persistent scalar widget
construction and synchronization for specialized panels, including Utility and
EQ, and the descriptor fallback. `gesture_adapter.rs` adapts numeric widget
phases to the existing Session gesture client. These are host modules: they
intentionally depend on the editor, canonical snapshots, pending requests and
worker acknowledgments. They are not an independently embeddable UI library.

Panel composition injects display codecs, compact presentation and a policy for
Tab navigation, toggle appearance, choice ordering and an optional numeric Begin
hook. Operator-specific identities and oscillator selection remain in the panel
composition. Shared bindings do not identify devices by name or type.

A binding captures project generation, installed panel owner, device ID, class,
type and parameter descriptor. Callback admission checks canonical membership in
that owner, the current state-aware parameter descriptor and availability, and
the emitting entity against the installed widget. The metadata cache is usable
only when its class and complete device state match the canonical device. Host
input/output gain descriptors are explicit controls outside device parameters.
Cancellation does not require a still-enabled parameter. Retirement uses exact
gesture ownership even after the canonical device or owner disappears.

Each numeric widget holds only the gesture key it successfully began. Change,
Commit and Cancel require that exact active key and target, current generation
and no already-reserved terminal. Successful terminal reservation relinquishes
the local key; the Session client retains delivery and acknowledgment ownership.
Retirement revokes the local key before widget cancellation and cannot cancel a
newer control's gesture for the same parameter. Synchronous owner finalization
first retires stale bindings, retains refused drafts, and uses the existing
post-terminal command barrier when another widget owns the gesture.

Owner finalization can complete a physical edit before GPUI delivers its queued
Begin/Change effects. After successful submission (or a successful unchanged
finish), a binding-local fence ignores that completed delivery cycle. A deferred
callback reopens only the same installed entity, fence epoch, generation and
owner. Retirement invalidates the epoch; refused finalization neither fences nor
accepts the local draft. This uses GPUI's existing FIFO effect delivery, with no
new host request queue or public widget event token. The rendered same-update
no-op drag regression reproduces reopening before this repair. At exact
e009db34 the six focused host tests pass, including the no-op finalization and
later genuine drag. Final integrated full local CI remains required.

Canonical, preview and pending values retain their existing separate meaning;
widget display is not audio-applied evidence. Compound EQ/Operator graph edits,
Return sends, preset finalization, context menus and project lifecycle retain
their existing owners. No second edit model or new queue is introduced.

W06 remains open for the independent embedding example/library boundary,
catalog action integration and the complete production/native acceptance matrix.
The host extraction alone does not complete those deliverables. Full local CI
on the integrated final commit and a finite optimized native test checklist are
separate from focused headless host regressions.

The focused host regression group is `bindings_tests` under the
`ui-test-support` feature. It exercises cross-parameter late phases, exact
same-parameter gesture leases, unowned retirement, replaced emitter identity,
pre-render generation retirement and same-update owner finalization. Existing
rendered Utility/EQ/Operator and Track/Return tests remain the behavioral
integration coverage; retain their save, Undo, invalid-input and preset cases
rather than duplicating them in this group. A test's presence is not a recorded
pass: consult exact-candidate local-CI evidence.

For an optimized native candidate, the finite follow-up checklist is: type and
save a Utility value then Undo; drag an EQ node and Undo once; cancel a numeric
drag with Escape; refuse an invalid typed value on navigation/save; switch
Track/Return owners during an edit; remove/replace the edited device; change an
Operator mode that hides a control; exercise keyboard focus after replacement.
Record the actual build/source and observed results separately. These checks
have not been performed for this extraction.
