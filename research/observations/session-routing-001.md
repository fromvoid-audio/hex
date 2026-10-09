# SESSION-ROUTING-001 — slot representation and per-track ownership

Status: reference UI observed; documented behavior; runtime comparison pending

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

## Documented behavior and implementation consequence

The [Live 12 Session View manual](https://www.ableton.com/en/live-manual/12/session-view/#recording-sessions-into-the-arrangement)
describes per-track exclusivity: a Session launch overrides that track's
Arrangement; a clip stop silences the track, and Back to Arrangement explicitly
restores it. Removing a slot's stop button prevents its scene from affecting
that track. This supports distinct no-op and stop cells and mixed per-track
Session/Arrangement playback. A global exclusive playback mode is insufficient.

The [launching clips manual](https://www.ableton.com/en/live-manual/12/launching-clips/#clip-launch-quantization)
describes None and Global clip quantization, plus Trigger, Gate, Toggle, Repeat
and Legato behavior. These remain separate future requirements.

## Remaining acceptance

Create an isolated minimal original set before exercising launches. Retain a
baseline and a single-variable stop-button variant; record current rate/block,
quantization, armed/record settings, launch modes and source markers. Compare
per-track continuation versus stop and Arrangement resume with transport and
audio/event evidence. Define frame alignment/tolerance before capture. Test
on-boundary launch, queued replacement/cancellation and stopped transport.

Hex's stored independent Session ownership, prepared inventory and additive
source mixing are source accepted; no canonical Session runtime/native comparison
is claimed here.

## Transport/clock follow-up — 8 October 2026

A fresh read of the [official Session manual](https://www.ableton.com/en/live-manual/12/session-view/)
documents that individual clip stops leave transport running, and two Control
Bar Stop presses return song position to 1.1.1. Hex currently resets to frame zero
on one `PlaybackCommand::Stop`; the exact first/second-press native behavior still
needs an isolated fixture. Do not infer it from the retained template/user set.
The manual passages reviewed do not establish Session phase across an active
Arrangement loop or transport seeking. Track-source clocks must be explicit;
Arrangement period copying cannot be reused blindly for launched sources.
