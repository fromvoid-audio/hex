# hex-devices

## Purpose

Implement original native device DSP, including a linear stereo Utility processor,
the registered eight-band stateful [eq eight](../../docs/eq-eight.md) effect,
prepare Hex's stateless sine instrument voice for MIDI playback, and own original
fixed-state DC/bass-mono filters registered through the stateful lattice contract.
Own the original four-operator phase-modulation kernel and its versioned,
registered analytic instrument definition. FM4 is selectable as a bounded
acyclic native instrument. The standalone original
[oscillator envelope kernel](../../docs/operator-envelopes.md) prepares four
independent finite amplitude laws. A separate
[standalone enveloped FM composition](../../docs/operator-fm.md) shares the
oscillator core; neither changes the registered FM4 envelope or state.
The [Operator control codec](../../docs/operator-state.md) owns a distinct
versioned opaque state and reusable metadata for those four envelopes. The
metadata registers the original acyclic waveform-aware Operator as a shared-context
analytic instrument when effective self-feedback is zero, and selects a bounded
native mutable runtime when it is nonzero. This is an original feedback law, not
full Live Operator behavior.
The [compact Operator kernel](../../docs/operator-context.md) prepares one
immutable DSP configuration and64-byte notes without duplicating envelopes or
the routing graph into each note. Registered playback uses a bounded borrowed
shared context and that compact note representation. The
[fixed tuning contract](../../docs/operator-fixed-tuning.md) adds per-oscillator
pitch-independent tuning through generic versioned state and metadata.
The [standalone and registered feedback contract](../../docs/operator-feedback.md)
defines the per-note recurrence, fixed 16-voice runtime, and current class-change
handoff limits. Larger native polyphony and broader seek/loop/export proof remain
separate work.

## Dependencies

This crate depends on the standard library, `hex-lattice`, and
`hex-lattice-contract` for validated linear stereo coefficients and the native
device codec/metadata contract. The caller supplies the project sample rate and
absolute note-relative frame positions; Utility processes caller-owned samples
without allocation. Standalone stateful filters have validated control-prepared
coefficients and fixed audio-owned histories. Utility version 2 exposes these
through generic descriptors and runtime preparation; version 1 receives neutral
filter defaults. Its prepared filter processors declare zero algorithmic
buffering/lookahead delay; filter phase/group delay is separate. Their output
tail remains conservatively unbounded while the runtime reports exact current
quiescence from enabled filter histories. See the [native effect-tail
contract](../../docs/native-effect-tails.md). The original sine instrument is
registered through the separate Lattice analytic contract; its table
initializes on control and copied note samplers preserve authored age/gate
semantics. See [the filter contract](../../docs/utility-filters.md).

## Does not own

This crate does not schedule notes, convert musical time, own the audio graph or
track mixing, manage MIDI state, access audio devices, or host external plugins.
FM4 uses an original bounded state codec; it does not select Live algorithm
numbers or implement feedback/chase/polyphony. See
[FM4 contract](../../docs/fm4.md).
