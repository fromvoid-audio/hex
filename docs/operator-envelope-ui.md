# Editable native Operator envelopes

Pulse2247 owns the first editable envelope graph in the native Lattice UI. The
graph reuses `hex-widgets::ResponseGraph`; EQ8 retains its existing pointer
editing behavior and does not opt into the new keyboard/fine-drag mode. This is UI gesture work only: the canonical Operator
parameter descriptors, saved schema, DSP and envelope law do not change.

Each graph is keyed by the stable `(DeviceId, operator index)` pair. The four
visible handles edit all six canonical values through the device-gesture actor:

| handle | edited values |
| --- | --- |
| initial | initial level |
| peak | peak level and attack time |
| sustain | sustain level and decay time |
| release end | release time; the held sustain-to-release segment remains fixed at 0.25 seconds |

Levels remain in their descriptor's linear `[0, 1]` domain and stage times in
seconds `[0, 120]`. The horizontal graph window is `max(1.5 × (attack + decay +
release + 0.25), 0.5)` seconds. It is captured once at gesture start, so changing
one stage cannot move the time axis under the pointer. Time edits clamp against
the other current stages and the fixed hold interval; the view may contain
unused horizontal headroom. This is an original interaction mapping, not a
measured Ableton Live parity claim.

Pointer drags, Shift-fine drags, and focused keyboard selection/editing map to
one scalar or paired canonical gesture. Existing numeric fields read the same
transient gesture snapshot, and Commit produces one grouped Undo. Invalid local
numeric drafts refuse a graph selection or edit and remain available for repair.
Escape, focus loss, page hiding and device removal cancel active gestures.
An owner change follows the existing deferred-switch contract while a gesture
is active; the old graphs retire after its terminal event. Project changes
retire old graph entities. Pointer capture keeps a drag alive through
release outside the graph; coordinates are bounded to the graph edge.

The interactive canvas fills the graph bounds. A rendered pointer regression
covers the former zero-height canvas, which rejected every handle hit, and
release after movement outside the plot. Capture is installed from the first
paint so a quick press and move does not depend on a second paint.

## Validation status

Three shared-graph checks and three rendered Operator checks pass, including
real pointer capture, cancellation, page retirement, invalid drafts and saved
Undoable keyboard edits. Full exact-source local CI and the focused PR are
recorded in Pulse2247. Source checks and native visual acceptance are tracked separately in
`todo.md` and Pulse2247. Automated GPUI input tests do not establish native
window feel or audible behavior. The user should review all four handles,
Shift-fine movement, keyboard editing, numeric synchronization, Escape/outside
release, one-step Undo and saved-project reopen in a development build.
