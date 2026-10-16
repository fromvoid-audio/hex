# Operator harmonic editor

PR158 / Pulse2342 implements canonical harmonic authoring and live continuation.
The user's 15 October clarification defines hidden partials: selecting 16 or 32
retains all 64 authored amplitudes, while only the visible prefix contributes to
new and already sounding voices. Expanding restores the cached values.

## Editing and state

Each oscillator has amplitudes 0–1, range 16/32/64, Repeat off/2/4/8 and a User
waveform. Dragging interpolates crossed bars; Shift locks the initial partial.
Vertical guides separate the visible harmonic columns. All/odd/even/Normalize
remain implemented but are inactive in the interface at the user's request.
The bar editor supports
Tab focus, Left/Right selection and Up/Down amplitude edits. Escape or focus loss
cancels an unfinished stroke. Normalize scales the visible authored amplitudes
so their maximum is one; zero banks remain unchanged. Hidden values are untouched.
The waveform overview follows the authored spectrum, range and Repeat.

A stroke uses the existing 64-parameter snapshot gesture: preview is transient,
commit adds one history entry, and save/Undo/navigation use existing gesture
barriers. Snapshot patching preserves host input/output gain staging through
metadata migration. The full 86-pixel upper field is reserved for painting,
with stacked 16/32/64 buttons at its right edge. The two stationary Envelope and
Oscillator tabs switch the upper graph while keeping the same lower inspector:
envelope attack/decay/release/Time<Vel, initial/peak/sustain/Vel, and Loop/Key
on the left; wave/preview, feedback/Repeat and phase/R/Osc<Vel/Q on the right. Phase shows percent of one
cycle and feedback uses its normalized percent display. The optional fixed multiplier remains reachable on the third envelope row.
The center display grows to fit the added fields; outer dial/rail sizing is retained. LFO editing remains
accessible from the right summary's separate focusable LFO link. Hex retains its
semantic colors, controls and bundled typography. Rendered bounds guard field,
range, tab and label/value containment; unsupported reference controls are not
added as decorative substitutes. Cached User values, range and Repeat
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
laws. The official Live 12 manual was checked on 16 October for control roles;
no new installed-version transfer-law measurements were made. This implementation
makes no measured numerical Live parity claim.

State v14 is 1,616 bytes. It preserves the complete v12/v13 1,488-byte payload
and appends four 32-byte companion records. v13 extended the builtin waveform
choice identities without changing payload length. Old waveform choices 0..3 and
User 4 remain stable; menu order is a presentation mapping. All v1–8/v11/v12/v13
states migrate explicitly; reserved v9–10 remain rejected. Strict length, finite
amplitude, enum/bool, range and mode checks reject malformed input. IDs 93–348 are
amplitudes, 349–352 range, 353–356 Repeat, 357–388 companion controls. There are 395
registered parameters; existing IDs and host gains are preserved.

### Companion laws and reference limits

The expanded menu includes quantized sine, numbered saw/square families, the
legacy rich saw/square (shown as d), triangle, original looped noise, white noise
and User. Numbered square variants contain the named count of nonzero odd
partials; saw variants contain the named count of consecutive partials. New
periodic tables use the existing strict Nyquist caps. Quantized/noise tables and
samples are independently generated. White noise hashes note age, unmodulated
phase step and operator stream; it is deterministic across partitions, with equal
pitch/onset streams intentionally repeatable. This is not a claim to reproduce
Live's random source. New presets seed a magnitude-only User approximation;
editing cannot preserve unknown builtin phase information. Simply switching
presets preserves an already cached User bank. Previews sample the actual law.

The following are explicit original equations, not measured Live transfer laws.
R defaults on; off seeds phase from the exact host performance frame when known,
or from a runtime-local elapsed origin otherwise. Copy/retarget preserves that
origin; a fresh/reset runtime starts a new local origin. Initial free-running
phase does not reconstruct earlier LFO pitch integration.

Osc<Vel uses `2^(4*amount*velocity/127)` on the authored ratio or Fixed multiplier,
with amount [-1,1]. Q rounds the resulting ratio/multiplier to a positive integer
(minimum 1) when the amount is nonzero. Frequency validation follows modulation;
amount 0 preserves the previous exact path even with Q on. Key uses
`clamp(2^(amount*(pitch-60)/12),1/64,8)` on per-operator gain. Existing Vel retains
its previous response and the separate final note-velocity multiplier.
Time<Vel scales stage durations by `2^(-4*amount*velocity/127)`, retaining exact
zero stages and the existing 20/60/60-second maxima.

Loop repeats attack/decay and a timed return from sustain to initial while held.
Beat retriggers from the current level at note-relative beat intervals. Sync
rounds its first repeat upward to a sixteenth while playing, then follows its
interval; stopped audition uses Beat behavior. Required unknown clock data
refuses processing before mutation. Large jumps use bounded arithmetic, without
replaying missed cycles. Trigger is a finite attack/decay/release interpretation:
NoteOff does not interrupt it, and a completed silent voice retains its identity
until physical NoteOff arrives. Exact reference boundary behavior remains open.

Nondefault companion settings use the existing mutable runtime and inline
per-voice envelope machines. The neutral analytic note payload remains eight
words. Active history transfer across different companion laws is explicitly
refused rather than silently reinterpreted. The standalone immutable note helper
reports unsupported processing for free-running or looping behavior; the native
registered runtime handles those cases. Host prerequisite #2646 / PR261 supplies
explicit clock observations, event-only keyboard admission and a finite 140-second
mutable-tail horizon; other memory, density and replay ceilings remain intact.

## Runtime and ownership

Never-authored builtins with neutral companions retain the existing analytic path. First paint imports
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
ramps, carrier filter integrators and voice histories inline. Each twin is 89,760 bytes on this 64-bit build;
the pair is 179,520 bytes, including inline envelope machines for all 64 voices. Both active and retained runtimes consume the current
mainline's existing 2 MiB instrument budgets. No cap is raised and no proposed
1,142-slot capture pool is installed. Current mainline's variable-size instrument
contract supersedes the historical 8,192-byte proposal in the proof documents.
The analytic provider charge is 2,880 bytes, still below 4,096.
The legacy feedback pair is 8,144 bytes under its unchanged 8,192-byte ceiling.
The expanded immutable builtin table payload is 1,639,200 bytes plus 5,120 bytes of
authoring magnitudes; process-lifetime owners are initialized on control. A bounded
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
The curve canvas explicitly fills its display; a rendered compact-canvas
regression catches the zero-height canvas that previously hid the preview.
Hiding the painter cancels an active stroke while retaining the preview cache
for the Envelope page. Theme style is rebuilt at render time; unchanged input does not recompute the Fourier preview.

## Verification and limits

Deterministic tests cover v1–8/v11 migrations, malformed banks, signed zero, cached
User restoration, strict Nyquist and a nonzero 64th partial, independent sine-sum
agreement across every cutoff from 1 through 64, fallible phase-basis reservation,
overview cache reuse/invalidation, Repeat and normalization bounds, real snapshot/history/save/open,
rendered painter gestures, 64 held/released voices in Arrangement and Session,
feedback/LFO/filter recurrence, enable-clock continuity, range reversals, partition
invariance and refusal preserving old PCM. The preset service round-trip covers
the complete v14/1,616-byte state,
including hidden harmonics, range/Repeat, host gains and one-step Undo/Redo
without changing device identity or neighboring devices. Callback probes cover
first paint, range changes, builtin/User reversal and release with zero total/net
allocations
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


### Historical macOS review candidate measurements (16 October 2026)

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


### Companion candidate validation (16 October 2026)

The companion revision adds the requested field separators, complete Wave menu,
Time<Vel, Vel, Key, Loop, R, Osc<Vel and Q while preserving Hex styling and the
existing range buttons. Five rendered UI regressions cover painting, containment,
menu-to-state identities, Undo and conditional loop controls. Complete preset
state round trips include all eight companion fields. The 64-note callback probe
covers Loop, Sync and Trigger with spectrum edits and release, with zero total
and net allocations/deallocations. A compensated Sync fixture matches dry PCM
after its seven-frame delay across block sizes 1, 127 and 257. Device tests and
strict device Clippy pass. Final full local CI, optimized delivery and native
user confirmation are recorded separately for the immutable candidate. The older
CPU results above do not measure the larger companion runtime; dense high-rate
FM performance remains unresolved.


The integrated default-stack routed Session test exposed excessive temporary
history copies after the inline envelope expansion. Legacy exporters now borrow
the source and return one voice at a time; history import/reset/copy do not
materialize a full temporary bank. Event admission uses a compact identity/age/
completion proof instead of cloning the runtime. Mixed Trigger and ordinary
release completion retains occurrence IDs until NoteOff and expires at the
maximum of one-shot completion and gate-relative release. A focused regression
checks exact expiry/reuse and atomic refusal. The original overflow reproducer,
all six engine harmonic-history tests, 261 device unit plus 26 integration tests
(two existing ignored), and nine Operator/EQ allocation fixtures pass. No test
stack increase, heap-layout change or memory/voice/precision cap reduction was
needed. ARM64 debug frame inspection reduced legacy read from over 256 KiB to
about 19 KiB and preflight to about 4 KiB; control construction remains about
177 KiB. These are compiler-frame observations, not physical audio timing proof.
