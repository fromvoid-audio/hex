# original Operator control state

Pulse2170 added control-side metadata and validated settings for the original
four-envelope FM kernel. Pulse2172 registers that metadata as the distinct
`hex.instrument.operator` builtin and prepares it through the shared analytic
context contract. This is a bounded original acyclic FM device with four periodic waveform
choices, not full Operator behavior or proof of Live numerical parity. Feedback,
filters, further waveform controls and remaining behavior remain separate work.

`hex.instrument.operator`, state version2, owns exactly240 bytes: the original
224-byte version1 prefix of 56 little-endian f32 fields followed by four
little-endian u32 waveform choices. Version1 migration validates its old fields,
preserves the first224 bytes bit-for-bit (including signed zero), and selects
sine for all four operators. Version2 appends one selector for each operator.
The first12 fields are ratio, level and phase in
operator order; the next16 are source-major modulation depths, including four
reserved zero diagonal entries; the next4 are carrier weights. The final24 fields of the original prefix are
initial, peak, sustain, attack, decay and release for each of four operators.
Parameter IDs are one-based field indices, excluding reserved diagonals; waveform
selectors use stable IDs57–60. There are56 editable descriptors:52 floats and
four choices (`sine`, `saw`, `square`, `triangle`). Existing
`hex.instrument.fm4` keeps its separate 144-byte state and global ADSR unchanged.

Every decode, migration and patch validates all fields and the acyclic routing
graph. Unknown types/versions, wrong lengths, nonfinite values, invalid ranges,
self-feedback and cycles are refused. Patches return a new opaque state, leaving
the original intact on failure. Migration accepts exactly version1 or version2;
the former migrates to version2 while the latter is validated and preserved.
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
velocity and four harmonic-cap indices packed into one u32 in the existing
eight-word payload. Sampling borrows the shared context; inline analytic sampling
explicitly refuses this processing class. See the [waveform contract](operator-waveforms.md)
for the original table bank, phase law and aliasing limits. Tests exercise every
editable field, malformed data, atomic cycle refusal and exact round trips.
Independent PCM coverage includes the device kernel and registered Arrangement,
Session, seek, loop and old-context handoff paths. Native listening and full Live
feature parity remain open.
