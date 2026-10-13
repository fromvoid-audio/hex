# private musical typing foundation

Pulse26 / canonical draft PR190. This is an isolated test-only host-input
protocol, not working musical typing, MIDI input or a prepared audio consumer.
`hex-engine/src/typing_foundation.rs` has no production module declaration.
Engine integration tests include its single source; the existing audio
allocation-probe test infrastructure reuses those tests and that same source.
No provider, engine renderer, command ring, UI/service actor, project codec,
Operator state or shared widget has been wired or changed.

## approved finite product contract

Opt-in typing arms one explicit stable track/device. The twelve lowercase keys
`a w s e d f t g y h u j` map to semitone pitches60–71, fixed velocity100.
NoteOn is offered only for a playing transport, admitted target, active window
and focus permitting musical input. Text editing, modals and modified keys
prevent new onsets. Owned key-up remains independent of current selection and
focus, allowing held notes while editing controls. Autorepeat and duplicate
physical downs never create another occurrence. No implicit Play or project
note editing, capture, recording, octave/velocity controls or MIDI support.

This module's `Focus` is a pure decision adapter. Actual GPUI focus/capture
hooks, text-widget classification and native key delivery are not installed.
No physical keyboard-unplug notification is established.

## identities, receipts and cancellation

Authored and live identities are distinct enum variants; the full-width live
press token is `(epoch, serial)` plus its original owner. No high-bit reservation,
fake ClipId/NoteId, authored clip or artificial gate deadline is used. A future
native runtime bridge must assign IDs from its shared occurrence allocator and
retain this tagged provenance; passing a press serial as a native occurrence
ID is not authorized by this model.

Checked serial exhaustion refuses a new press while retaining existing
releases. Epoch exhaustion still returns the mandatory cancellation watermark
and permanently prevents rearming beyond it. Queue tickets0/u64::MAX are
invalid, matching the existing command channel's reserved values. A queue
receipt only changes Offered to Queued; it cannot set Applied. The external
audio-transaction result is separate. A capacity of32 does not imply that any
particular offer was enqueued, consumed or applied.

On refusal removes the matching pending press, suppressing further downs until
key-up. A failed release admission returns an external epoch cut that disarms
all typing-owned presses; it must be published even when the ring is full.
Rearming requires observation of the previous cut. Delayed acknowledgements,
old-epoch onsets and old cuts cannot recreate or alter newer presses.

The reference ledger consumes monotonically ordered queue tickets, including
refused actions, and applies a trusted cancellation watermark before its
captured queue prefix. Tests enumerate both old-onset/cut arrival orders.
There are no atomics or ring in this proof: future Acquire/Release publication,
callback entry cuts and transport races require real bridge tests. A caller
cannot treat these deterministic interleavings as proof of concurrent delivery.

## release and resource ownership

The ledger reserves up to64 live occurrences; actual admission combines them
with externally supplied authored/other reservations under64 analytic or16
Operator mutable voices. It validates an externally supplied remaining-event
budget within0–256. These inputs are not a native admission certificate; the
future bridge must derive and consume them against the actual combined batch,
provider/Nyquist checks and all owned storage without weakening existing caps.

Held, ReleasePending and Releasing are separate. Off remains bound to the
original owner and creates a pending release even while paused. Pause produces
no release batch; resumed rendering must dispatch and acknowledge pending
releases before its first sample. Returning a batch does not change the ledger;
only explicit release-application and retirement acknowledgements do. Released
reservations remain occupied until retirement, including across rearming.
No envelope time, PCM, chase or retirement deadline is invented here.

Disarm, window inactivity, audio disconnect and queue-full recovery mark
typing-owned gates for release. Stop/seek/reset cuts clear the transient ledger
to model the existing global reset boundary. These operations cannot edit
authored identities; production reset/retained-tail behavior still needs its
ordinary host transaction. Computer keyboard peripheral-disconnect detection
is not implemented merely because an AudioDisconnected reason exists.

All protocol types have fixed inline storage and no Drop. Tests measure total
and signed-net allocations/bytes for construction, offers, receipts, release,
pause/resume and retirement. This is not a callback allocation, full playback
budget, performance, native listening or hardware deadline claim.
Rust1.94 on this Linux64 build measures keyboard824B, ledger3632B,
release batch3080B, action48B and result56B. Stack/owner copies and eventual
bridge scratch must all be charged separately; these are not total host costs.

## remaining integration and acceptance

Await coordinator handoff before touching engine preview/service/UI owners.
Required later work: real arm preparation and acknowledgement, complete palette
and ledger accounting, native-provider admission, shared authored/live voice
dispatch at the single freely positioned instrument, actual queues/priority
cuts/cancellation, compatible history handoff and retired-owner release,
GPUI focus/key capture, audio lifecycle and visible errors/recovery. Then run
exact-source full localCI, real callback allocation and changed-path timing
checks, optimized macOS/input/listening acceptance and explicit human review.

Full26 audio input/monitoring/recording, controllers/MPE, external MIDI output
and Session capture remain separate. Held-note2372 and LFO review2384 remain
pending until an actual usable input pipeline and native candidate exist.
HOLD remains: blocked/draft, no gate/check receipt or readiness registration.
