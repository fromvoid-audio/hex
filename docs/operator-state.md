# original Operator control state

Pulse2170 added control-side metadata and validated settings for the original
four-envelope FM kernel. Pulse2172 registers that metadata as the distinct
`hex.instrument.operator` builtin. Zero effective self-feedback uses the shared
analytic context contract; nonzero effective feedback selects the bounded native
mutable runtime. This is a bounded original acyclic FM device with four periodic
waveform choices, not full Operator behavior or proof of Live numerical parity.
The standalone and registered bounded self-feedback contracts are documented in
[Operator feedback](operator-feedback.md). Filters, further waveform controls,
transport-specific behavior and remaining Operator behavior remain separate work.

`hex.instrument.operator`, state version8, owns exactly380 bytes: the original
224-byte version1 prefix of 56 little-endian f32 fields, four little-endian u32
waveform choices (version2's 240-byte prefix), then four 12-byte tuning records.
Each tuning record stores a u32 fixed flag, f32 frequency in Hz, and f32 multiplier.
Version1 migration validates and preserves its in-range prefix bit-for-bit (including
signed zero), selects sine, and defaults every oscillator to ratio tuning.
Version2 migration preserves its in-range 240-byte prefix and defaults ratio tuning.
The first12 fields are ratio, level and phase in
operator order; the next16 are source-major modulation depths, including four
reserved zero diagonal entries; the next4 are carrier weights. The final24 fields of the original prefix are
initial, peak, sustain, attack, decay and release for each of four operators.
Parameter IDs are one-based field indices, excluding reserved diagonals; waveform
selectors use stable IDs57–60. There are92 editable descriptors, including eleven LFO and four oscillator enable controls; the earlier77 comprise52 original floats, four
waveform choices (`sine`, `saw`, `square`, `triangle`) and one derived routing
preset choice (`custom`, `serial`, `parallel`, `two pairs`), plus four fixed switches and eight
frequency/multiplier floats. Tuning uses stable IDs62–73. Routing preset ID61
is computed from the exact sixteen modulation depths and four carrier weights;
it adds no serialized field. Four amplitude-velocity amounts are appended after
the version3 tuning records in version4, with stable IDs74–77 and domains[0,1].
Each scales its oscillator amplitude before routing by
`1 - amount + amount * (velocity / 127)`. Zero is neutral and keeps the existing
global velocity law intact. This is an original bounded law, not a claim of
numerical parity with Live. Version5 appends four f32 self-feedback amounts after
the exact version4 304-byte prefix, with stable IDs78–81 and domains[0,1]. They
retain their authored values when their oscillator has an incoming cross-
modulation edge; metadata marks that control unavailable and the effective
feedback amount is zero until the edge is removed. Routing presets preserve the
authored amounts. Named choices replace those twenty routing fields atomically. `custom`
leaves routing intact and is not a persisted mode; editing the topology changes
readback to `custom` unless it exactly matches a named layout. See the
[routing preset contract](operator-routing-presets.md). Existing
`hex.instrument.fm4` keeps its separate 144-byte state and global ADSR unchanged.

Every decode, migration and patch validates all fields and the acyclic routing
graph. Unknown types/versions, wrong lengths, nonfinite values, invalid ranges,
invalid diagonal routing and cycles are refused. Patches return a new opaque state, leaving
the original intact on failure. Migration accepts versions1–8. Versions1–3
preserve their in-range existing prefix byte-for-byte and append neutral amplitude-
velocity amounts; version3 fixed tuning remains intact. Version4 is validated
as an exact 304-byte prefix and migrates with neutral feedback. Version5 validates
and preserves in-range fields in the 320-byte prefix, then appends neutral LFO defaults.
Version6 retains the320-byte layout and strictly validates the revised envelope
domains; it appends the same LFO defaults. Legacy versions1–5 validate their
original0–120s envelope domain then clamp attack to0–20s and decay/release to
0.001–60s. Version6 and7 refuse out-of-domain durations. Version7 adds
eleven validated four-byte LFO fields (IDs82–92). All earlier versions append
the same disabled defaults. See [Operator LFO](operator-lfo.md) for the timing,
modulation, live-transfer and admission contract. Inactive fixed and feedback settings are validated too. See the
[fixed tuning contract](operator-fixed-tuning.md).
Storage needs no special Operator branch: this uses the existing opaque
native-device state contract.

Oscillator/routing domains reuse FM4's exact descriptors. Envelope amplitudes
are linear values in[0,1]; attack is seconds in[0,20] and decay/release in[0.001,60]. Original defaults are
initial0, peak1, sustain0.7, attack0.005s, decay0.1s and release0.1s for each
operator. These bounds/defaults are Hex choices. The public Live manual supplies
the control inventory, not measured transfer curves; see the
[envelope law](operator-envelopes.md) and [FM composition](operator-fm.md).

`OperatorSettings` decodes/prepares on control. The registered analytic factory
creates a bounded immutable context and compacts each prepared note to four phase
steps, velocity and four harmonic-cap indices packed into one u32. The existing
eight-word analytic note payload stores four prepared amplitude scales in its two
previously reserved-zero words; it remains64 bytes and does not change the
generic contract. Nonzero feedback or enabled LFO deliberately exposes no analytic context;
generic event-source compilation routes it through the fixed mutable runtime.
Inline analytic sampling explicitly refuses this processing class. See the
[waveform contract](operator-waveforms.md)
for the original table bank, phase law and aliasing limits. Tests exercise every
editable field, malformed data, atomic cycle refusal and exact round trips.
Independent PCM coverage includes the device kernel and registered Arrangement,
Session, seek, loop and old-context handoff paths. Native listening and full Live
feature parity remain open.

## oscillator-enable suffix — Pulse2335 / draft PR164

Version8 appends four strict little-endian u32 boolean words to the exact364-byte
version7 prefix, in oscillator order a/b/c/d at offsets364/368/372/376. Stable
IDs513–516 use Boolean descriptors and default true. IDs93–512 remain reserved
for the separate harmonic work; version9 may append its suffix after all380
prefix bytes. Versions1–7 migrate with all four flags enabled. Wrong lengths,
nonbinary words, wrong parameter types and future versions refuse atomically.
Authored levels/routing/tuning/feedback/envelopes/LFO bytes remain independent
and unchanged. LFO decoding remains fixed to[320,364) for versions>=7; legacy
envelope clamping retains its literal<6 cutoff.

This checkpoint implements codec/descriptors only. DSP enable masking, active
analytic continuation, mutable adoption, canonical history/save integration and
badge interaction are pending and unverified. Do not present the flags as a
working musical toggle until those boundaries are integrated and tested. See
[enable requirements](operator-enable.md) and
[approved continuation direction](operator-enable-continuation.md).
