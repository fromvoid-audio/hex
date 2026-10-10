# eq8 compatible parameter history

Pulse2296 repairs effect-only Arrangement and Return graph handoff for the registered EQ8.
A nonquiescent EQ8 suffix previously became an outgoing effect tail whenever
its authored parameters changed, even though EQ8 can ramp new coefficients while
retaining its filter history. This interrupted the active state at the edit.

The optional native runtime parameter-retarget proof defaults to false. EQ8
certifies matching sample rate, channel mode and enabled-bank topology; changed
frequency, gain, Q or filter shape can retain history under that proof. The chain
checks ordered owner/device/type/runtime-version, bypass, matrix and rate identity
and requires both candidate twins to certify before adopting a changed entry.
Uncertified devices and incompatible changes retain the existing tail path.
The exact unchanged-configuration path remains unchanged.

The Arrangement effect-only classifier applies this proof only when the existing
mix, suffix matrix and instrument-position guards also match. A separately owned
mutable-instrument release tail still owns its old effect suffix; this change does
not copy its ordinary filter history into a second owner. Audition lease transfer
is separate Pulse2253 work and is not implemented by this prerequisite.

Return handoff uses the same compatible chain proof to adopt the old current
chain once instead of also capturing it as a parallel effect tail. Filter history
is valid residual state across source or routing changes; an identical contributor
signature is unnecessary. Existing independently carried Return tails remain
separate owners, and unchanged exact-configuration behavior stays intact.

`cargo run --release -p hex-engine --example profile_eq8_parameter_history -- 128`
measures a registered EQ8 track with held MIDI/sine input and alternating gain/Q
edits at 48 kHz. It separates accepted handoff and following render timing, uses
200 warmup blocks and 2,000 measured blocks by default, and reports deterministic
PCM checksums. Preparation and retired-owner destruction are outside both timing
intervals. Supported blocks are 128/256 frames; this headless fixture is neither
native listening acceptance nor a physical callback deadline test.

Focused Arrangement/Return PCM and prepared callback allocation regressions pass.
The repaired profiler completes six longer rapid-edit runs; the old baseline
refuses with `ReleaseBankFull`, so matched shorter timings and explicit limits
are recorded in `docs/performance.md`. Full final-tree local CI is the merge gate. Live reference response/tail parity remains
unverified; the implementation is independent.
