# original Operator fixed tuning

Pulse2205 adds ratio or fixed tuning independently to each of four oscillators.
The public [Live12 Operator manual](https://www.ableton.com/en/live-manual/12/live-instrument-reference/#operator)
describes pitch-independent Fixed mode and frequency times multiplier. Hex uses
that control inventory; the numerical domains and transfer law below are original
choices, not measured Live parity.

Fixed mode uses frequency in Hz times multiplier, independent of MIDI pitch.
Frequency is0.1–20,000 Hz; multiplier is0.01–1,000 (the minimum is the exact
f32 representation used by metadata). Defaults are ratio mode,440 Hz and1.
Stored ratio remains editable and remembered while fixed mode ignores it.
Every decode and patch validates finite values, flags, bounds and inactive
settings. Note preparation also requires the effective frequency strictly below
Nyquist for every oscillator, even inaudible ones. A note-free configuration
may store a product above Nyquist; adding/preparing such a note refuses it.
Candidate compilation precedes canonical history commits, so a rejected edit
preserves the prior project and audio state.

All-ratio preparation uses the existing quantizer exactly. Mixed tuning computes
fixed or note-derived frequency on control, then uses the same rounded phase-step
law. Waveform harmonic caps follow those actual steps. Existing note-relative
phase/retrigger, envelopes, routing and gate laws remain unchanged. No frequency
conversion, allocation or replay is added to sampling. The arm64 shared context
is688 bytes and a compact note remains48 bytes (5,488 bytes for100 notes sharing
one context, excluding host records, shared table banks and ownership storage).

State version3 is288 bytes. It preserves the full version2 240-byte prefix and
appends four12-byte records: little-endian u32 fixed flag, f32 Hz and f32 multiplier.
Version1's224-byte prefix and version2's240-byte prefix survive migration exactly.
Parameter IDs62–73 expose one boolean and two floats per oscillator. Generic
catalog controls, opaque storage, project commands and undo/redo require no
Operator-specific storage or UI branches.

Automated evidence:77 device tests pass; new independent fixed-tone and mixed-FM
oracles cover pitch independence, waveform caps, invalid values and Nyquist.
Three native admission tests include a fixed250 Hz PCM oracle. Session history
checks atomic refusal and undo/redo;20 storage tests include opaque version3
round trips. A callback probe measures zero total and signed-net allocations and
bytes during fixed-waveform render, direct seek and shutdown; construction and
reclaim are outside those scopes. These are headless fixtures, not native
listening, device deadlines or complete Operator/Live parity. Filters, LFO,
feedback, additional waveforms and quality work remain separate tickets.
