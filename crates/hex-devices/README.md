# hex-devices

## Purpose

Prepare and sample Hex's original stateless sine instrument voice for MIDI playback.

## Dependencies

This crate uses only the Rust standard library. The caller supplies the project sample rate and absolute note-relative frame positions.

## Does not own

This crate does not schedule notes, convert musical time, mix tracks, manage MIDI state, access audio devices, or host external plugins.
