# Live Session slot observations

- Reference: installed Ableton Live 12 Suite, 12.4.6
  (2026-09-10_0de5c8fa9a), universal executable, initial arm64 target.
- Executable SHA-256: `5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`.
- Metadata/hash rechecked 8 October 2026 before this study.
- Host: Apple Silicon/macOS; visible reference sample rate 44.1 kHz.
  Device/block size not inspected; no audio measurement was made.
- Fixture: startup Untitled set contained existing template/user content. No
  clips, tracks, notes, routing or settings were edited, saved or discarded.
  This is not the disposable minimal launch fixture required for timing proof.
- UI tooling: native CUA accessibility, installed Suite app selected by exact path.
- Static analysis: none.

## Reproduction and observations

1. Confirm Live inactive, launch installed Suite. Initial selection timed out;
   inventory confirmed the same process running, then it was selected.
2. Observe transport stopped, global launch quantization displayed as 1 Bar.
   This is the current set's value, not evidence of a factory default.
3. Use View → Toggle Arrangement/Session View. Observe horizontal scene rows,
   vertical tracks, per-track Clip Stop controls and separate scene controls.
4. Right-click an unoccupied MIDI slot. Its context menu exposes Remove Stop
   Button (Cmd+E) and Insert Empty MIDI Clip (Cmd+Shift+M). Menu help describes
   adding/removing stop controls on selected slots. Do not execute either item.
5. Dismiss the menu and restore Arrangement via Tab. Transport remains stopped.

No playback transition was tested. UI capability evidence is not an audio
behavior, launch-timing, empty-slot-default or release-tail acceptance result.
