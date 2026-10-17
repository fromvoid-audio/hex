# shared native DSP kernels

Pulse #2671 / PR288 extracts the first real shared kernel family into `hex-dsp`.
The crate depends only on `std`; a kernel-only consumer builds with
`cargo check -p hex-dsp --locked`, and `cargo tree -p hex-dsp --locked` has no
children. It owns mathematics, not registry/state, host resources or devices.

The existing `StereoFrame`, `StereoMatrix` and `InvalidMatrix` definitions move
without arithmetic changes. Lattice reexports their existing paths; Utility and
all existing host/device consumers retain the same types and processing. Identity
preserves every sample bit; nonidentity matrices preserve the original IEEE input
policy, and composition retains finite checked coefficients and serial order.
Three existing matrix tests move to the owning crate rather than being copied.

New validated `Decibels` and `LinearAmplitude` units distinguish amplitude dB from
power dB. Conversion retains `10^(dB/20)` and `20*log10(amplitude)` arithmetic.
Nonfinite inputs, negative amplitude and overflowing conversion refuse explicitly.
Finite underflow is zero; zero has no finite dB representation, leaving device
floor/display policy with consumers. Gate and Saturator use the common forward
conversion on control. Runtime laws, payloads, type/parameter IDs, histories,
resources, latency, tails and device-specific ranges are unchanged.

This extraction adds no dependency, FFI, allocation, locking or callback work.
Prepared gain validation stays on control; no caller silently clamps bad values.
The architecture guard and crate READMEs register the new dependency boundary.
Further filter, detector, oscillator, ramp/delay/FFT families remain W03 work and
must acquire real production consumers and evidence as they are extracted.

## source and verification

Baseline source: `c79929b396de1f82c16509392d7457e0b84ee60f`, containing the W02
bootstrap with unchanged production DSP. Extracted kernel source:
`900fa581ee459a8d6ae7179081f883851de94ee9`. The later evidence/documentation
commit does not change processing. Exact final PR source and full-CI digest are
recorded with PR288; targeted checks alone are not merge evidence.

Four kernel fixtures cover matrix identity bits, noncommutative composition,
coefficient/composition refusal and gain anchors/round trips/silence/numeric
boundaries. Existing device/lattice unit suites, registry authoring fixtures,
affected strict Clippy and architecture guard pass. Kernel-only compilation is
verified. Full final-source local CI remains required, including existing audio
allocation probes; kernel tests do not replace callback ownership evidence.

## before/after optimized profile

Command: `cargo run --release -p hex-engine --example profile_lattice_effects --locked`.
Hardware: Apple M5 Pro, macOS 26.6.2 (25G83), arm64; rustc 1.94.0
`4a4ef493e`, Cargo 1.94.0. Fixture: 100 tracks, 48 kHz stereo, 128-frame blocks,
512-frame repeated asymmetric source, 100 warmup blocks and 2,000 measured blocks.
Each case first compares every rendered sample with the standalone ordered oracle;
the timed render must reproduce the same digest. Timing uses `Instant` per block.

Raw [before](../research/observations/native-dsp-2671/before.txt) and
[after](../research/observations/native-dsp-2671/after.txt) retain all cases,
percentiles/maxima, above-period counts, error and PCM digests. All five admitted
case digests match exactly before/after; oracle maximum error is zero. The
300-processor chain remains explicitly refused by the existing admission budget.

| case / processors | before p50 µs | after p50 µs | before p99 µs | after p99 µs |
| --- | ---: | ---: | ---: | ---: |
| baseline / 0 | 23.750 | 20.875 | 774.750 | 109.708 |
| saturator / 100 | 93.833 | 72.166 | 9904.041 | 194.125 |
| compressor / 100 | 242.333 | 170.542 | 19058.792 | 515.709 |
| gate / 100 | 82.167 | 67.167 | 7018.542 | 226.209 |
| gate + compressor / 200 | 273.250 | 197.625 | 9179.458 | 560.208 |

Other compilation/test load was uncontrolled, especially during the baseline.
The large timing variation is not evidence of an extraction speedup or physical
callback safety. These runs establish reproducible PCM and no observed median
regression in this fixture. Native device deadlines, broader density, reference
laws and listening remain separate W20 acceptance work.
