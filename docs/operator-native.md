# registered Operator instrument

Pulse2172 registers `hex.instrument.operator` in the existing native instrument
catalog. The control codec exposes73 descriptors and opaque version4 state,
migrating versions1–3 with neutral amplitude-velocity amounts; storage, history and UI do not have
Operator-specific branches.

The admitted DSP is the original bounded acyclic four-oscillator core with
independent envelopes, carrier/modulation routing, and per-operator sine, saw,
square or triangle selectors and per-oscillator fixed tuning. Preparation validates the full state and oscillator
domains on control. It retains the existing sine table and, when any non-sine
shape is selected, a reference to one process-wide immutable harmonic-limited
table bank. Each note stores four phase steps, velocity, packed cap indices and
four control-prepared amplitude-velocity scales in the existing bounded 8-word
shared-note payload. The audio path borrows the shared
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

This milestone does not implement mutable feedback, filters, additional
waveforms beyond these four, unbounded routing, full Operator controls or Live numerical parity. The injected
mutable runtime remains test-only. Native listening and device-deadline behavior
require separate acceptance; see [performance measurements](performance.md).

Fixed oscillators use frequency in Hz times a multiplier, independent of MIDI
pitch. Ratio oscillators retain the previous exact phase-step law. Per-operator
amplitude velocity uses the original scale documented in the
[Operator state contract](operator-state.md). See the [fixed tuning contract](operator-fixed-tuning.md)
for tuning bounds and validation.
