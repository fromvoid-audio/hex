# private unregistered latency layout proof

This integration-test module is absent from the production module tree. It
registers no processor, exports no public API, writes no schema and activates no
compensation. PR184 schema22 and all existing runtime code remain unchanged.

The graph adapter uses actual canonical TrackId, DeviceId, ReturnId and RouteId
identities, current prefix/instrument/suffix ordering, canonical sends and
additive ReturnFeeds. Synthetic source/device declarations are explicitly Known
or Unknown. Missing declarations are Unknown. Nodes use canonical vector order
for ready ties; feed enumeration is canonicalized without changing gain owners.
The proof calculates maximum incoming arrival, checked serial latency sums and
nonnegative incoming edge delays. Main can include synthetic old-tail arrivals
and a prior latency floor; an insufficient requested ceiling refuses.

Synthetic bypass has zero effective latency because the current runtime passes
dry samples; this does not alter the production Return admission guard.
Continuous positive-delay edges require supplied history coverage. This coverage
is a structural input, not certification of physical samples, epochs or DSP
compatibility. Those remain requirements for actual runtime handoff.

All limits are caller-supplied test inputs, never a new production policy cap.
The checked demand report charges measured stereo-frame size, each proposed
rounded ring capacity, two transactional sample banks, proposed Vec headers and
(cursor, valid-frame, epoch) metadata, actual allocated layout-vector capacities,
and caller-supplied active/retained ownership byte totals. Overflow and exhausted
coexistence budgets refuse. No delay ring is actually allocated or rendered.
Graph-adapter/sort scratch and allocator overhead are not asserted to be included
in this persistent layout demand; there is no complete process-memory/CPU proof.
No assumption assigns host rings to the processor's 4096-byte/twin limit.

The 16-frame declarations are synthetic fixtures for the proposed base-rate FIR
pair timing, not certification of that unregistered producer. Fixed linear-phase
reference delay versus current public latency documentation still requires a
reviewed contract. Unknown/nonzero Return public adoption stays refused.
Actual delay rings, sample/event timestamps, rollback, source pre-roll,
transport/latency/rate handoff, retained-tail history, export range/drain policy,
mutable-instrument latency declarations, and native/listening/deadline evidence
remain unimplemented. This proof does not complete Pulse28 or2252.
