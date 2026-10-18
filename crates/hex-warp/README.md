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

The WSOLA path covers Forward and Alternating with all envelope settings, for Transients and fixed Preserve grids.
Detected attack neighborhoods remain at the exact inverse-map positions; the
intervening bed uses one stereo-linked similarity search and normalized overlap
ring. Candidate scoring uses inexpensive samples; only selected grains use the
bandlimited reconstruction. Coherent overlaps preserve DC and low-confidence
noise uses bounded shared energy compensation. The combined plan, filter bank,
search allowance and ring are quoted before allocation under the existing caps.
Algorithm version8 invalidates older prepared PCM.

Physical analysis combines a fast energy-rise path with streaming four-band
spectral increase and stereo complex-phase prediction. A spectral novelty peak
must also pass a sample-level innovation-concentration check before local onset
refinement; changing FFT leakage in a sustained tone is not an attack. Stereo
evidence adds independent channel magnitudes instead of summing opposite phases.
FFT/window/two-generation stereo history storage joins the summed scratch quote.
Fixed-grid points are scheduling boundaries, not physical attacks or filter
clamps. Lower envelope settings apply the authored exponential gain once after
normalization, across each logical Preserve interval. Physical attacks inside a
fixed interval do not reset that envelope; overlap grains never restart it.
Unity time/pitch copies the original waveform before any authored decay. At Forward100, no grid-triggered envelope or traversal reset is needed.
The authored grid-density limits still validate before analysis or output birth.

Alternating traverses the available body forward first, then reflects only the
safe tail between protected physical guards. One signed traversal/search and
normalized turn overlap serve both channels. Compression stays forward. The
private scalar plan checks native traversal and nonduplicated endpoints; the
output intentionally smooths turns instead of promising raw reversed indexing.
Short safe bodies shrink grain shoulders to retain available native material.
Continuous source-wrap boundaries remain logical edges, not attack guards. A
stereo cyclic-innovation check distinguishes actual wrap attacks; signed cycle
origins retain phase. With no safe body, a bounded endpoint splice cannot sample
the upcoming protected attack. One/two-sample material is never turned into an
indefinitely held or repeated attack by an empty-interval clamp.

This remains a partial checkpoint. Adaptive grain sizing, the complete short-edge
and variable-rate matrix, Stop integration and the full
musical/listening matrix remain open. Other control combinations still use the
prior Beats path. Tones and Texture retain their own algorithms.
There is no commercial-reference or listening acceptance claim.

`beats_wsola` checks phase-independent pitch/RMS, linked stereo, DC/noise energy,
peaks, attack timing and cancellation. The Engine's
`beats_wsola_default_preparation_locks_drums_and_keeps_bass_continuous` exercises
canonical default controls through preparation/cache and partitioned/seeked
playback at1.5x/2x. The create-new `beats_diagnosis` example accepts an optional
`transients` argument; without it the original fixed-division evidence recipe
is unchanged. Synthetic WAVs are fixed-gain listening candidates, not reference
recordings. Original red and intermediate experiment evidence remains separate.
