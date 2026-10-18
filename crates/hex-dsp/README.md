# hex-dsp

## Purpose

Reusable safe DSP mathematics independent of the Hex host. The initial production
kernels are the existing memoryless stereo matrix/frame and validated amplitude
decibel units. Stereo processing/composition arithmetic and identity bit behavior
are preserved. Gate and Saturator consume the same control-side gain conversion.

Matrix coefficients must be finite; composition refuses overflow. Nonidentity
application preserves the prior IEEE sample propagation policy rather than
clamping user audio; identity preserves bits including negative zero/NaN payloads.
There is no runtime storage/history, latency, allocation, reset or sample-rate
assumption in these kernels. Gain construction refuses nonfinite/negative or
overflowing results; finite underflow is silence, and silence has no finite dB
value. Device floors, control ranges and transfer characters stay with devices.

## Dependencies

Rust standard library only. No workspace, external math, FFI, platform, device,
project, I/O or UI dependency. Kernel tests run with `cargo test -p hex-dsp`.
Lattice reexports the stereo types to preserve existing authoring/host APIs.

## Does not own

Device IDs/codecs/registration, project state, musical clocks, voice allocation,
threading, runtime admission, assets, audio I/O, storage, UI, licensing of factory
content or a claim of reference parity. Further kernels are extracted only with
real consumers and independent tests; this is not an empty speculative library.
