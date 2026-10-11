# Editable native Operator envelopes

Pulse2247 owns the first editable envelope graph in the native Lattice UI. The
graph reuses `hex-widgets::ResponseGraph`; EQ8 retains its existing pointer
editing behavior and does not opt into the new keyboard/fine-drag mode.
Pulse2337 updates the display mapping and authoring bounds; the envelope DSP
laws and audio ownership remain unchanged.

Each graph is keyed by the stable `(DeviceId, operator index)` pair. The four
visible handles edit all six canonical values through the device-gesture actor:

| handle | edited values |
| --- | --- |
| initial | initial level |
| peak | peak level and attack time |
| sustain | sustain level and decay time |
| release end | release time; sustain extends to the fixed final-third release anchor |

Levels remain in their descriptor's linear `[0, 1]` domain. Attack authors
0–20 seconds; decay/release1ms–60 seconds. The fixed plot gives attack and
decay up to one third each, with decay starting after attack; release starts
at two thirds and ends at up to the full width. Sustain fills the gap. Maximum
20/60/60s produces node positions1/3,2/3,1. Minimum0/1ms/1ms produces0,0,2/3,
matching the endpoints of two user-supplied Live screenshots on11October2026.
Their private copies are `research/private/operator-envelope-2337/{maximum,minimum}.png`;
the screenshot app/build identity was not independently established by this chat.

An original invertible logarithmic mapping keeps short times usable: attack
uses a1ms log1p pivot; decay/release use a1ms lower endpoint. No parameter edit,
Commit/Cancel, Undo, page change or reopen automatically zooms or fits the plot.
Attack moves decay cumulatively; release stays in its own section. Intermediate
numerical mapping and native Live parity remain unverified. Version6 state
strictly validates the new bounds; valid old1–5 times clamp without notice.
See [state](operator-state.md) and [controls](device-controls.md).

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

Pulse2337 focused checks cover reference endpoint geometry, inverse mapping,
canonical keyboard commit/Undo/save, immediate pointer/readout feedback and
cancellation/outside release at the60s cap. Before-fix negative controls fail
for the original auto-fit geometry and120s authoring bounds. Full exact-final
local CI remains required before merge; native testing belongs to the user.
