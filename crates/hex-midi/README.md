# hex-midi

## Purpose

Optional macOS controller discovery and connection, on a joining control worker.
The selected MIDI channel supplies channel pressure, pitch bend and CC1 only.
The callback publishes a coalesced fixed snapshot through an injected bounded
sink; worker-owned connection destruction cancels the sink before closing it.

## Dependencies

Standard library and pinned `coremidi =0.8.0` on macOS. Native FFI remains
inside this dependency boundary; Hex workspace code has no unsafe code. The
input port drops before its owning client, on the joining control worker.
Direct CoreMIDI avoids midir 0.11.0's Core Foundation 0.10.1 conflict with
GPUI's exact 0.10.0 pin and midir 0.10.3's ALSA links conflict with CPAL.
Other platforms return an explicit unsupported error without a MIDI backend.

## Does not own

Engine contracts, project state, DSP, note allocation, audio queues, MIDI notes,
sustain, recording, MPE, application preferences, or GPUI. No native enumeration
occurs until the user requests discovery; tests use synthetic messages.
