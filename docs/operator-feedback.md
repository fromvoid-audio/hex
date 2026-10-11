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

## standalone parameter retarget kernel (Pulse2284 and Pulse2285)

`PreparedOperatorFeedbackRetarget` is a fixed, copyable control-prepared value
borrowed while processing every voice on the runtime, including notes admitted
after the edit. It linearly
interpolates oscillator levels, carrier weights and effective self-feedback in
their authored domains. The default duration is five milliseconds rounded to
the nearest sample (221 frames at 44.1 kHz; 240 at 48 kHz). Callers may select
1–4,096 frames. The first rendered sample uses the exact old values, and after
the final ramp sample the voice contains the exact new feedback values and
continues through ordinary processing without a parameter jump. Phase, age,
NoteOff, waveform, tuning, envelopes, modulation, velocity and oscillator
history remain unchanged.

Preparation refuses unsupported configuration edits and changes into or out of
the mutable-feedback processing class before altering voice state. For the
registered host runtime, unsupported same-device edits carry an explicit
quiescence requirement: a held voice or released feedback tail refuses graph
admission, while an idle committed runtime may be replaced with a fresh law.
Ordinary source replacement keeps its existing detached-tail policy. Retargeting
an active ramp starts from its interpolated value at the caller-provided sample
boundary. `continue_from_shared_ramp` prepares this continuation without a live
voice, so the shared parameter trajectory remains defined even if all voices
expire before its endpoint. The caller advances one shared ramp clock
consistently for all voices. `enroll_in_retarget` enrolls a newly prepared
target voice at that same global boundary without rendering. A host can call it as soon as it applies a
NoteOn, before a later same-offset NoteOff; the voice then begins its envelope
at age zero on the shared ramp. `process_frame_joining_retarget` is an atomic
convenience for enrollment and the first sample. At the endpoint both use the
ordinary target-configuration path. This keeps carried and newly admitted
voices on the same parameter trajectory without replaying their phase,
envelope or gate history.

Each voice records the unique identity of the configuration that prepared it,
so the kernel can pair it with that exact source and ramp target. The identity
is not content-derived. A future host history-copy path that moves an exact
voice and active ramp to separately constructed but equal configurations must
preflight all voices, then use `rebind_target_after_exact_history_copy` and
`rebind_after_exact_history_copy_with_ramp`. At clock zero an unprocessed
source voice retains its source identity, while an already enrolled target
voice is rebound to the replacement target. A voice carried directly from a
preceding ramp retains that ramp's source token and participant bit. After ramp
processing, only the exact old target identity is remapped. Legacy
`process_frame` remains permissive to support its prior history-copy semantics,
and refuses a voice while its ramp is active.

On this 64-bit arm64 build, the standalone configuration is 744 bytes (40 bytes
larger for retarget preparation data and identity), a feedback voice is 160
bytes (8 bytes for its configuration/active-ramp token), and one shared ramp is
216 bytes. Sixteen voices plus one shared ramp occupy 2,776 bytes. The
registered runtime with its integrated ramp and clock measures 4,032 bytes per
twin and 8,064 bytes for the actual pair, under the established 8,192-byte
ceiling. The runtime continues to admit sixteen voices. These are storage
measurements, not callback deadline claims.

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
generic profile does not time `process_frame` with nonzero feedback. That
recurrence was covered by unit oracles until a registered-host profile is
captured under the protocol below; do not infer live performance from those
older samples.

## registered Operator feedback profiler (Pulse2289)

`crates/hex-engine/examples/profile_operator_feedback.rs` is a reusable
release-mode harness for the actual registered Operator through canonical
`PreparedRevision` and `SessionPlayback`. Its fixture sets nonzero effective
self-feedback, admits held MIDI notes, and exercises supported level/feedback
state replacements through the same prepared revision and history-handoff
interfaces used by Arrangement/Session playback. It records distinct candidate
edit/preparation/runtime setup, history handoff and following render timings.
Steady-state rendering has its own setup interval. Retargeting rotates across
tracks and continues to measure held voices while updates are applied.

Default workload is 48 kHz, one track, one held note, 128-frame blocks, 100
warmup blocks, 2,000 measured blocks and both steady/retarget scenarios. The
bounded options select 1 or 100 tracks, 1 or 16 simultaneous notes per track,
128- or 512-frame blocks, scenario, warmup and measured counts. A density check
may be enabled to confirm 17 simultaneous notes are rejected against the current
16-voice limit. Before timing, the harness verifies exact PCM equality for one
512-frame render versus four 128-frame renders and records its checksum.
The partition guard also rejects non-finite or entirely silent output. Its
one-track/one-note fixture verifies silence rejection directly. Measured
warmup-plus-run frames are checked against the fixture's 120-beat (60-second at
the default tempo) note gate, so the reported notes remain held during every
requested block.
Retarget samples alternate the registered state between feedback 0.24 / level
0.82 and feedback 0.63 / level 0.57; both use the default 5 ms ramp.
Each retarget handoff edits one track, rotating across a 100-track workload.
At 512-frame blocks, an edited track's following render contains the first 240
ramp frames and then 272 frames at the target values; other tracks remain on
their current steady state. At 128 frames that following block remains within
the ramp. The retarget checksum covers measured iterations only, excluding
warmup.

For example, after a quiet release build:

```sh
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target cargo build --release -p hex-engine --example profile_operator_feedback --locked
target/release/examples/profile_operator_feedback --tracks=100 --notes=16 --block=128 --warmup-blocks=100 --measured-blocks=2000 --scenario=both --density-check
```

The executable prints setup, handoff and render sample totals and p50/p95/p99/max
values, a PCM checksum, workload/build platform options, and the registered
runtime twin/pair plus target-proof storage declared by the public contracts.
It explicitly reports whole `SessionPlayback` host bytes as unavailable because
there is no public accounting API. Do not infer that total from the device pair
or add a runtime API solely to improve profiler output. Per-render wall-clock
measurements include `Instant` timing overhead. Candidate preparation is timed
separately and is not part of the handoff or render sample.

Capture `rustc -Vv`, the exact source commit and tree digest, release binary
SHA-256, hardware/OS identity, full command, logs and timestamp with each
measurement. Run the profile with no concurrent compilation or unrelated CPU
load where practical; repeat runs and retain each raw log. Three repeated release workload rounds are retained under
`artifacts/profiles/operator-feedback-2289/`; the accepted baseline and its
limitations are recorded in `docs/performance.md`. This offline microbenchmark does not prove CPAL
callback deadlines, device stress, listening quality, or parity/performance
against Ableton Live. See `docs/performance.md` for accepted measurements and
open gates.

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
16 active occurrences, 256 events per block, and a measured 4,032-byte twin on
the current 64-bit arm64 build. The paired runtime uses 8,064 declared bytes,
within the existing 8,192-byte pair ceiling. Before the standalone retarget
kernel's identity and voice state, the baseline was 3,640 bytes per twin and
7,280 bytes paired. It does not claim 64-voice Operator support.
Density admission uses the declared 16-voice capacity, so a denser source is
refused before the runtime is published. There is no stealing, truncation,
callback allocation, lazy table initialization, device construction, locking or
logging. NoteOn preflight verifies finite note setup and release-end arithmetic;
admitted callback events then initialize their bounded pitch/phase data and use
the fixed voice slots. NoteOff occurs at its exclusive event offset before an
equal-offset NoteOn. Exact history copy still requires exact versioned Operator
state, sample rate, feedback amounts and release configuration. The separate
retarget path is explicitly opted into by a control-prepared proof bound to the
exact candidate target configuration. At adoption, the runtime checks the actual
source's immutable law and derives the ramp start from its current shared
boundary; proof validity does not depend on an intermediate canonical revision
having reached audio. The target proof occupies 112 bytes on the current 64-bit
build and is accounted separately from the unchanged 8,064-byte runtime pair,
within the existing `MAX_TRACKS * 4,096` aggregate control-proof budget. It
permits only oscillator-level, carrier and
effective-feedback changes while preserving waveform, tuning, envelopes,
modulation, rate and velocity laws. One shared ramp and clock cover every
carried voice and any NoteOn admitted during the ramp; same-offset NoteOff
follows state-only enrollment at age zero. Rapid edits continue from the actual
interpolated boundary. Exact copy during an active ramp rebinds configuration
identities while preserving future PCM. Unsupported or stale proofs refuse
before changing either twin or the owning track.

The registered-runtime compatibility preflight uses the same read-only per-voice
retarget validator as sample rendering. It checks target identity, note harmonic
caps, source/participant identity, feedback at the current and next ramp
boundaries, elapsed offset and checked voice-age advancement without evaluating
an oscillator sample or mutating voice history. Rendering consumes the validated
fixed frame data and then advances DSP state. This removes DSP work from the
compatibility probe by construction. Three interleaved before/after release
rounds retain identical PCM and lower single-track/16-note handoff medians;
dense timings overlap. See `docs/performance.md` for measured scope and limits.

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
shutdown, with all construction and reclamation outside measurement. Source
tests cover supported Arrangement handoff, later NoteOn during the shared ramp,
block partition invariance, held/released voices, rapid edits, same-offset
NoteOn/NoteOff, exact copy during an active ramp and unsupported waveform
refusal. The local audio allocation probe also exercises live Operator parameter
handoff and graph retirement with zero total and net callback-thread allocation.
Native listening and physical device deadline acceptance remain open
(Pulse2229).


## shared checked sample frame — Pulse2294

The registered runtime computes current and next global ramp parameters once per
sample and lends an immutable crate-private frame to its voices. The frame is bound
to the exact borrowed ramp, target configuration and elapsed offset. Every voice
still checks its own note/configuration, participant identity, current feedback
boundary and checked age increment before mutation. Pitch, velocity, phase, gate,
envelope age and feedback history remain per voice. The public checked per-voice
API creates the same frame itself; its refusal and audio semantics are preserved.
Copied-ramp or wrong-clock frames refuse atomically. No runtime field, schema,
capacity or allocation is added:4032B/twin,8064B/pair,16voices and256events remain.

Two focused regressions compare shared/per-voice processing through distinct notes,
release, mid-ramp NoteOn, chained edits and endpoints, and reject mismatched frames
without changing voice state. Existing independent-recurrence/partition and actual
callback tests remain in place. Preserved old/new release executables additionally
produce identical partition and retarget PCM checksums across all18 interleaved
workload runs; measured scope and limitations are in docs/performance.md. This
reduces ramp work, not the unchanged steady-state or whole-graph handoff costs;
higher-capacity and physical deadline acceptance remain Pulse2226/2108.


### Measured envelope-query inlining

The existing scalar and four-envelope queries carry inline hints so prepared
voice sampling can optimize their small held/release branches at the call site.
Arithmetic, checked preparation, independent envelope stages, release endpoints,
public signatures and registered runtime storage remain unchanged. This is not
a recurrence cache or approximation. Matched source/binary measurements and
exact PCM guards are retained in docs/performance.md (Pulse2300); higher
polyphony, native deadlines and unsupported retarget types remain separate.
