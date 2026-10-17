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

Canonical, preview and pending values retain their existing separate meaning;
widget display is not audio-applied evidence. Compound EQ/Operator graph edits,
Return sends, preset finalization, context menus and project lifecycle retain
their existing owners. No second edit model or new queue is introduced.

W06 remains open for the independent embedding example/library boundary,
catalog action integration and the complete production/native acceptance matrix.
The host extraction alone does not complete those deliverables. Full local CI
on the integrated final commit and a finite optimized native test checklist are
separate from focused headless host regressions.
