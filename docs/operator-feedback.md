# standalone Operator feedback kernel

This is an original bounded DSP contract, not a measured recreation of Live's
feedback transfer law. It extends the independently testable standalone
Operator kernel and provides a bounded registered native path. It does not claim
full Operator feature parity, general mutable-instrument chase, or verified Live
numerics.

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

## standalone parameter retarget kernel (Pulse2284)

`PreparedOperatorFeedbackRetarget` is a fixed, copyable control-prepared value
borrowed while processing the voices that existed at the edit. It linearly
interpolates oscillator levels, carrier weights and effective self-feedback in
their authored domains. The default duration is five milliseconds rounded to
the nearest sample (221 frames at 44.1 kHz; 240 at 48 kHz). Callers may select
1–4,096 frames. The first rendered sample uses the exact old values, and after
the final ramp sample the voice contains the exact new feedback values and
continues through ordinary processing without a parameter jump. Phase, age,
NoteOff, waveform, tuning, envelopes, modulation, velocity and oscillator
history remain unchanged.

Preparation refuses unsupported configuration edits and changes into or out of
the mutable-feedback processing class before altering voice state. Retargeting
an active ramp starts from its interpolated value at the caller-provided sample
boundary; the caller must advance one shared ramp clock consistently for all
participating voices. New voices created under the target configuration during
an active ramp are not members of that ramp. Their admission/playback behavior
is intentionally left for the native host integration contract (Pulse2229).
Each voice records the unique identity of the configuration that prepared it,
so the kernel can pair it with that exact source and the ramp's exact target.
The identity is not content-derived. A future host history-copy path that moves
an exact voice to a separately constructed but equal configuration must call
`rebind_after_exact_history_copy` after proving the copied voice history and
configuration are identical. Legacy `process_frame` remains permissive to
support its prior history-copy semantics, and refuses a voice while its ramp is
active.

On this 64-bit arm64 build, the standalone configuration is 744 bytes (40 bytes
larger for retarget preparation data and identity), a feedback voice is 160
bytes (8 bytes for its configuration/active-ramp token), and one shared ramp is
216 bytes. Sixteen voices plus one shared ramp occupy 2,776 bytes. The existing
registered runtime without an integrated ramp declares a measured 3,808-byte
twin and 7,616 bytes for its pair. One shared ramp per twin would add 216 bytes,
for 4,024 bytes per twin and 8,048 bytes per pair under the established 8,192-
byte ceiling. The runtime continues to admit sixteen voices. These are storage
measurements, not callback allocation or deadline claims. The standalone
kernel is not yet wired to live registered parameter edits.

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

## retarget kernel timing sample — 10 October 2026

A single release comparison on Apple M5 Pro, macOS26.6.2 and rustc1.98.1
used48kHz,16 voices,100 warmups and2,000 measured episodes. Each episode
contains256 frames (two128-frame blocks); fresh voices and the shared ramp
were prepared outside timing. The ramp episode uses240 ramp frames followed
by16 ordinary frames. Ordinary processing took0.603s total with episode
median/p95/p99 of259.6/450.7/701.9µs; ramp processing took1.007s with
438.6/723.6/1,044.4µs. Totals include warmups; percentiles exclude them.
The paths ran sequentially, without interleaving or controlling host load.

The exact probe source, output, binary hash, compiler, hardware and base
revision metadata are retained in
`artifacts/profiles/operator-retarget-kernel-2284/`. The probe was built from
the isolated kernel change on base100a623 before this branch's documentation
integration. This sample demonstrates additional ramp work; it does not
establish sustained held-note throughput, native callback deadlines or a
Live performance comparison. Native integration remains Pulse2229.

## registered native runtime

Pulse2225 exposes four authored amounts through Operator state version5. The
exact version4 304-byte prefix is preserved; four little-endian f32 amounts are
appended for a 320-byte state. IDs78–81 use [0,1]. Presets and topology edits
retain authored values. When an oscillator has an incoming cross-modulation
edge, its feedback control is unavailable and its effective amount is zero;
removing the edge reactivates the saved value. If every effective amount is
zero, the existing shared analytic path remains selected.

Any nonzero effective amount selects a fixed-array mutable runtime. It declares
16 active occurrences, 256 events per block, and a measured 3,808-byte twin on
the current 64-bit arm64 build. The paired runtime uses 7,616 declared bytes,
within the existing 8,192-byte pair ceiling. Before the standalone retarget
kernel's identity and voice state, the baseline was 3,640 bytes per twin and
7,280 bytes paired. It does not claim 64-voice Operator support.
Density admission uses the declared 16-voice capacity, so a denser source is
refused before the runtime is published. There is no stealing, truncation,
callback allocation, lazy table initialization, device construction, locking or
logging. NoteOn preflight verifies finite note setup and release-end arithmetic;
admitted callback events then initialize their bounded pitch/phase data and use
the fixed voice slots. NoteOff occurs at its exclusive event offset before an
equal-offset NoteOn. History transfer requires exact versioned Operator state,
sample rate, feedback amounts and release configuration.

An active same-device transition between analytic and mutable classes is
conservatively refused on Arrangement and retained Session MIDI routes. The
already committed desired project edit remains canonical; the currently
applied audio keeps its prior graph and PCM until a later publication can be
admitted. This follows the canonical-versus-applied contract in
[runtime.md](runtime.md) and its Pulse2180 acceptance gate; this device change
does not claim project/history rollback or add a device-specific recovery UI.
The engine tests cover both transition directions on both routes, exact-cut
NoteOn admission once on both routes, quiescence after the maximum 4,800-frame
release, and Arrangement and Session source removal while the detached old
feedback occurrence keeps its authored gate and release without retriggering.

Registered Operator source tests also compare full linear prepared export with
partitioned playback, verify bounded Arrangement seek preroll, and render two
transport-loop wraps with partition-independent feedback attacks. These tests
exercise the explicitly admitted paths; they do not establish native-device
listening, physical callback deadlines, Session-loop behavior, or every
generic pause/stop and seek edge case. The actual registered Operator callback
allocation probe covers onset, held processing, exclusive NoteOff, release and
shutdown, with all construction and reclamation outside measurement.
Parameter edits prepare a candidate configuration; this change does not
establish how active feedback voices respond to changed values. Continuous live
updates to supported Operator parameters remain a separate acceptance item
(Pulse2229).
