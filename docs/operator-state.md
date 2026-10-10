# original Operator control state

Pulse2170 adds control-side metadata and validated settings for the original
four-envelope FM kernel. This is a preparation contract, not a selectable device
or proof of Live numerical parity. Native admission depends on Pulse2169's
shared-context playback and retirement contract. Feedback, filters, additional
waveforms and the remaining Operator behavior remain separate work.

`hex.instrument.operator`, state version1, owns exactly224 bytes: 56
little-endian f32 fields. The first12 fields are ratio, level and phase in
operator order; the next16 are source-major modulation depths, including four
reserved zero diagonal entries; the next4 are carrier weights. The final24 are
initial, peak, sustain, attack, decay and release for each of four operators.
Parameter IDs are one-based field indices, excluding reserved diagonals. There
are52 editable descriptors. Existing `hex.instrument.fm4` keeps its separate
144-byte state and global ADSR unchanged.

Every decode, migration and patch validates all fields and the acyclic routing
graph. Unknown types/versions, wrong lengths, nonfinite values, invalid ranges,
self-feedback and cycles are refused. Patches return a new opaque state, leaving
the original intact on failure. Migration currently accepts version1 only and
preserves its exact bytes, including valid signed zero. Storage needs no special
Operator branch: this uses the existing opaque native-device state contract.

Oscillator/routing domains reuse FM4's exact descriptors. Envelope amplitudes
are linear values in[0,1]; times are seconds in[0,120]. Original defaults are
initial0, peak1, sustain0.7, attack0.005s, decay0.1s and release0.1s for each
operator. These bounds/defaults are Hex choices. The public Live manual supplies
the control inventory, not measured transfer curves; see the
[envelope law](operator-envelopes.md) and [FM composition](operator-fm.md).

`OperatorSettings` decodes/prepares on control. `OperatorMetadata` implements
the shared metadata contract without registering an instrument factory. Tests
exercise every editable field, malformed data, atomic cycle refusal and exact
round trips. Independent continuous-sine/power-curve PCM oracles check all four
operator bindings with NoteOff during attack, decay and sustain. The complete
54-test device suite passes, including independent forward/reverse routing and
signed-carrier PCM checks; native integration and listening remain open.
