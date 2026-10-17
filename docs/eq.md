# eq

Pulse #2654 / PR #270 replaces new-insert EQ8 with an original sparse `eq`
(`hex.eq`). It is a zero-latency native stereo parametric EQ, with at most 64
stored nodes across stereo, left/right and mid/side banks. Disabled nodes count
against this bound. Existing `hex.eq8` files remain supported and retain their
original state codec and DSP. The browser offers `eq`; both devices display `eq`.
This is a bounded interaction-inspired design, not FabFilter processing or full
Pro-Q feature parity. Dynamic EQ, linear phase and EQ matching are outside it.

## Editing and identity

New instances start flat with no nodes. A blank graph press previews one node
under the pointer and keeps dragging it during that same held press. The outer
5% on each side of the inset logarithmic plot selects low-cut/high-pass on the
left and high-cut/low-pass on the right; the interior creates a bell. Existing
node hits take precedence over these creation zones. Bell creation maps pointer
frequency/gain; cuts map frequency/Q and retain zero gain and the default 12 db
cut slope. The shape stays fixed when dragging across a zone boundary.

Release commits creation and its final position as one undoable canonical state
and identity-reservation transaction. A release outside the graph commits the
clamped endpoint. While held, the visible preview uses a provisional stable ID
from the canonical allocator/high-water mark; it does not change audio or need a
worker acknowledgment. Escape, focus loss, a missing held button, changed source,
allocator, bank, selection or project discard the preview. Commit compares both
the captured source state and allocator, so concurrent edits refuse rather than
rebase the node. A later publication or pointer move cannot resume a released
creation. The second press of a double-click does not create a second node.
Existing hits select/drag. Bell/shelf dragging edits frequency/gain;
cut/notch dragging edits frequency/Q. Selected-node controls expose enabled,
shape, frequency, gain, Q and cut slope (12/24/36/48 db/octave). The channel-bank
buttons choose the bank receiving new nodes; the global mode selects audible
banks. Remove deletes the selected node and all its automation atomically.
Scalar gestures, opaque state edits, undo/redo and project/preset persistence use
the existing canonical device path.

IDs are checked project identities, never frequency-sort positions or reusable
slots. Parameter IDs are `1000 + node_id * 8 + field` (seven fields). The opaque
codec carries a monotonic node high-water mark, including in presets. Creation
uses the larger of that mark and the current project allocator, atomically
reserving the range with source-state and allocator comparisons. Undo retains
the project's allocator high-water mark. Exhaustion or a stale source refuses the
edit; it never wraps or overwrites another node.

Legacy panels retain their existing controls and add **edit nodes**. That explicit
upgrade converts all 40 authored slots, including disabled values and inactive
banks, into stable nodes in one history transaction and reserves 40 identities.
Device identity, bypass and host trim remain unchanged. Undo restores the original
opaque EQ8 state. Representative PCM tests cover all five banks, all three channel
modes and the legacy shapes exactly, including the historical mid/side and tiny
history laws. Imported shelves retain the old S interpretation. New shelves use
actual Q. Editing a legacy shelf's Q/shape explicitly switches it to the Q law;
legacy S shelves require that edit before gain/Q/shape automation. Already invalid
enabled legacy S/gain combinations fail preparation visibly; disabled authored
values remain preserved.

## Panel layout

The forty-pixel top row contains mode/output controls, channel banks/count and
analyzer/listen controls, separated by shared theme dividers. The forty-pixel
footer keeps selected-node controls, automate and remove together. Frequency
uses a reusable 74-pixel micro dial so full values and units fit; other controls
keep the shared compact dial, selector and button presentations. The shared host
[bindings](native-device-bindings.md) own their identity, gesture and retirement
behavior; panel composition supplies presentation only. Control widths
never shrink into neighboring labels or readouts. Automation adds a separate
sixteen-pixel row containing its field/page controls and playhead readout, while
the response/automation plot retains the remaining height.

## EQ-owned arrangement automation

The **automate** button edits the selected node's frequency, gain, Q, shape,
slope or enabled curve on sixteen-quarter-note-beat pages. Click adds a point;
drag edits beat/value; **− point** removes it. A point drag previews locally and
commits once on release. Curves use strict, unique, ascending unsigned Q32.32
arrangement beats. Frequency and Q interpolate logarithmically; gain interpolates
linearly in db; discrete controls hold the preceding value. Before/after a curve,
the first/last point is held. The codec allows 64 lanes, 160 total points and 64
points in one lane, within the unchanged 4096-byte device payload. Capacity,
invalid data and duplicate-time refusals preserve the previous state.

These are EQ-owned lanes in versioned opaque device state, not a general device
automation schema or recording system. Playback and prepared linear export use
the same content-aligned arrangement clock, including ordinary and compensated
Track/Return paths. Loops follow arrangement position; linear export advances.
Stopped monitoring and unavailable/warmup clocks use authored base values.
Coefficient updates use an absolute arrangement-frame cadence of 64 samples,
with 64-sample coefficient ramps. Seeks clear histories and evaluate the new
position; bypass advances private processing and automation while preserving dry
samples. Band storage is allocated to the exact node count on control (maximum 64), and
never resized on audio. No project compilation, allocation, logging or I/O occurs in the EQ
callback. Mode changes reset histories; compatible node history follows stable ID.

The response plot is explicitly **authored response**, cached by effective state
and sample rate. It does not animate the automated response. The spectrum remains
live; the selected automation value's **playhead** readout is display telemetry,
not an exact delayed-callback timestamp. Transient listen selectors carry stable
node IDs and fade over 256 frames, independently of authored state/history.

## Live input and output spectra

The **spectrum** action explicitly enables analysis for the selected device while
audio runs. It replaces the easy-to-miss wave glyph; an inactive analyzer has no
live trace. The flexible EQ shows two actual signal layers on identical
frequency/dB axes. Input is sampled immediately before the entire native entry,
including host input trim; output is sampled after processing and host output
trim. The **input** action defaults on and hides only input. It does not disable
output capture or replace input with an inferred response curve. Legacy EQ8
continues to show its output layer. Silence, unavailable targets and inactive
transport are not fabricated into animated traces.

Each at-most-256-frame callback chunk stages both phases independently by exact
offset and stable owner/device/type. Incomplete, overlapping, out-of-bounds or
failed renders discard the entire pair. Final caller output validation, target
epoch, generation, sample rate and activation revision must all agree before
one paired packet is queued. Refusals consume a sequence, preventing the next
FFT window from spanning a gap. Existing output-only observer/capture/reader
APIs remain adapters; legacy output-only captures have no input result.

The existing 32-packet queue, one-result channel and single worker/reaper remain.
Each packet carries two fixed stereo arrays. The worker adds input rolling and
ordered windows (two 4096-frame stereo boxes, 65,536 payload bytes total), reuses
the existing FFT scratch sequentially and publishes both transforms together at
at most 30 Hz. Native storage accounting includes these boxes and the enlarged
actual queue payload. Both layers clear together after 100 ms of capture age;
context/rate/generation changes and sequence gaps reset both windows. FFT work,
allocation and final reclamation stay outside the callback. These bounded
headless contracts do not establish native deadline or listening acceptance.

## Evidence and native checks

Focused automated checks cover codec truncation/ranges/capacity, identity
reservation and undo, imported high-water marks, legacy PCM and invalid shelves,
cut responses/Q, live/offline/compensated/Return automation partition/loop/seek/
linear-export behavior, rendered creation/drag/curve editing/upgrade/history, and
zero total/net allocation during maximum-density processing, bypass, history
copy and reset. An actual prepared Track/Return callback fixture also checks
initial admission, held stable-ID listen during parameter publication, smooth
release, immediate cancellation and shutdown retirement; construction and final
reclamation stay outside the measured callback scopes. Full exact-candidate local CI, optimized delivery and native
interaction/listening results are recorded separately by the PR coordinator.
Headless fixtures do not prove physical device deadlines or human acceptance.

Finite native checklist for the exact optimized candidate:

1. Insert eq; press each outer edge and the interior, keep holding and drag the
   new cut/bell across the plot, then release. Check immediate visible preview,
   fixed shape, final frequency/Q or gain, outside release, Escape/focus-loss
   cancellation, single-step undo/redo, double-click suppression and save/reopen.
   Adjust Q/shape and all cut slopes; check bypass/remove too.
2. Upgrade an existing EQ8, compare all authored channel banks and sound, undo the
   upgrade, then upgrade and add nodes again. Confirm settings and trim survive.
3. Add/drag/delete automation points for gain/frequency/Q and stepped controls;
   listen through playback, seek, loop and bypass, and compare a linear export.
   Check the playhead value moves; the response line remains explicitly authored.
4. Arm listen and hold/release a node while editing its parameters; check smooth
   transitions and the live spectrum. Add/remove other nodes and confirm the
   selected node's automation/listen never moves to another node.
5. Exercise 64 nodes and the documented lane/point bounds; verify useful refusals,
   responsive editing, no dropped audio and unchanged saved projects.
