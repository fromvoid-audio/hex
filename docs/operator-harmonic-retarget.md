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

The initial three example tests cover owned-buffer/copy accounting,
capacities/budget and full coefficient/shape-boundary storage. Targeted tests and strict example
Clippy pass; the initial prototype syntax error was repaired before these runs.
This is not harmonic DSP, PCM, callback allocation or CPU evidence.

### full64-partial alternative and explicit contract decision

Further compiled types compare genuinely device-wide law storage with separately
owned mutable primary/scratch histories. This alternative remains design-only:
no implementation uses it and it is incompatible with today's per-twin-owned
buffer declaration. It MUST NOT be admitted by declaring only pointer bytes.

|64-partial layout|fully counted bytes|under8192|
| --- | ---: | --- |
|two self-contained twins|12496|no|
|one shared law, two twins retaining cached steps|8512|no,320bytes over|
|one shared law, two twins reconstructing steps|7520|yes,672bytes spare|

The7520-byte sum is two1728-byte mutable twins, one4040-byte shared law,
16bytes for Arc strong/weak counters and one8-byte retained graph-owner handle.
The separate target proof remains24bytes. Allocation bookkeeping and host graph
metadata are not included, matching current payload-accounting scope. The
shared law includes exact canonical v6 bytes, target f32 coefficients, target
settings/normalization, prepared timing, all f64 source partial/shape/scalar
boundaries, configuration identities and a source-freeze status. Late-fill
source words are actual AtomicU64 fields (f64 bit patterns), not hidden buffers
or unsafe interior mutability. Their complete storage is counted. Both twins
retain their OWN phases, f64 recurrence history, gates, ages, occurrence IDs,
per-note/device LFO phases, velocities/pitches and transition elapsed clock.
Rollback cannot share those mutable fields.

This removes redundant per-note step arrays by storing MIDI pitch and rebuilding
quantized steps with exactly the original frequency/tuning arithmetic order.
The example's fifth test compares all128 pitches at8000/44100/48000/192000Hz,
including fractional/extreme ratios and mixed fixed tuning, to actual registered
prepared payloads; exact u64 steps and Nyquist refusal match. This is bounded
source-equivalence evidence for those fixtures, not CPU evidence, all-state
proof, active-LFO cap acceptance or native validation. Velocity scaling/cap
reconstruction still needs its own integration regressions. Recomputing steps
adds powf/division/rounding work per rendered voice; CPU measurements may reject
the alternative even though it fits memory. No new process-wide pitch table or
unaccounted cache is assumed.

The minimal contract alternative for a primary decision is an explicit
host-owned device-wide law lease with an8192-byte TOTAL budget including both
mutable twins and EVERY unique shared byte/reference-counter/owner handle.
Existing per-twin devices retain their present contract. Such a change needs
generic admission, accounting and retirement APIs; it cannot be introduced as
an Operator-only underdeclaration. It potentially overlaps PR159 host surfaces
and therefore needs its frozen/API handoff before edits. Current traits require
each twin to copy/own everything; this prototype intentionally implements none
of them and does not evade that rule.

Required lease/freeze rules before such integration: control allocates the
candidate's bounded law; one audio owner fills source fields only AFTER full
handoff preflight, then freezes them before either twin renders. Source fields
remain immutable thereafter. Both twins use the same snapshot with independent
clocks/history. Failed adoption must leave the previous law untouched; retry
needs a fresh candidate or full actual-boundary equivalence validation, not
configuration-ID equality alone. No control writer can race audio publication.
Every callback handle replacement must have a graph-held lease so it cannot
perform a final drop; all law/tail reclamation stays on control. Detached tails
keep their frozen snapshot. Atomic fields alone do not prove these rules; the
protocol, failed-block rollback and aggregate active/tail accounting require
tests before use. There is no implemented lease/freeze protocol in this PR.

Do not conclude full64 support is impossible from the self-contained layout,
or accept16 as the requested feature. The measured alternative exposes the
specific ownership/API and CPU decisions needed to continue. No numerical law,
precision, event/voice capacity or8192-byte limit has been silently changed.

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

## concrete sole-pair ownership proposal (not approved or integrated)

Supersedes the Arc/atomic lease hypothesis above. `OwnedPair<64>` in the finite
layout probe owns one `PairLaw<64>` and two inline `PairHistory` fields. All
sampling data is inside the same allocation except existing process-wide
immutable sine/builtin tables. No runtime sibling borrows a pointer to another
sibling, no independently mutable shared law, no reference counts, and no
control-side writer retains access. On this x86_64 ABI: law4040 + histories
2*1720 + transaction8 = **7488 bytes**, alignment8. No separately owned buffers.
The historical7520 candidate remains useful but its Arc lifetime protocol is
unnecessary. Target proof24 is source-independent handoff metadata, not an
owned sampling buffer; existing host ledgers/graph metadata remain separately
admitted. Allocator metadata is excluded exactly as in existing payload rules.
Do not put a history/source/cache in proof or host metadata to evade admission.

Ownership is **device pair**, retained by the engine's existing graph owner;
it is neither a global engine law nor a law shared across devices/tails. Each
cloned instance gets its own complete pair. Graph retirement, not the callback,
frees that allocation. No new final-drop path is permitted. Full active and
retired graph counts remain subject to existing graph/tail bounds.

The following is the proposed lifecycle, not implemented behavior:

1. Control prepares immutable target bytes, harmonic coefficients, normalization,
   envelope timing, routing, tables and rate. It allocates the pair with a
   source-not-staged transaction state. Target validation rejects invalid
   coefficients, rates, routing, configuration versions and capacities before
   handoff. Neither history may render until admission and source staging.
2. Admission validates actual complete pair bytes against8192, before callback
   ownership. Preparation never computes an applied-source boundary from a
   control snapshot. The audio owner's preflight validates BOTH old histories,
   occurrence ledger compatibility, current rate/clock generation, transition
   support and absence of an open speculative block. A configuration ID alone
   cannot validate an applied boundary. Validate every graph participant before
   any active graph commit.
3. After read-only preflight, stage only the INACTIVE candidate: evaluate the
   actual old primary's interpolated f64 partial/builtin/scalar boundary at its
   committed elapsed frame; copy voice phases, recurrence memories, gate/age,
   occurrence IDs, note/device LFO fraction/cycle and velocity/pitch; initialize
   both candidate histories identically with new elapsed zero. This write uses
   exclusive `&mut` candidate ownership. Old pair remains immutable throughout.
   Target fields never change. Source fields become frozen for this adoption.
4. Publish only after all candidates have staged successfully. The graph swap
   is the existing single transaction boundary, not a per-device early commit.
   If staging/host validation aborts, discard or invalidate the inactive
   candidate on control retirement and continue the untouched old primary.
   A retry must re-stage from the NOW-current primary, never reuse a source
   based on an obsolete frame. No old-source pointer is retained. Preflight
   failures must leave old output, clocks, ledgers and candidate target intact.
5. During an open render block the law is read-only. Safe field splitting gives
   the sampler `&PairLaw` and `&mut scratch_history`; primary is not writable.
   Begin copies primary history to scratch (1720 bytes), scratch renders, commit
   changes primary selector, abort preserves primary. Source freeze and target
   are unchanged by either outcome. Event preflight and clock checks remain
   read-only. A law cannot retarget during an open block; existing block
   segmentation/adoption points govern audible time, not a new callback lock.
6. Repeated edits, cancellation and Undo all prepare a new target and stage
   from the last COMMITTED audible boundary. They do not reverse elapsed time,
   reset oscillator phase, restart LFOs or interpolate from the original
   authored source. Interrupted builtin/user/builtin transitions retain all
   four builtin weights and64 f64 partial coefficients. Target normalization
   is prepared before interpolation. Exact scalar/LFO interruption semantics
   must match frozen PR154, including its clock seam and cap rejection.
7. Detached tails retain their complete old pair/law and continue their own
   committed clocks until the existing bounded tail end. Removal never borrows
   the replacement's law. Reset clears both histories and transition clock and
   establishes direct target-as-source under sole ownership; it cannot revive
   stale voices. A history clone/snapshot includes the law, both histories and
   transaction state; restoration is forbidden across mismatched target/rate,
   clock generation, or open-block state. Callback checkpoint is history-only
   because the law is immutable throughout a transaction. Whole-pair snapshots
   are control-allocated, never made via an allocating callback clone.

### public seam and compatibility decision

The current `PreparedMutableInstrumentRuntime::declared_twin_bytes` rule and
`create_runtime` factory explicitly require each twin to own/copy all sampling
data; `NativeInstrumentRuntimePair` constructs two independent boxed trait
objects and admission multiplies the declaration by two. `PreparedNativeInstrument`
only offers this factory for mutable instruments. A7488-byte combined object
cannot truthfully implement that factory. No dummy twin or understated Arc
handle is an acceptable adapter.

Minimal change proposed: an OPTIONAL generic prepared **pair-owned mutable
runtime capability**, default absent on existing instruments. Its declaration
is complete paired payload bytes and it creates ONE pair owner. Host dispatch
selects exactly one mutable capability, rejecting ambiguous dual factories.
The pair interface exposes the existing transactional/preflight/retarget/reset/
quiescence guarantees while encapsulating its two histories. Existing independent
factories retain their existing per-twin accounting, copy rules,8192 admission
and render behavior. Operator implements the new capability; the capability and
admission policy belong to lattice/engine, not an Operator-specific engine
branch. This is an explicit public API extension requiring primary approval
and PR159 freeze/sequence before source integration. Engine callers should
retain their transaction API, but the factory/accounting seam necessarily gains
this new capability; claiming literally unchanged public types would be false.

Alternative: preserve self-contained twins and explicitly increase paired
payload admission from8192 to at least12496 (6248 each), with engine-wide memory
and callback checkpoint review. That preserves the factory ownership contract
and cached per-note steps, but increases every relevant admission/graph budget
and copies6248 bytes at block begin versus1720 for the coupled candidate.
Both alternatives preserve16 voices and256 borrowed events. The paired candidate
requires new generic transaction dispatch/accounting tests, but copies less
checkpoint data and avoids sibling-law aliases. Neither layout measurement
proves synthesis correctness or callback deadlines.

Required contract tests before integration acceptance: oversized/lying factories;
existing-device fallback; ambiguous capabilities; target/source lifetime and
retirement without final callback drop; read-only preflight rejection; partial
multi-device staging failure; abort/retry at a later boundary; begin/process/
commit/abort partition equivalence; source immutability during failed blocks;
repeated edits/Undo with active voices and feedback; detached-tail independence;
reset/removal/clone; rate and clock-generation rejection; full16voice/256event
boundaries; allocation instrumentation; source/target PCM/spectral/aliasing
fixtures and all frozen LFO clock/cap regressions. The current probe proves
layout accounting only, not these lifecycle laws.

### phase reconstruction CPU fixture

`operator_harmonic_cpu` is an original, unregistered additive wavetable fixture,
not a replacement for Operator. It exercises16 voices,4 oscillators, up to64
partials, integrated u64 phases and f64 recurrence feedback, comparing cached
steps against exact original-order reconstruction ONCE per voice per block.
Cost adds16 powf plus64 multiply/divide/round operations per segment. That means
768000 powf/s at48000Hz partition1,6000 at128,3000 at256; both variants perform
up to4096 partial lookups/frame. High pitches/low rates reduce this workload
using a strict unmodulated Nyquist cap. It does not establish active-FM/LFO
aliasing law. No normalization, envelope, event dispatch or LFO processing is
present; production performance still requires those complete workloads.

The test compares every PCM frame and final phase/feedback history for partitions
1/17/64/128/256 against a continuous cached render at four rates. The benchmark
uses seven interleaved warmed trials, reports median/range, and excludes table
construction and preparation. It uses no allocation in the render function by
source inspection; allocation instrumentation and physical/native deadlines
remain required. Measurements are environment-specific, not an acceptance
threshold or proof of hard real-time safety.

Measured 11 Oct2026 in the existing Hex Linux cloud instance: x86_64 KVM,
Intel Xeon Platinum8573C,5 exposed CPUs, Rust/cargo1.94.0, default release
profile. These are portable VM measurements, not Mac/audio callback evidence.
Command: `cargo run -p hex-devices --example operator_harmonic_cpu --release
--locked --offline` after `.hex-tools/activate.sh`. Executable SHA256
`5f91003c69029aaf62cdc7e1d81d498b6c1c5b8f64fc1027d4f878886f756140`.
All durations below are for4096 frames, milliseconds. Trial ranges expose
contention; ratios below1 do not demonstrate a reconstruction optimization.

|rate|partition|cached median [min,max] ms|reconstructed median [min,max] ms|ratio|
|---|---|---|---|---|
|8000|1|19.858 [19.423,20.347]|21.548 [20.999,22.896]|1.0851|
|8000|17|19.744 [19.437,21.067]|19.760 [19.638,21.127]|1.0008|
|8000|64|20.038 [19.589,22.571]|19.647 [19.425,21.207]|0.9805|
|8000|128|19.932 [19.685,21.539]|19.918 [19.565,20.537]|0.9993|
|8000|256|19.737 [19.518,19.903]|20.009 [19.410,20.530]|1.0138|
|48000|1|30.762 [30.234,33.519]|33.221 [31.744,34.020]|1.0799|
|48000|17|30.917 [29.926,31.808]|31.243 [29.909,34.668]|1.0106|
|48000|64|32.735 [30.413,61.803]|35.054 [30.867,50.989]|1.0708|
|48000|128|30.528 [29.947,41.782]|30.196 [29.879,46.350]|0.9891|
|48000|256|45.407 [31.574,61.994]|36.396 [30.510,61.213]|0.8015|
|192000|1|38.698 [30.763,45.669]|40.472 [32.024,46.993]|1.0458|
|192000|17|34.507 [29.922,44.856]|33.111 [30.328,56.173]|0.9596|
|192000|64|55.641 [30.037,60.555]|60.322 [31.799,64.058]|1.0841|
|192000|128|31.024 [29.907,65.947]|37.354 [30.223,59.798]|1.2040|
|192000|256|31.532 [30.317,63.695]|31.881 [30.081,58.852]|1.0111|

The worst measured reconstructed median is60.322ms/4096 at192000Hz partition64,
versus a21.333ms physical duration: this fixture does NOT establish real-time
acceptance at that rate. At48000Hz partition128,30.196ms/4096 is below85.333ms
physical duration, but it omits the rest of the engine and has no deadline
instrumentation. No claim of acceptable physical performance follows.

The separate layout probe's100000 warmed-size copies took1.221ms for1720 bytes
and3.903ms for6248 bytes (12.2/39.0ns per copy); these are repeated hot byte-array
copies, not measured production checkpoint/dispatch latency. Executable SHA256
`78961b3d329e9b8066e82fe037117e8ee1cf1f3fb47cc396bb7a04e72dbce296`.
These measurements support a copy-size trade-off only.

User-supplied image handoff via PR156 owner on11 Oct confirms adjustable harmonic
bars and16/32/64 selectors. The two Mac-local clipboard paths are inaccessible
in this cloud instance; PR158 comment6107630360 requests accessible copies.
Do not claim the other settings observed from those images. Native E05 remains
blocked until the human opens a safely bound disposable fixture. PR160
comment6107606343 requests the tested old-instance native bootstrap from2348;
this is not a new published-environment fault.

## approved direction and current integration sequence

Direct human11Oct authorizes the recommended pair-owned design provided it
preserves high-quality music production, Live parity or improvement; primary
verified authorization. No further approval is needed for this pair ownership
contract. `instrument_pair.rs` now provides an optional single-owner factory,
complete-pair admission and guarded transactions. Existing independent factories
are unchanged. Operator selection/clock/engine dispatch is not connected yet.
Five new lattice contract tests exercise admission-before-creation and lying
payload rejection, failed-event PCM preservation, later rollback and checkpoint,
actual-boundary interrupted retarget/retry without resetting phase/feedback,
wrong proof/generation/open-block refusal, partitions and detached-owner
independence. Their original scalar fixture is a contract test, not production
Operator DSP or feedback/aliasing acceptance.

Main399a68c was merged into this same StartTask branch. Actual main Operator
state is nowv6/320B (envelope149), conflicting with frozen LFOv6/364B. PR154 owns
reconciliation, proposedv7/364B with strictmainv6 migration. PR164 enable owns
next strictboolsuffix, proposedv8/380B(4u32); harmonics thenv9. No shared Operator
writes until owners publish agreed exact versions. Reserve harmonicIDs93–512
(256 coefficients93–348, ranges349–352; remaining IDs merely reserved), enable
IDs513–516. Current descriptor count limit256 does not admit the combined356
parameters; primary has a concrete512control-side limit versus bounded spectrum
parameter alternative to select. No admission limit changed yet.

PR164's parent-approved direction is a separate bounded analytic effective-level
continuation with exact provider/provenance and owned held/release/tail snapshots,
not conversion of every Operator to mutable. It preserves onset/gate and ramps
from actual applied level, not stale canonical level. PR158 does not absorb it.
Freeze its host provider APIs before harmonic bridging touches related handoff
surfaces. No all-Operator mutable conversion is authorized. A harmonic pair must
still preserve existing shapes and import an active analytic source correctly
when first painting introduces custom state; that cross-class seam is not
implemented or proven by the generic pair tests. No future-note-only substitute,
quiescence-only adoption or phase reset is allowed. All shared host edits require
frozen owner patches/sequence, and all prototype CPU limitations above remain.

Direct user image is now attached and visually inspected in this chat: confirms
64bars/64selected,16/32/64selector,Userwave/preview and companion controls as
listed in research inventory. Exact build, ranges/modes and sound laws remain
unproved. Background darkgray belongs PR156; no shell/navigation edits here.

Independent harmonic-kernel proof is now test-only in operator_harmonics.rs;
it is not registered state or callback DSP. Four tests establish fundamental
bit-equivalence with original sine-table samples, finite signed coefficient
validation,64th-harmonic nonzero output/coherent spectral bins, strict Nyquist
boundary/rate caps and convex f64 source interpolation with an L1 peak bound.
Its signed[-1,1] mathematical input and L1 normalization are original proof
choices, NOT selected visible painter ranges or observed Live semantics. The
base/LFO phase cap does not prove FM/PM sidebands alias-free; no such claim is
made. State/UI registration, production preflight and complete modulation
acceptance remain separate work after canonical dependency sequence.

Old-instance native helper was relayed in PR158comments6107705308/6107707776.
Verified exactSHA2566a52c766c30f01b0635305e68cefc54c1875541254b6b7cea96cdaeea42e9f3f
(after removing only the extra Markdown fence newline), Debiantrixie/amd64,
APT/dpkg-deb/Python/archive-keyring prerequisites and no existing conflicting
native/apt directories. Rootless workspace extraction uses signed APT indices
and package hashes, inherited proxy/CA and command-scoped network grant. Saved
old environment version is unchanged; no new-version publication claim.
