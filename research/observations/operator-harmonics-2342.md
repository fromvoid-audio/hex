# Operator harmonic authoring: preflight, 11 October 2026

Pulse2342 / draft PR158. Status: implementation gated; this is not acceptance
evidence or a finalized capability contract.

## primary documentation

The [official Live12 Operator manual](https://www.ableton.com/en/live-manual/12/live-instrument-reference/#operator),
sections31.9.2.2,31.9.2.3 and31.9.10.6, was read11October2026.
Its finite documented inventory includes harmonic-amplitude painting, editing
built-in shapes into user waveforms, waveform overview, harmonic/amplitude
readback, Shift confinement to one partial, 16/32/64 editing ranges, Repeat
extension with rolloff, all/even/odd drawing, Normalize, oscillator copy/paste,
and AMS export/import. Companion controls include phase/retrigger, feedback,
fixed/ratio tuning and frequency-from-velocity with quantization. This inventory
establishes neither numerical laws nor defaults nor installed-build geometry.

## installed reference gate

Existing operator-visual-002 records installed Live12.4.6
(2026-09-10_0de5c8fa9a), executable SHA256
5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949.
That study did not establish harmonic-editor interaction/settings. This Linux
instance has no inspected installed Live or native Mac window. Require a fresh
Mac build/hash check, Oscillator-tab screenshot and original interaction study
before selecting supported extra controls or claiming layout agreement. Record
actual Repeat choices, partial amplitude domain, range-change retention,
Normalize behavior and preset-to-user transition; do not infer their laws.

## source and ownership findings

Exact StartTask source:24a06b87d6b967b955409cbdcaaf767fb68d81da, based on
1e5b5691419c98aefa1ad29cfb4c496486542b5e. Clean task branch was fetched and
checked out. operator_state.rs actually declares version5,320bytes and IDs
through81; operator-native.md says version6 and therefore needs reconciliation
when the owning implementation lands. Generic opaque state carries storage,
history and migration; no Operator-specific storage DTO is needed.

operator_waveform.rs owns a process-wide static table bank, not user-authored
tables. fm4/operator/context.rs owns rate-specific immutable configurations,
compact note phase steps/caps and a configuration identity. Existing sampling
must stay independent and preserve the all-sine path. The analytic context
reports size_of(Self); heap table ownership must not evade declared bounds.
An arbitrary per-edit static bank leak is not an acceptable ownership strategy.

The mutable feedback runtime supports level/carrier/feedback ramps only.
Waveform changes with committed voices/tails are explicitly refused. Its
8064-byte paired payload approaches the8192-byte pair ceiling. Whether custom
harmonic edits must retarget this path or retain explicit admission refusal is
an unresolved coordinator decision; do not silently extend this public contract.

PR156 owns operator_panel.rs navigation/render_body/settings layout,
preview.rs activation/Fine and device_panel.rs context/style. Its owner reports
frozen navigation y86,height16,90px Envelope/Oscillator halves,86px graph and
details below; publication SHA is pending. The painter should be a new module.
PR154 owns LFO and must provide state-version/descriptor allocation and shared
DSP/UI freeze sequencing before edits. PR159 owns capture; engine/audio runtime
changes require a touched-file/API exchange. Ownership requests were posted to
PR156/154/159; this does not establish owner agreement or completed delivery.

## outstanding required evidence

No feature code, harmonic tests, callback allocation/CPU results, final-source
full local CI, native before/after comparison, painting/Undo/save-reopen or
listening evidence exists for this PR yet. All remain required; missing or failed
checks block merge. Native interaction is coordinated with the user per AGENTS.md.
No merge, release, paid CI or deployment is authorized for this task.

Cloud activation reports Rust/cargo1.94.0. Supported command-scoped network
permission enabled task fetch without proxy changes. Cross-thread codex queue
failed opening its read-only state database; maintenance2348 has sanitized
environment-version/failure evidence. This is separate from native acceptance.
