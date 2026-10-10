# registered Operator instrument

Pulse2172 registers `hex.instrument.operator` in the existing native instrument
catalog. The control codec and52 descriptors remain the generic metadata and
opaque version1 state introduced earlier; storage, history and UI do not have
Operator-specific branches.

The admitted DSP is the original bounded acyclic four-oscillator sine core with
independent envelopes and carrier/modulation routing. Preparation validates the
full state and oscillator domains on control, then creates an immutable shared
configuration with its preinitialized sine table. Each note is represented by
four phase steps and velocity in the existing bounded shared-note payload. The
audio path borrows the shared configuration to sample it. The inline analytic
sampler explicitly refuses this processing class, so the device cannot silently
fall back to sine-only playback.

The instrument is selectable through the existing catalog and generic parameter
panel. A track still has one freely positioned instrument slot, and replacing
that slot remains an explicit project command. Arrangement, Session, prepared
revision handoff, shared export and the native prepared callback use the same
registered definition. Generic opaque state round-trips and project undo/redo
preserve edits.

This milestone does not implement mutable feedback, filters, extra waveforms,
unbounded routing, full Operator controls or Live numerical parity. The injected
mutable runtime remains test-only. Native listening and device-deadline behavior
require separate acceptance; see [performance measurements](performance.md).
