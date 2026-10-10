# hex-lattice

## Purpose

Provide validated stereo transforms and registered, bounded stateful effect
definition/runtime contracts, plus analytic and optional mutable instrument
preparation/runtime contracts.
See [instrument slots](../../docs/lattice-instruments.md) for the separate note seam.

## Dependencies

This crate uses the Rust standard library and `hex-lattice-contract`. It defines stereo frames,
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
committed-state quiescence for later retirement.
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

## Does not own

This crate does not own device algorithms, graph scheduling, project state,
audio I/O, plugin hosting, UI, sample storage, authored musical event generation,
or discontinuity chase policy.
