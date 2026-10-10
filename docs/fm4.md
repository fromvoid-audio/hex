# Original four-operator kernel

Pulse2119 is the independently testable kernel prerequisite of Operator19. The
module `hex-devices::fm4` prepares four sine oscillators and a bounded acyclic
modulation graph. Its FM4 definition is admitted to the built-in instrument
catalog and canonical project playback through the immutable analytic sampler.
The
[Operator reference inventory](../research/observations/operator-contract-001.md)
retains the complete goal and unmeasured behavior. No proprietary implementation,
waveform, preset or UI asset is copied.

## Units and independent policy

Oscillators have note-relative ratios, linear levels and initial phase in turns.
Preparation admits rates8–192kHz, MIDI pitch0–127 and velocity1–127; oscillator
frequencies must remain below Nyquist. Ratios1/128–128, levels0–8, initial phase
[0,1), modulation depth−8–8 turns and output weights−1–1 are original bounded
domains, not inferred Live ranges or conversion laws. Carrier output is the
weighted sum; no normalization, limiter or hidden gain is applied. Velocity
scales the final amplitude linearly. Maximum absolute output is32.

`modulation[source][target]` applies the source's current sine output and level
as phase modulation of the target. Preparation computes a stable topological
order across all four operators. Cycles and self-feedback are refused, including
cycles disconnected from output. This is a periodic phase-modulation FM kernel,
not a claim about Ableton's discretization or exact eleven algorithm mappings.

One shared output envelope uses original linear attack/decay/sustain/release.
Times are seconds0–120, rounded to nearest frame at preparation; sustain is0–1.
Zero attack/decay skip those stages. Authored gate is the first release frame;
release begins at the held level at that gate, including note-off during attack
or decay. Release ends exclusively at gate+release duration. A zero-length gate
or release has defined silence. Evaluation uses age-minus-gate instead of an
overflow-prone end-frame sum. Per-operator envelopes and reference curves remain
separate work.

## Ownership and integration gate

`PreparedFm4` owns only fixed arrays/scalars and is328bytes on this arm64 build.
Sampling borrows a control-created `SineTable`, uses authored age/gate, and has no
mutable history, allocation, deallocation, I/O, lazy initialization or waiting
by source inspection. This is not a callback allocation instrumentation claim.
Lookup interpolation and the legacy sine multiplication order are unchanged.
Direct age sampling supports this standalone kernel's seeks without replay.

The bounded analytic sampler payload now has41 `u64` words and is344bytes on
64-bit targets. `PreparedFm4::into_analytic_note` packs this validated standalone
note into that owned, copyable representation without raw pointers or heap
owners. Its sampler reads the packed fields by reference, rather than rebuilding
the328byte `PreparedFm4` value on every frame. The call initializes the shared
sine table on the control side; sampling only uses its already-prepared lookup.
`Fm4Definition` exposes this kernel through the generic native-instrument
contract and built-in registry. Its version-1 authored state is exactly
36 little-endian `f32` values (144 bytes): four operator ratio/level/phase
tuples, a source-major 4×4 routing-depth matrix, four carrier weights, then
attack/decay/sustain/release. Stable IDs1–36 follow that field order; the four
self-routing IDs are reserved, remain encoded as zero and are not exposed as
editable parameters. The other32 values have generic descriptors. The state is
validated as one graph before it is returned, so a patch
that creates self-feedback or a cycle is rejected without changing the prior
state. Unknown IDs, wrong types, unsupported versions and non-exact payload
lengths are refused. The initial patch is an original single sine carrier with
ratios1–4, no modulation, and a short ADSR; it is not a Live preset or a claim
about Ableton's algorithms. Projects can select the bounded FM4 instrument and
edit its generic parameters through the existing native panel. The wider
sampler grows `BankEntry` to352bytes and increases each fixed SessionPlayback
release/span scratch by10,019,840bytes relative to the prior40byte sampler; see
[lattice instruments](lattice-instruments.md) for checked ceilings and scope.
Configuration lifetime across copied release tails and bounded voice/event
identity use the source-tested shared analytic continuity runtime. Mutable
feedback chase/replay still needs a separate runtime contract. The distinct
registered Operator now supports [fixed tuning](operator-fixed-tuning.md);
registered FM4 remains ratio-only. FM4 fixed tuning,
additional waveforms and partials,
filters/LFO/modulation, polyphony/stealing, spread/glide, antialiasing, native
controls and measured reference parity are not supplied here. Below-Nyquist
base oscillators do not imply band-limited FM sidebands.

Preparation still validates every authored oscillator and route, even when an
operator cannot reach a carrier. It then marks only nonzero-level carriers and
their nonzero-level ancestors as audible, storing zero in the existing prepared
level slots for all other operators. Sampling skips those slots without
changing the payload size, the order of audible operators, or their floating-
point evaluation. A zero-level intermediate modulator cuts off its upstream
ancestors; signed-zero carrier weights and modulation depths do not make an
operator audible. Tests compare the optimized sampler and packed note
bit-for-bit with the previous full four-operator evaluation across quiet,
normal and full velocities, gates and release ages, including signed-zero and
zero-level-middle cases. Unreachable oscillators still undergo Nyquist
validation.

## Source and performance evidence

FM4 tests compare isolated carrier frequencies/levels/phases, additive
output, two- and four-stage graphs against independently calculated sine formulas.
They cover arbitrary routing order, cycles and malformed numbers, envelope
boundaries/early releases, zero stages, velocity, overflow-safe large ages and
sample-identical block partitions1/64/128/256/513. The packed sampler is compared
sample-exactly with the standalone kernel across its attack, gate and release
boundaries. All40 device tests and strict affected-package all-target Clippy pass. The
admission change adds Arrangement/Session, streamed hybrid, export, save/reopen,
panel and callback-probe coverage; full local integration checks remain with the
integration owner. Native listening remains separate.

`cargo run --release -p hex-devices --example profile_fm4 --locked` on Apple M5Pro
arm64/macOS26.6.2:48kHz,100 held voices, four-stage routing,128frames,100warmups,
2000timed blocks. One run totals0.389s; block p50/p95/p99/max are
189.2/240.3/298.1/517.3µs against a nominal2666.7µs period. Preparation and table
creation occur before timing; monotonic wall timing includes per-block rendering
and excludes duration sorting. Background compilation/load is uncontrolled.
This kernel microbenchmark excludes engine scheduling, stateful effects, callbacks,
UI and devices; it does not establish100-track DAW performance or Live parity.
Registered playback workload and callback-allocation evidence are recorded with
the native admission change. Native listening and physical-device acceptance
remain separate.
