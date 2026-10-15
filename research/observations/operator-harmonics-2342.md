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
8064-byte paired payload approaches the8192-byte pair ceiling. The direct human decision requires live harmonic retarget during active voices/
tails. Quiescence-only adoption is not acceptable. The bounded ownership/API
contract remains pending primary selection; design probes are authorized,
production integration is not.

PR156 owns operator_panel.rs navigation/render_body/settings layout,
preview.rs activation/Fine and device_panel.rs context/style. Its owner reports
frozen navigation y86,height16,90px Envelope/Oscillator halves,86px graph and
details below; published frozen SHA9c7cdbd; it is not assumed merged/accepted. The painter should be a new module.
PR154 LFO frozen SHA2152db14025836c329425e363ee2a03871c2fd36 declares
v6/364-byte state, IDs82–92, preserving the prior320-byte prefix. Its independent
LFO processing/clock semantics are not this PR scope. PR159 owns capture; engine/audio runtime
changes require a touched-file/API exchange. Ownership requests were posted to
PR156/154/159; this does not establish owner agreement or completed delivery.

## outstanding required evidence

No feature integration, production harmonic tests, instrumented callback
allocation/CPU results, passing final-source full local CI, native before/after
comparison, painting/Undo/save-reopen or listening evidence exists yet.
Design-only layout/step/PCM probes and VM CPU measurements are documented in
docs/operator-harmonic-retarget.md; they do not close feature acceptance. All remain required; missing or failed
checks block merge. Native interaction is coordinated with the user per AGENTS.md.
No merge, release, paid CI or deployment is authorized for this task.

Cloud activation reports Rust/cargo1.94.0. Supported command-scoped network
permission enabled task fetch without proxy changes. Cross-thread codex queue
failed opening its read-only state database; maintenance2348 has sanitized
environment-version/failure evidence. This is separate from native acceptance.

## user crop: coordinator-witnessed finite inventory

11 October2026: primary coordinator thread01a129d3-59fc-7092-802f-0d4793af80ac
reports independently viewing the user-uploaded Oscillator crop. User said
“and this is what the oscilator tab looks like in ableton live” and “those blue
bars are an adjustable harmonics paint interface”. Cloud has not received the
image bytes. Mac clipboard assets44f40b8e-3cd1-4862-ba84-15c09e9fb45a.png and
3cdd1e9d-3a6c-4e6b-9d54-f69e5dc9c1aa.png are currently local to the Mac.
Exact installed build and image authorship are not established by this crop.

|visible item|reported state|relationship/evidence limit|
|---|---|---|
|harmonic painter|64 vertical blue bars|required authoring view/count, user confirms adjustable|
|range selectors|16/32/64,64 selected|retention/domain behavior unobserved|
|navigation|Envelope left, Oscillator right|PR156 owns stable shell/navigation|
|Wave and overview|User dropdown, waveform preview|preset-to-user interaction unobserved|
|Feedback|0%|existing DSP control; custom dependency unobserved|
|Repeat|Off dropdown|other choices and numerical law unobserved|
|Phase|orange R toggle,0%|existing phase/retrigger; behavior/dependencies unobserved|
|Osc<Vel|0, orange Q toggle|frequency/quantization semantics need study/manual evidence|
|timing row|Attack0.00ms,Decay1.00s,Release400ms,Time<Vel0%|preserve existing envelope scope|
|level row|Initial−∞dB,Peak0dB,Sustain0dB,Vel50%|preserve existing envelope scope|
|loop/key row|Loop None,Key0%|allowed modes/ranges unobserved|
|center background|dark gray, not perfectly black|direct human direction; PR156 owns shell color|

This inventory supports finite visible layout and the64-partial requirement.
It does not prove allowed ranges, Normalize context menu, Repeat modes,
normalization/aliasing laws or numerical/sound parity. Treat phase/feedback/
tuning/envelope as existing independent controls to preserve; any coupling to
custom harmonic editing remains unobserved and must not be invented. Treat
range/Repeat/Normalize/drawing modes as authoring-related hypotheses supported
by the manual, with actual dependency/domain semantics still pending. Native
E05 is blocked pending a human-opened safely bound disposable copy; no competing
control is authorized. Accessible image copies are requested via PR158.

## User clarification — 15 October 2026

The user reports that reducing the range to 16 deactivates partials 17–64 while
retaining their authored amplitudes; expanding to 64 restores their visibility
and sound contribution. This is a direct user requirement/observation, without
a fresh executable identity or measured output. Implement range as an active
partial count, not just viewport zoom, and never erase the hidden values.

The user does not know the numerical amplitude range, Normalize behavior or
Repeat options and authorizes sensible independently defined behavior, informed
by available Operator documentation. These unknown reference laws no longer
require another product-permission question. Use bounded nonnegative linear
amplitudes (0 silent, 1 full authored amplitude), with an explicitly documented
normalization and Repeat law before production integration. Do not claim those
numerical laws were measured in Live. Native appearance/listening checks remain
separate from that authorization.
