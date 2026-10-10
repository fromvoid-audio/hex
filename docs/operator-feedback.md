# standalone Operator feedback kernel

This is an original bounded DSP contract, not a measured recreation of Live's
feedback transfer law. It extends the independently testable standalone
Operator kernel only. Feedback controls are not serialized or admitted by the
registered native instrument, and no callback, project handoff, seek, Session,
transport-loop or export integration is claimed.

For oscillator `i` at sequential sample `n`, the standalone voice uses:

```text
phase_i[n] = base_phase_i[n]
           + sum_j(cross_depth[j][i] * shaped_j[n])
           + feedback_i * shaped_i[n - 1]
shaped_i[n] = waveform_i(phase_i[n]) * level_i * envelope_i[n] * velocity_scale_i
output[n] = global_velocity * sum_i(carrier_i * shaped_i[n])
```

The initial previous-sample values are zero. Self-feedback is a phase offset in
turns and is bounded to `[0, 1]`. It uses the prior sample's complete shaped
oscillator output, including the existing waveform, level, envelope and
per-operator velocity scale, before the global velocity multiplier and carrier
weight. Existing acyclic cross-modulation still reads current-sample shaped
outputs. If an oscillator has any incoming cross-modulation edge, its own
feedback amount must be zero. Multiple other oscillators may have independent
self-feedback simultaneously.

`PreparedOperatorConfiguration::prepare_feedback_voice` validates feedback and
prepares a note on the control side. The returned compact sequential voice owns
its prepared note, frame age, optional NoteOff frame and four previous shaped
outputs. Each `process_frame` borrows the same immutable configuration and a
prepared sine table. It performs no allocation, table initialization or setup.
The caller must keep using the configuration that prepared the voice. This API
is standalone; source inspection and unit tests do not prove native audio
callback admission or allocation behavior.

NoteOff latches at the next frame to render. The envelope is evaluated at that
same age, so it begins release from its instantaneous attack/decay/sustain
level. A second NoteOff is refused without changing state. There is no feedback
decay independent of the oscillator envelopes. Once every envelope reaches its
exact zero endpoint, the voice clears its feedback history and returns exact
silence. A successful sample advances frame age exactly once; age overflow is
refused atomically so the same state can be retried or inspected.

All-zero feedback uses the existing compact analytic sampler and is compared
bit-for-bit in tests. The feedback path has an independent full-output sine
recurrence oracle covering per-operator velocity and envelopes, plus periodic
shape recurrence checks using the existing validated wavetable bank. Partition
tests process the same state continuously across blocks of1,17,64,127,128 and256
frames. Invalid numeric amounts, forbidden incoming-edge combinations,
exclusive release ends, bounded output, duplicate NoteOff and age overflow are
covered. The periodic-shape test independently implements the frame recurrence
and amplitude order, while intentionally reusing the bank that defines those
existing waveform shapes.

## performance evidence

On an Apple M5 Pro running macOS 26.6.2, the existing release profile was run
three times at 48 kHz with 100 held voices, a four-stage chain, 128-frame blocks,
100 warmup blocks and 2,000 measured blocks. The base was
`ccc62324f089120175840ed5799ca221b02febf6`; the after run used this working tree
on `codex/operator-feedback-kernel`. The three run logs and available metadata
are under the ignored `artifacts/profiles/operator-feedback-{before,after}/`
folders. Both measured the existing direct and
compact memoryless routes, verified exact PCM equality, and reported full note,
shared context and compact note sizes of 624/704/64 bytes. Baseline totals were
0.792/0.840/0.824 s standalone and 0.783/0.731/0.843 s compact (medians 0.824 s
and 0.783 s). After totals were 0.705/0.660/0.655 s standalone and 0.904/0.662/
0.798 s compact (medians 0.660 s and 0.798 s). The after profile binary SHA-256 is
`2cfe557c75af7be885e0959221f0eef3950b97c7a06fc0961258de569154f098`; baseline
binary SHA-256 and source-tree digests were not captured at measurement time.
The metadata records the known source revision/base, local run-log timestamps,
hardware and observed Rust compiler (`1.98.1`); it marks the missing hashes
explicitly. The compact voice measured 152 bytes; configuration remained
704 bytes.

These short runs were taken on a shared, uncontrolled host and do not establish
a speedup, regression, deadline or native callback behavior. The existing
profile does not time `process_frame` with nonzero feedback. That recurrence is
covered by unit oracles only; measure it separately before making performance
claims.

Native admission remains a separate change. It must define prepared occurrence
identity, state/scratch copying and rollback, context pairing, active tail
behavior, pause/stop, seeking, transport loops and export together before any
registered device control can expose feedback.
