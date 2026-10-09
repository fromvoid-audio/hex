# SESSION-LAUNCH-001 — quantization boundaries and launch settings

Status: planned; public-documentation vocabulary reviewed, native behavior not observed.

The installed reference was re-inventoried on 9 October 2026:
`/Applications/Ableton Live 12 Suite.app`,
12.4.6 (2026-09-10_0de5c8fa9a), universal arm64/x86_64 executable;
SHA-256 `5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`.
Inventory: `artifacts/live-quantization-reference-inventory.log`. Host:
Apple M5 Pro, 15 physical CPU cores, macOS 26.6.2, arm64. No binaries were changed.

## Documentation evidence

Ableton's [Live 12 launching-clips manual](https://www.ableton.com/en/live-manual/12/launching-clips/#clip-launch-quantization),
reviewed 9 October, describes disabling clip launch quantization and inheriting
the global setting. It also documents distinct trigger, gate, toggle and repeat
launch modes, and legato handoff preserving the previous clip's position.
These are separate requirements, not established Hex capabilities. The online
manual's Live 12 version label does not establish exact behavior in installed
12.4.6. Timing at boundaries, changing meter and queued-setting edits needs
measured evidence; do not infer those rules from chooser labels.

## Planned discriminating fixture

Create an original disposable set with two short MIDI patterns on one track;
distinct pitches/onsets identify which pattern owns the output. No effects,
gain changes, expression, fades, normalization or dither. Use matching 48 kHz
audio capture at 128 frames, record device/actual negotiated format, and retain
event timestamps plus unaligned capture. No existing user project is used.

| Variable | Experiment | Distinguishing evidence |
| --- | --- | --- |
| Transport | Launch while stopped, paused and playing | Transport transition, first audible frame, selected source and queued indicator |
| Boundary | Trigger just before, at and after a quarter/grid boundary | Actual due frame; equality versus next-boundary rule |
| Meter | Repeat under 4/4, 3/4 and 6/8, then across a meter change | Quarter-grid versus bar-length alignment and anchor |
| Loop/Seek | Launch across Arrangement loop wrap; seek with a queued target | Whether grid follows performance time, Arrangement position or another anchor |
| Override | Global setting, clip override and override disabled | Which setting controls clip/scene launches and stop cells |
| Queued edit | Change global/clip setting after requesting a launch | Captured versus rescheduled target |
| Retrigger/Cancel | Request current clip again; replace/cancel queued target | Current source continuity, pending target and due transition |
| Launch mode | Trigger, gate, toggle, repeat; held/released input | Exact input-edge and repetition behavior, separately from quantization |

Capture baseline plus one changed variable, fixture/output hashes, reference
device settings and timestamps before marking any case observed. No automatic
audio alignment: it could hide a launch timing defect. Quantify onset uncertainty
from device latency and event delivery; frame-exact parity is not inferred from
mouse clicks or UI paint. Boundary tests need controlled timestamped triggering.

## Current Hex scope and unknowns

The engine currently offers Immediate and a fixed FourQuarter grid in monotonic
performance time, with prepared periodic tempo conversion, transactional route
application and typed pending cancellation. Existing kernel/callback regressions
test Hex's contract independently. A compact selector is being wired into the
canonical Session UI; its setting is local and not project-persisted.

No meter-aware bar grid, arbitrary subdivisions, per-clip launch overrides,
gate/toggle/repeat/legato launch modes or reference timing parity is claimed.
Native experiments are pending an unlocked Mac; no fixture, capture, numerical
comparison or decompilation for this question has been completed. Use observed
results to specify the next engine/project boundary, and version persisted
settings with explicit migration when introduced.
