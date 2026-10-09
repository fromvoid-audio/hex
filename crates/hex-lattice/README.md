# hex-lattice

## Purpose

Provide small validated linear transforms for stereo signal processing.

## Dependencies

This crate uses only the Rust standard library. It defines stereo frames,
immutable finite 2×2 matrices, ordered composition, and explicit memoryless
latency metadata.

## Does not own

This crate does not own device algorithms, graph scheduling, project state,
audio I/O, plugin hosting, UI, or sample storage.
