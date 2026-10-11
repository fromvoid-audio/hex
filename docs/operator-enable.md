# Operator oscillator enable controls — Pulse2335

Status: requirements and source inspection; implementation and acceptance pending.
Focused draft: [PR164](https://github.com/PORTALSURFER/hex/pull/164).

## Required behavior

Each oscillator owns an independently persisted canonical boolean. Clicking its
shell badge toggles that boolean and selects the matching context. Enabled badges
have the oscillator color; disabled badges have a dark fill. A white focus border
is independent of enable state. Display letters remain lowercase through
`theme::text`, without altering saved source identifiers. Other shell controls
continue selecting context without toggling enable.

Use generic validated boolean descriptors and the existing canonical gesture,
history and persistence contracts. Disabling must actually affect prepared audio;
it must preserve authored level, waveform, tuning, routing, envelope and feedback
values so re-enabling restores them. A UI-only flag or destructive level edit
does not satisfy this requirement.

## Proposed signal contract, awaiting ownership and architectural review

Apply the enable mask to oscillator amplitude before both outgoing modulation and
carrier summation. Disabled output is zero; it does not bypass an intermediate
oscillator or connect its incoming modulation directly to its targets. Preserve
the authored graph and its validation, including currently inaudible tuning.
Keep note age, phase, gate and envelope progression while disabled. Do not
retrigger notes or reset transport when toggling. Preserve feedback processing
class by retaining the existing authored/effective feedback classification.

The existing mutable runtime can ramp compiled oscillator level between zero and
the authored level using its original 5 ms shared transition. Rapid toggles must
continue from the actual applied boundary. Its recurrence currently stores shaped
outputs; under a zero level those become zero, rather than freezing an old signal.
Whether this exact contract carries through the forthcoming LFO/harmonic runtime
and active analytic notes is an explicit design gate, not an implemented promise.
Source review confirmed that active analytic notes retain their old sampler on
both Arrangement and Session publication. A level mask alone therefore fails
held-note toggling. Resolve this with a deliberately approved provider/engine
continuation contract or an agreed integrated runtime policy before implementation;
do not silently convert all Operator notes to the current16-voice mutable class.
Analytic smoothing also needs a deliberate contract; the mutable ramp does not
provide it. Compare against the actually applied source, not an intermediate
canonical revision that may never have reached audio.

Feedback stores the rendered post-level signal, not a hidden unmuted recurrence.
Existing inaudible-ancestor pruning can also zero enabled upstream histories
when their last carrier path is disabled. Re-enable must resume continuing
age/phase without claiming the feedback trajectory of an oscillator left audible.
Detached old tails keep their owned law and ordinary bounded retirement; toggles
affect the current owner. Downstream effect tails may remain audible.

## Shared ownership gates

PR156 owns badge/header/navigation geometry until its published handoff. PR154
owns LFO state/runtime/clock additions. PR158 owns the 64-harmonic waveform and
paired-runtime integration. Main currently has envelope-domain state version6,
320 bytes and IDs through81; the earlier frozen LFO checkpoint also used version6
with364 bytes and IDs82–92. Never accept two layouts under one version or assign
enable IDs inside reserved harmonic fields. Agree the integrated prefix, strict
version order, descriptor allocation and source freeze before shared-state edits.
Bound the existing feedback decode slice to its original endpoint before appending
a suffix; its current open-ended loop would exceed the four-entry array. Freeze
the old envelope migration cutoff at version6 and remove descriptor validation
indices relative to the growing descriptor-array tail. Recalculate fixed runtime
compatibility-payload storage against the final integrated8192-byte pair limit.

No native input or compilation begins while another chat owns its lease.

## Acceptance checklist

- Strict boolean decode, wrong type/version/length/value refusal, atomic patch
  failure, migration with all four oscillators enabled and preserved prior fields.
- Independent carrier and modulator PCM oracles, all-disabled exact silence,
  all-enabled legacy output preservation and no rerouting around a disabled stage.
- Held/released notes, disable/re-enable without onset duplication, correct age,
  phase and gate, rapid toggle/Undo cancellation, block partition invariance and
  source removal/tail retirement on Arrangement and Session routes.
- Canonical one-edit gesture, Undo/Redo, save/reopen and retained authored values.
- Four independent rendered badge states, matching context selection and
  non-badge navigation without an unintended edit; preserve invalid-draft refusal.
- Relevant real Operator callback allocation/deallocation probes and declared
  runtime/proof bounds; final full local CI on exact commit and unchanged digest.
- User native pointer/keyboard, active playback/listening, Undo/Redo and reopen
  checklist remains separate evidence. No Live numerical or CPU parity claim.

Reference scope and unknowns: [bounded enable evidence](../research/observations/operator-enable-2335.md).
