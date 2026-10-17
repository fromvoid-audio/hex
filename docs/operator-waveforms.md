# native Operator periodic waveforms

Historical implementation evidence below retains its original state/version and
limits. Current Vector is v15 with globals, controllers, eleven algorithms and
selected polyphony up to 32; see [current globals](vector-global.md),
[state](operator-state.md) and [catalog reconciliation](native-device-catalog.md).

Pulse2183 adds four independently selectable oscillator shapes to native
Operator: `sine`, `saw`, `square` and `triangle`. Choice metadata uses IDs57–60
and the generic native-device parameter contract. State version2 keeps the
original224 bytes as its exact version1 prefix, then stores four little-endian
u32 choice indices. A version1 state migrates to version2 with all four shapes
set to sine. See the [state contract](operator-state.md).

## table and phase law

The existing sine table and its arithmetic path are unchanged. Non-sine shapes
use one immutable process-wide bank built lazily on the control thread when a
non-sine configuration is prepared. The bank contains three shapes, ten
harmonic caps `1, 2, 4, 8, 16, 32, 64, 128, 256, 512`, and2048 phase samples plus
one periodic guard sample per table. The sample data is exactly245,880 bytes;
the24-byte `OnceLock` cell includes its16-byte `Box` descriptor on the
measured64-bit target, for245,904 bytes before allocator bookkeeping. A prepared
configuration stores only a static bank reference, so contexts and notes do not
own or clone table allocations. Allocation failure is reported during control
preparation. The callback never initializes, locks, allocates, frees or drops
the bank.

Tables are generated from these original bipolar Fourier choices, with phase
`theta = 2*pi*x`:

- saw: `(2/pi) * sum((-1)^(n+1) * sin(n*theta) / n)` for `n = 1..cap`;
- square: `(4/pi) * sum(sin(n*theta) / n)` for odd `n <= cap`;
- triangle: `(8/pi^2) * sum((-1)^((n-1)/2) * sin(n*theta) / n^2)` for odd `n <= cap`.

The finite discontinuous-shape sums can overshoot near edges (Gibbs behavior).
Runtime sampling uses linear interpolation in the fixed 2048-point table. These
normalizations and phase origins are Hex's original choices, not measured Live
transfer laws.

Each note stores four quantized wrapping phase increments and one packed u32 of
four byte-sized harmonic-cap indices; sine operators use cap byte zero. For a
non-sine operator, preparation selects the highest table cap whose product with
its quantized phase step is strictly below `2^63`, using checked-width integer
comparisons. A zero phase step is DC and would choose the highest cap; current
validated pitch/rate/ratio limits produce a nonzero step. If quantization lands
at or above half a cycle, no cap is valid and a non-sine note is refused during
preparation. The pre-existing oscillator-frequency validation still runs for
every operator, including inaudible operators. Callback validation checks the
packed cap is in range and is the canonical highest valid cap using only a
bounded constant-time test.

This cap selection constrains harmonics of each base oscillator. It does not
band-limit nonlinear FM sidebands: strong or multistage modulation can still
alias. Oversampling, quality modes, phase distortion and additional oscillator
algorithms remain deferred. The four choices are not a full Operator
reimplementation or a numerical-parity claim.

## evidence and provenance

Tests check the bank's fixed data size and periodic guards, independent DFT
harmonic amplitudes for each shape, strict Nyquist-cap boundaries, invalid
choice/state rejection, signed-zero-preserving version1 migration, generic
choice round trips, malformed packed payload refusal, mixed-shape registered
context output and exact equality between standalone, compact and registered
sampling. The all-sine configuration continues through the previous route and
is compared bit-for-bit with the explicit all-sine selector path. Existing
route, envelope, rate and release tests remain in place. The shared context and
packed note stay under the4096-byte and64-byte contract ceilings.

The implementation was informed by the installed Ableton Live12.4.6 reference
build recorded in [reference identity](research/reference.md) and the public
[Live 12 Operator manual](https://www.ableton.com/en/live-manual/12/live-instrument-reference/#operator).
The reference describes periodic oscillator shapes and harmonic-count variants;
it does not establish the table formulas, cap ladder, interpolation, defaults,
transfer curves or numeric parity implemented here. No Live assets, tables or
audio were copied.

The release profile command is `cargo run --release -p hex-devices --example
profile_operator_waveforms`. It checks exact sine-path PCM and mixed standalone
versus compact PCM before timing100 voices at48kHz in128-frame blocks. It also
reports control-side bank-plus-configuration preparation time. This profile is
headless microbenchmark evidence only; it does not prove callback deadlines,
physical-device performance, 100-track usability or native listening acceptance.
