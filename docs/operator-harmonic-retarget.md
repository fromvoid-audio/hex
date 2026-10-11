# harmonic live-retarget contract — Pulse2342 / PR158

Status: required design constraints, not an implemented or verified runtime.
On11October2026 the user required live harmonic retargeting during active
feedback voices and release tails. Waiting for quiescence does not satisfy this
feature. Installed-reference inventory remains a prerequisite for final control
selection; this document defines Hex's independent transition requirements,
not numerical Ableton parity.

## canonical state and prepared ownership

Sequence against frozen LFO checkpoint2152db14025836c329425e363ee2a03871c2fd36:
version6,364bytes, IDs82–92. Reserve harmonic IDs from93; append a separately
versioned validated payload without altering the existing prefix. The maximum
authoring domain must be finite and no greater than64 nonnegative partials per
oscillator. Final amplitude encoding, editable ranges and normalization controls
require the installed study and a measured storage layout before implementation.
Unknown lengths/versions, nonfinite or out-of-domain amplitudes and inactive
malformed settings fail atomically. Migration preserves all prior bytes and
existing waveform selections; existing projects cannot become custom implicitly.

All canonical mutations use generic device patches and session gestures. One
stroke has one Begin, bounded Changes and one Commit or Cancel; interpolation
fills crossed partial indices, with every write inside the chosen domain.
Undo/Redo and Cancel publish canonical states through the same prepared worker
path. View values cannot substitute for saved or applied audio state.

No process-wide per-edit table leak, unaccounted heap buffer, callback Arc
reclamation or proprietary table is permitted. The existing mutable factory
contract requires each runtime to own/copy every sampling value: the host does
not retain the factory for it. Count all owned fixed buffers in both
declared_twin_bytes and fixed_storage_bytes. Retarget proofs stay separately
bounded and owned by the candidate graph until control retirement.

Keep16voices,256events, the8192-byte pair limit and existing aggregate/tail
limits. Both feedback and enabled-LFO paths must pass these bounds. A compact
registered representation may remove redundant prepared fields only after
proving existing waveform PCM, feedback recurrence and LFO clock behavior are
preserved. Do not shrink capacity, alter standalone DSP arithmetic, hash away
exact state comparison or increase limits to make the feature fit. Before source
integration, compile a real storage layout and record both twin/pair/proof sizes;
a paper estimate is not admission evidence. If the representation cannot fit,
return the concrete layout to the coordinator before implementation proceeds.

## sample law and transition

Custom sampling must use original additive synthesis of the stored partials,
with a documented phase origin, coefficient precision, normalization and
interpolation law. Harmonics are admitted strictly below Nyquist at the actual
prepared rate and quantized oscillator step. Preserve the LFO owner's
conservative octave excursion cap. Never renormalize separately per pitch cap
unless explicitly part of the authored law; truncation must not unexpectedly
boost the remaining fundamental. FM/feedback sideband aliasing remains an
explicit limitation of this base-harmonic bound.

Use one device-wide bounded transition clock, default5ms rounded to samples,
with1–4096frames supported by preparation. The transition interpolates harmonic
coefficients in the synthesis domain; any normalization factor is incorporated
into those coefficients before interpolation. The first sample uses the actual
committed source boundary and the endpoint uses the exact prepared target.
Retarget during a transition begins from its actual interpolated coefficients,
not the preceding canonical target. Specify coefficient rounding explicitly and
bound its error; do not silently requantize the source at adoption.

Preserve oscillator phase, envelope age, latched NoteOff, previous shaped
feedback outputs and LFO/device phases. Sample the changing harmonic law before
level/envelope/velocity shaping and feedback-history update, retaining existing
routing order. New notes admitted during the transition share its clock rather
than starting a private ramp. Same-offset event ordering stays unchanged. Base
tuning/envelope/routing/clock edits retain their existing separate compatibility
rules; harmonic adoption cannot accidentally authorize them.

Control prepares a proof bound to the exact target configuration. Callback
preflight validates the actual applied source, target, clock, every carried voice
and finite storage before mutating either twin. A skipped or superseded desired
revision cannot be assumed applied. Failed preparation/admission leaves prior
PCM/history active and desired canonical state visible through existing status.
Scratch rollback must include harmonic trajectory, clock and every history field.

An edit applies to held and releasing voices on the same active device owner.
Detached old-runtime tails retain the law/transition snapshot they owned at
detach; later edits to a replacement owner do not rewrite retired graphs.
Tail duration remains envelope bounded: transitions cannot extend release or
resurrect a quiescent occurrence. Pause freezes clocks; reset clears voices and
transition; Stop/seek/source removal retain established host lifecycle rules.
Escape/gesture cancellation and Undo retarget back through ordinary publication;
they do not rewind performed audio, reset feedback or jump immediately to old
coefficients. Switching panel selection terminates/revalidates gestures using
PR156's existing ownership guard.

## required implementation evidence

Independent harmonic/DFT/PCM oracles must cover fundamental and mixed partials,
zero coefficients, normalization, highest partial, strict Nyquist boundaries,
rates, FM and feedback recurrence. Test malformed/migration state, generic
storage reopen, atomic stroke history and Undo/Redo/Cancel. Live tests need
held/released voices, transition endpoints, mid-transition retarget/cancel,
new notes and exact history copy, block partitions, stale proofs, rollback,
detachment and no tail extension, with LFO disabled and enabled.

Measure total and signed-net callback allocation counts/bytes for harmonic
adoption, chained edits, release and retirement; sensitivity calibration must
remain effective. Measure preparation, handoff and rendering CPU separately,
including16voices/all4oscillators/maximum partials, with exact source and binary
hashes. Existing waveform/LFO regression checks and full local CI must pass on
the final unchanged tree. Native screenshots, painting/Undo/reopen and listening
remain required evidence. Missing/failed/blocked checks prevent merge.

## compiled storage feasibility — 11 October 2026

The design-only example `operator_harmonic_layout` compiles concrete `repr(C)`
types; it does not register or sample a device. Rust1.94.0 on Linux x86_64
measures the following layout with alignment8. Mac arm64 is not measured here.

| partials per oscillator | twin bytes | pair bytes | under8192 |
| --- | ---: | ---: | --- |
|16|3944|7888|yes,304bytes spare per pair|
|32|4712|9424|no,1232bytes over|
|64|6248|12496|no,4304bytes over|

For16partials,3932bytes are fields and12bytes are padding. The full inline
field ledger is calculated by the example. Largest components are16voice
histories2176bytes, source coefficients512bytes, exact v6 prefix364bytes,
target coefficients256bytes, envelope timing caches192bytes, existing-waveform
source weights128bytes, source scalar boundary96bytes and target settings48bytes.
The separate16-byte velocity array avoids padding every voice. Every voice
retains four phase steps, four integrated phases, four f64 feedback outputs,
age, gate, occurrence identity and its own16-byte LFO phase/cycle. The device
has an additional LFO phase/cycle. Exact canonical bytes include inactive
fields and signed zeros; there is no compatibility hash.

No owned heap buffers exist. Copy/no-drop checks cover the entire candidate.
The only references are to the existing process-wide immutable sine/table bank;
their pointer/slice descriptors are included. Existing static table data remains
the separately documented process-wide allocation, not per-edit custom data.
All custom source/target coefficients are inline. The factory/analytic context
would need separate declaration/accounting at integration; this probe does not
measure those unimplemented types or whole-host/stack storage. The24-byte
identity-bound target proof is separate from the pair and contains no source
owner. Events remain borrowed from the host, with the unchanged256-event
ceiling; the device does not allocate a redundant event queue. Event preflight
is not implemented by this example.

Four f64 weights per oscillator retain all four original builtin shapes in an
interrupted transition. A single source waveform choice would be insufficient
for rapid builtin/user/builtin edits. The source partial coefficients stay f64,
including normalized/interpolated values. Target authoring values are f32 with
a control-prepared f64 normalization scale. This candidate therefore does not
re-encode the actual source boundary into a lower-precision authoring format.
It still requires implementation tests for exact boundary/endpoints and all
existing waveform paths; storage capacity alone cannot prove those laws.

The compact candidate removes duplicated velocity/cap data (reconstructible
from unchanged voice velocity, phase steps and canonical settings), duplicate
target scalar arrays and duplicate prepared envelope amplitudes (exact f32
canonical words converted to f64). This requires preserving current arithmetic
order when implemented and measuring the additional callback work. No existing
DSP type or LFO implementation has been changed. The standalone kernel remains
untouched. This is a feasibility candidate, not a proven replacement.

With this representation,64partials cannot fit: just the voice histories,
v6 bytes, f32 target and f64 source coefficients require5612bytes per twin,
before timing caches, clocks and shape/scalar transitions. This is a lower
bound for this representation only, not a proof that all representations fail.
The concrete safe candidate is a16-partial authoring ceiling with unchanged
16voice/256event/memory limits; it is not yet a selected product control set.
If32/64 editing is required initially, a different bounded representation needs
its own arithmetic/ownership proof. Raising the pair ceiling, reducing voices,
requantizing interrupted ramps or hiding buffers behind pointers are not valid
alternatives under the current authorization.

Three example tests cover owned-buffer/copy accounting, capacities/budget and
full coefficient/shape-boundary storage. Targeted tests and strict example
Clippy pass; the initial prototype syntax error was repaired before these runs.
This is not harmonic DSP, PCM, callback allocation or CPU evidence.

## finite native study handoff

When existing desktop interaction owners release safe reference time, the native
coordinator should collect only the following missing observations:

1. Recheck installed Live version/hash; capture the selected oscillator's
   Oscillator tab at known display scale, showing painter/preview/range switches
   and all companion controls, without altering the user's project.
2. On an original disposable fixture, record painting readback/domain, dragging
   across partials, Shift confinement, all/even/odd modes and zeroing a partial.
3. Record16/32/64 switching and hidden-partial retention, builtin-to-user
   transition, copy/paste and whether painting changes actual oscillator output.
4. Record Normalize's displayed defaults/toggle behavior, exact Repeat choices
   and visible enabled/dependent conditions; do not infer their numeric DSP laws.
5. Identify phase/retrigger/feedback/frequency-velocity/quantization controls in
   this tab and their dependencies, separately from existing tuning/envelopes.

The official manual inventory already supports these study questions; no
installed harmonic-editor observations are claimed. Cloud layout/source checks
cannot substitute for this study. Later Hex before/after native screenshots,
painting/Undo/reopen and listening remain separate implementation acceptance.
