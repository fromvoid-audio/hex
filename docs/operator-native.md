# registered vector instrument

The original four-operator FM synth is named **vector** in the browser, catalog
and device header. Its stable `hex.instrument.operator` identity, versioned
state and parameter IDs remain unchanged so existing projects retain their
instrument and settings. Internal Operator module names and reference research
still use the historical name.

Pulse2172 registers `hex.instrument.operator` in the existing native instrument
catalog. The control codec exposes99 descriptors and opaque version11 state.
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
16 simultaneous voices, 256 events per block and a measured 4,032-byte twin
(8,064 bytes paired). Authored
amounts remain stored when incoming FM makes a control unavailable; effective
zero retains analytic playback. The analytic API does not expose a context for
mutable feedback state, preventing a silent wrong-law fallback. See the
[feedback contract](operator-feedback.md) for the original recurrence,
capability limits, storage accounting and validation. Active transitions into or
out of the mutable-feedback processing class can still refuse audio admission
while desired project edits remain canonical. The bounded retarget path supports
live oscillator-level, carrier and effective
feedback changes through the registered runtime, with one shared five-millisecond
ramp per twin. Its 8,064-byte pair remains fully charged to the aggregate
instrument budget; unsupported
waveform, tuning, envelope, modulation, sample-rate and velocity-law edits are
refused while history is active and may use a fresh runtime only after
committed voices and feedback tails are quiescent. See [the feedback contract](operator-feedback.md)
for the exact transfer and acceptance limits.

The original [Clean carrier filter](operator-filter.md) supports LP/HP/BP/notch/
morph, 12/24 slopes, frequency, resonance and morph through saved shared controls.
Other circuits, additional waveforms beyond these four, unbounded routing, remaining
Operator controls and Live numerical parity remain open. Native listening and
physical device deadlines require separate acceptance; see
[performance measurements](performance.md).

Fixed oscillators use frequency in Hz times a multiplier, independent of MIDI
pitch. Ratio oscillators retain the previous exact phase-step law. Per-operator
amplitude velocity uses the original scale documented in the
[Operator state contract](operator-state.md). See the [fixed tuning contract](operator-fixed-tuning.md)
for tuning bounds and validation.
