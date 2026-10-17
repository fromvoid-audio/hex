# hex-lattice

## Purpose

Provide validated stereo transforms and registered, bounded stateful effect
definition/runtime contracts, plus analytic and optional mutable instrument
preparation/runtime contracts.
See [instrument slots](../../docs/lattice-instruments.md) for the separate note seam.

## Dependencies

This crate uses the Rust standard library, `hex-lattice-contract` and `hex-dsp`.
It reexports the existing stereo frame/matrix API from host-independent `hex-dsp`,
which owns that arithmetic without device or host contracts. It defines
immutable finite 2×2 matrices, ordered composition, and explicit memoryless
latency metadata, plus the borrowed native-device definition registry and
preconstructed transactional effect and instrument twins with checked compatible
histories. Prepared processors report known algorithmic delay in frames or
`None` when unknown; this describes fixed buffering/lookahead, not filter
phase/group delay, interface latency or effect tails. Prepared effect processors
declare a finite sample-frame tail bound or default conservatively to unbounded;
their runtimes expose a read-only quiescence query. Arrangement track-chain
handoff retains detached nonquiescent suffixes under bounded engine budgets;
return-bus retirement remains separate. See [native effect
tails](../../docs/native-effect-tails.md). Mutable instrument event
batches are caller-owned and preflighted before scratch-state processing; their
separate runtime factory declares a finite release-tail bound and exposes
committed-state quiescence for later retirement. Borrowed native clocks expose
optional exact performance origins and actual transport state; unavailable
observations remain explicit `None`, including stopped audition performance time.
The runtime pair also exposes immutable event-only preflight for control-side
pitch/capacity admission without a fabricated clock. Success never bypasses the
complete event and clock validation required by real processing.
Analytic instruments may also prepare an immutable shared context and compact
copyable note payloads; the host enforces per-context and per-revision byte
bounds, while the engine owns revision and release leases.

`NativeRuntimeChain::declared_storage_preflight_bytes_for_instances` estimates
the chain header, requested entry storage, and both processor-runtime twins
from declared fixed payload sizes before runtime factories run. It is not a
guarantee about allocator-rounded vector capacity. After construction,
`accounted_storage_bytes` uses the actual entry-vector capacity and charges both
declared twin payloads. The shared immutable configuration payload is capped at
4,096 bytes per device; its Arc handle is part of each runtime entry, while the
payload remains owned by the prepared graph and is excluded from runtime bytes.
Shared prepared definitions and allocator bookkeeping are also excluded;
processor definitions must keep their runtime byte declaration consistent with
the fixed payload they construct. Callers must compare actual accounting with
the preflight estimate and enforce their aggregate cap.

The `configuration_profile` example measures the real
`NativeRuntimeChain::configuration_matches_range` comparator over the maximum
regular-track and return-chain inventory using explicitly synthetic opaque
state. See [configuration comparison](../../docs/configuration-comparison.md)
for fixture bounds, invocation, and interpretation. It does not construct
deployable effects or measure an audio callback deadline.

The optional `PreparedAnalyticContinuation` capability validates a bounded opaque
parameter frame, provider format, compact note and source/target compatibility.
Its immutable five-millisecond clock is sampled at absolute performance frames;
the engine owns current-device versus detached-tail provenance, leases and
retirement. Unsupported providers default to no continuation capability. Invalid
admitted frames or clocks return explicit errors rather than silently changing
musical state. See [enable continuation](../../docs/operator-enable-continuation.md).

## Does not own

This crate does not own device algorithms, graph scheduling, project state,
audio I/O, plugin hosting, UI, sample storage, authored musical event generation,
or discontinuity chase policy.


Prepared analytic instruments and mutable factories declare known algorithmic
latency or explicitly unknown, using the same fixed-buffering definition as
effects. `NativeInstrumentClock` can expose exact performance beats, arrangement
frame and loop pass at the node's delayed input. Clock-aware effect range methods
preserve older processors' default behavior, including their bespoke bypass
handling. `zero_input_drain_for_range` charges checked intrinsic latency plus
residual tail; `output_tail_for_range` continues to report residual tail alone.
Graph timing, transport history, compensation buffers and handoff policy remain
engine responsibilities.

The optional `conformance` feature exposes control-side authoring fixtures for
codec/descriptor contracts, transactional effects and instrument capabilities.
It uses only existing traits and allocates outside callbacks. See
[authoring](../../docs/native-device-authoring.md) for checks and finite limits.
