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
When the response graph owns keyboard focus, **Backspace** removes its selected
node through that same atomic history command. An empty graph consumes the key;
text/numeric editors retain their own Backspace behavior. Held gestures,
creation previews, pending/stale work and old project generations cannot remove
a node. Focused Escape cancels a held graph pointer before editor bindings.
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

The graph remains the main editing area; the compact sidebar contains mode/output,
channel banks, spectrum/listen actions and the six selected-node controls. Shared
host [bindings](native-device-bindings.md) own control identity, gestures, draft
finalization and retirement. The remove action stays separate from parameter
controls. Text action labels retain intrinsic nonwrapping widths and padding;
rendered tests check graph/sidebar geometry at supported narrow and wide widths.

## Automation workflow boundary

Pulse #2855 / PR #457 removes the EQ-local **automate** button, curve editor,
point controls and their graph-event mode following the user's 19 October 2026
workflow decision. Selecting a node always exposes its ordinary enabled, shape,
frequency, gain, Q and slope controls alongside the response graph. It does not
open a device-local automation editor.

Node parameters retain their existing stable `DeviceId` / `NodeId`-derived
`ParameterId` bindings and state-aware metadata. This is the identity boundary
for future central DAW automation; it does not implement Arrange automation mode,
lanes or recording. Those remain separate work under Pulse #2839 / PR #443 and
the common native parameter work #2674. No new automation protocol is introduced.

Existing EQ-owned lane bytes, codec validation, playback/export evaluation and
canonical history remain intact. Ordinary edits do not clear authored lanes;
removing a node still removes only that node and its associated lanes atomically,
with Undo restoring both. No project or preset conversion is performed.

The response plot remains the **authored response**, cached by effective state
and sample rate. Enabled band centers and shoulders are sampled within the
existing 256-point graph limit, including all 64 bands. The finite polyline does
not animate automation or guarantee a global interpolation error bound; see
[quality cycle 1](eq-quality-cycle1.md). Spectrum and stable-ID transient listen
behavior are unchanged.

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
linear-export behavior, rendered creation/drag/ordinary parameter editing/upgrade/history, and
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
3. Select a node and confirm there is no automate entry or local curve editor.
   Type frequency/gain/Q and edit shape/slope/enabled; verify single-edit Undo/Redo
   and correct values after switching nodes. Existing saved lane data must remain
   intact; central Arrange lane editing is not implemented by this change.
4. Arm listen and hold/release a node while editing its parameters; check smooth
   transitions and the live spectrum. Add/remove other nodes and confirm the
   selected node's automation/listen never moves to another node.
5. Exercise 64 nodes and the documented lane/point bounds; verify useful refusals,
   responsive editing, no dropped audio and unchanged saved projects.
