# spectrum analysis

## bounded kernel — Pulse2287

`hex-audio::spectrum` provides original observational analysis for the planned
EQ8 output analyzer (Pulse2250). It is backend telemetry, separate from native
musical device processing. A separate reusable crate is unnecessary while this
backend is its only consumer. The kernel adds no callback capture, worker, UI,
project state, schema migration or audible processing.

`SpectrumKernel::new` validates rates from 8,000 through 192,000 Hz and allocates
fixed transform storage on control. `analyze` accepts exactly 4,096 finite
stereo frames; invalid length or samples refuse before scratch changes. Each
channel uses an independent radix-2 FFT with a periodic Hann window. Averaging
channel power preserves opposite-polarity stereo material. Coherent-gain
normalization calibrates a bin-centered full-scale sinusoid to 0 dBFS; an
amplitude of 0.5 gives approximately -6.0206 dBFS.

The 96 logarithmic bands span 20 Hz through the smaller of 20 kHz and 0.49 times
the sample rate. `band_bounds_hz` exposes the same geometry to future display
code. Each band reports its strongest bin amplitude with a -120 dBFS floor;
values above full scale remain positive. Bands narrower than FFT resolution use
the nearest positive-frequency bin, so adjacent low-frequency bands may share a
value. Frequency resolution is sample rate / 4,096, not the number of display
bands. This is a peak-amplitude spectrum, not integrated band RMS or a claimed
Live analyzer match.

Analysis reuses fixed boxed f64 workspace and fixed output arrays. All finite
f32 input magnitudes remain within f64 transform/power range. Inspection shows
no per-analysis heap operation; callback integration has not yet been measured.
`reset` clears transform scratch; assembling a fresh rolling window, gap
handling, cadence and result freshness belong to Pulse2288.

Focused regression tests cover frequency/gain, silence, anti-phase stereo,
rate endpoints, reset and malformed input followed by valid analysis. Final
exact-source full local CI, PR review and merge evidence live in Pulse2287.
Live post-node capture, accepted-render publication, retirement, callback
overhead and native visual/listening acceptance remain Pulse2250.
