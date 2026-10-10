# hex-lattice

## Purpose

Provide validated stereo transforms and registered, bounded stateful effect
definition/runtime contracts, plus registered immutable analytic instrument preparation.
See [instrument slots](../../docs/lattice-instruments.md) for the separate note seam.

## Dependencies

This crate uses the Rust standard library and `hex-lattice-contract`. It defines stereo frames,
immutable finite 2×2 matrices, ordered composition, and explicit memoryless
latency metadata, plus the borrowed native-device definition registry and
preconstructed transactional processor twins with checked compatible histories.

## Does not own

This crate does not own device algorithms, graph scheduling, project state,
audio I/O, plugin hosting, UI, or sample storage.
