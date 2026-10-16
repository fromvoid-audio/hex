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
barriers. Snapshot patching preserves host input/output gain staging through
metadata migration. The harmonic tool row stays inside the graph area, above
the shared stationary page tabs; rendered bounds guard against overlap. The top
field is reserved for painting; the waveform overview sits beside the wave
selector below the tabs, with repeat beside feedback. Hex retains its semantic
colors, controls and bundled typography. Cached User values, range and Repeat
survive Undo, Redo and save/open.
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

State v12 is 1,488 bytes: the complete v11 416-byte filter prefix followed by four records
of 64 little-endian f32 amplitudes and three u32 words (range, Repeat, mode).
Modes distinguish never-authored builtin, active User, and cached User/builtin.
All v1–8 and v11 states migrate explicitly; reserved v9–10 remain rejected. Strict length, finite amplitude, range,
Repeat and mode checks reject malformed input. IDs93–348 are amplitudes,
349–352 range, 353–356 Repeat. The registered inventory has 363 parameters;
existing IDs and host gains are preserved.

## Runtime and ownership

Never-authored builtins retain the existing analytic path. First paint imports
actual held/released voices into an independently transactional mutable runtime.
Both candidate twins validate the borrowed source before copying its note ages,
gates, occurrence IDs and phases. Feedback/LFO sources export their actual
recurrence and integrated phase state. Compatible subsequent edits retarget from
the actual applied boundary. Harmonic changes use a 5 ms coefficient ramp;
independent filter and level/enable/feedback ramps retain their original clocks when their
endpoints do not change. Undo to a builtin keeps a continuation-capable compiled
runtime for that instrument identity; this choice is not serialized.

Adoption requires unchanged source identity, timing and processor topology.
Routed MIDI follows the effective source owner while retaining the destination
instrument template. Session occurrence identities stay independent of the
Arrangement loop pass; Arrangement imports honor historical floors and loop cuts.
Unsupported histories refuse publication before changing the previous committed
graph. Historical detached tails retain their original sampler law. Current-owner
Session bridges are imported once and removed from the analytic release bank.
The mutable admission is 64 occurrences without effective feedback/LFO, and 16
with feedback/LFO; preparation refuses source density beyond that class's limit.

The runtime owns all authored spectra, interpolated source coefficients, scalar
ramps, carrier filter integrators and voice histories inline. Each twin is 26,896 bytes on this 64-bit build;
the pair is 53,792 bytes. Both active and retained runtimes consume the current
mainline's existing 2 MiB instrument budgets. No cap is raised and no proposed
1,142-slot capture pool is installed. Current mainline's variable-size instrument
contract supersedes the historical 8,192-byte proposal in the proof documents.
The analytic provider charge becomes 2,584 bytes, still below 4,096. A bounded
256-entry control-side identity vector adds 4,096 bytes plus its 24-byte header
to a full context table. Source descriptors retain immutable source identities,
not copies of note buffers.

The custom oscillator excludes every partial at or above fundamental Nyquist;
FM sidebands are not a complete anti-aliasing solution. Eight independent
Clenshaw residues evaluate the sine series. A process-lifetime immutable
131,073-f64 phase basis (1,048,584 payload bytes) is initialized on control and
borrowed directly during sampling. Reservation is fallible and its result, including
an allocation failure, is cached on control; sampling never initializes the basis.
It contains no authored values and has no callback reclamation. Original builtin tables and builtin endpoints are retained. Filter preparation
propagates invalid settings/rates rather than panicking, and interrupted painter
ownership cancels the gesture through the ordinary control path.

The full 64-partial path uses fixed eight-lane fused recurrences and an analytic
sine-basis reconstruction, while smaller Nyquist caps retain the general path.
The strict cap avoids wide division. Settled spectra are borrowed, inactive LFO
clock arithmetic is skipped, and zero modulation avoids phase conversion. Flat
envelope decay levels avoid an exponential with a zero multiplier. These paths
preserve f64 coefficient precision, voice admission and owned twin bytes.

The waveform overview caches bounded plotted geometry per control, keyed by exact
custom amplitudes, range, Repeat and preview ownership or builtin choice. Parameter
values are borrowed in one scan and shared by the bars and overview. During
rendering, this scan uses the current borrowed device parameters: the generic
instrument renderer temporarily removes its cache entry while composing the
view. Event handling can read that cache after composition. Reading the absent
cache during composition previously left the bars and overview stale even when
a mouse stroke changed canonical state. The rendered regression now adjusts a
painted bar by keyboard after a redraw and covers the restored 16-partial range.
Theme style
is rebuilt at render time; unchanged input does not recompute the Fourier preview.

## Verification and limits

Deterministic tests cover v1–8/v11 migrations, malformed banks, signed zero, cached
User restoration, strict Nyquist and a nonzero 64th partial, independent sine-sum
agreement across every cutoff from 1 through 64, fallible phase-basis reservation,
overview cache reuse/invalidation, Repeat and normalization bounds, real snapshot/history/save/open,
rendered painter gestures, 64 held/released voices in Arrangement and Session,
feedback/LFO/filter recurrence, enable-clock continuity, range reversals, partition
invariance and refusal preserving old PCM. Callback probes cover first paint,
range changes, builtin/User reversal and release with zero total/net allocations
and deallocations. Existing transactional rollback/admission tests remain active.

Run the production CPU fixture with:
`cargo run -p hex-devices --locked --release --example operator_harmonic_runtime`.
Historical pre-optimization evidence: on the shared AMD EPYC 9V74 x86-64 cloud, four oscillators each using all 64
amplitudes, 256-frame blocks and 128 timed blocks produced these milliseconds:

| Rate | Voices | Median | p95 | Maximum | Block deadline |
| --- | ---: | ---: | ---: | ---: | ---: |
| 44,100 | 16 | 1.4576 | 1.6838 | 1.9601 | 5.8050 |
| 44,100 | 64 | 5.7028 | 9.5163 | 22.3692 | 5.8050 |
| 48,000 | 16 | 1.4517 | 1.7957 | 2.0633 | 5.3333 |
| 48,000 | 64 | 5.5368 | 6.1863 | 8.2673 | 5.3333 |
| 96,000 | 16 | 1.4130 | 1.6504 | 2.0517 | 2.6667 |
| 96,000 | 64 | 5.5443 | 6.2868 | 7.0454 | 2.6667 |

Timing covers the registered runtime's sampling, not a substitute kernel. It
excludes preparation, event validation, host mixing and effects. Input construction
and PCM checksum traversal occur outside timing; checksums must be finite and
nonzero. Dense maximum polyphony is **not deadline-safe on this cloud machine**;
these measurements do not establish 100-track scalability or native latency.
Native macOS visual/listening and physical input-to-audio acceptance remain
user-owned, separately from automated merge validation.


### macOS review candidate measurements (16 October 2026)

The optimized production fixture on Apple M5 Pro retains four oscillators,
64 authored partials, 256-frame blocks and the 53,792-byte runtime pair. The
`--varied` fixture uses different coefficients, ratios and phases;
`--fm` adds a feedforward modulation chain. `--cpu-handshake` times 512 blocks
and flushes ready/done markers around rendering, allowing an external `ps` CPU
sample while the process waits on stdin. Centisecond process CPU deltas include
buffer fill, timing and PCM checksum traversal, but exclude preparation/warmup.
They are diagnostic rendering upper bounds, separately from wall deadlines.

| 96 kHz workload | Voices | Process CPU ms/block | Wall median / p95 / max ms |
| --- | ---: | ---: | ---: |
| uniform spectra | 16 | 0.781 | 1.154 / 20.579 / 135.848 |
| uniform spectra | 64 | 2.500 | 3.442 / 9.949 / 42.891 |
| varied spectra | 16 | 0.938 | 1.092 / 3.435 / 10.941 |
| varied spectra | 64 | 2.949 | 3.414 / 5.361 / 12.607 |
| varied spectra, FM | 16 | 1.621 | 2.288 / 16.652 / 65.980 |
| varied spectra, FM | 64 | 4.668 | 5.464 / 7.085 / 11.171 |

The 96 kHz block deadline is 2.667 ms. Concurrent native builds and applications
were active; these wall results do not establish callback deadline safety. Dense
maximum polyphony, particularly FM at high sample rates, remains an explicit
performance limitation and open native acceptance item. Voice/partial counts,
f64 precision and memory budgets were not reduced. A separate two-voice kernel
prototype preserved oracle accuracy but improved median kernel latency only
12.4%, below the selected 25% threshold; it was not integrated into the runtime.
Local ignored artifacts retain benchmark output, executable hashes, the external
CPU harness and prototype evidence. Physical audio, screenshots, painting,
Undo/reopen and listening still require exact-candidate native confirmation.
