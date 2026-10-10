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
