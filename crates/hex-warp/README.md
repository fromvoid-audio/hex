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

## Beats redesign checkpoint (Pulse 2847)

The first WSOLA slice covers automatic Transients, Forward, envelope100.
Detected attack neighborhoods remain at the exact inverse-map positions; the
intervening bed uses one stereo-linked similarity search and normalized overlap
ring. Candidate scoring uses inexpensive samples; only selected grains use the
bandlimited reconstruction. Coherent overlaps preserve DC and low-confidence
noise uses bounded shared energy compensation. The combined plan, filter bank,
search allowance and ring are quoted before allocation under the existing caps.
Algorithm version4 invalidates older prepared PCM.

This is deliberately a partial checkpoint. Analysis currently uses the bounded
energy detector. Multiband onset refinement, fixed-grid boundaries independent
of physical attacks, adaptive grain sizing, Alternating/Stop/envelope integration
and the full musical/listening matrix remain open. Other control combinations
still use the prior Beats path. Tones and Texture retain their own algorithms.
There is no commercial-reference or listening acceptance claim.

`beats_wsola` checks phase-independent pitch/RMS, linked stereo, DC/noise energy,
peaks, attack timing and cancellation. The Engine's
`beats_wsola_default_preparation_locks_drums_and_keeps_bass_continuous` exercises
canonical default controls through preparation/cache and partitioned/seeked
playback at1.5x/2x. The create-new `beats_diagnosis` example accepts an optional
`transients` argument; without it the original fixed-division evidence recipe
is unchanged. Synthetic WAVs are fixed-gain listening candidates, not reference
recordings. Original red and intermediate experiment evidence remains separate.
