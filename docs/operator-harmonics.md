# Operator harmonic editor

PR158 / Pulse2342 implements canonical harmonic authoring and live continuation.
The user's 15 October clarification defines hidden partials: selecting 16 or 32
retains all 64 authored amplitudes, while only the visible prefix contributes to
new and already sounding voices. Expanding restores the cached values.

## Editing and state

Each oscillator has amplitudes 0–1, range 16/32/64, Repeat off/2/4/8 and a User
waveform. Dragging interpolates crossed bars; Shift locks the initial partial.
All/odd/even filters select which bars a stroke affects. The bar editor supports
Tab focus, Left/Right selection and Up/Down amplitude edits. Escape or focus loss
cancels an unfinished stroke. Normalize scales the visible authored amplitudes
so their maximum is one; zero banks remain unchanged. Hidden values are untouched.
The waveform overview follows the authored spectrum, range and Repeat.

A stroke uses the existing 64-parameter snapshot gesture: preview is transient,
commit adds one history entry, and save/Undo/navigation use existing gesture
barriers. Cached User values, range and Repeat survive Undo, Redo and save/open.
Painting a builtin seeds its magnitudes and selects User. Choosing a builtin
retains the last User bank; choosing User restores it. Painting a builtin again
starts a fresh bank from that builtin. Sine seeds the fundamental, saw 1/n,
square odd 1/n and triangle odd 1/n². These positive sine phases are original;
they do not promise phase-identical builtin reconstruction.

Repeat is an original, explicitly chosen law: with N active partials and C
copies, repeat the first N/C amplitudes C times, scaling copy k by 1/(k+1).
Rendering divides the effective coefficients by max(1, sum of coefficients).
This conservative amplitude bound is separate from the Normalize editing action.
The user authorized sensible amplitude/Normalize/Repeat behavior where reference
measurements were unavailable. Earlier reference notes identify Live's harmonic
editor inventory, but no installed Live measurements establish these numeric
laws. A fresh official-manual request from this cloud was refused by its network
proxy. This implementation makes no measured Live parity claim.

State v9 is 1,452 bytes: the complete v8 380-byte prefix followed by four records
of 64 little-endian f32 amplitudes and three u32 words (range, Repeat, mode).
Modes distinguish never-authored builtin, active User, and cached User/builtin.
All v1–8 states migrate explicitly. Strict length, finite amplitude, range,
Repeat and mode checks reject malformed input. IDs93–348 are amplitudes,
349–352 range, 353–356 Repeat. The registered inventory has 356 parameters;
existing IDs and host gains are preserved.

## Runtime and ownership

Never-authored builtins retain the existing analytic path. First paint imports
actual held/released voices into an independently transactional mutable runtime.
Both candidate twins validate the borrowed source before copying its note ages,
gates, occurrence IDs and phases. Feedback/LFO sources export their actual
recurrence and integrated phase state. Compatible subsequent edits retarget from
the actual applied boundary. Harmonic changes use a 5 ms coefficient ramp;
independent level/enable/feedback ramps retain their original clocks when their
endpoints do not change. Undo to a builtin keeps a continuation-capable compiled
runtime for that instrument identity; this choice is not serialized.

Adoption requires unchanged source identity, timing and processor topology.
Unsupported histories refuse publication before changing the previous committed
graph. Historical detached tails retain their original sampler law. Current-owner
Session bridges are imported once and removed from the analytic release bank.
The mutable admission is 64 occurrences without effective feedback/LFO, and 16
with feedback/LFO; preparation refuses source density beyond that class's limit.

The runtime owns all authored spectra, interpolated source coefficients, scalar
ramps and voice histories inline. Each twin is 18,408 bytes on this 64-bit build;
the pair is 36,816 bytes. Both active and retained runtimes consume the current
mainline's existing 2 MiB instrument budgets. No cap is raised and no proposed
1,142-slot capture pool is installed. Current mainline's variable-size instrument
contract supersedes the historical 8,192-byte proposal in the proof documents.
The analytic provider charge becomes 2,548 bytes, still below 4,096. A bounded
256-entry control-side identity vector adds 4,096 bytes plus its 24-byte header
to a full context table. Source descriptors retain immutable source identities,
not copies of note buffers.

The custom oscillator excludes every partial at or above fundamental Nyquist;
FM sidebands are not a complete anti-aliasing solution. Eight independent
Clenshaw residues evaluate the sine series. A process-lifetime immutable
131,073-f64 phase basis (1,048,584 payload bytes) is initialized on control and
borrowed directly during sampling. It contains no authored values and has no
callback reclamation. Original builtin tables and builtin endpoints are retained.

## Verification and limits

Deterministic tests cover v1–8 migrations, malformed banks, signed zero, cached
User restoration, strict Nyquist and a nonzero 64th partial, independent sine-sum
agreement, Repeat and normalization bounds, real snapshot/history/save/open,
rendered painter gestures, 64 held/released voices in Arrangement and Session,
feedback/LFO recurrence, enable-clock continuity, range reversals, partition
invariance and refusal preserving old PCM. Callback probes cover first paint,
range changes, builtin/User reversal and release with zero total/net allocations
and deallocations. Existing transactional rollback/admission tests remain active.

Run the production CPU fixture with:
`cargo run -p hex-devices --locked --release --example operator_harmonic_runtime`.
On the shared AMD EPYC 9V74 x86-64 cloud, four oscillators each using all 64
amplitudes, 256-frame blocks and 128 timed blocks produced these milliseconds:

| Rate | Voices | Median | p95 | Maximum | Block deadline |
| --- | ---: | ---: | ---: | ---: | ---: |
| 44,100 | 16 | 1.3583 | 1.5516 | 1.9703 | 5.8050 |
| 44,100 | 64 | 5.2988 | 6.5870 | 15.3696 | 5.8050 |
| 48,000 | 16 | 1.3291 | 1.5552 | 1.8769 | 5.3333 |
| 48,000 | 64 | 5.3254 | 6.0898 | 13.7941 | 5.3333 |
| 96,000 | 16 | 1.3390 | 1.4731 | 1.5618 | 2.6667 |
| 96,000 | 64 | 5.3842 | 6.1420 | 14.5705 | 2.6667 |

Timing covers the registered runtime's sampling, not a substitute kernel. It
excludes preparation, event validation, host mixing and effects. Input construction
and PCM checksum traversal occur outside timing; checksums must be finite and
nonzero. Dense maximum polyphony is **not deadline-safe on this cloud machine**;
these measurements do not establish 100-track scalability or native latency.
Native macOS visual/listening and physical input-to-audio acceptance remain
user-owned, separately from automated merge validation.
