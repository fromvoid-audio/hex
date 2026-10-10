# original Operator control state

Pulse2170 added control-side metadata and validated settings for the original
four-envelope FM kernel. Pulse2172 registers that metadata as the distinct
`hex.instrument.operator` builtin and prepares it through the shared analytic
context contract. This is a bounded original acyclic FM device with four periodic
waveform choices, not full Operator behavior or proof of Live numerical parity.
A standalone bounded self-feedback kernel is specified in
[Operator feedback](operator-feedback.md), but its controls are not part of this
state schema or the registered native device. Filters, further waveform
controls and remaining behavior remain separate work.

`hex.instrument.operator`, state version4, owns exactly304 bytes: the original
224-byte version1 prefix of 56 little-endian f32 fields, four little-endian u32
waveform choices (version2's 240-byte prefix), then four 12-byte tuning records.
Each tuning record stores a u32 fixed flag, f32 frequency in Hz, and f32 multiplier.
Version1 migration validates and preserves its prefix bit-for-bit (including
signed zero), selects sine, and defaults every oscillator to ratio tuning.
Version2 migration preserves its full 240-byte prefix and defaults ratio tuning.
The first12 fields are ratio, level and phase in
operator order; the next16 are source-major modulation depths, including four
reserved zero diagonal entries; the next4 are carrier weights. The final24 fields of the original prefix are
initial, peak, sustain, attack, decay and release for each of four operators.
Parameter IDs are one-based field indices, excluding reserved diagonals; waveform
selectors use stable IDs57–60. There are73 editable descriptors:52 original floats, four
waveform choices (`sine`, `saw`, `square`, `triangle`) and one derived routing
preset choice (`custom`, `serial`, `parallel`, `two pairs`), plus four fixed switches and eight
frequency/multiplier floats. Tuning uses stable IDs62–73. Routing preset ID61
is computed from the exact sixteen modulation depths and four carrier weights;
it adds no serialized field. Four amplitude-velocity amounts are appended after
the version3 tuning records in version4, with stable IDs74–77 and domains[0,1].
Each scales its oscillator amplitude before routing by
`1 - amount + amount * (velocity / 127)`. Zero is neutral and keeps the existing
global velocity law intact. This is an original bounded law, not a claim of
numerical parity with Live. Named choices replace those twenty routing fields atomically. `custom`
leaves routing intact and is not a persisted mode; editing the topology changes
readback to `custom` unless it exactly matches a named layout. See the
[routing preset contract](operator-routing-presets.md). Existing
`hex.instrument.fm4` keeps its separate 144-byte state and global ADSR unchanged.

Every decode, migration and patch validates all fields and the acyclic routing
graph. Unknown types/versions, wrong lengths, nonfinite values, invalid ranges,
self-feedback and cycles are refused. Patches return a new opaque state, leaving
the original intact on failure. Migration accepts versions1–4. Versions1–3
preserve their full existing prefix byte-for-byte and append neutral amplitude-
velocity amounts; version3 fixed tuning remains intact. Version4 is validated
and preserved. Inactive fixed settings are validated too. See the
[fixed tuning contract](operator-fixed-tuning.md).
Storage needs no special Operator branch: this uses the existing opaque
native-device state contract.

Oscillator/routing domains reuse FM4's exact descriptors. Envelope amplitudes
are linear values in[0,1]; times are seconds in[0,120]. Original defaults are
initial0, peak1, sustain0.7, attack0.005s, decay0.1s and release0.1s for each
operator. These bounds/defaults are Hex choices. The public Live manual supplies
the control inventory, not measured transfer curves; see the
[envelope law](operator-envelopes.md) and [FM composition](operator-fm.md).

`OperatorSettings` decodes/prepares on control. The registered factory creates a
bounded immutable context and compacts each prepared note to four phase steps,
velocity and four harmonic-cap indices packed into one u32. The existing eight-
word note payload stores four prepared amplitude scales in its two previously
reserved-zero words; it remains64 bytes and does not change the generic contract.
Sampling borrows the shared context; inline analytic sampling
explicitly refuses this processing class. See the [waveform contract](operator-waveforms.md)
for the original table bank, phase law and aliasing limits. Tests exercise every
editable field, malformed data, atomic cycle refusal and exact round trips.
Independent PCM coverage includes the device kernel and registered Arrangement,
Session, seek, loop and old-context handoff paths. Native listening and full Live
feature parity remain open.
