# Original Operator carrier filter

Pulse2326 integrates an original Clean digital filter into the registered
`hex.instrument.operator`. The historical [low-pass proof](operator-filter-lowpass-proof.md)
and [packing proof](operator-filter-packing-proof.md) remain private-stage evidence,
not the registered implementation. Admission uses the declared instrument-memory
policy from Pulse2589: both twins, every carrier history and transition are charged
inside the existing combined 2 MiB active / separate 2 MiB tail budgets, with the
256-instance bound. Effects retain their independent smaller per-instance ceiling.

## Finite signal contract

Each voice owns four independent filters. Each filters its raw carrier signal
before that carrier's existing amplitude/velocity envelope and carrier weight.
Cross-modulation and self-feedback continue to use the original shaped oscillator
signals; filters do not enter the oscillator feedback loop. Existing LFO routing
remains pitch-only. There is no filter envelope, cutoff modulation destination,
analog saturation or modeled analog circuit in this implementation.

Supported types are low-pass, high-pass, normalized band-pass, notch and morph.
The slope labels select one biquadratic section (12) or two cascaded sections
(24). LP/HP asymptotes are12/24 dB per octave; BP is6/12 dB per octave on each
side. Notch and morph response follows the selected output mixture. The only supported circuit is Clean. The old view-only
warm/ms2/sallen-key labels have been removed. Morph crossfades LP → normalized BP
→ HP over 0–100%; at 50% it is BP. Resonance maps to
`Q = 1/sqrt(2) + (20 - 1/sqrt(2)) * (resonance/100)^2`.
Authored frequency is 20–20000 Hz; effective cutoff is
`min(authored_frequency, 0.45 * sample_rate)` at 8000–192000 Hz. Preparation
computes `g = tan(pi * cutoff / rate)`, `k = 1/Q`; no callback trigonometry.

The section is a double-precision topology-preserving state-variable filter:
`a=1/(1+g*(g+k))`, `v1=a*(ic1+g*(x-ic2))`, `v2=ic2+g*v1`,
`hp=x-k*v1-v2`, `ic1'=2*v1-ic1`, `ic2'=2*v2-ic2`.
Outputs mix LP=`v2`, BP=`k*v1`, HP=`hp`; notch=LP+HP. For fixed coefficients
the independent transfer oracle is the prewarped analog denominator
`s^2+k*s+1`. For zero input the section energy obeys
`E'-E=-4*g*k*v1^2 <= 0`, including changing positive coefficients. The admitted
parameter range is compact, with strictly positive g/k, and bounded section
outputs feed the second section. Tests stress extreme and rapid parameter motion;
no clamp, signal threshold, timeout or silent nonfinite fallback is used.

There is no buffering or compensatable latency; ordinary filter phase response
remains. The IIR state itself has infinite history. Its output is multiplied by
the existing envelope, whose release has an exact finite endpoint. Voice removal
therefore occurs at the existing declared release endpoint with exact silence,
without claiming an IIR threshold is silence. New/reused notes and reset clear
carrier histories. Bypassed voices retain their histories until reset/reuse;
fully dry sampling takes the original path bit-for-bit and parks filter histories.

## State, edits and ownership

Version11 has 416 bytes: the exact 380-byte v8 prefix followed by nine
little-endian u32 words. Seven words encode enabled, type, slope, circuit,
frequency f32 bits, resonance f32 bits and morph f32 bits. Word7 is reserved zero;
word8 is a strict Boolean continuation flag. Enabling latches that flag so later
bypass remains in the mutable runtime and can finish its dry fade. Editing an
inactive, never-enabled filter does not select mutable processing. Legacy versions
1–8 migrate to disabled/unarmed defaults; malformed inactive values are also
rejected. Unregistered reserved versions9–10 are rejected, not reinterpreted.
Oscillator enable v8 is integrated; parallel harmonic/global/pitch work must
explicitly reconcile formats when integrated. Filter parameter IDs20001–20007 avoid those lanes' reserved IDs.
Undo of the very first enable can restore an unarmed analytic state; that class
exit retains the host’s existing admission policy while voices are active.
Seeking an active mutable voice likewise explicitly refuses rather than clearing
its history; Stop resets it and pause preserves it.
All seven controls use generic descriptors, project commands, undo/redo and the
normal project serialization. No local UI filter values remain.

Compatible changes ramp prepared g/k/output weights, wet mix and cascade mix
linearly over 5 ms (nearest sample), starting at the actual current boundary.
Rapid edits/reversals preserve the in-flight boundary. Existing compatible level,
carrier and feedback edits also ramp from their actual current parameters.
Exact history copies preserve the ramp clock. Unsupported oscillator/envelope/
source/topology changes retain the host's existing explicit admission/refusal
behavior. Filter-only voices retain 64 slots; feedback/LFO voices retain 16,
with 256 events per block. Capacity changes require normal host admission.

First activation can import sounding shared analytic voices only through a new
explicit opt-in contract. The target validates the exact old oscillator law,
rate/table identity, old prepared notes, source topology/content identity, clocks,
ages/gates and bounded occurrence ledger. It copies phases and ages, not replayed
PCM; the boundary sample is dry and subsequent samples fade into zero-state
filters. Existing feedback/LFO activation copies recurrence and clocks, including
an in-progress supported feedback ramp. Changed/ambiguous sources, pending
launches, active analytic enable-coefficient continuations or competing retained
releases do not take this bridge. Every preflight
is read-only; both candidate twins must accept before mutation. Providers that do
not opt in keep their prior behavior. Analytic continuity descriptors are built
on control within the existing track/clip/device/source bounds. No old owner is
created, cloned or finally dropped on the callback.

The filter bank is control-allocated and included in each twin's declared bytes.
Callback preflight copies only fixed voice proof state, not the boxed bank.
History copies overwrite preallocated state; reset does not replace heap owners.
The normal host checkpoint, failed-block rollback, source proofs, ledger, retained
tail and control-side retirement machinery remains responsible for lifecycle.

## Evidence and remaining acceptance

Device tests compare impulse DFTs with an independent complex transfer oracle,
exercise rate/cutoff/resonance/type/slope extremes, malformed codecs, migrations,
rapid edits/bypass, original parameter ramps and finite release/reuse. Engine
checks cover sounding first activation in Arrangement and Session for analytic,
feedback and LFO voices. Allocation probes cover first activation, repeated live
edits, rendering, release and shutdown with zero total allocation/deallocation.
Export checks compare canonical prepared WAV PCM with differently partitioned
registered playback. UI tests exercise shared binding, focus and undo/redo.

The public reference is the [Live Operator manual](https://www.ableton.com/en/live-manual/12/live-instrument-reference/#operator).
These are original laws, not numerical Live parity. Native macOS geometry,
listening, reference behavior and installed-bundle acceptance remain separate;
Linux source checks cannot establish them. See the PR's exact-commit full local
CI report and retained profiler output for measured evidence.


Three x86_64 Linux release runs after integrating oscillator enable: 8 tracks ×
16 voices × four active operators, 128-frame blocks, 100 warm-up / 1000 measured
blocks at 48 kHz. Median elapsed totals were2240.932 ms feedback-only versus
2700.423 ms filtered morph/24-slope/resonance80 (+20.5%). Filtered mean block
cost ranged2671.729–2769.145 us; p99 ranged3700.352–3935.917 us and maximum
5483.642–8398.079 us. Baseline p99 ranged3241.139–5939.040 us, maximum
4945.741–20935.685 us, illustrating shared-host scheduling outliers. Preparation
was0.211–0.226 ms baseline /0.200–0.203 ms filtered. Declared feedback pair8064
bytes, filtered pair12752 bytes including both banks and ramps. These offline
results approach/exceed the2666.667 us block period and do not prove CPAL
physical deadlines or a native Mac baseline. Reproduce with
`cargo run --release -p hex-engine --example profile_operator_filter --locked`.

Oscillator-enable PR164 merged during implementation and is included in this
branch. The combined state retains its full enable suffix, and filter voices
carry enable reversals through the same current-parameter transition.
