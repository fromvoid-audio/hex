# Original four-operator kernel

Pulse2119 is an independently testable prerequisite of Operator19. The module
`hex-devices::fm4` prepares four sine oscillators and a bounded acyclic modulation
graph. It does not yet appear in the native catalog or project playback. The
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

The existing registered analytic note is40bytes with three payload words. Do not
force this328byte configuration into it through raw pointers, heap owners or
unmeasured bank growth. Configuration lifetime across copied release tails,
bounded voice/event identity, transactional processing and feedback chase/replay
need a separate accepted runtime PR. Feedback, fixed tuning, additional waveforms
and partials, filters/LFO/modulation, polyphony/stealing, spread/glide, antialiasing,
native controls and measured reference parity are not supplied here. Below-Nyquist
base oscillators do not imply band-limited FM sidebands.

## Source and performance evidence

Eight FM4 tests compare isolated carrier frequencies/levels/phases, additive
output, two- and four-stage graphs against independently calculated sine formulas.
They cover arbitrary routing order, cycles and malformed numbers, envelope
boundaries/early releases, zero stages, velocity, overflow-safe large ages and
sample-identical block partitions1/64/128/256/513. All28devices tests and strict
device all-target Clippy pass; full local integration checks are recorded in the
PR and Pulse. Native listening remains future work after registry admission.

`cargo run --release -p hex-devices --example profile_fm4 --locked` on Apple M5Pro
arm64/macOS26.6.2:48kHz,100 held voices, four-stage routing,128frames,100warmups,
2000timed blocks. One run totals0.389s; block p50/p95/p99/max are
189.2/240.3/298.1/517.3µs against a nominal2666.7µs period. Preparation and table
creation occur before timing; monotonic wall timing includes per-block rendering
and excludes duration sorting. Background compilation/load is uncontrolled.
This new-kernel baseline excludes engine scheduling, stateful effects, callbacks,
UI and devices; it does not establish100-track DAW performance or Live parity.
