> State v9 now appends harmonic authoring after the unchanged enable prefix.
> The completed harmonic runtime is described in [operator harmonics](operator-harmonics.md).

# Operator oscillator enable controls — Pulse2335

Status: integrated source; final automated and native acceptance pending.
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

## Signal contract

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
Active analytic notes use the approved provider/engine continuation contract in
[the continuation document](operator-enable-continuation.md), with the same
five-millisecond coefficient law. Ordinary built-in analytic polyphony is
preserved. Compatibility and reversal use the actually applied source, including
coalesced publications, rather than an intermediate desired canonical revision.
The harmonic painter's authored-bank runtime remains its separate owner's work.

Feedback stores the rendered post-level signal, not a hidden unmuted recurrence.
Existing inaudible-ancestor pruning can also zero enabled upstream histories
when their last carrier path is disabled. Re-enable must resume continuing
age/phase without claiming the feedback trajectory of an oscillator left audible.
Detached old tails keep their owned law and ordinary bounded retirement; toggles
affect the current owner. Downstream effect tails may remain audible.

## Shared ownership gates

PR156 navigation is integrated at `9bbdad836d24bdcbd6d2348ef78ecde6d322d7b3`.
PR154 LFO dependency is integrated at `a982a62409613e1afef0164509c779690d59481b`,
and PR159 capture at `3f1ef2279696de0acb7dec2941dfdbe5fe53c6e2`. PR158 released
the constructor/context/enable seams while retaining harmonic table/pair ownership.
Strict version7 is 364 bytes; version8 is 380 bytes with IDs513–516, leaving
IDs93–512 reserved for harmonic fields and future version9 append-only data.
Feedback/LFO decode endpoints and the legacy envelope cutoff remain fixed.
Runtime compatibility and retained canonical state are charged to existing caps.

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
