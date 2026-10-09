# hex-lattice

## Purpose

Provide small validated linear transforms for stereo signal processing.

## Dependencies

This crate uses the Rust standard library and `hex-lattice-contract`. It defines stereo frames,
immutable finite 2×2 matrices, ordered composition, and explicit memoryless
latency metadata, plus the borrowed native linear-device definition registry.

## Does not own

This crate does not own device algorithms, graph scheduling, project state,
audio I/O, plugin hosting, UI, or sample storage.
