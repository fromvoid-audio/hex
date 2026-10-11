# Analytic enable continuation — Pulse2335

Status: integrated source in draft PR164, awaiting validation of the final tree.
The coordinating parent approved the narrow provider contract on 11 October 2026;
PR154/158 released the required shared seams and PR156 navigation is integrated.
The harmonic painter's paired runtime and authored table banks remain PR158 work.

## Integrated boundaries

The generic lattice contract provides a target-bound eight-word parameter frame,
checked absolute transition clock and opt-in provider interface. Operator exposes
four effective oscillator levels while preserving authored state. Compatibility
requires equal provider format, sample rate and authored bytes except the four
strict enable words. Unsupported edits retain the existing old-context policy;
invalid admitted frames, notes or clocks produce explicit refusal.

The engine carries explicit current-owner versus detached-tail bindings. Each
current owner has one bounded transition; detached samplers snapshot their origin
transition and retain its target-context lease. Arrangement, Session and capture
share the same sample at the absolute performance boundary. Mutable feedback/LFO
uses the same fixed five-millisecond coefficient ramp without resetting phases,
clock, onset age or gate. The ordinary built-in zero-feedback path remains analytic.

Two fixed-length transition tables are allocated on control using fallible
reservation, require exactly `MAX_TRACKS` capacity, and never resize or exchange
allocation ownership in callback handoff/render. Both capacities are charged to
actual and paired preroll storage under the unchanged 32 MiB cap. The shared
sampler remains inside the existing 344-byte enum footprint. Operator context
accounting includes its retained canonical state payload and Arc bookkeeping.
Post-repair storage and callback instrumentation remain pending.

Canonical badge clicks enqueue Boolean parameter edits with queue-refusal rollback
and select the clicked oscillator. Enabled fill and focus border are independent.
History/save/reopen and rapid pending-click fixtures are added but not yet run.

## Why a host contract is required

The current publication code carries active analytic notes with their old
immutable sampler when instrument state changes. Compiling disabled levels to
zero therefore affects future notes but fails held-note acceptance. Routing every
Operator through the existing16-voice mutable runtime changes admission/polyphony
and resource behavior, and is not a justified incidental toggle implementation.
Mixing two complete outputs during a crossfade would interpolate rendered timbre,
not the oscillator coefficients used by the existing mutable transition.

## Bounded provider contract

Add an opt-in provider-approved analytic continuation capability. Its fixed
opaque scalar state represents effective oscillator levels for Operator; the
engine never interprets Operator IDs or reaches into its state bytes. Provider
compatibility compares the actually applied source context with the prepared
target, permitting only enable-mask differences while requiring equal sample
rate, authored levels, waveform, tuning, phase, envelope, velocity and routing
laws. Validate compact-note compatibility as well. Unsupported providers/edits
retain the existing old-context policy.

One preallocated transition per current device owner stores bounded start
coefficients, target context identity, original5ms duration and absolute
performance-frame start. The provider samples the candidate compact note against
interpolated levels at that boundary, retaining its original onset age and gate.
Use the same level law as mutable feedback voices rather than a final-output
crossfade. New NoteOn during the ramp uses the same shared coefficient boundary.
No additional NoteOn, envelope reset, callback preparation, interior context
mutation, unbounded history, or per-note mutable clock is introduced.

Rapid reversal, coalesced publication and Undo derive their starting scalars from
the actual applied transition. A control-prepared target proof must not presume
that a prior desired revision reached audio. Every source/target/note/provenance,
slot/byte budget and release arithmetic check occurs before candidate ownership
mutation. Refusal leaves both twins and the applied graph intact. Define exact
endpoint behavior and equal-offset NoteOff/NoteOn ordering in tests.

## Current owner and detached tails

Add explicit sampler-binding provenance. Track index, context identity, effect
tail group and Session continuation metadata alone do not establish whether a
note belongs to the currently edited device owner. A compatible enable edit
updates its owned held and releasing voices; previously detached tails keep
their origin law.

If source removal, scene change or an incompatible edit detaches a note during a
ramp, copy the finite transition descriptor and original performance anchor into
its sampler. Continue that transition to its endpoint and ordinary release end
under the retained target-context lease. Copying only the context would jump to
its target level. Subsequent edits of the current owner cannot retarget that tail.
No callback final Arc/context drops; use existing staged context leases and
control-side graph retirement.

The existing shared sampler is much smaller than the preserved344-byte inline
variant. A compact fixed transition may fit its current enum footprint, but
verify sizes and account for all slot/proof/context storage rather than assuming
that headroom. Retained-context slot/byte backpressure remains enforced; no
second old context is needed merely to evaluate one level ramp.

## Owning seams

| File | Responsibility |
| --- | --- |
| `crates/hex-lattice/src/instrument.rs` | Opt-in provider compatibility and bounded continuation scalar/sample contract; default unsupported |
| `crates/hex-engine/src/shared_analytic.rs` | Prepared target proof, exact applied-context validation and lease/budget evidence |
| `crates/hex-engine/src/midi_note_bank.rs` | Compact sampler binding/provenance and provider sampling with fixed transition |
| `crates/hex-engine/src/midi_template/spans.rs` | Preserve onset/gate and snapshot transition on detachment |
| `crates/hex-engine/src/session_playback.rs` | Preallocated current-owner transition slots |
| `crates/hex-engine/src/session_playback/handoff.rs` | Atomic continuation plan for Arrangement and Session, rapid edit and rollback |
| `crates/hex-engine/src/session_playback/render.rs` | Borrow fixed coefficient state at absolute performance boundary; no allocation/locks |
| `crates/hex-devices/src/operator_state.rs` and agreed prepare/context seams | Strict canonical bools/version migration and provider implementation, after PR154/158 handoff |
| `crates/hex-ui/src/editor/device_panel/operator_panel.rs` | Generic canonical badge gesture and independent enabled/focus appearance after frozen PR156 integration |

PR158's paired harmonic runtime remains its owner's design and implementation.
Agree which integrated modes remain analytic and which use paired processing
before implementing either continuation or enable adaptation.

## Finite verification

Use a small original provider fixture for opt-in/default refusal, actual-source
coalescing, atomic refusal and detached-transition ownership. Operator fixtures
cover isolated carrier and modulator, all-off silence, held/released notes,
new notes during a ramp, rapid reversal/Undo and source removal during a ramp.
Run both Arrangement and Session routes across partition sizes; compare the
analytic coefficient ramp to an independent oscillator-level oracle, and compare
mutable feedback against its independently implemented recurrence including
inaudible ancestor pruning. Keep unrelated edits/provider fallback unchanged.
Test exact endpoint, source lease capacity/backpressure and no partial mutation.

Strict migrated-state/malformed suffix, history/save/reopen, rendered four-state
badges and actual callback allocation/deallocation probes accompany those tests.
Measure final integrated owned bytes against existing caps and preserve16-voice/
256-event mutable limits. Full exact-source localCI and user native/listening
acceptance remain required and unrun. This proposal establishes no Live
numerical transition, recurrence or CPU parity.

## Focused source evidence — 11 October 2026

The initial contract's five tests and codec's three tests passed on unchanged
source digest `2c05efcdabf7d450d6f98484969eba6ae4d41c34e53d5b28910a7958acf89ac2`
at checkpoint `69520d89f7f99ac5e4739538d555ca72d3644beb` with paused dirty integration.
Retained evidence: `artifacts/local-ci/operator-enable-focused-20261011/summary.json`.

At clean checkpoint `fb31d4d`, two independent Operator coefficient-oracle tests,
the full LFO test subset, ten engine continuation/ownership/refusal tests and the
shared-context regression subset passed. Evidence is retained under
`artifacts/local-ci/operator-enable-integrated-20261011` and
`artifacts/local-ci/operator-enable-regressions-20261011`.
These custom focused runs record partial validation, not full PR validation.

The capture regression then aborted with a default-test-thread stack overflow.
Moving both fixed transition tables from inline arrays to control-allocated
storage repairs that construction risk in source; no larger test stack was used.
The post-repair engine/capture/preroll run, strict Clippy, UI fixtures, callback
allocation probes and full final exact-commit/digest local CI remain pending.
User native interaction/listening and physical deadline evidence remain separate.
No numerical Live transition or CPU parity is claimed.

## Post-repair focused evidence — 11 October 2026

At clean commit `63f6bb6a8a7e0152f9f070d07c43f4d820eac13b`, source digest
`882ddfdf95ee1e2192a00ff4ef384c8e4c3c72afb06b5b126a70e3af13e17994`,
ten continuation, nine capture and twelve preroll tests passed on the default
thread stack (two existing preroll tests ignored); strict engine/device Clippy
passed. Measured sampler size is 344 bytes, transition slot 96 bytes, and two
256-slot tables consume 49,152 payload bytes per runtime. The fixture's actual
20,683,840 bytes are below admitted 20,685,888 and unchanged 33,554,432-byte cap.
Evidence: `artifacts/local-ci/operator-enable-postrepair-20261011/summary.json`.
The mutable-specific filter selected zero tests; that step is not acceptance.

After a two-line LFO-tab Owner argument repair, clean `3948b38` ran all five
mutable LFO tests and three callback probes successfully. Mutable runtime is
3,920 bytes, twins 7,840 against unchanged 8,192-byte cap, proof eight bytes and
sixteen voices. Analytic, feedback and free/synced LFO probes cover held/release
coefficient reversals and assert zero total allocation plus zero signed-net
count/bytes, with construction/oracles/reclamation outside measured scopes.
Evidence: `artifacts/local-ci/operator-enable-ui-repair-20261011/summary.json`.
UI filters in that run omitted `ui-test-support` and selected zero; they do not
establish UI acceptance.

With the feature enabled and an Envelope-preserving fixture correction, clean
`9ea80fa` ran both canonical badge/history/save/reopen and pending/refusal tests
successfully. Both navigation fixtures failed because PR154 removed local LFO
placeholders and changed the preview enum indices. PR156's owner confirmed this
source evidence against frozen `9bbdad8`; its runtime navigation hunks remain.
The focused semantic-selector fixture repair, accepted inset in the canonical
LFO row and updated descriptor inventory are source-only and await rerun.
Evidence: `artifacts/local-ci/operator-enable-ui-expectation-20261011/summary.json`.
All focused runs are partial; full final exact-source CI and native/listening
acceptance remain open. The Mac build lease was explicitly released.
