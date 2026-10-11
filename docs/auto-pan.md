# auto pan

`hex.auto-pan` is an original free-running stereo amplitude modulator registered
through Lattice. Its version-1 opaque state contains amount (`0..1`, default
`0.5`), rate (`0.05..20 hz`, default `1 hz`), and right-channel phase
(`0..360 degrees`, default `180`). It uses a sine LFO only. The left LFO is
`(1 + sin(2π × phase)) / 2`; the right LFO uses the same clock plus its phase
offset. Each channel gain is `1 - amount + amount × LFO`. Phase starts at zero,
is sampled before each clock advance, and wraps once per cycle.

Preparation accepts sample rates from 8 kHz through 192 kHz. It calculates the
five-millisecond live-edit ramp length before callback use. Compatible same-rate
runtime transfer retains phase and every in-flight scalar ramp. An edited amount,
rate, or right-phase target independently ramps from its current value; phase
uses the shortest circular path, with an exact half-turn choosing positive 180
degrees. The ramp advances before calculating that sample's gains, reaches its
target on its final configured frame, and the clock then advances with the
current rate. Reset returns phase to zero and snaps the scalar values to the new
prepared targets.

Bypass returns the input bit for bit while advancing finite-sample phase and
ramps. Amount zero is exactly unity. A frame with a non-finite channel is passed
through unchanged and does not advance history. Runtime state is fixed scalar
storage, callback work has no allocation or lock, algorithmic latency is zero,
and the finite output tail is zero because modulation cannot produce sound from
silence. Runtime quiescence therefore describes output only; it does not discard
clock history needed for future input.

This is a limited original effect based on public control vocabulary, not a
measured recreation of Live's Auto Pan/Tremolo. Beat synchronization, retrigger,
waveform choices, spin, envelope/follower behavior, and modern modes are not
implemented. The generic native parameter view exposes its registered controls;
a compact dedicated panel is tracked separately as Pulse2322. Native appearance
and listening acceptance remain open.
