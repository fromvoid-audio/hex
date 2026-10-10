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
histories. Mutable instrument event batches are caller-owned and preflighted
before scratch-state processing; runtime factories declare finite output-tail
bounds and expose committed-state quiescence for later control-side retirement.
Analytic instruments may also prepare an immutable shared context and compact
copyable note payloads; the host enforces per-context and per-revision byte
bounds, while the engine owns revision and release leases.

## Does not own

This crate does not own device algorithms, graph scheduling, project state,
audio I/O, plugin hosting, UI, sample storage, authored musical event generation,
or discontinuity chase policy.
