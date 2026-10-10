# hex-devices

## Purpose

Implement original native device DSP, including a linear stereo Utility processor,
prepare Hex's stateless sine instrument voice for MIDI playback, and own original
fixed-state DC/bass-mono filter DSP before runtime admission.

## Dependencies

This crate depends on the standard library, `hex-lattice`, and
`hex-lattice-contract` for validated linear stereo coefficients and the native
device codec/metadata contract. The caller supplies the project sample rate and
absolute note-relative frame positions; Utility processes caller-owned samples
without allocation. Standalone stateful filters have validated control-prepared
coefficients and fixed audio-owned histories; they are not yet registered for
desktop use. See [the filter contract](../../docs/utility-filters.md).

## Does not own

This crate does not schedule notes, convert musical time, own the audio graph or
track mixing, manage MIDI state, access audio devices, or host external plugins.
