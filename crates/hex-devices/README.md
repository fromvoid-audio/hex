# hex-devices

## Purpose

Implement original native device DSP, including a linear stereo Utility processor,
prepare Hex's stateless sine instrument voice for MIDI playback, and own original
fixed-state DC/bass-mono filters registered through the stateful lattice contract.
Own the original four-operator phase-modulation kernel and its versioned,
registered analytic instrument definition. FM4 is selectable as a bounded
acyclic native instrument; mutable feedback and full Operator behavior remain
outside this crate's current runtime contract. The standalone original
[oscillator envelope kernel](../../docs/operator-envelopes.md) prepares four
independent finite amplitude laws; it is not connected to the registered FM4.

## Dependencies

This crate depends on the standard library, `hex-lattice`, and
`hex-lattice-contract` for validated linear stereo coefficients and the native
device codec/metadata contract. The caller supplies the project sample rate and
absolute note-relative frame positions; Utility processes caller-owned samples
without allocation. Standalone stateful filters have validated control-prepared
coefficients and fixed audio-owned histories. Utility version 2 exposes these
through generic descriptors and runtime preparation; version 1 receives neutral
filter defaults. The original sine instrument is registered through the separate
Lattice analytic contract; its table initializes on control and copied note
samplers preserve authored age/gate semantics. See [the filter contract](../../docs/utility-filters.md).

## Does not own

This crate does not schedule notes, convert musical time, own the audio graph or
track mixing, manage MIDI state, access audio devices, or host external plugins.
FM4 uses an original bounded state codec; it does not select Live algorithm
numbers or implement feedback/chase/polyphony. See
[FM4 contract](../../docs/fm4.md).
