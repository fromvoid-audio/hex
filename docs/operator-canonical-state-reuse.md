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

For an exact current v8 input, the retained payload saving is 380 bytes plus
two 64-bit Arc counters (396 bytes) per otherwise separate canonical payload.
The input payload now has another owner and can stay alive after the caller
drops its state. The temporary encoder allocation still occurs on control and
is dropped before returning the prepared instrument. No reduction in peak
preparation allocations is claimed. Payloads are exact-length Arc slices;
there is no retained Vec capacity hidden by this selection.

The declared context byte charge and every admission cap remain unchanged,
including its conservative payload charge when an Arc is shared. The proposed
harmonic layouts/pool remain disconnected. This removes duplicate retained
payload storage only: it does not establish complete physical ownership below
32 MiB or solve harmonic aggregate admission. No new aggregate policy is added.

Focused tests inspect the actual prepared context's payload identity and
lifetime, retain signed zero/disabled oscillator/inactive LFO data, distinguish
unrelated equal owners, verify all v1–7 migration and v1–5 duration normalization,
and retain malformed-state refusals. Accepted notes have bit-exact sounding PCM
and provider endpoint/compatibility checks against independently encoded
canonical states; unsupported pitch boundaries must return the same error.

Harmonic amplitude, Normalize, Repeat, range retention and builtin-to-User laws
remain unresolved native reference decisions. Linux tests do not establish
native painting, listening, CoreAudio, physical deadlines or human acceptance.
The coordinating HOLD and blocked/draft status remain in effect.
