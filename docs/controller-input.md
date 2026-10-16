# controller input

The existing editor status bar's **midi** button opens a temporary controller
panel above the bar. **controller input · discover ↻** discovers up to 127
CoreMIDI input endpoints; choose a port and channel 1–16 in the panel. Outside
click or Escape dismisses it and restores focus. The midi button highlights while
connected and its tooltip reports off, pending or connected. The panel takes no
editing viewport height. The selected enabled instrument
receives channel pressure, pitch bend and CC1; hardware notes, sustain, recording
and MPE are outside this workflow. Input is off by default and is not persisted.
Unsupported instruments or legacy Vector modes are rejected before port opening.
Verified-streaming documents refuse controller discovery and activation; this
workflow currently uses the resident borrowed-client publication bridge.

`hex-midi` owns the joining control worker and connection. Its command and result
channels each hold four entries. Hardware messages do not enter those queues:
exact-length channel messages with 7-bit data update one callback-owned state,
and the injected sink publishes one packed atomic snapshot. CC121 resets all
three values. Other channels, malformed messages, notes, SysEx and clock are
ignored. Native packet timestamps are deliberately unused;
controllers take effect at the next outer render boundary, adding up to one
render block of scheduling latency plus native input delivery latency.
Intermediate changes coalesce intentionally. A snapshot is frozen for all render
subsegments, compensation paths, keyboard monitoring and matching detached tails.
Three controller event slots supplement the existing 256 note-event capacity.
Unchanged snapshots are idempotent in Vector's runtime.

Each engine lease binds project generation, track ID and instrument instance ID.
Selection changes cancel the old lease before preparing a new one. Project
replacement, removal, deselection and connection errors cancel terminally; late
callback writes cannot reactivate a cancelled lease. Same-instance parameter
revisions retain the activation. Preparation checks compiled instrument state,
sample rate and bypass as well as instance identity; preview graphs use their
actual preview project for this check. A controller-only publication at the same
immutable revision reprepares and preserves an existing keyboard lease's fixed
committed voices, phase, envelopes, age and monitoring clock. The retired owner
relinquishes cancellation and input-reading authority; ordinary project
publications keep the existing keyboard cancellation policy. Unsupported target
runtimes drop inherited host expression state instead of receiving neutral
expression events they cannot process. Previously targeted mutable history receives a
neutral reset after deselection, including same-instance retained tails. Offline
export constructs no lease and remains neutral. Stop can retain physical values;
the next note render applies the current snapshot after resetting its runtime.

CoreMIDI unique IDs identify endpoints; names are display data only. Activation
and discovery request IDs prevent stale worker results from affecting a newer
selection. A fixed byte-stream decoder accepts running status and multiple
messages per packet, ignores interleaved realtime bytes and SysEx, and resets
when its 65,536-byte or 1,024-packet callback limit drops remaining input.

A 500 ms control-side endpoint-presence poll detects port loss. MIDI silence is
never interpreted as disconnection. Discovery and opening happen only after user
interaction; the desktop creates an idle worker. Callback work does not allocate,
queue, enumerate devices, log or wait in Hex code. This source inspection is not
physical CoreMIDI timing or allocation evidence.

The dependency is pinned to [coremidi 0.8.0](https://docs.rs/coremidi/0.8.0/coremidi/).
Its safe client, input port and packet APIs keep native FFI inside the dependency
boundary. Direct CoreMIDI avoids midir 0.11.0's Core Foundation pin conflict with
GPUI and midir 0.10.3's ALSA links conflict with CPAL. Hex's GPUI, CPAL and unsafe
policies remain unchanged. Non-macOS builds expose an unsupported backend.

Native acceptance remains separate: choose a controller and channel; play an
Arrangement clip, a Session clip and the computer keyboard; vary bend, pressure
and wheel; change instrument parameters; select another track; unplug the port;
confirm the sound returns to neutral and other MIDI channels/notes are ignored.
