# Native effect output tails

## Purpose

This document defines the output-tail contract for prepared Lattice effect
processors and its current Arrangement track-chain retirement behavior. It does
not establish return-bus tail retention, plug-in hosting guarantees, or native
publication readiness.

## Prepared policy

`PreparedNativeProcessor::output_tail()` returns one of:

- `OutputTail::Finite { max_tail_frames }`: after the final nonzero input frame,
  at most this many subsequent zero-input frames may be needed before the
  runtime reports quiescent. The guarantee covers all supported finite input
  histories and every delay, feedback, envelope, filter, or other state that
  can affect future output.
- `OutputTail::Unbounded`: the implementation makes no finite deadline promise.
  A runtime can still report quiescent after it actually drains or resets.

The unit is **sample frames at the prepared processor's sample rate**. This is
not beats, wall-clock time, algorithmic latency, device buffering, or the tail
of another processor. A zero bound means that the runtime must already be
quiescent after the final nonzero input frame. Hosts must validate finite bounds
against their own finite processing budget before admission and use checked
arithmetic for frame positions. A large value such as `u64::MAX` is not
automatically admissible.

Existing processor implementations inherit `Unbounded`. This preserves
conservative behavior for processors that have not implemented a proven bound.
Implementations must not infer a finite bound merely from a few observed quiet
blocks.

## Runtime quiescence

`NativeProcessorRuntime::is_quiescent()` is a read-only query of the runtime's
current prepared configuration. `true` means no subsequent zero-input frame
can produce output, including from a delayed or recursive state that is silent
in the current block. A processor may conservatively keep returning `false`.
The default is `false` for compatibility with older implementations.

The query must be bounded, allocation-free, infallible, lock-free, and must not
mutate the runtime. It may inspect only state owned by that runtime. A host must
not query a speculative history twin to conclude that the committed processor
has drained.

The host must provide actual zero PCM to a processor when advancing its tail.
Skipping a processor, bypassing its state update, or observing silence at its
output is not itself evidence of quiescence. Quiescence is about future output
under the current prepared configuration; a configuration change creates a new
prepared runtime and its own state/history handoff rules.

## Utility filter behavior

Utility's DC removal and bass-mono filters are recursive filters with a tiny
internal history floor. They can eventually become exactly silent for a
particular input history, and `UtilityFilterProcessor::is_quiescent()` checks
the active stages' actual state: the DC input/output histories and, when
bass-mono is active, both stereo Linkwitz–Riley biquad paths. Reset clears those
histories. Hidden filter history for a disabled stage does not produce output
under the current fixed configuration.

Utility currently advertises `OutputTail::Unbounded`. The fact that tests show
individual inputs eventually decay to exact zero is not a proven maximum tail
for every admitted input and parameter combination. It therefore must not be
retired on a guessed frame deadline. The query is useful to a future owner that
can observe the committed runtime and keep processing zero input until it
returns true, or deliberately reset/stop it according to an explicit transport
policy.

## Arrangement track suffixes

SessionPlayback retains a committed, nonquiescent Arrangement track suffix
when a revision detaches its owner because the track/device chain is removed,
reordered, changes instrument position, track mix, suffix matrix, bypass, or
device state. The old suffix continues with zero input using its captured
matrix and track mix. Unchanged chains transfer history once to the candidate
and do not also create an outgoing copy. A mutable source release may still
retain its suffix with the source tail under the existing release path.

Finite declarations are admitted only when their combined suffix bound is at
most ten seconds at the prepared sample rate, with checked frame arithmetic.
Unbounded processors remain eligible but are advanced until their committed
runtime actually reports quiescent. Admission refuses atomically when the
aggregate retained suffix inventory exceeds 4,096 entries or existing runtime,
voice, or tail-group bounds. Stop and Seek reset retained processor state.
Activity is committed only after a complete render block succeeds; failed
speculative blocks do not retire or advance committed histories. The render
path feeds each retained suffix at most once per chunk, including when a source
release and its suffix share that chunk.

Prepared effect instances retain an immutable shared `NativeDeviceState` for
exact configuration identity. The state payload is capped at 4,096 bytes per
device. Project limits allow up to 16,384 Arrangement effect-device states, so
a worst-case callback revision-handoff full-chain identity check can inspect up to 64 MiB of
payload bytes when distinct Arcs require content comparison. Cloned equal Arcs
use a pointer-identity fast path. This comparison occurs at revision handoff, including on the audio callback,
not in the per-sample loop. Its worst-case cost remains unmeasured; the
4,096 retained-suffix cap does not bound this separate handoff comparison work.
The shared configuration payload is not copied into runtime twins or charged
as DSP runtime bytes; it remains owned by immutable prepared state.

## Evidence and remaining scope

The Lattice and Utility tests verify a delayed residual that remains nonzero
after its input ends, no premature quiescence, eventual finite-bound completion
across block partitions, reset, legacy conservative defaults, and Utility's
DC/bass-mono combinations. Engine tests compare detached suffix PCM against an
uninterrupted old-chain oracle across partitions and cover unchanged-chain
history transfer, bypass, and parameter changes. These results do not establish
a general plug-in guarantee, return-bus parity, every failure/density path,
physical-device deadlines, or audible DAW acceptance.
