# hex-warp

## Purpose

Original bounded, deterministic stereo audio transformation during control-side
preparation. Resampling, transient preservation, pitch-synchronous and textured
granulation, and linked stereo spectral processing have distinct implementations.
Inputs are borrowed immutable PCM and validated finite maps; output is prepared
PCM. Cancellation, output, scratch and work limits are explicit. These algorithms
implement the declared Hex mode contracts, not Ableton's private DSP identity.

## Dependencies

Only the Rust standard library. The engine translates canonical project metadata
into this crate's narrow audio/map/settings contracts and owns caching/budgets.

## Does not own

Project editing/history, source decode, files, UI, threads, queues, callbacks,
retirement, cache lifetime, device I/O, or foreign algorithms/binaries.
