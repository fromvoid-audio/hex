# Original oscillator envelope kernel

Pulse2167 is the standalone mathematical prerequisite of per-operator envelopes
2161 and native Operator19. `hex-devices::operator_envelope` prepares four
independent amplitude envelopes. It is not registered, connected to FM routing,
or exposed as a playable device. Existing FM4's global linear ADSR, persisted
state and analytic note payload remain unchanged.

The [reference inventory](../research/observations/operator-contract-001.md)
records the public Operator control inventory. The equations and bounds below
are original choices, not measured Ableton behavior or a numerical parity claim.

## Units and finite laws

Each `OperatorEnvelope` validates Initial, Peak and Sustain as finite linear
amplitudes in [0,1], and Attack, Decay and Release as finite seconds in [0,120].
Levels need not be descending. Preparation accepts sample rates 8,000–192,000 Hz
and rounds each duration to the nearest frame, with half frames rounded upward.
Each stage is at most 23,040,000 frames. Preparation computes reciprocals and
curve rates; sample queries do not convert musical time or allocate storage.

For note-relative age `a`, attack frames `A`, decay frames `D`, initial `I`,
peak `P`, sustain `S`, and `k = -ln(1e-5)`, the held level is:

- `a < A`: `I + (P-I) * a/A`.
- `A <= a < A+D`: `S + (P-S) * exp(-k * (a-A)/D)`.
- At or after the decay endpoint: exactly `S`.

Zero attack skips immediately to Peak; zero decay skips immediately to Sustain.
The implementation uses ordered subtraction rather than overflow-prone sums.
At NoteOff age `g`, release starts from the instantaneous held value `H(g)`,
even during attack or decay. For release duration `R`, released age `r = a-g`
has level `H(g) * exp(-k*r/R)` for `r < R`, then exactly zero. Zero release is
silent from NoteOff. The continuous exponential endpoint is a relative 1e-5
from its target; the exact endpoint snap is deliberate. Adjacent sampled values
can differ more for very short stages, including a one-frame stage.

A zero gate with nonzero release starts from the held value at age zero: Initial
when attack is nonzero, Peak after a zero attack, or Sustain after both zero
attack and decay. This differs deliberately from existing FM4's zero-gate
silence. No shared FM4 law or existing project is silently changed.

`PreparedOperatorEnvelope::level(age_frames, gate_frames)` returns `f64` linear
amplitude for use before oscillator routing. Its absolute-age query has no
mutable history or replay and therefore gives the same bits for the same input
regardless of query order or block partition. A future event-driven voice must
latch its actual NoteOff age; this kernel does not schedule or invent that event.

## Ownership and remaining integration

Prepared envelopes contain only fixed scalars and are Copy. The four-envelope
bank returns one level per oscillator and the maximum release-frame count.
That bound describes these envelopes only, not feedback, filters, external
effects, or full instrument quiescence. No lazy table initialization, heap
ownership, locks, I/O or final asset drops occur in sample queries by source
inspection. This is not an instrument callback allocation-probe claim.

Each queried exponential stage evaluates an exponential. No SIMD, recurrence,
CPU throughput or real-device deadline improvement is claimed. Any later curve
optimization must preserve independent output oracles and measure its workload.

Pulse2161 remains open for applying these four levels before FM routing and
connecting bounded voice events/rollback. Runtime revision continuity2163,
seek/chase2164 and loop wraps2165 remain separate gates before a mutable native
Operator can be exposed. Envelope looping, beat/sync/trigger modes, scaling,
retrigger/free-running phase, Live algorithms, feedback and numerical reference
measurements are not implemented by this module.

## Evidence

Independent reference tests use powers of the declared cutoff rather than the
prepared exponential/reciprocal path. Complete sampled curves cover held and
released values, NoteOff during every segment and at its boundaries, increasing
and decreasing levels, all zero-duration combinations, zero gate, malformed
numbers/rates, half-frame rounding at 44.1/48/192 kHz, and maximum `u64` ages.
Four distinct curves are compared independently and queried with changed
partition sizes and reverse order. Device tests also retain existing FM4 and
Utility coverage. Native listening is inapplicable until device integration.
