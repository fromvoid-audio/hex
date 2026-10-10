# Original enveloped four-operator DSP

Pulse2168 composes the independent envelopes2167 before oscillator routing,
as a finite DSP prerequisite of2161/native Operator19. The standalone
`hex_devices::fm4::PreparedOperatorFm4` validates an original acyclic graph and
four [oscillator envelopes](operator-envelopes.md). It is not registered,
stored in projects, or exposed in the device UI. Existing playable FM4 remains
on its original global ADSR and 41-word analytic payload.

## Signal contract

Oscillator ratios, phase, levels and routing use the same validated original
[FM4 domains](fm4.md). Every oscillator is checked against Nyquist before
inaudible ancestors are pruned. Sample queries use absolute note age/gate in
frames and borrow a control-created sine table.

For each oscillator in the prepared topological order, sum prior shaped
oscillator signals times authored modulation depths, then evaluate the
phase-modulated sine. Multiply that sine by its oscillator level and its own
instantaneous envelope amplitude before using the result as another oscillator's
modulator or as a carrier. Sum weighted carriers and apply note velocity once
after routing. Thus stopping a modulator's envelope removes its phase modulation
while another oscillator may continue its own release.

A private shared oscillator core keeps the evaluation source in one place.
Its compile-time shaping policy leaves the existing FM4 oscillator arithmetic
unchanged, with the original global envelope applied after routing. The packed
analytic callback sampler and payload layout are untouched. Existing FM4 tests
compare exact bits against both the packed sampler and its pre-refactor full
four-operator evaluator.

Prepared objects own only fixed arrays/scalars; no allocation, locks, I/O,
lazy initialization or final shared drops occur in sampling by source
inspection. NoteOff age is supplied by the caller; this pure DSP does not manage
voices or events. Its release-frame bound is the largest of the four envelope
releases, with exact silence afterward. It does not bound feedback, filters or
effect tails. There is no feedback, limiter or antialiasing; carrier magnitude
is bounded by the existing oscillator/carrier domains at32. Below-Nyquist base
frequencies do not imply band-limited FM sidebands.

## Verification and performance limits

Independent continuous-sine and power-law oracles check isolated/additive
carriers, two-stage modulation and a four-stage graph in an arbitrary operator
order, four different curves, zero stages and NoteOff during every envelope
segment. Complete sampled outputs agree within5e-5 absolute amplitude, allowing
for sine-table interpolation. A separate silent-modulator/releasing-carrier
oracle uses2e-6. Partition/reverse-order queries are bit-identical; malformed
rates/pitches/velocities and inaudible out-of-Nyquist oscillators are refused.
All49 device tests and strict all-target Clippy pass.

A private-helper refactor initially regressed the existing standalone FM4
microbenchmark. Three interleaved release samples on Apple M5 Pro/macOS26.6.2
(48kHz,100 held voices,4-stage graph,128frames,100warmups,2000measured blocks)
had median total0.351→0.398s. Inlining and a compile-time per-operator shaping
policy removed that consistent overhead. Fresh paired medians were
0.349→0.352s (about0.9%), with both note sizes remaining328bytes. Other background
load was uncontrolled; no concurrent Cargo ran during either measurement.
These short headless times do not establish sustained deadlines or performance
of the new enveloped path, and the profile has no PCM hash. Exact old-output
proof is supplied by the independent source regressions instead.

Original/initial/repaired binaries, source identities and all logs remain in
`artifacts/profiles/operator-envelope-fm-20261010/`; final comparison is
`repaired-paired-results.json`. Baseline source is
`4e822eb2c88ba924b06f27bc90454df170878b79`; the repaired capture records tracked
diff SHA-256 `dc5e371de8f302a294c705598cf5c718ab62aadddd2f391fffbf0b8771f66ac9`
and the new production module hash. Repaired binary SHA-256:
`839681923b56b57640bfee53e196f5f2935f4ec1c166dd1a45b94d7de474c900`.
Subsequent changes only document this evidence.

Runtime event/voice ownership and rollback, device state/UI registration,
seek/chase2164, loop wraps2165, feedback/filter/LFO and numerical Live parity
remain open. This DSP composition does not complete2161's native integration.
