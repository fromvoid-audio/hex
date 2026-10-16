# private checked semitone frequency multiplier

Pulse #2327 / canonical PR189 retains its full pitch-envelope/tuning/UI scope.
This isolated control-math precursor is imported only by test harnesses. It
changes no production Operator registration, state/schema, codec, provider,
runtime, engine, UI, parameter domain or admission cap. The earlier private
[signed envelope](signed-envelope-precursor.md) remains unchanged and unconnected.

## existing mathematical relationship

The existing MIDI frequency law in `hex-devices` uses
`440 * 2^((pitch - 69)/12)`. This proof prepares the relative factor
`2^(semitones/12)` and multiplies a positive finite base frequency. It is not
a Fine, cents, transpose, Spread, envelope-amount or Live circuit mapping.
No canonical musical range is chosen: the finite input must produce a positive
representable f64 factor. Overflow and underflow-to-zero are refused rather
than clamped. Representable subnormal factors are not silently flushed.

Preparation owns `powf`. Applying the factor accepts integer sample rates
8,000 through 192,000 Hz and validates the positive finite shifted frequency
strictly below Nyquist. It uses the existing positive phase quantization law,
nearest with halfway rounded upward, scaled by 2^64. Rounded phase must be at
least one and strictly below 2^63, checked before casting. Tiny positive
frequencies with an unrepresentable phase step refuse. No saturated cast,
limiter, silent Nyquist clamp or artificial harmonic-table law is introduced.

Both signed zero semitone inputs produce a neutral factor. When the prepared
factor is exactly one, the valid base's f64 bits remain unchanged. A checked
control-value replacement computes the complete candidate before changing the
destination; refusal preserves it. This is not a voice/history retarget,
phase integration, audio transaction or application ACK API.

The helper validates the final shifted frequency. Mathematically, a base above
Nyquist can shift below it; this does not authorize changing the existing native
fixed/ratio preparation policy or how an envelope should interact with that
policy. Those product contracts remain open.

## source evidence and limits

Independent exact octave ratios cover +/-12, +/-24 and +/-48 semitones and
neutral zero. Known fractional roots, reciprocal products, the exponential
analytic identity and twelve equal semitone steps provide separate oracles.
Actual registered Operator note preparation, with all four oscillator ratios
explicitly set to one, produces the same phase steps for matched MIDI pitches
at 8/11.025/44.1/48/192 kHz. The tests cover nonfinite values, factor and
frequency overflow/underflow, invalid rates/base, Nyquist and neighboring f64
values, half-phase ties, sub-resolution frequencies and atomic refusal.

Selected 64-bit Rust storage is 8 bytes for the prepared multiplier and 16 for
the prepared frequency/step. There are no owned buffers, queues or heap roots.
These measurements are standalone math storage, not full callback admission.

The existing permitted `hex-audio` dev-only allocation harness compiles the same
single private module under `allocation-probe`. Preparation, frequency queries,
copy/replacement and refusal must have zero total count/bytes and zero signed
net count/bytes; existing sensitivity calibration remains required. This is a
standalone control-math probe, not native callback, lock, deadline or publication
evidence. Exact source/commands/full local-CI/log digests are recorded in PR189.

## full original issue remains unmet

No normalized signed-envelope level is interpreted as semitones automatically.
Fine remains fractional-ratio reference terminology with its actual numerical
domain/combination unchosen. Fixed-frequency multiplier behavior remains intact.
Pitch envelope amount/curve/application, transpose/Spread, phase/Nyquist/table
admission for changing pitch, original held/released ownership and clocks,
retarget/chase/loop discontinuities, lossless storage, strict canonical state
and migration, real UI/history/preset/save behavior, Arrangement/Session/export,
changed callback allocation/performance and native/reference acceptance remain
required for whole #2327. No production DSP or whole-issue readiness is claimed.
