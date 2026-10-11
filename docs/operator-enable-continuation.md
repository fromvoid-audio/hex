# Proposed analytic enable continuation — Pulse2335

Status: narrow contract direction approved by the coordinating parent on
11October2026; shared PR154/158 ownership/version allocation remains pending.
No implementation, storage measurement or passing runtime regression is claimed.

## Disjoint source preparation

`crates/hex-lattice/src/analytic_continuation.rs` now contains the proposed
target-bound eight-word frame, checked absolute transition clock and provider
interface. It is deliberately not registered/exported until the shared lattice
registration seam is agreed. Five prepared tests cover independent query order/
partitions, exact endpoints/overflow/refusal, rate rounding, toy-provider actual
boundary reversal versus a detached snapshot and incompatible law/frame/note
refusal. Formatting/whitespace review passes; the module is not yet compiled or
tested. These fixtures do not verify Operator or engine integration.

No existing engine/provider trait/state/DSP/UI file has changed in this checkpoint.
PR159 capture-owner freeze is needed only for its actual overlapping render,
publication, tail-metadata and storage-accounting hunks. Disjoint contract work
does not wait on the entire engine PR. Operator suffix work still needs the
corrected PR154/158 version/descriptor/ownership sequence.

## Why a host contract is required

The current publication code carries active analytic notes with their old
immutable sampler when instrument state changes. Compiling disabled levels to
zero therefore affects future notes but fails held-note acceptance. Routing every
Operator through the existing16-voice mutable runtime changes admission/polyphony
and resource behavior, and is not a justified incidental toggle implementation.
Mixing two complete outputs during a crossfade would interpolate rendered timbre,
not the oscillator coefficients used by the existing mutable transition.

## Preferred bounded provider contract

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

## Proposed owning seams

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
