# spectrum analysis

## bounded kernel — Pulse2287

`hex-audio::spectrum` provides original observational analysis for the
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
no per-analysis heap operation; integrated callback probes are described below.
`reset` clears transform scratch; assembling a fresh rolling window, gap
handling, cadence and result freshness belong to Pulse2288.

Focused regression tests cover frequency/gain, silence, anti-phase stereo,
rate endpoints, reset and malformed input followed by valid analysis. Final
exact-source full local CI, PR review and merge evidence live in Pulse2287.
Pulse2250 owns live post-node integration and its separate native
visual/listening acceptance.

## bounded capture worker — Pulse2288

`hex-audio::spectrum_worker` consumes accepted stereo output supplied by its
caller; it does not own or install the audio tap. Capture submits
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
result carries its target context, caller-supplied capture start and FFT
completion time. The CPAL adapter stamps the callback start for each fixed
chunk, conservatively dating later chunks in that callback no later than they
were rendered. `try_latest` drains at most the channel capacity and discards
results more than 100 ms after the represented capture time. This is a bounded
freshness estimate, not proof of physical capture or display latency under
worker starvation.

Construction occurs on control. Only the capture producer moves into a callback;
its submit path copies bounded inline data and performs one nonblocking push.
After a successful stream pause, the lifecycle owner drops the stream and its
callback-owned producer, joins the worker reaper, then releases the output
lease. Failed pauses retain all three. Dropping the reaper requests exit but
does not join.

Deterministic worker tests cover cadence, changing context/revision, gaps,
malformed/rate input, exhaustion, idle freshness and lifecycle. An opt-in capture
probe asserts zero total allocation count/bytes and zero signed net count/bytes,
with construction and reclamation outside measurement. It tests submission, not
native APIs or physical deadlines. Full exact-source local CI and focused PR
evidence are tracked by Pulse2288.

## live EQ8 post-node display — Pulse2250

The prepared renderer can observe one selected node after its own processing.
Identity includes track or return owner, canonical device ID and device type;
only current graph owners are observed, never retired tails. The callback stages
at most 256 frames with exact render offsets, publishes only complete finite
final-output blocks, and marks failed or rolled-back renders as sequence gaps.
With no active analyzer or before the selected project publication reaches its
activation revision and exact control-context generation, playback uses the
ordinary unobserved render path. The immutable generation belongs to the audio
publication; it is not a project schema field. Legacy publication APIs use
generation zero, which remains valid. The UI supplies its pending project
generation, and capture and submission both verify actual generation and rate
against the active publication. Target epochs supplement this proof; reused
device IDs after opening another project cannot satisfy it.

The compact EQ8 panel toggle selects one exact EQ8 node. A bounded control queue
and desired epoch invalidate stale callback targets immediately, including when
the queue is full. Results are filtered by target token, project generation,
sample rate and activation revision; later ordinary parameter revisions remain
eligible. The shared response graph draws a muted spectrum behind its editable
EQ curve. The trace clears when its target is removed, its owner/project changes,
or capture becomes stale. Existing audio admission remains 8,000–192,000 Hz;
this analyzer does not widen or add to playback's existing rate contract.

Controller ticks retire the target and clear its cached trace when the device
panel closes, clip view becomes active, focus moves to another owner, the project
generation changes, or the exact EQ8 node disappears or changes type. Ordinary
parameter revisions on the same active device preserve the trace. Retirement
drops the paused stream/callback before joining the analyzer worker, then releases
the output lease. Source tests cover stable owner/type identity, activation
floors, bounded staging offsets/overlaps, target queue races and exhaustion,
lifecycle order, stale context and stale capture age. Run the headless callback
comparison with `cargo run -p hex-audio --release --example
profile_eq8_spectrum -- 128 4000`; it checks PCM equivalence separately from
timing. Remaining evidence: exact-tree local CI, native UI interaction and
listening, and any physical-device deadline claim. The analyzer is original
telemetry; no Live algorithm or visual parity is claimed.
