# prepared seek cancellation

## scope

This contract closes the interval between publishing an interactive,
state-preserving Arrangement seek and the audio callback installing that graph.
Publication success means the bounded queue accepted the candidate; it does not
mean the callback applied it. Canonical edits, project replacement, transport
invalidation, and shutdown can therefore cancel a published conditional fresh
runtime before it becomes active.

The cancellation API applies only to
`InstallFreshRuntimeIfUnchanged`. It does not alter `InheritRuntime` or ordinary
`InstallFreshRuntime` publication. A cancelled candidate leaves the current
runtime active and is returned through the existing bounded retirement queue.

## decision and ordering

Each conditional publication creates one control-side decision cell containing
its full immutable audio revision and an atomic state:

- `pending`: callback has not committed to apply; control may cancel.
- `cancelled`: control won the compare-and-swap; callback must return the
  candidate without installing it.
- `applying`: callback won the final compare-and-swap immediately before the
  active graph replacement; cancellation is too late.
- `complete`: callback has applied or returned the candidate.

The revision and state share one allocation, so there is no split key/state
read, torn key, truncated revision, or key/state ABA window. Audio publication
revisions advance monotonically and refuse overflow. `Control` retains only the
latest conditional token, and its mutable publisher/cancellation caller is the
canonical UI control thread. A new publication cannot be admitted while the
previous publication gate is pending. Failed queue admission stores no token and
rolls back the gate.

The callback checks the existing active-revision, route-generation,
command-prefix and pending-Stop fence. Immediately before `active.replace`, it
performs `pending -> applying`. This CAS is the linearization point:

- If UI cancellation changes `pending -> cancelled` first, callback publishes
  the exact candidate rejection and returns its ownership through the normal
  retirement queue. The old graph continues playing.
- If callback changes `pending -> applying` first, cancellation reports
  `applying`. The UI retains the pending seek until that exact audio revision
  is applied or rejected; the ordinary canonical graph publication follows the
  current project state.

Stop keeps its separate priority path. A Stop admitted before the callback's
fresh-runtime fence rejects that candidate; a Stop admitted after the apply
decision is consumed after the swap and stops the newly active graph. Neither
case waits for cancellation or graph preparation.

During shutdown, the callback atomically completes each queued/deferred token.
It reports `fresh runtime cancelled` only when explicit UI cancellation won
before shutdown drained it; an ordinary shutdown drain does not masquerade as
a user cancellation.

## ownership and callback budget

The decision cell is allocated before queue admission on the control side. The
callback never clones or allocates an `Arc` for it. The token moves with its
`Publication` through the incoming slot, deferred candidate, active graph,
retirement ring, or defensive held-retirement slot. Callback code only performs
atomic loads/CAS/stores and moves the existing publication into its bounded
return queue. Final token and graph drops remain on control-side reclamation.

At most five distinct decision cells can be live: one active publication, one
queued or deferred publication (the queue and deferred slot cannot both own the
same publication), two retired publications, and one held-retirement
publication. `Control`'s latest reference aliases one of these cells; it does
not create another cell. A failed enqueue drops both token references on the
control side. Ordinary publications create no decision cell.

The cancellation probe measures callback begin/render for a cancelled
conditional publication and requires zero total and signed-net allocation
changes. It separately confirms that control reclamation performs the final
drop. This headless probe does not establish physical-device deadline or native
callback API behavior.

## evidence and remaining validation

The focused audio tests cover cancellation before the final CAS, the callback
winning the CAS first, mismatched revisions, enqueue rollback, successor
publication, Stop on both sides of the decision, and ordinary inherited
publication. The UI helper test exercises the same cancellation call used by
published-seek invalidation and keeps the pending state until the callback's
exact rejection revision arrives.

The GPUI interaction and actual-device behavior still require a human check in
the combined native build. No UI launch or physical audio device is part of the
headless test evidence.

## held device previews (pulse2881 / pr483)

A stateful Arrangement play request can use the currently applied device preview
without ending the held parameter gesture. The private seek intent and pending
seek retain its existing `DevicePreviewIdentity`, prepared revision Arc and exact
preview view metadata. The resulting pending playback and callback observation
retain the same identity; a preview-derived runtime never becomes a canonical
candidate merely because a seek installed it. Canonical seeks exclude device
and tuning preview sources. Tuning-preview seeking is not added here.

Admission requires the matching applied output, project generation/revision,
gesture key/sequence and prepared Arc, no pending terminal/edit/recovery, and the
existing coherent Arrangement route/fresh-runtime fence. Preparation result,
publication and callback reconciliation recheck the source. A newer sequence of
the same held gesture retains only the user's latest play intent while waiting
for the matching preview to settle. Terminal commit/cancel, another gesture,
project replacement or device removal invalidates the old intent. Pause cancels
pending play without releasing the parameter gesture. A runtime already handed
to the existing publication worker retains its token until the response supplies
its cancellable revision; ownership still retires through the existing path.

Headless tests deliver actual pointer/key events, real Service preparation,
publication and callback blocks. They cover stationary/dragged play and pause,
sequence replacement before and after publication, cancel/commit, replacement,
device removal, retained preview metadata and ordinary canonical seeking. The
node-position test additionally checks 48 tick/repaint cycles and actual Undo/
Redo. These are not a native framebuffer flicker disproof or physical audio
qualification. The user confirmed that the reported hold is an EQ band node held with the
mouse; MIDI keyboard-note holding is not the reproduction.

Finite native follow-up, owned by the coordinator: in a qualified build with a
real output, hold and drag an EQ node while pressing Space to play/pause, move
again during preparation, then commit and Undo/Redo. Repeat Escape, device/project
replacement and numeric text editing (Space stays text); observe curve/control
stability during sustained drag. Check the originally reported flicker separately
and retain its actual source/build and interpretation rather than declaring it
fixed from these headless receipts. No app operation is performed by this source
unit.

### busy resident publication and seek retirement (20 october 2026)

A prepared seek can remain `CandidateReady` in `pending_playback` when the real
resident forwarding cell is busy. Before a later worker preview replaces that
slot, receive now returns the exact matching candidate through the existing
Service seek-worker retirement path. It retains only the latest intent for the
same live held gesture, generation and canonical revision. Terminal, project or
foreign-owner replacement does not restart it. In-flight publication keeps its
existing token, response and conditional-cancellation protocol.

The single fallback result is the existing retirement backpressure owner. New
seek requests latch only scalar intent while it is occupied; tick retries it
before admitting preparation or consuming another result. Thus a UI-held retired
result and a new `CandidateReady` cannot coexist in the admitted path. A test-only
assertion checks this precondition at each handback; no runtime panic, queue or
owner slot was introduced.

The regression first failed on unchanged production at the actual
Busy-to-newer-preview receive cutpoint, then passed after the repair. Its scalar
witness records exact request/prepared-root identity and the real Service
handback result; it does not claim native destruction timing. A second component
case places a real completed runtime in the existing fallback and checks repeated
request admission and exact-owner retry. That seed is explicitly not a simulated
worker-Busy result. Existing Session worker tests independently cover occupied
retirement, shutdown handback and worker-thread destruction. Native visible
flicker, physical input/audio and optimized combined-build acceptance remain open.

### held preview publication races (20 october 2026)

The publication response finishes invalidation without clearing the latest
already-latched intent. A replacement preview may be intermediate: its identity
must share the old and currently active gesture key, generation and canonical
revision, with old <= incoming <= active sequence. Foreign, terminal and older
identities retain their existing refusal behavior. Admission still waits for the
exact latest applied preview and its original prepared-root/fence checks.

Cancellation result is distinct from the invalidation request. If callback is
Applying, the exact pending identity survives until actual terminal observation.
If Complete, its applied revision must match before it can restore audible
metadata; a completed returned/canceled candidate remains canceled. Pause after
callback installation therefore reconciles the actually installed preview and a
later held Space can capture a fresh fence. No new runtime owner, counter, queue,
publication API or callback work is added.

Three real GPUI/Service/publication/callback regressions failed at the intended
assertions on f855 production; all three passed after the repair, alongside the
11 existing focused review controls. The final immutable component receipt is
recorded in Pulse2881; the completed-return cancellation control and actual audio
CAS/allocation controls are included there. These automated observations do not
establish native framebuffer smoothness, physical input/audio deadlines or user
acceptance. Sole merger owns final prospective-tree qualification and merge.
