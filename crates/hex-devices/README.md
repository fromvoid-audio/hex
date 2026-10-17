# hex-devices

## Purpose

Own the independently tested, borrowed-PCM [Sampler playback kernel](../../docs/sampler.md)
for pitched stereo regions, loops, reverse, linear ADSR and manual slice selection.
This is a standalone prerequisite; Sampler is not yet in the native catalog.
Host device assets, panel, remaining Simpler functionality and measured reference
agreement remain open under Pulse2679. No sample PCM is embedded in device state.

Implement original native device DSP, including a linear stereo Utility processor,
the stateless nonlinear [Saturator](../../docs/saturator.md),
the stereo-linked [Compressor](../../docs/compressor.md),
the original hysteresis [Gate](../../docs/gate.md),
the free-running stereo [Auto Pan](../../docs/auto-pan.md),
the sparse 64-node stateful [eq](../../docs/eq.md), with lossless explicit
upgrade from the retained [legacy EQ8](../../docs/eq-eight.md) effect,
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
separate work. The independent [EQ band audition kernel](../../docs/eq-eight-audition.md)
provides a transient, bounded selected-band monitor without adding authored
device state or changing the ordinary EQ cascade.

## Dependencies

This crate depends on the standard library, `hex-lattice`, `hex-dsp` and
`hex-lattice-contract` for validated linear stereo coefficients and the native
device codec/metadata contract. The caller supplies the project sample rate and
absolute note-relative frame positions; Utility and Saturator process
caller-owned samples without allocation. Saturator gain coefficients and
validated settings are prepared on the control side; its runtime has no history,
latency, or output tail. Compressor timing and smoothing coefficients are
prepared on the control side; its fixed audio-owned envelope and gain histories
are carried across compatible live parameter revisions. Standalone stateful
Gate thresholds, floor gains and frame durations are prepared on the control
side; its fixed latch, hold and gain-ramp state is carried across compatible
same-rate revisions. Gate processing adds no lookahead or output tail.
Standalone stateful filters have validated control-prepared coefficients and fixed audio-owned
histories. Utility version 2 exposes these
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

The `device_conformance` example and registry-discovered authoring test use the
opt-in lattice test helpers without a desktop, project, GPUI or CPAL. See
[authoring](../../docs/native-device-authoring.md) for JSON reports and limits.

Auto Pan and EQ expose optional state-aware parameter semantics through the
neutral contract. Existing DSP, codecs and envelope evaluation are unchanged;
EQ declarations retain node identity and legacy shelf restrictions. The authoring
report includes a populated dynamic fixture. See
[parameter semantics](../../docs/native-parameter-semantics.md).
