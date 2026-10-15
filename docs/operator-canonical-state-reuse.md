# Operator canonical input-state ownership (#2342 / PR158)

Preparation continues to decode, validate and prepare the configuration before
running the existing canonical encoder. If the resulting `NativeDeviceState`
has the exact same type, version and bytes as the validated input, preparation
clones the input state handle rather than retaining the encoder's new payload.
State equality compares raw bytes, so signed zero and inactive authored fields
are preserved. Migrated, normalized or otherwise different canonical states
retain the required newly encoded allocation. Unsupported state, lengths,
flags, values and configuration errors retain existing ordering and refusal.

This uses the existing control-side `NativeDeviceState::clone`; there is no
public API, codec/version, provider-frame, callback, engine, kernel or context
interning change. Separate equal-content input owners remain separate. Context
configuration identity is still independently prepared; this is not a change
to shared-context reuse, expired leases, sampling or retirement.

For an exact current v9 input, the retained payload saving is 1,452 bytes plus
two 64-bit Arc counters (1,468 bytes) per otherwise separate canonical payload.
The input payload now has another owner and can stay alive after the caller
drops its state. The temporary encoder allocation still occurs on control and
is dropped before returning the prepared instrument. No reduction in peak
preparation allocations is claimed. Payloads are exact-length Arc slices;
there is no retained Vec capacity hidden by this selection.

The context still conservatively charges the payload even when its Arc is shared.
With the v9 harmonic suffix the complete analytic provider charge is 2,548 bytes;
it remains under the existing 4,096-byte provider limit. The original optimization
continues to preserve payload ownership, signed zero and independent identities.
See [operator harmonics](operator-harmonics.md) for the production runtime,
authored laws, admission and validation limits.
