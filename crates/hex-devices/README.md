# hex-devices

## Purpose

Implement original native device DSP, including a linear stereo Utility processor,
and prepare Hex's stateless sine instrument voice for MIDI playback.

## Dependencies

This crate depends on the standard library and `hex-lattice` for validated
linear stereo coefficients. The caller supplies the project sample rate and
absolute note-relative frame positions; Utility processes caller-owned samples
without allocation.

## Does not own

This crate does not schedule notes, convert musical time, own the audio graph or
track mixing, manage MIDI state, access audio devices, or host external plugins.
