# private original Operator low-pass proof

Pulse #2326 / canonical PR212 retains the complete original filter-processing
and UI-binding scope. This is a private mathematical precursor only. The new
module is compiled by test harnesses through `#[path]`, not exported by
`hex-devices`, registered, encoded, bound to UI, or used by playback. No existing
provider, runtime, project state, codec, descriptor or preview value changes.

## chosen original law

The approved Hex-original law is a bilinear two-pole Butterworth low-pass with
fixed Q = 1/sqrt(2). Sample rate is an integer from 8,000 through 192,000 Hz;
finite cutoff is 20 through min(20,000, 0.45 * sample rate) Hz, inclusive.
Out-of-domain values are refused, not clamped. This is a nominal 12 dB/octave
low-pass, not a declared reference circuit or digital high-frequency slope
equivalence. There is no resonance-percent mapping, drive, nonlinear circuit,
24 dB cascade, filter envelope or modulation implementation.

For k = tan(pi * cutoff / rate), d = 1 + sqrt(2)k + k²:

```text
b0 = b2 = k²/d                 b1 = 2k²/d
a1 = 2(k² - 1)/d               a2 = (1 - sqrt(2)k + k²)/d
H(z) = (b0 + b1 z^-1 + b2 z^-2)/(1 + a1 z^-1 + a2 z^-2)
y[n] = b0 x[n] + b1 x[n-1] + b2 x[n-2] - a1 y[n-1] - a2 y[n-2]
```

Preparation owns trigonometry. Processing uses f64 coefficients and independent
physical f64 sample history for the two lanes; output rounds once to f32.
No threshold, limiter, saturation or nonfinite-to-silence fallback is inserted.
Both input lanes and computed outputs must be finite and representable in f32.
A refused frame leaves both lane histories untouched; successful earlier frames
remain committed. This does not implement a whole-chunk audio transaction or
application ACK. A future host must preflight/process scratch and validate the
actual final output before committing its own block.

Exact history copy requires equal prepared law and rate. Other configurations
refuse without mutation. There is no changed-cutoff retarget or smoothing law.
Reset clears all history. Stable device/occurrence authorization, bypass-history
advancement, Pause/Stop/seek/loop behavior and detached output ownership remain
production work, not properties established by this standalone reset operation.

## source proof boundaries

Independent oracles use closed impulse values at cutoff = rate/4, denominator
Jury inequalities and conjugate pole norms, and an impulse DFT compared with the
prewarped analog Butterworth magnitude 1/sqrt(1 + frequency_ratio^4). Checks
include DC, the authored -3 dB cutoff, Nyquist, all rate/domain boundaries,
nonfinite refusal, channel isolation, sign symmetry and finite f32 extrema.
Arbitrary partitions, exact history continuation, reset and incompatible-copy
refusal test observable PCM and state. A finite step exceeding f32 output range
demonstrates atomic right-lane refusal without partially advancing the left lane.

On the selected 64-bit Linux target, prepared storage is 48 bytes, runtime 112
bytes (including 64-byte stereo history), alignment 8. Two standalone copies
would occupy 224 bytes. These measurements exclude future voice integration,
prepared ownership, metadata, proofs, retained tails, checkpoints and host
buffers, and are **not callback admission** or a complete memory charge.

`hex-audio/tests/operator_filter_proof_allocations.rs` imports only this private
kernel under the existing non-default `allocation-probe` test harness. It uses
the already permitted dev-only allocation counter without a manifest/policy
change. Construction is outside measurement; processing, exact history copy,
reset, incompatible copy and invalid/overflow refusal must have zero total
allocation count/bytes and zero signed-net count/bytes. The existing sensitivity
calibration remains required. This is standalone kernel coverage, not a
synthetic audio callback, native allocation or physical deadline proof.

The explicit optimized test probe alternates baseline-copy and active-filter
order across five trials at 48 kHz, 128 stereo frames, 200 warmup blocks and
1,000 measured blocks per trial. Both paths store output and use a checksum;
construction and timing-vector allocation are outside timed blocks. Keep every
trial's p50/p99/maximum and checksum, including scheduling outliers. This measures
one private kernel, not voices, scenes, graphs, callbacks or hardware deadlines.

The selected Linux x86_64 cloud run passed six targeted tests in both debug and
optimized builds (the timing test is ignored in ordinary runs and explicitly
ran separately), strict Clippy and the standalone allocation test plus both
counter sensitivity controls. Five paired optimized trials retained these
results in nanoseconds per 128-frame block:

| trial | copy p50 / p99 / max | filter p50 / p99 / max |
| --- | --- | --- |
| 0 | 36 / 40 / 62 | 670 / 1151 / 416481 |
| 1 | 35 / 37 / 46 | 671 / 678 / 681 |
| 2 | 36 / 53 / 78826 | 648 / 655 / 662 |
| 3 | 34 / 36 / 77 | 649 / 656 / 281117 |
| 4 | 34 / 53 / 127 | 671 / 680 / 243894 |

Median trial p50 is 35 ns for copy and 670 ns for filter. Copy checksum is
247.73547649383545 and filter checksum 259.2021469026804 in every trial. Maxima
are retained without trimming; these are private cloud observations, not a
deadline or scalability claim. Exact commands, log hashes, source identity and
full clean local-CI result belong in the canonical draft PR evidence.

## remaining original issue acceptance

The recursive history has **no proven finite instrument tail**. Stable fixed
coefficients and observed decay do not establish a finite retirement bound for
all admitted histories. No arbitrary tail deadline, zeroing threshold or cap
increase is introduced. The existing mutable-instrument finite-tail contract
must be satisfied before production admission.

Whole #2326 still requires chosen supported shapes/slopes/circuits/resonance,
filter-envelope/modulation placement and rate, per-voice versus device ownership,
stable transition/reversal/bypass semantics, complete strict canonical state and
migration, generic UI commands/history/save, authored and actual live-input
lifecycle, compatible transfer and detached tails, complete unchanged-cap
accounting, Arrangement/Session/export equivalence, actual changed callback
allocation probes and optimized workload measurements. Full exact local CI on
this precursor does not cover those missing requirements. Native optimized
interaction/listening and installed-reference numerical evidence remain pending;
the law and source checks make no Live parity claim.
