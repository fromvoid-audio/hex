# canonical output capture

Pulse2346 / [PR159](https://github.com/PORTALSURFER/hex/pull/159) provides an
engine prerequisite for export30 / PR152. It does not implement export selection,
file writing, UI, preroll, seek or publication.

## control and render contract

After preparing and constructing a `SessionPlayback` and completing any accepted
history inheritance, call `output_capture_plan(&ids, frame_capacity)` on control.
`OutputCaptureId::Track(TrackId)` and `OutputCaptureId::Return(ReturnId)` identify
canonical owners; `ReturnId` is the project's canonical Return bus key. The
request order is the output-buffer order. Duplicate or unknown keys are refused.
Retired-only IDs may be explicitly selected while present in this runtime's
current/retained owner registry. The engine does not infer an export selection.

The plan binds the exact runtime instance, prepared revision and successful
inheritance generation. Another runtime, or another inheritance on the same
runtime, requires a new plan. A missing expired owner is refused before render
mutation. Completed effect groups remain retained until control publication,
so an otherwise valid selected retired owner may produce silence. Analytic
release entries may expire at render commit; their retired-only keys then cease
to be selectable. IDs are never reassigned to a replacement track by array index.

Admission supports a linear Arrangement export runtime, or an interactive runtime
at an Arrangement-only cut: every current route owns Arrangement, no pending
launch exists, and its transport loop is disabled. These conditions are checked
both when building the plan and before every captured render. Linear export
continues to ignore authored transport loops and refuses public state inheritance.
Interactive capture uses the actual interactive canonical render and legitimate
public inheritance; it does not convert a runtime or manufacture history. This
cut does not establish deterministic offline preroll or seamless resumption.

The selection count is bounded by `MAX_OUTPUT_CAPTURES` (the sum of current
project track and Return limits), independent of historical owner count. Plan
frame capacity must be in `1..=MAX_OUTPUT_CAPTURE_FRAMES` (16,384). DSP still
processes chunks of at most 256 frames. Control reserves plan storage; its
`storage_bytes()` reports owned header/vector capacity, excluding the shared
revision. Callers allocate their sample storage on control before rendering.
Original TrackId metadata is inline fixed storage in SessionPlayback and charged
through its actual runtime size in single and paired admission/checkpoint budgets.
No native runtime budget is increased.

Borrow one stereo `Frame` slice per ID in an `OutputCapture`, then use
`render_with_capture(main, &mut capture)`, or `render_with_pages_and_capture`
with the existing borrowed page provider. Output count must exactly match the
plan, every slice must fit the requested block and the fixed 16,384-frame bound, and the request must fit the
plan capacity. Empty selections are allowed. The normal Main-only and device
`PostNodeObserver` APIs remain available.

Capture is speculative until the complete render returns `Ok`. Only then does
`valid_frames()` expose the committed prefix through `output(index)`. Every
refusal invalidates the entire view, sets validity to zero and clears supplied
capture slices within the fixed count/frame bounds, plus Main PCM,
including late-chunk failures. Runtime clocks, routing, committed mutable state
and processor histories follow the existing whole-block transaction. The caller
may convert or write captured samples only after success. No callback plan
building, allocation/free, locking, file I/O or routing policy change is needed.

## signal and historical ownership

All canonical contributors render once, including unselected tracks. Track
capture includes processed audio and instrument output, effective mute/solo,
track gain/pan and applicable releases before Main summation. Empty chains are
included. Shared Returns consume all normal sends once; Return capture includes
actual processing, output matrix, gain/mute and residual tails before Main/master.
Main master processing is excluded from every capture. Nonlinear Main processing
means summing captures is not a Main reconstruction contract.

Retired effect/instrument suffix groups explicitly retain their original TrackId.
Current and retained generations with the same ID accumulate in that ID's slice.
A removed owner remains its original ID, even when another track occupies its
former index. Old tails keep their original mix, pan, processor history and sonic
routing: a tail routed directly to Main by the existing kernel is not newly sent
through the replacement Return. Retired Return groups retain their original
ReturnId, matrix and gain/mute. A gain-only compatible retarget retains the
existing runtime path rather than creating an artificial old generation.

## confidence and limits

Independent PCM fixtures cover shared selected/unselected sends, fader/pan,
Return matrix/gain/mute, empty chains, nonlinear processing, mutable held/released
history, sample offsets and partitions. Actual public inheritance fixtures cover
owner deletion/replacement and carried retired generations; late-chunk refusal
must leave retry PCM/history intact. Allocation probes measure total and signed
net count/bytes, with construction and reclamation outside measured scopes.
Record final exact-source validation in PR159; fixture coverage is not a claim
of every density/fault path or native callback behavior.

Physical throughput, native macOS/CoreAudio interaction, listening acceptance
and reference parity remain separate manual gaps. The active Pulse30 workflow
cannot accept the new prerequisite link; that tracker limitation does not change
this engine contract and is not bypassed here.
