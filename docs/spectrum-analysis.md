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

## bounded capture worker — Pulse2288

`hex-audio::spectrum_worker` consumes accepted stereo output supplied by its
caller. It does not yet install a tap in the prepared callback. Capture submits
1–256 finite frames through a 32-batch rtrb SPSC queue. Full or invalid
submissions consume a sequence but drop only analysis; the next accepted batch
exposes the gap. Sequence overflow refuses further submission until the owner
constructs a new producer and generation. Rates remain 8,000–192,000 Hz.

The worker owns fixed rolling and ordered 4,096-frame buffers and the kernel.
It drains at most 32 batches per poll, sleeping five milliseconds when idle.
Changes in target, generation, rate or sequence continuity reset the window.
Applied parameter revisions preserve contiguous actual output and stamp its
latest revision. The global 30 Hz analysis limit survives context changes;
unchanged windows are not recomputed after capture stops.

A bounded one-result channel separates worker results from control. Each
result carries its target context, worker receive time and FFT completion time.
`try_latest` drains at most the channel capacity and discards results more than
100 ms past worker receipt. This is receipt-based freshness, not proof of
physical capture or display latency under worker starvation. Actual callback
integration must validate accepted-render publication, queue overflow/gaps,
revision/target retirement and end-to-end freshness before exposing a trace.

Construction occurs on control. Only the capture producer moves into a callback;
its submit path copies bounded inline data and performs one nonblocking push.
After callback quiescence, return the producer to control and consume it in
`shutdown_and_join`; its destruction and worker join stay off callback. Dropping
the worker handle signals exit but does not join. The owner must use explicit
shutdown to verify retirement.

Nine deterministic worker tests cover cadence, changing context/revision, gaps,
malformed/rate input, exhaustion, idle freshness and lifecycle. An opt-in capture
probe asserts zero total allocation count/bytes and zero signed net count/bytes,
with construction and reclamation outside measurement. It tests submission, not
the future live callback tap, native APIs or physical deadlines. Full exact-source
local CI and focused PR evidence are tracked by Pulse2288.
