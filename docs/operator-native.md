# registered Operator instrument

Pulse2172 registers `hex.instrument.operator` in the existing native instrument
catalog. The control codec exposes77 descriptors and opaque version5 state.
Versions1–4 migrate with neutral feedback; versions1–3 also receive neutral
amplitude velocity. Storage, history and UI use generic native-device contracts.

The admitted DSP is the original bounded acyclic four-oscillator core with
independent envelopes, carrier/modulation routing, and per-operator sine, saw,
square or triangle selectors and per-oscillator fixed tuning. Preparation validates the full state and oscillator
domains on control. It retains the existing sine table and, when any non-sine
shape is selected, a reference to one process-wide immutable harmonic-limited
table bank. Each note stores four phase steps, velocity, packed cap indices and
four control-prepared amplitude-velocity scales in the existing bounded 8-word
shared-note payload. The zero-effective-feedback analytic path borrows the shared
configuration to sample it. The inline analytic sampler explicitly refuses this
processing class, so a selector cannot silently fall back to sine-only playback.

The instrument is selectable through the existing catalog and generic parameter
panel. A track still has one freely positioned instrument slot, and replacing
that slot remains an explicit project command. Arrangement, Session, prepared
revision handoff, shared export and the native prepared callback use the same
registered definition. Generic opaque state round-trips and project undo/redo
preserve edits.

The routing preset control is derived from the stored routing matrix and carrier
weights; it is not an additional serialized mode. Named topology selections
update the existing routing parameters through the ordinary immutable device
patch contract. See the [routing preset contract](operator-routing-presets.md).

Nonzero effective feedback selects the registered bounded mutable runtime with
16 simultaneous voices, 256 events per block and a measured 3,808-byte twin
(7,616 bytes paired). Authored
amounts remain stored when incoming FM makes a control unavailable; effective
zero retains analytic playback. The analytic API does not expose a context for
mutable feedback state, preventing a silent wrong-law fallback. See the
[feedback contract](operator-feedback.md) for the original recurrence,
capability limits, storage accounting and validation. Active class-changing
handoffs can refuse audio admission while desired project edits remain canonical;
this does not establish live parameter updates on held feedback voices. The
standalone bounded retarget kernel currently has no registered runtime wiring;
one shared 216-byte ramp per twin would bring the pair to 8,048 bytes, below the
existing 8,192-byte ceiling.

Filters, additional waveforms beyond these four, unbounded routing, remaining
Operator controls and Live numerical parity remain open. Native listening and
physical device deadlines require separate acceptance; see
[performance measurements](performance.md).

Fixed oscillators use frequency in Hz times a multiplier, independent of MIDI
pitch. Ratio oscillators retain the previous exact phase-step law. Per-operator
amplitude velocity uses the original scale documented in the
[Operator state contract](operator-state.md). See the [fixed tuning contract](operator-fixed-tuning.md)
for tuning bounds and validation.
