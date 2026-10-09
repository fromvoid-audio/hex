# Performance requirements and baseline — 8 October 2026

## Clip fade integration profile

The before/after run uses `cargo run --release -p hex-engine --example profile_loop`
on Apple M5 Pro, macOS 26.6.2: resident 100 tracks × 100 clips, 100 sustained
active source loops plus 9,900 sparse clips, 48 kHz, 128-frame blocks, 40 warm-up
and 200 measured blocks with an explicit seek to zero before each. No output
device is used. Hardware was collected separately; power mode and background
load were not controlled. These short runs are provisional, not sustained
device deadline or UI responsiveness acceptance.

| Scenario | Before median / p95 µs | Final neutral median / p95 µs | 100 faded clips median / p95 µs |
| --- | --- | --- | --- |
| Global loop disabled | 16.67 / 18.33 | 17.71 / 24.42 | 25.75 / 28.92 |
| 1,024-frame global loop | 17.33 / 27.25 | 17.92 / 28.79 | 25.54 / 29.04 |
| One-frame global loop | 6.58 / 7.25 | 7.21 / 10.00 | 7.46 / 10.21 |

Initial inline envelope metadata gave neutral medians 21.71 / 21.42 / 9.08 µs.
Moving the neutral/faded dispatch outside the sample loop and storing only
non-neutral prepared fades in control-allocated boxes reduced that observed
overhead. The final neutral values still differ from the initial baseline;
do not claim zero overhead or a statistically established regression budget.
Final neutral maxima were 47.71 / 58.33 / 147.21 µs; before maxima were
28.08 / 676.12 / 15.71 µs, illustrating scheduling noise in these short runs.

`cargo run --release -p hex-engine --example profile_loop -- --fades` uses
4,800-frame linear fade-in/out domains on the
100 sustained clips. The measured first block exercises the fade-in; it does
not benchmark a distant fade-out or overlapping ramps. Neutral digests remain
`9b1054eca0514125` (disabled/1,024-frame) and `011daa8529897725` (one-frame).
Faded digests are `146c961d5b9249f2` and `d80ac658736bb725`, unchanged by the
representation repair. The schema-4 legacy fixture's 507,150-frame, 44.1 kHz
PCM16 stereo export remains byte-identical, SHA-256
`952c8d29e303b10942d47300381b87b074d75828077e1b759f744b29a2bba681`.
Logs are preserved locally under ignored `artifacts/profiles/` as
`fades-before.log`, `fades-neutral.log`, `fades-active.log` and
`fades-neutral-export.log`.

Run `cargo run --release -p hex-engine --example profile`.
This renders the fixed four-track/eight-source demo at 48 kHz. Each block size
has 200 warm-up blocks and 4,000 timed blocks. Timing uses Instant; result frames
pass through black_box. This is a CPU microbenchmark with resident media, not
a device test, realistic large project, GPUI measurement, or comparison with Live.

```text
 64 frames: p50=0.7µs p99=0.8µs max=1.0µs deadline=1333.3µs
128 frames: p50=1.4µs p99=1.7µs max=27.8µs deadline=2666.7µs
256 frames: p50=2.3µs p99=2.9µs max=45.0µs deadline=5333.3µs
512 frames: p50=4.2µs p99=5.4µs max=54.5µs deadline=10666.7µs

```

During the GPUI smoke check, CoreAudio negotiated 44.1 kHz stereo. Transport and
meters responded; the visible closure maximum reached 0.45 ms with zero reported
late callbacks at the observed point. No ten-minute stress test, backend underrun
instrumentation, allocator instrumentation, or audible approval was completed.
GPUI accessibility currently exposes the window but not custom control labels;
keyboard/screenshot interaction was used. Accessible controls remain work.

## Gates before performance claims

- Representative workloads at 64/128/256 frames: callback p99 below half deadline;
  no observed deadline violations in a ten-minute run. Include backend faults.
- Measure UI frame time and interaction latency under audio load, dense clips,
  scrolling, waveform rendering, and editing.
- Instrument allocations and graph reclamation; exercise full command queues,
  seek/stop storms, overload, and device disconnect/reconnect.
- Compare Hex and Live only on equivalent workloads/settings on the same host.
- Track regressions over repeated release builds. Microbenchmark numbers alone
  do not establish a faster or more stable DAW.

## Product targets — not yet demonstrated

100 tracks is a routine supported workload, not an exceptional stress case.
These initial budgets turn responsiveness into acceptance criteria; record the
CPU model, core count, OS, power mode, device, plugin versions, sample rate and
buffer size with every result. Track count alone cannot bound arbitrary DSP cost.

| Workload | Initial acceptance target |
| --- | --- |
| 100 active stereo audio tracks, 10,000 clips, five lightweight native devices per track (500 instances) | At 48 kHz / 128 frames, callback p99 below half the block deadline; no backend underruns or callback deadline misses over 30 minutes with editing and scrolling. |
| 100 mixed audio/MIDI tracks with instruments and external VST3 devices | Define reproducible device/preset fixtures and measure the same gate; publish supported workload per tested machine, not a universal plugin-count guarantee. |
| One 32-device serial chain, parallel racks, groups, sends and sidechains | Measure critical-path cost separately from total CPU; verify sample alignment at every merge and bound scheduling overhead. |
| Add/remove/move/trim clips and undo/redo under the 100-track workload | Input-to-visible acknowledgment p95 ≤ 16 ms and p99 ≤ 32 ms; no synchronous media decode, plugin loading or full project compilation on the UI thread. |
| Play/Stop with a prepared graph | Consume the request by the next callback after successful handoff; no deliberate transport sleep or graph compilation on Play. Measure request-to-audible output separately. |

Expand the matrix to 64/256-frame buffers and 44.1/96 kHz on representative
Apple Silicon and x86-64 machines. Keep sustained deadline failures, UI stalls,
thermal behavior and memory growth visible; averages alone are insufficient.
A slow or overloaded plugin must produce an intelligible diagnostic, not freeze
editing or silently discard the user's devices.

New or changed media can require preparation. Acknowledge the edit immediately,
prepare it off-thread, and visibly distinguish pending from audible state. Specify
bounded handoff/backpressure and cancellation before implementation; never claim
that a pending edit is already sounding. Preserve playback of the last valid graph
until a complete replacement can be safely installed.

## Modern processor strategy

- Parallelize independent tracks and graph branches with a bounded, deadline-aware
  worker pool. Preserve serial dependencies within chains; all cores cannot make
  an inherently serial chain parallel. Design worker scheduling/lifetime before
  adding it to the callback, and measure coordination overhead at small buffers.
- Use cache-friendly audio layouts, reusable buffers, block DSP and measured SIMD
  kernels. Evaluate ARM64 NEON and x86-64 AVX2 paths through safe abstractions with
  feature detection and a portable fallback. Do not assume every x86-64 CPU has
  the same instruction set, or relax the workspace unsafe-code rule implicitly.
- Investigate macOS real-time scheduling and heterogeneous-core coordination at
  the audio-platform boundary; validate supported APIs before adopting them.
  Keep decoding, waveform analysis and graph preparation outside real-time work.
- Let GPUI's GPU-backed rendering serve compact visible content; virtualize dense
  tracks/clips and cache waveform tiles. Evaluate GPU compute for offline jobs
  only when measurement justifies transfer/synchronization costs. Using every
  accelerator is not an objective independent of deadline safety.
- Benchmark scalar/SIMD and single/multicore paths for correctness and speed on
  actual supported hardware. Prefer the fastest reliable path for each workload;
  document fallback behavior, energy/thermal tradeoffs and regressions.

The current four-track benchmark does not meet these acceptance gates. The
block-span offline renderer and metadata snapshot history need measured
large-project evaluation before native editor adoption or scalability claims.
See [latency.md](latency.md) for aligned playback and immediate-start constraints.

## Clip-span renderer comparison — 8 October 2026

Run cargo run --release -p hex-engine --example profile_arrangement. Resident
100 tracks × 100 clips, 128-frame blocks, 40 timed iterations on aarch64 macOS;
short sparse clips and some muted tracks, no devices or disk/UI load. An independent
all-clips-per-frame scalar baseline compares exact output before timing. Matching
digest: 8d42f8b9d0f1d630. Observed baseline p50 730.75 µs / p95 894.08 µs / max
902.58 µs; block-span p50 9.96 µs / p95 17.38 µs / max 20.08 µs. This establishes
improvement for this sparse fixture only. Span discovery still scans every clip
per block; no sustained CoreAudio deadline or 100-active-track/device gate passed.

## Canonical edit/history cost — 8 October 2026

Run cargo run --release -p hex-project --example profile_edits. Apple M5 Pro,
aarch64 macOS, 100 tracks × 100 clips sharing one metadata asset; 32-step snapshot
history, 10 warmup and 200 alternating gain transactions. Observed p50 31.4 µs,
p95 81.3 µs, p99 114.6 µs, max 204.0 µs. This measures metadata/history only,
not worker queue latency, compilation, GPUI acknowledgment or audible changes.
Current snapshot costs fit this fixture; keep the full end-to-end UI gate open.

## Active 100-track renderer fixture — 8 October 2026

Run `cargo run --release -p hex-engine --example profile_arrangement -- --active`.
Apple M5 Pro / aarch64 macOS, 48 kHz, 128-frame blocks, 100 tracks × 100 clips.
Every track has one sustained looping clip; remaining clips are sparse and all
tracks are unmuted. Five scalar-oracle warmup positions precede 40 timed seek/render
positions. Exact frame comparisons occur outside timing for every measured block.
No devices, disk work, native callback or UI load are included.

Observed scalar p50 683.00 µs / p95 936.75 µs / max 1010.79 µs; block spans
p50 18.58 µs / p95 33.25 µs / max 40.46 µs. Matching digest:
`a7db7404bf70aac8`. This extends coverage to 100 simultaneously active audio
tracks with 10,000 resident clips; it does not establish sustained output, plugin
chains, realtime allocation safety or UI latency. Full device workload gates remain
open.

## Headless canonical mixed-route kernel — 8 October 2026

The release `profile_session_playback` example builds a real canonical 100-track
instrument project with 50 Session and 50 Arrangement routes, four/eight voices
per source, varied gain/balance and 0.73 master gain. Its 72,000-frame Arrangement
loop differs from the 96,000-frame Session source period. Before timing, 160,000
frames rendered in one call exactly match 31-frame partitions across multiple
wraps, including both final cursors. Construction and comparisons are off timer.

At 48 kHz, 100 warmups precede 4,000 measured 128-frame renders. Observed
p50/p95/p99/max: 54.583/94.375/100.625/244.292 µs; zero samples exceed the
2,666.667 µs block period. Checksum `1995978491398321`; final performance cursor
524,800 and Arrangement cursor 20,800. Checksums are outside each render timer.
Log: `artifacts/session-kernel-profile.log`. This is an offline, constant-tempo,
original-sine kernel workload under ordinary development activity. It does not
establish native device deadlines, repeated-tempo-loop timing, graph handoff,
plugin/device chains, minimum-window latency or sustained hardware stress.

Final sorted-bank implementation rerun retains checksum `1995978491398321` and
both final cursors. p50/p95/p99/max: 54.250/96.000/105.291/205.500 µs,
zero of 4,000 samples above period. Log:
`artifacts/session-kernel-final-profile.log`. This run shares the same scope and
includes normal development checks; the change between observations is not a
controlled speed comparison.

Periodic-clock extension: the example now verifies mixed output against an
independently compiled finite Arrangement oracle before timing. Session-selected
tracks use their canonical MIDI sources; the other tracks use three-beat source
periods to express the Arrangement wraps. The oracle uses an expanded ordinary
tempo map and never calls the ongoing Session queries or periodic clock. Both
constant and changing-tempo workloads retain exact PCM/31-frame partition equality.

`CARGO_INCREMENTAL=0 cargo run --release -p hex-engine --example
profile_session_playback -- --loop-tempo` selects a 120,000-frame, three-beat loop:
120 bpm for the first beat, then 60 bpm for two beats. A canonical 240 bpm marker
at beat five is outside the loop and must not affect Session scheduling. The
independent comparison covers 260,000 frames and multiple wraps. The original
constant workload still compares 160,000 frames and retains its prior checksum.
Same Apple M5 Pro, 48 kHz/128 frames, 100 warmups and 4,000 timed blocks; setup,
finite oracle, partition comparison and checksum computation remain off timer.

| Workload | p50 µs | p95 µs | p99 µs | Max µs | Above 2.667 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Constant tempo, independent finite oracle | 53.375 | 95.750 | 103.375 | 174.583 | 0 / 4,000 |
| Repeating tempo segment, independent finite oracle | 52.666 | 95.084 | 102.917 | 182.125 | 0 / 4,000 |

Logs: `artifacts/session-loop-final-constant-profile.log` and
`artifacts/session-loop-final-tempo-profile.log`. Checksums respectively:
`1995978491398321` and `1856004344199951`; performance cursor 524,800 for both,
Arrangement cursors 20,800 and 44,800. These offline original-sine measurements
do not establish sustained native deadlines, plugin chains, UI response or
revision handoff. Normal desktop/development activity was not isolated, so no
speedup comparison follows from the percentile differences.

## Canonical gain-edit Session handoff — 9 October 2026

Command: `CARGO_INCREMENTAL=0 cargo run --release -p hex-engine --example
profile_session_handoff`. Initial log: artifacts/session-handoff-gain-profile.log.
Apple M5 Pro/macOS arm64, ordinary desktop/development activity. The shared
original fixture has 100 real instrument tracks, 50 Session and 50 Arrangement
routes, with four/eight voices and varied gain/balance. Playback reaches frame
24,000 while Session notes are held. An eight-entry canonical Editor performs
100 alternating 0.75/0.25 gain edits on the first track, preparing each exact
revision with the prior inventory and stable sine table before constructing the
candidate runtime. Each accepted handoff is followed by 64 rendered frames.

Only `try_inherit_state_from` is timed. Graph/edit preparation, construction,
independent finite compilation/render comparisons, checksum and final old-graph
drops are outside timing. Every post-cut block matches independent finite PCM,
and transfer leaves the previous runtime's cursors unchanged. No excluded warmup;
100 samples, nearest-rank percentiles/upper median. Initial p50/p95/p99/max:
2.750/3.791/5.000/5.375 µs. Checksum `27829135380449`; both final cursors 30,400.

This verifies and profiles the unchanged-source gain path. Source edits, tempo
edits, removed-track tails and capacity rollback remain separate acceptance
cases; this initial run does not establish the full handoff contract, physical
device deadlines, plugin chains or native publication/retirement. The benchmark
shares its canonical fixture/oracle with `profile_session_playback` under
`examples/support/session_fixture.rs`, without adding a second project model.

Repeating-tempo extension: `profile_session_handoff --loop-tempo` starts at frame
144,000, after the first 120,000-frame/three-beat tempo cycle and at a new Session
source occurrence. The effective tempo alternates 120/60 bpm; a later authored
240 bpm point stays outside the repeating segment. The same 100 gain edits and
independent finite PCM checks pass. Initial p50/p95/p99/max are
2.375/3.084/3.334/3.917 µs; checksum `27242448344792`, final performance frame
150,400 and Arrangement frame 30,400. Log:
artifacts/session-handoff-loop-tempo-profile.log. It profiles gain transfer on a
variable clock, not changing the tempo during a handoff.

After the control/capacity repairs, both gain workloads retain their exact PCM
checksums. Final constant p50/p95/p99/max: 2.833/3.875/5.000/5.125 µs;
repeating-tempo: 2.291/3.167/13.459/22.416 µs. Logs:
artifacts/session-handoff-gain-final-profile.log and
artifacts/session-handoff-loop-tempo-final-profile.log. The larger variable-tail
timings are observed values under normal concurrent desktop activity; this short
run cannot attribute them to DSP or establish a sustained deadline guarantee.

Post-handoff render regressions also retain both earlier independent PCM and
partition digests. Constant tempo p50/p95/p99/max: 55.750/98.291/107.959/200.625 µs;
repeating tempo: 52.708/97.750/108.625/182.958 µs. Both runs observe zero blocks
above the 128-frame period among 4,000 samples, with checksums
`1995978491398321` and `1856004344199951` respectively. Logs:
artifacts/session-handoff-baseline-constant-profile.log and
artifacts/session-handoff-baseline-tempo-profile.log. These are offline kernel
regressions; publication/retirement and physical stress remain unmeasured.

## Prepared Playback wrapper — 9 October 2026

The same canonical mixed 100-track workload now supports `--playback-wrapper`,
constructing `Playback::try_from_prepared_with_session` off-thread and selecting
the scene through its typed action API. Both whole and 31-frame partitions match
the independent finite compiler PCM before timing. This profiles the prepared
wrapper used by native integration, without a device or publication queue.

Commands: `CARGO_INCREMENTAL=0 cargo run --release -p hex-engine --example
profile_session_playback -- --playback-wrapper`, optionally `--loop-tempo`.
Apple M5 Pro/macOS arm64 under ordinary desktop/development activity, 100 warmup
and 4,000 timed 128-frame blocks at 48 kHz. Setup, oracle and checksums are outside
timing; the benchmark dispatches its selected runtime once per block.

| Prepared wrapper workload | p50 µs | p95 µs | p99 µs | Max µs | Blocks above period |
| --- | ---: | ---: | ---: | ---: | ---: |
| Constant tempo | 55.875 | 99.250 | 110.125 | 264.500 | 0 / 4,000 |
| Repeating tempo | 53.041 | 96.833 | 106.208 | 189.667 | 0 / 4,000 |

Checksums remain `1995978491398321` and `1856004344199951`; final performance
frame is 524,800 while Arrangement wraps to 20,800 / 44,800 respectively. Logs:
artifacts/native-wrapper-constant-final-profile.log and
native-wrapper-tempo-final-profile.log. These reruns follow native publication
source acceptance and legacy clock promotion validation; earlier initial
wrapper logs remain available separately.
Native publication/action queues, graph handoff/reclamation, allocator auditing,
UI load, plugin chains and physical sustained deadlines remain separate gates.
This wrapper run does not establish complete native device acceptance.

Post-telemetry source rerun — 9 October: after committed route-generation
tracking was integrated, the same wrapper workloads still match the independent
finite PCM and partition oracles. Constant tempo reports p50/p95/p99/max
55.542/99.750/110.916/225.917 µs; repeating tempo reports
53.292/99.416/108.375/207.541 µs. Both record zero above-period samples out of
4,000 at 48 kHz/128 frames, with unchanged checksums
1995978491398321 / 1856004344199951. Logs:
`artifacts/session-telemetry-wrapper-constant-profile.log` and
`artifacts/session-telemetry-wrapper-tempo-profile.log`. These render-only
measurements include the new kernel stamp bookkeeping, but exclude atomic
callback snapshot publication, GPUI, devices and hardware scheduling. They do
not resolve the separate route-control outlier below.

## Prepared callback route snapshots — 9 October 2026

`CARGO_INCREMENTAL=0 cargo run --release -p hex-audio --example
profile_session_routes -- 100` (or `256`) builds a real canonical project with
one authored Session note per instrument track, then exercises the prepared
callback directly. The same commands drive an independent headless runtime;
every block compares PCM and route identities outside the timers. This is a
callback-control seam benchmark, without CPAL or a physical stream, not a
standalone atomic-copy or full callback-render timing.

Apple M5 Pro/macOS arm64, 48 kHz, 128-frame render blocks, 100 warmup and 4,000
timed samples per path under ordinary development activity. Changed control
samples alternate immediate scene launch/StopAll and include route validation,
voice capture and snapshot publication. Render, enqueue, verification and
allocation remain outside timing. Snapshot readers use caller-owned fixed
storage. Unchanged begin-block calls use 64-call batches, reported per call,
because single-call timing is below the platform timer resolution.

| Tracks / path | p50 µs | p95 µs | p99 µs | Max µs | Above 2.667 ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| 100 / unchanged control entry | 0.005 | 0.008 | 0.010 | 0.188 | 0 / 4,000 |
| 100 / changed control entry | 50.292 | 62.417 | 110.208 | 181.792 | 0 / 4,000 |
| 100 / snapshot read | 0.291 | 0.375 | 0.459 | 6.042 | 0 / 4,000 |
| 256 / unchanged control entry | 0.005 | 0.005 | 0.007 | 0.289 | 0 / 4,000 |
| 256 / changed control entry | 60.917 | 419.250 | 450.791 | 544.041 | 0 / 4,000 |
| 256 / snapshot read | 0.625 | 0.750 | 0.916 | 14.667 | 0 / 4,000 |

Final logs: artifacts/session-routes-100-counted-profile.log and
session-routes-256-counted-profile.log; route checksums
`12837325117716804784` / `15291336853823228928`. Earlier uncounted runs remain
in the 100-final and 256 profile logs. The earlier 256 run had a **12,372.125 µs**
maximum changed-control sample (p99 635.541 µs). Preserve that outlier: the later
green run does not establish deadlines or explain the earlier delay. Scheduler
interruption versus processing cost is unresolved; concurrent producer/reader,
allocator auditing, device stress, UI load and plugin chains remain open gates.

Independent 256-track follow-up (`artifacts/session-routes-256-followup-profile.log`)
retains the same exact route/PCM checksum. Changed control p50/p95/p99/max is
137.792/423.541/532.000/734.750 µs, with zero above-period samples out of 4,000;
snapshot read is 0.625/0.750/1.000/10.833 µs. Batched unchanged entry is
0.005/0.006/0.007/0.158 µs. This second counted run still does not reproduce or
explain the earlier 12.372 ms delay; the unresolved gate remains open.

Ordered-Stop integration rerun: the current captured-prefix callback path passes
on the verified Apple M5 Pro host (15 physical CPU cores, macOS 26.6.2, arm64)
the same PCM/route oracles and checksums at both 100 and 256 tracks. Changed
control p50/p95/p99/max is 44.167/58.792/65.084/186.458 µs (100) and
62.250/425.250/461.625/611.875 µs (256), with zero above-period samples in
each 4,000-sample series. Snapshot read p99/max is 0.667/15.958 µs and
0.875/11.250 µs; batched unchanged entry p99/max is 0.011/0.125 µs and
0.008/0.114 µs. Logs: `artifacts/ordered-stop-routes-100-profile.log` and
`artifacts/ordered-stop-routes-256-profile.log`. This verifies the benchmark on
the refined callback source, not physical scheduling or resolution of the
historical outlier. Actual ordered-Stop behavior is covered by the separate
callback regressions, not the alternating scene/StopAll benchmark workload.

The route table payload uses 16,384 bytes (256 × 64) plus a 40-byte atomic
header; owner/queue objects and allocator metadata are additional memory.
A maximum snapshot update writes 2,567 atomic fields. Unit instrumentation
verifies no slot visits/writes on unchanged control/render snapshot publication;
engine due-route commits accumulate a local changed flag without an extra
comparison scan. Callback no-allocation remains source inspection, not allocator
instrumentation. Batched control-entry averages are not per-callback deadlines.

## Additive ongoing MIDI observation — 8 October 2026

The extended release
`profile_midi_spans` first checks a scaled 64-voice ongoing source into a nonzero
destination against independent finite note scheduling. It then times 100
prepared replicas with varied gain/balance, each holding 64 voices, at the same
64-frame window starting at frame 3,000, 48 kHz. Setup, destination clearing and
checksums are outside the timer; ten aggregate warmups precede 100 samples.
Observed p50/p95/p99/max are 1029.458/1125.542/1214.291/1484.625 µs, with
one sample above the 1,333.333 µs block period. Checksum: `24975204808600`.
The unchanged legacy overwritten-render sections also execute in this run.
Log: `artifacts/midi-additive-profile.log`. This short fixed-held-window check
is not a canonical 100-track workload, callback allocation audit or sustained
deadline proof; dense sources need aggregate admission and measured device gates.

## Borrowed track-render regression observation — 8 October 2026

`CARGO_INCREMENTAL=0 cargo run --release -p hex-engine --example
profile_arrangement -- --active` after the track/mix extraction retains the exact
independent scalar match and historical digest `a7db7404bf70aac8`. The existing
48 kHz, 100-track/10,000-clip, 128-frame, 40-iteration fixture reports scalar
p50/p95/max 934.38/1309.33/1483.50 µs and block-span
28.12/39.75/41.04 µs. Log: `artifacts/track-mix-active-profile.log`.
This run includes normal development activity and is not a controlled paired
before/after speed comparison. It verifies this fixture's PCM, not sustained
CoreAudio timing, mixed Session routing, MIDI density or device-chain budgets.

## Control-side source conversion observation — 8 October 2026

`cargo run --release -p hex-media --example profile_resample` on Apple M5 Pro arm64,
one-second generated stereo input, two warmups/eight timed conversions, digest
outside timing: 44.1→48 kHz median 4.945 ms/max 5.666; 48→44.1 median 3.993/max
4.287; 48→16 median 4.173/max 4.264. With eight samples nearest-rank p95 equals
max. This is a bounded single-run conversion microbenchmark, excluding disk I/O,
waveform preparation and whole-app queue latency; no comparative or realtime
claim follows. Filter identity/version and limits are in hex-media README.

## Canonical loop profile — 8 October 2026

Run `cargo run --release -p hex-engine --example profile_loop`. Apple M5 Pro,
arm64; 100 tracks × 100 clips, 100 active sustained clips and 9900 later sparse
clips, 48 kHz / 128 frames, 40 warmups and 200 timed blocks. Position resets before
each block outside timing. Timed section is only prepared Playback::render;
source setup, seeks and output digest are outside it. No audio device. The 1024
frame region starts at zero and does not wrap within the measured 128-frame block.

| Region | Initial median / p95 / max µs | Repaired median / p95 / max µs | Output digest |
| --- | --- | --- | --- |
| disabled | 18.92 / 34.58 / 104.33 | 17.04 / 22.71 / 79.12 | 9b1054eca0514125 |
| enabled, 1024 frames | 18.83 / 36.17 / 104.42 | 16.71 / 19.92 / 95.54 | 9b1054eca0514125 |
| enabled, 1 frame | 1054.17 / 1325.83 / 1464.29 | 6.58 / 7.38 / 17.75 | 011daa8529897725 |

Initial short-loop playback rescanned clips for every period. The repair renders
a full period once when it fits in the output, then repeats exact mixed frames
using bounded caller-buffer copies. Non-power-of-two periods, intro, mid-phase
seek, partial cycles and varied blocks retain exact output in tests. This relies
on the current stateless audio-source renderer; future stateful DSP/MIDI cannot
reuse rendered periods without their own reset/advance contract. These short CPU
profiles establish neither sustained callback deadlines nor full-device scalability.

## Native export correctness check — 8 October 2026

A fresh development bundle exported revision 1 to
`artifacts/native-export-check.wav`: 507,150 frames, stereo PCM16 at 44.1 kHz.
The file matched `artifacts/native-loop-linear-render.wav` byte-for-byte, SHA-256
`952c8d29e303b10942d47300381b87b074d75828077e1b759f744b29a2bba681`. This is a
single-fixture output-continuity check, not a throughput benchmark, listening
acceptance, or real-time deadline measurement. Native export runs off the UI and
audio callback; no export-duration or UI-responsiveness timing was recorded.

## Native idle redraw repair — 8 October 2026

A muted 100-track/10,000-clip fixture was selected during native inspection; UI
calls timed out and process CPU was approximately 98.6%. The sample localized
474/565 main-thread samples to UniformList painting. Its stack does not confirm
the loaded project, so these values are regression evidence, not a reproducible
dense-project benchmark. Source unconditionally notified GPUI every 16 ms.
Conditional invalidation now keeps bounded polls but redraws only visible changes,
including playing transport, paused seeks and acknowledgment. Idle callback-time
diagnostics refresh at most once per second. Five pure policy tests pass; the
fresh empty-project process was observed around 1.4% CPU. Native dense acceptance
was interrupted during active app use and remains open. Do not present the two
CPU snapshots as an equivalent-workload speedup or sustained timing measurement.

## Control-side compile profile — 8 October 2026

Run `cargo run --release -p hex-engine --example profile_compile`. Apple M5 Pro,
arm64 macOS; 100 tracks × 100 clips, 100 sustained active clips plus 9,900 sparse,
one shared finite 64-frame stereo source at 48 kHz, 128-frame render verification,
32-step history, 10 warmup and 200 measured samples. Each edit sample executes a
Gain plus MoveClip pair; compilation is timed separately using Instant. Fixture
construction and source generation are excluded. The compiled render matched an
independent fixture reference exactly (digest c437c65c105b9dbd).

| Component | Median µs | p95 µs | p99 µs | Max µs |
| --- | --- | --- | --- | --- |
| Canonical edit pair | 47.21 | 139.17 | 231.46 | 284.58 |
| Control-side compile | 71.29 | 133.54 | 244.38 | 261.42 |

This is control-side component timing only. It excludes media decoding, worker
queue latency, GPUI acknowledgment, graph publication and physical callback
performance, and does not establish the full dense-project responsiveness gate.

## Mixer neutral-path review — 8 October 2026

The first balance/master implementation added a second per-channel multiplication
inside every clip span and a master pass even at unity. Review moved combined
track-gain/balance coefficients into control-side preparation and skips the final
master pass at unity. Non-neutral master remains post-sum. No callback allocations
or extra buffers were added; this is source inspection, not allocator instrumentation.

A paired `cargo run --release -p hex-engine --example profile_loop` on Apple M5 Pro,
macOS 26.6.2, arm64, 100 tracks × 100 clips, 48 kHz, 128-frame blocks, 40 warmup and
200 measured renders produced these microbenchmark results. Both runs use neutral
balance/master, no device or GPUI timings:

| Workload | Before median/p95/max µs | After median/p95/max µs |
| --- | --- | --- |
| Loop disabled | 36.92 / 51.46 / 147.33 | 31.12 / 36.12 / 38.71 |
| 1024-frame loop | 24.54 / 31.96 / 102.88 | 27.17 / 33.62 / 42.46 |
| One-frame loop | 12.42 / 12.71 / 38.33 | 10.92 / 11.38 / 17.50 |

The mixed median results do not establish a general performance improvement;
background load/thermal variability was not controlled. Output digests remain
`9b1054eca0514125` for disabled/1024 and `011daa8529897725` for one-frame loops.
Physical deadline and dense native acceptance remain open.

Separately, migrating and rendering the prior schema-4 native loop fixture with
center/unity defaults reproduced the earlier 507,150-frame PCM16 WAV byte-for-byte:
SHA-256 `952c8d29e303b10942d47300381b87b074d75828077e1b759f744b29a2bba681`.
This is neutral render continuity, not Live pan-law or listening acceptance.

## Source-phase trim profile — 8 October 2026

Back-to-back preserved pre-phase and current release executables used the existing
100-track × 100-clip fixture: 100 active, 9,900 sparse, 48 kHz, 128-frame blocks,
40 warmup and 200 measured iterations. Apple M5 Pro/macOS 26.6.2; background load
and power were uncontrolled. Logs: `artifacts/profiles/trim-before.log` and
`trim-neutral.log`.

| Project loop | Before median / p95 / max µs | After median / p95 / max µs |
| --- | --- | --- |
| disabled | 18.21 / 44.21 / 105.96 | 21.58 / 48.75 / 152.83 |
| 1,024 frames | 18.21 / 39.04 / 1,700.25 | 24.92 / 65.58 / 490.38 |
| one frame | 7.62 / 9.92 / 33.08 | 7.54 / 16.42 / 45.33 |

All output digests match the preserved executable. The 507,150-frame legacy
44.1 kHz CLI export remains SHA-256
`952c8d29e303b10942d47300381b87b074d75828077e1b759f744b29a2bba681`.
Phase-zero loops retain a single modulo; one-shots index directly. Nonzero phase
uses bounded u64 wrap addition without wide division. The phase field enlarges
compiled clip metadata. Higher medians and variable tails remain an investigation
item; these short noisy runs cannot establish a stride-caused regression or
sustained device performance. Reversed/repeated paired profiles and measured
nonzero-phase workloads are required before performance acceptance.

A reversed-order pair reproduced the direction in the first two medians:
current 21.00/21.08/6.83 µs versus preserved 18.62/18.71/7.46 µs; digests remain
identical. This warrants a focused compiled-metadata layout investigation, without
attributing all timing differences to phase. The added `--phase` benchmark flag
sets phase 17 in each active 64-frame loop (also combinable with `--fades`). Its
medians were 25.54/21.50/7.46 µs, p95 43.29/47.96/17.04 µs and maxima
233.50/114.67/37.71 µs. Digests were `afb57d7684830125` for disabled/1,024-frame
project loops and `c31596df7d4ff725` for one-frame loops. Logs are
`trim-reversed-{before,after}.log` and `trim-phase.log`. These are bounded offline
render workloads, not plugin-chain, device or 100-track native acceptance.

## Compact compiled metadata — 8 October 2026

Source phase now shares optional boxed metadata with a non-neutral fade. Neutral
clips use no metadata allocation. The current `size_of::<Clip>()` is 72 bytes;
a reconstruction of the exact prior field types/order measured 80 bytes on the
same target (`artifacts/profiles/layout-stride.log`). Thus 10,000 neutral compiled
clips use 80,000 fewer inline bytes, excluding vector capacity and media. Active
phase/fade clips have a control-owned metadata allocation; this tradeoff is not
a total-memory reduction for every workload. No allocation/reclamation occurs in
the render path; the prepared-graph ownership policy is unchanged.

Four short paired runs per workload alternated before→after and after→before
order using the preserved inline-phase executable and final release executable.
Workload/rate/blocks/warmup remain as above. The table gives the median of the
four per-run medians in µs, ordered disabled / 1,024-frame / one-frame project loop.

| Active clip state | Inline phase | Compact metadata |
| --- | --- | --- |
| neutral | 20.75 / 20.79 / 7.62 | 19.81 / 19.12 / 7.29 |
| phase 17 | 22.31 / 21.77 / 7.33 | 21.12 / 21.14 / 7.23 |
| fades | 27.60 / 27.44 / 7.06 | 27.23 / 27.25 / 7.17 |
| fades + phase 17 | 30.25 / 29.84 / 7.08 | 30.48 / 30.23 / 7.13 |

All paired digests are identical. Raw medians/p95/max/digests and alternating
order are retained in `artifacts/profiles/layout-pairs.json` and corresponding
`layout-*.log` files. Background load/power remain uncontrolled; these comparisons
support the compact common-case direction, not statistical performance certainty.
Combined fade/phase results are slightly higher in this run and stay a follow-up
item. The 507,150-frame legacy CLI export remains byte-identical at the SHA above.
228 workspace tests, strict Clippy, guards/fmt and rebuilt desktop bundle pass.
New regression: repeatedly changing and resetting phase retains an active fade
and restores independently expected source samples. Sustained device, long dense
editing and plugin chains remain separate acceptance gates.

## Original instrument baseline — 8 October 2026

`cargo run --release -p hex-engine --example profile_instrument` measures the
shared Playback renderer on Apple M5 Pro, 15 logical CPUs, 24 GiB RAM. Fixture:
100 unmuted instrument tracks, 16 original sine notes per track, 120 bpm, 48 kHz
stereo, gates two quarters long with onsets four quarters apart; pitches 48–83,
velocity100, gain/master1. This alternates 100 simultaneous tones with silent
gaps; it is not 64 voices on each track or a plugin/device-chain stress test.

Each block size runs 100 warm-up blocks and 3,000 measured blocks. Instant
measures each render and total elapsed time; output is passed to black_box.
No callback/device, UI interaction, allocation instrumentation or listening is
measured. Latest primary run in artifacts/profiles/synth-100track-release.log:

| Block frames | Measured frames | Total elapsed | Maximum block | Nominal audio interval |
| --- | --- | --- | --- | --- |
| 128 | 384,000 | 0.032 s | 266.8 µs | 2,666.7 µs |
| 512 | 1,536,000 | 0.103 s | 332.8 µs | 10,666.7 µs |

Earlier runs used the flawed interval query that omitted early notes; those
numbers are invalid for this workload. This table uses the repaired renderer.
Sustained timing distributions,
dense per-track voices, long MIDI histories and device deadline stress remain
required. Source inspection confirms prepared table/steps, fixed traversal stack
and no render allocation/ownership changes; it is not allocator instrumentation.

Legacy schema4 audio still exports 507,150 stereo44.1 kHz frames with SHA-256
952c8d29e303b10942d47300381b87b074d75828077e1b759f744b29a2bba681.
The isolated four-note44.1 kHz native export matches the CLI render byte-for-byte
(176,400 frames; SHA-256eea5ecf8cb5d996ce02afa155fb77b194a0dae8a33c340babdd53fef4de2c242).
Neither comparison proves Live parity or audible/device stress acceptance.

## MIDI control-side editing baseline — 8 October 2026

`cargo run --release -p hex-session --example profile_midi_edits` runs on the same
Apple M5 Pro/15 logical CPU/24 GiB machine. It prepares 100 instrument tracks
with64 notes each, one256-quarter clip per track, 48 kHz,32 history slots, no
audio assets. Notes are spaced four quarters apart with one-quarter gates,
pitches48–83, velocity100, trackgain0.1 and master1. One warm-up compile precedes
20 real insert/remove pairs. Each Instant interval includes the canonical edit,
snapshot history, prepared graph validation and disposal on the control side;
there is no device callback or GPUI input/paint measurement.

The40 transaction sample recorded in artifacts/profiles/midi-edit-100track-release.log:
median208 µs,nearest-rank95th percentile344 µs, maximum639 µs. One subsequent
track-gain edit and compile took223 µs. These short-run results do not prove
large history/save capacity, all-core utilization, native interaction latency or
sustained device deadlines. No SIMD or multicore change is justified by this
baseline alone. Keep the example as a reproducible control-side workload.


## Dense MIDI scene measurement — active

Host rechecked for this unit: Apple M5 Pro, arm64, macOS 26.6.2. Measure release
scene construction/cache reuse and row-bucket hit/cycle resolution at 600 and
65,536 canonical notes. These are control/UI geometry costs; do not describe them
as GPU paint, input-to-display latency, device deadlines or Live comparison.
Native 600-note interaction and minimum-window layout remain separate gates.

Release command: `cargo test -p hex-ui --release benchmark_dense_scene_build_and_hit -- --ignored --nocapture`.
The diagnostic fixture spans 4 beats for 600 notes and 256 beats for 65,536 notes.
The single-run log `artifacts/profiles/midi-scene-benchmark-20261008.log` reports:

| Visible notes | Scene build | 100 row-index hits |
| --- | --- | --- |
| 600 | 25.9 µs | 2.6 µs |
| 65,536 | 1,059.6 µs | 219.8 µs |

This times one construction and 100 repeated queries at a fixed position; it is
not a distribution or worst-case overlap measurement. Cache reuse/invalidation
is unit-tested separately. Native 600-note painting and late-note edit/save now
pass after repairing Canvas sizing; GPU/input-to-display timing remains unmeasured.

## prepared MIDI thumbnail work bound — 8 October 2026

Source inspection: compilation emits at most 32 deterministic gate samples per
clip during admitted finite occurrence traversal. A sorted immutable clip index
is published with project/graph revision; UI lookup and viewport cropping use
those samples with at most 128 note shapes per track row. Painting does not scan
authored notes or expand loops. Visible clip containers remain separately drawn;
this does not establish globally bounded row children for overlapping clips.
Source tests and native sparse/cropped preview acceptance pass; no measured
input-to-paint deadline or dense/minimum-layout claim follows from these bounds.

## Prepared tempo conversion — 8 October 2026

Command: `CARGO_INCREMENTAL=0 cargo run --release -p hex-engine --example
profile_tempo_conversion`. Logs: `artifacts/prepared-tempo-profile.log`,
`artifacts/prepared-tempo-tests.log`, `artifacts/prepared-tempo-clippy.log`.
Host: Apple M5 Pro, macOS arm64 (`uname -m` and CPU brand query).
The 1024-segment map and query arrays are constructed before timing. After eight
warmup traversals, 200 passes convert 819,200 event beats and 409,600 block frames:
1,228,800 total calls in 43.689 ms, 35.6 ns/call, 28,125,966 calls/s; checksum
15572411654917454540. Timing includes iteration, checked result handling, black-box
inputs and checksum overhead. This is one mixed-query throughput measurement,
not a worst-case callback deadline, sustained device load, no-allocation instrument
result or cross-CPU comparison. Calls borrow immutable canonical data; graph
construction and retirement remain control-side responsibilities.

## Prepared MIDI source edge query — 8 October 2026

Apple M5 Pro, macOS arm64. Command: `CARGO_INCREMENTAL=0 cargo run --release
-p hex-engine --example profile_midi_template`. Project/source/template and
fixed buffer preparation are outside timing, followed by 100 excluded warmup
queries. A dense 128-note one-shot source returns 255 edges in each 256-frame query.
10,000 measured queries (2,550,000 actual returned edges, 2,560,000 capacity bound)
take 32.933 ms, 3,293.3 ns/query; checksum 655350000. Timing includes query/checksum
iteration and black-box buffer consumption. Log: artifacts/midi-template-profile.log.
This is one dense one-shot query microbenchmark; it does not measure the repeat
path's worst case, many tracks, active-voice DSP, plugins, GPUI, callback deadline
tails or allocation instrumentation. Source inspection separates control-only
allocation in admission from borrowed fixed-storage queries.

Acceptance logs: artifacts/midi-template-query-tests.log (8 tests),
artifacts/midi-template-density-tests.log (3 independent oracle tests),
artifacts/midi-template-clippy.log and artifacts/midi-template-desktop-build.log.
Full engine suite passed 63 tests through the executor; final root targeted tests
and desktop build pass.

## Prepared ongoing instrument render — 8 October 2026

Apple M5 Pro, macOS arm64. Command: `CARGO_INCREMENTAL=0 cargo run --release
-p hex-engine --example profile_midi_spans`. Log: artifacts/midi-spans-profile.log.
The fixture holds 64 simultaneous sine notes starting at frame 0 with four-beat
gates. Queries start at frame 3000 (62.5 ms) and render 64 frames at 48 kHz.
Project/maps/source indexes, 100 templates, the sine table and fixed buffers are
prepared before timing; 100 single-source warmups are excluded.

1,000 single-source renders (64 spans each, 4,096,000 voice-sample operations)
take 8.554 ms, 8,554.3 ns/render. Then 100 serial blocks of 100 sources
(6,400 voices/block, 10,000 source queries, 40,960,000 voice-sample operations)
take 81.521 ms, 8,152.1 ns/source query or 0.81521 ms/aggregate block on average.
The aggregate loop includes source scratch rendering and stereo accumulation;
checksum/black-box consumption is included. Checksum: 6298016000.

A 64-frame/48 kHz device block lasts 1.333 ms, but these elapsed averages do not
establish sustained callback deadlines, tail percentiles or device acceptance.
The fixture is held-note one-shot DSP, not a repeat-query worst case, plugin
chains, heterogeneous projects or UI interaction. No allocation instrumentation,
graph-wide admission or multithreaded scheduling is claimed. Existing playback
does not yet use this ongoing instrument API.

Acceptance logs: artifacts/midi-spans-query-tests.log (7 tests),
artifacts/midi-spans-density-tests.log (3 tests),
artifacts/midi-spans-desktop-build.log. Executor full engine tests pass 74 tests;
strict all-target engine Clippy/fmt and root architecture/style guards pass.

## Serial aggregate kernel distributions — 8 October 2026

Command: `CARGO_INCREMENTAL=0 cargo run --release -p hex-engine --example
profile_midi_aggregate`. Logs: artifacts/midi-aggregate-profile.log and
artifacts/midi-aggregate-clippy.log. Host: Apple M5 Pro, macOS arm64. Three
workloads each measure 10,000 blocks of 64 frames at 48 kHz, mixing 100 prepared
copies of one source into caller-owned scratch. These are repeated templates,
not a canonical 100-track graph or device callbacks. Project/source preparation,
buffers and 100 per-workload warmups are excluded. Per-block Instant timing
includes queries, DSP and stereo scratch accumulation; checksum, sample
collection and final sorting are outside each block timer. Normal desktop and
concurrent development activity were not isolated.

| Workload | Spans/block | p50 µs | p95 µs | p99 µs | Max µs | Blocks above 1.333 ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Four-voice loop, including quiet windows | 0–400 | 51.292 | 79.666 | 128.625 | 646.250 | 0 / 10,000 |
| 64 held/releasing voices per source | 6,400 | 769.250 | 1,207.667 | 1,450.667 | 2,297.667 | 225 / 10,000 |
| 32-voice loop, including release seams | 3,200–6,400 | 758.333 | 1,151.292 | 1,415.041 | 2,501.708 | 178 / 10,000 |

The four-voice source has two-beat gates in a four-beat loop. The held source
has four-beat gates with windows across sustain and release. The dense loop
has four-beat gates and a source start marker at beat 2: no intro onsets, then
32 voices per repeat and 64 during release overlap. Fixed windows include
gate boundaries, successive loop boundaries and a later seek; this is not
continuous transport or the worst interval-tree traversal. Aggregate wall times
are 521.876 / 8,294.142 / 6,874.091 ms; checksums are
16505033972808 / 19917604994896 / 19761779344704 respectively.

The dense kernel exceeded the nominal block period despite its lower average.
Do not derive a safe 100×64-voice callback admission from these averages. The
results justify sustained deadline distributions and explicit aggregate work
admission, while leaving device scheduling, plugins, publication/retirement,
allocation instrumentation, heterogeneous sources and native interaction open.
The conservative source-start-0 32-note loop refusal is tracked in issues.md.

## Source-start-0 first-tail aggregate rerun — 8 October 2026

With the admission repair, profile_midi_aggregate now uses source start 0 for the
32-voice loop, including its real intro/first-tail overlap. Same command, hardware,
64-frame/48 kHz, 100 prepared copies and 10,000-block methodology as above; log:
artifacts/midi-aggregate-first-tail-profile.log. Normal desktop and concurrent
development activity remain present.

| Workload | Spans/block | p50 µs | p95 µs | p99 µs | Max µs | Blocks above 1.333 ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Four-voice loop, including quiet windows | 0–400 | 50.916 | 86.584 | 155.334 | 1,284.209 | 0 / 10,000 |
| 64 held/releasing voices per source | 6,400 | 812.750 | 1,691.416 | 2,261.041 | 3,897.875 | 1,376 / 10,000 |
| Source-start-0 32-voice loop/release seams | 3,200–6,400 | 792.042 | 1,610.792 | 3,920.000 | 781,659.667 | 774 / 10,000 |

Wall times: 536.219 / 9,729.767 / 12,368.246 ms. Checksums remain
16505033972808 / 19917604994896 / 19761779344704. The approximately 0.782-second
maximum is elapsed wall time, including possible OS descheduling; this experiment
cannot identify its cause or establish a DSP stall. Dense-source percentiles and
missed nominal periods still rule out a physical admission claim. These remain
serial repeated-source kernels, not a canonical 100-track graph, plugin-chain
benchmark, isolated real-time callback or sustained device deadline acceptance.

## Canonical Session inventory/control edits — 8 October 2026

Command: `CARGO_INCREMENTAL=0 cargo run --release -p hex-engine --example
profile_session_inventory`. Log: artifacts/session-inventory-profile.log.
Targeted example strict Clippy passes (session-inventory-profile-clippy.log).
Apple M5 Pro/macOS arm64; normal desktop/development activity was not isolated.

The original fixture is an actual canonical Project: 100 instrument tracks, three
scenes, 200 independently owned looping MIDI cells, 50 explicit stops, 50 absent
slots and 2,000 notes at 48 kHz. Cells hold varied 4/8/12/16-note chords, two-beat
gates in four-beat loops, varied pitches/velocities/pan. Eight history slots.
Fixture construction and stable sine-kernel construction are outside timing.
Each workload records 100 samples with nearest-rank percentiles (upper median);
cold preparation has one excluded warmup. Final artifact destruction and template
identity checks are outside each timer.

| Control workload | p50 µs | p95 µs | p99 µs | Max µs |
| --- | ---: | ---: | ---: | ---: |
| Cold inventory preparation | 326.833 | 433.584 | 485.000 | 497.667 |
| Canonical gain transaction and reused inventory | 19.792 | 34.250 | 56.750 | 80.250 |
| Canonical note insert/remove and inventory | 21.500 | 25.166 | 27.000 | 33.291 |

Measured source-template identity checks pass: gain edits retain 200/200 sources;
note insertions/removals retain 199/200 and rebuild the changed source. Each edit
includes the Editor transaction/history and inventory preparation. Reachable
structural envelope sums each track's maximum across its cells: 1,300 active
voices, 1,300 onsets, 2,600 span hits; guarded traversal/window ceilings total
1,638,400 / 32,600. These ceilings are not measured work or a CPU admission limit.

This benchmark excludes Arrangement graph compilation, exact PreparedRevision
worker integration, queues, graph retirement, UI layout/interactions, rendering
and device callbacks. It supports control-side source reuse at this workload;
it does not establish end-to-end edit latency, many-plugin behavior, mixed-route
physical deadlines or Live performance parity. Source integration acceptance
remains separate while the worker artifact is being wired.

Full-revision extension: the same example now also executes gain changes through
Editor::execute_prepared, producing Arrangement graph, Session inventory and
preview in the candidate closure. The fixture's 100 Arrangement tracks are empty;
all 2,000 notes belong to Session. This measures the real artifact preparation
contract, not a full-song Arrangement edit or the worker event loop.

Rerun log: artifacts/session-inventory-full-revision-profile.log. The three prior
workloads have p50/p95/p99/max (µs) of 349.584/476.625/481.625/481.875,
19.833/24.166/28.583/45.041 and 21.375/23.791/24.625/28.458 respectively.
The added full-revision gain workload records 20.834/29.458/35.083/35.792 µs
across 100 edits; all 200 source-template identities remain reused. One excluded
full-revision preparation precedes that series; artifact destruction and reuse
checks remain outside timers. Targeted strict example Clippy passes in
artifacts/session-inventory-full-revision-clippy.log. UI, worker queues, DSP,
physical callbacks and mixed Arrangement/Session deadlines remain unmeasured.

## Session audio acceptance workload — planned 9 October 2026

Session audio is being implemented under docs/session-audio.md. No audio-cell
timings are measured yet. Extend the canonical prepared/mixed workload with 100
audio tracks at 48 kHz and 128-frame blocks, two independent Session one-shots
per track, an Arrangement companion and an explicit Stop scene. Use original
bounded stereo sources with an independent frame-slice PCM oracle. Run one
shared-asset fixture to verify reuse, then distinct per-track assets to expose
media/cache costs; report source lengths, unique bytes and resident totals.

Time cold preparation, unchanged-source gain edits, one changed source window,
scene launch/retrigger, route changes and ongoing render separately. Fixture and
PCM construction, oracle comparisons, hashing and final artifact destruction stay
outside timers; allocation inherent in timed preparation remains included. Include active one-shots and exhausted sources rather than
timing silence alone. Verify output outside timing across irregular partitions,
tempo edits and graph handoff. Record p50/p95/p99/max and counts above the nominal
2.667 ms block period with the exact command, hardware and background workload.

Then measure native worker-to-visible edit response and physical callbacks under
the same active load. Headless timing cannot prove native responsiveness, device
deadlines, plugin capacity or comparative Live performance. Retain any outliers;
later quiet runs do not establish their cause or erase them.

The current native total resident cap is 8,000,000 converted frames. Sharing an
asset must count its PCM once, while 100 distinct sources must fit within that
total for this bounded fixture. At 48 kHz this is roughly 166.7 seconds of unique
stereo material across the project, not per track. Long multitrack projects still
require the planned streaming/media-budget architecture; short shared-source
benchmarks must not be presented as acceptance of unrestricted 100-track songs.

Include sparse-grid size as a separate control variable. Before scene indexing,
LaunchScene scanned the full prepared cell list and resolved track slots, so two
short scenes do not characterize a project with hundreds of scenes. Compare
the small grid with a valid near-limit grid (up to 65,536 total cells, at most
16,384 audio/MIDI clips combined, remaining cells explicit Stop). Shared short
media keeps this within the resident budget. Measure single scene launch and
the bounded admitted action burst separately. An allocation-free bounded scan
does not by itself establish a deadline guarantee. If cell-count scaling breaks
the budget, prepare a scene index on the control side rather than add callback
allocation or relax the requirement. Dense-grid baseline and accepted indexed
measurements are recorded below.

Historical fixture review checkpoint: the original stereo signal and scalar
frame-slice oracle were implemented in examples/support/session_audio_fixture.rs
before executable timers and release results. The later measured checkpoints follow. The proposed 65,536-frame assets
use 524,288 bytes shared or 52,428,800 bytes across 100 distinct assets, within
the existing 8,000,000-frame cap. Each Session scene selects only 4,096 frames
per track: its distinct-asset active PCM region is 3,276,800 bytes. Resident size
therefore does not equal the active cache working set. Report both and avoid
claiming full-song/cache-stress acceptance from this fixture.

Active render samples should advance through the whole source interval, with
retrigger outside the timer only when needed, rather than repeatedly measuring
its first 128 frames. Oracle arguments must represent elapsed source frames
since the launch anchor, including relaunches at nonzero performance frames.
The Arrangement companion oracle is deliberately scoped to this fixture's
neutral fades and zero source phase; it is not a generic alternate renderer.

### First runnable checkpoint — 9 October, provisional

The fixture's private-ID construction and inferred-index Clippy errors are
repaired. Two root 100-track integration regressions pass for shared and distinct
PCM: exact source slices across irregular partitions and exhaustion, retained
Session owners, scene relaunch at a nonzero performance frame, tempo-edit runtime
handoff, Stop cells and return to Arrangement. Prepared cells retain the resident
PCM pointers. Logs: `artifacts/session-audio-large-root-{tests,clippy}.log`.
These are headless correctness checks, not timing or physical acceptance.

First shared release run: `cargo run --release -p hex-engine --example
profile_session_audio`. Retained log:
`artifacts/session-audio-profile-shared-first-checkpoint.log`. Apple M5 Pro,
15 cores, 24 GiB, macOS 26.6.2/arm64; Codex and two existing Hex app processes
were present, with no workload isolation or confirmed playback state.

| Workload | Samples | p50 µs | p95 µs | p99 µs | Max µs |
| --- | ---: | ---: | ---: | ---: | ---: |
| Cold prepared revision | 100 | 8.375 | 8.583 | 12.833 | 13.542 |
| Active Session audio render | 4,000 | 9.458 | 10.167 | 12.666 | 92.208 |
| Exhausted Session audio render | 4,000 | 6.500 | 8.375 | 8.959 | 52.459 |

Cold preparation excludes five warmups; render excludes 100. Active rendering
advances across the 4,096-frame region and retriggers only at exhaustion outside
the timer. PCM/oracle comparisons, hashes and final drops stay outside timers.
The render series have zero samples above the nominal 128-frame/48-kHz period.
Keep the maximum outliers; this short run does not prove sustained deadlines.

This first executable embedded host labels and reported the shared region's
logical per-track bytes without distinguishing unique active PCM. Those reporting
limitations are repaired in source: the shared active region is 32,768 unique
bytes, versus 3,276,800 logical bytes summed across tracks. The portable executable
now leaves host facts to external run metadata. Gain/source-edit preparation,
route/scene command timings and final distinct-mode release results are pending,
so the complete planned workload is not accepted yet. Initial source/binary hashes
were not captured before subsequent reporting edits; retain this as a preliminary
checkpoint, with a stable source/binary manifest required for final runs.

### Repaired 100-track audio workload — 9 October, before scene indexing

Both shared and distinct release workloads now measure preparation, canonical
prepared edits, direct route actions, advancing active render and exhausted render.
The source has separate switch and same-clip retrigger series; pre-action rendering
and post-action scalar PCM comparisons occur outside action timers. Same-clip
retrigger is checked against continued playback to detect a missing source reset.
The first switch-series sample still selects the initially playing clip; its other
99 samples switch. Correct that parity before calling it 100 switches.

Commands: `cargo run --release -p hex-engine --example profile_session_audio`
and the same command with `-- --distinct-assets`. Host: Apple M5 Pro, 15 cores,
24 GiB, macOS 26.6.2/arm64. Codex and two existing Hex processes were present;
playback state and workload isolation were not established. Retained logs:
`artifacts/session-audio-profile-{shared,distinct}-repaired-pre-index.log`.
Source/binary hashes and limitations are recorded after the runs in
`artifacts/session-audio-repaired-pre-index-manifest.json`; this is not a complete
snapshot of every dependency. The profile source hash is
`33afe6a851cdbac02ea3916e1f2e7317f23b21882e398485b0af10f389fffc10`.

Each entry below is **p50 / p95 / p99 / maximum**, in microseconds.

| Workload | Shared asset | 100 distinct assets |
| --- | --- | --- |
| Cold prepared revision | 8.041 / 11.875 / 12.916 / 15.000 | 11.125 / 13.166 / 20.458 / 22.750 |
| Gain edit with full preparation | 16.625 / 20.916 / 29.625 / 89.125 | 22.250 / 27.375 / 31.958 / 32.833 |
| Source edit with full preparation | 17.083 / 21.416 / 28.625 / 78.625 | 23.459 / 29.292 / 33.334 / 33.666 |
| Scene launch and Play | 2.833 / 2.958 / 3.250 / 12.750 | 2.833 / 3.666 / 7.208 / 8.250 |
| Clip switch series | 0.125 / 0.167 / 0.167 / 0.250 | 0.125 / 0.167 / 0.583 / 0.583 |
| Same-clip retrigger | 0.125 / 0.167 / 0.375 / 0.667 | 0.125 / 0.167 / 0.250 / 0.292 |
| Stop scene | 2.041 / 2.167 / 2.333 / 2.667 | 1.916 / 2.042 / 2.458 / 6.542 |
| Back to Arrangement | 0.292 / 0.375 / 0.458 / 5.542 | 0.333 / 0.375 / 0.417 / 0.417 |
| Active 128-frame render | 9.625 / 13.542 / 16.958 / 31.709 | 10.834 / 15.042 / 20.000 / 113.084 |
| Exhausted 128-frame render | 6.541 / 7.959 / 11.125 / 59.167 | 6.625 / 10.375 / 15.125 / 90.041 |

Control series contain 100 samples, cold preparation excludes five warmups, and
render series contain 4,000 samples after 100 warmups. Each render series has zero
samples above the nominal 128-frame/48-kHz period. Gain/source edits retain PCM
for all 200 Session clips, checked outside timers. Allocations inherent in full
control-side preparation are included; fixture construction, PCM generation,
oracles, hashing and final graph/PCM destruction are excluded. Direct actions do
not exercise callback queues, publication or worker-to-visible latency.

Shared resident PCM is 524,288 bytes with 32,768 unique active bytes; distinct
resident PCM is 52,428,800 bytes with 3,276,800 unique active bytes. Both modes have
3,276,800 logical active bytes summed across tracks. These short resident one-shots
prove neither long-song capacity nor streaming, devices, native input or deadlines.
Earlier expanded checkpoints remain preserved, including distinct active/exhausted
maxima of 116.667/122.750 µs. The quieter repaired run does not explain those outliers.

### Near-limit scene-grid baseline — 9 October

`cargo run --release -p hex-engine --example profile_session_grid` compares 256
tracks with two versus 256 scenes. Track-major stored cells deliberately interleave
scenes. Each track has one original 32-frame shared-PCM clip in the first scene;
all other cells are explicit Stop. An independent binary-exact scalar oracle checks
Audio/Stop/Audio launches at advancing anchors before timing. This isolates stored
cell-count scaling, not near-limit clip compilation, MIDI voices or plugin work.

| Grid | Action width | p50 µs | p95 µs | p99 µs | Maximum µs |
| --- | ---: | ---: | ---: | ---: | ---: |
| 512 cells | 1 | 22.167 | 29.417 | 32.708 | 95.917 |
| 512 cells | 32 | 374.542 | 472.375 | 516.708 | 662.042 |
| 65,536 cells | 1 | 37.000 | 43.292 | 50.500 | 154.167 |
| 65,536 cells | 32 | 1183.750 | 1280.709 | 1329.583 | 1716.792 |

Each series has 1,000 samples after 20 warmups; all have zero nominal-period
exceedances. Rendering advances seven frames after every timed sample. Fixture,
preparation, oracle, render and final drops are outside timers. The width-32 series
models the admitted command count with direct offline calls; it does not exercise
the native ring. Same host/background conditions as above. Retained baseline:
`artifacts/session-grid-profile-before.log`, source hash and binary hash in the
repaired pre-index manifest. The full-cell scan contributes substantial scaling;
prepare a scene index and compare the identical workload before acceptance.

### Prepared scene index — accepted source, 9 October

The control-side inventory now prepares sorted canonical-cell indices and
scene-ID ranges. Launch performs a binary scene lookup and visits only that
scene's cells. Public scene/cell ordering and relative order within each scene
remain canonical, including empty scenes. Reservations are fallible; callback
launch builds no index, allocates nothing new and retains existing transactional
route handling. The index payload adds 8 bytes/cell plus 24 bytes/scene on this
64-bit host: 530,432 bytes at 65,536 cells/256 scenes, or 4,144 bytes at 512/2.
Allocator/vector overhead is additional.

The identical unchanged release profile produced:

| Grid | Action width | p50 µs | p95 µs | p99 µs | Maximum µs |
| --- | ---: | ---: | ---: | ---: | ---: |
| 512 cells | 1 | 21.959 | 29.375 | 30.667 | 126.250 |
| 512 cells | 32 | 368.208 | 459.958 | 499.209 | 573.458 |
| 65,536 cells | 1 | 13.333 | 17.291 | 21.916 | 29.125 |
| 65,536 cells | 32 | 391.458 | 419.416 | 504.125 | 566.375 |

The dense burst median falls 66.9%, while the small-grid burst remains near its
baseline. All series retain zero nominal-period exceedances. The small-grid
single-action maximum increased from 95.917 to 126.250 µs; retain it rather than
claim uniform improvement. Independent oracle checks, full engine tests and
strict all-target Clippy pass. Root audio and Session suites pass. Retained result:
`artifacts/session-grid-profile-after.log`; post-run source/binary hashes and exact
unchanged profile hash: `artifacts/session-scene-index-manifest.json`.
This does not establish physical callbacks, queue overhead, UI response, worst-case
clip/voice count, long-song capacity or equivalent Live performance. Track-route
slot lookup still searches up to 256 routes per selected cell; future optimization
requires a measured workload and preserved order/ownership semantics.

### Final indexed 100-track rerun — 9 October

Root corrected the switch-series parity so the first action selects B after an A
launch: all 100 samples now switch, separate from the 100 same-clip retriggers.
Both release modes pass their independent PCM/source-reset/handoff checks after
scene indexing, and strict engine all-target Clippy and formatting pass again.
Post-run source, workspace dependency source, lockfile, compiler and binary hashes
are in `artifacts/session-audio-indexed-manifest.json`; this is not a snapshot of
vendor sources. Logs: `artifacts/session-audio-profile-{shared,distinct}-indexed.log`.
Host, short working sets, sample counts, timer scope and native limitations remain
as stated above. Each tuple is p50 / p95 / p99 / maximum in microseconds.

| Workload | Shared | Distinct |
| --- | --- | --- |
| Cold preparation | 10.167 / 11.917 / 13.500 / 14.500 | 12.250 / 16.292 / 17.458 / 19.041 |
| Gain edit/preparation | 18.500 / 22.000 / 25.375 / 26.541 | 24.750 / 32.417 / 53.958 / 57.875 |
| Source edit/preparation | 18.958 / 19.417 / 19.500 / 19.666 | 24.709 / 29.583 / 32.000 / 37.667 |
| Scene launch/Play | 2.708 / 2.750 / 2.916 / 3.375 | 2.750 / 3.708 / 3.709 / 3.791 |
| Clip switch | 0.125 / 0.166 / 0.250 / 1.542 | 0.125 / 0.167 / 0.208 / 0.292 |
| Same-clip retrigger | 0.125 / 0.125 / 0.208 / 0.209 | 0.125 / 0.167 / 0.583 / 2.750 |
| Stop scene | 1.958 / 2.042 / 2.208 / 2.667 | 1.833 / 2.000 / 2.625 / 11.250 |
| Back to Arrangement | 0.292 / 0.334 / 0.375 / 0.416 | 0.292 / 0.334 / 0.417 / 0.417 |
| Active render | 9.541 / 10.292 / 12.750 / 51.708 | 10.709 / 14.750 / 18.333 / 105.667 |
| Exhausted render | 6.625 / 6.750 / 8.500 / 77.833 | 7.375 / 9.916 / 13.209 / 47.416 |

All render series again have zero nominal-period exceedances. Index preparation
adds control work and memory; its cost is included in preparation/edit timings.
Earlier outliers and the distinct gain/retrigger/Stop maxima remain recorded;
this rerun does not establish their cause or a sustained deadline bound.
Root boundary log: `artifacts/session-scene-index-root-boundary-tests.log`;
desktop build: `artifacts/session-scene-index-root-desktop-build.log`;
strict all-target engine Clippy: `artifacts/session-scene-index-final-root-clippy.log`.

### Prepared callback allocation probe — implementation unit, 9 October

Size medium, risk moderate. The next finite gate instruments the existing
`prepared::Callback` API in a headless integration-test executable, without CPAL
or UI. Opt-in command: `cargo test -p hex-audio --features allocation-probe
--test callback_allocations`. Acceptance requires zero allocation count/bytes and
zero net allocation count/bytes inside each measured callback scope, plus PCM,
command-order and lifetime checks outside those scopes. Cover active shared and
100-distinct-asset audio, exhaustion/retrigger, admitted command bursts and priority
Stop, prepared inheritance/fresh installation, retirement backpressure and shutdown.
Do not retain external strong graph/media references in the final-destruction case.
Control graph construction, queue admission, oracle construction and reclamation
remain outside callback measurements. Instrumentation is not a timing benchmark.

The pinned dev-only `allocation-counter` 0.8.1 dependency replaces the allocator in
its test executable and counts activity on the current thread. Its safe measurement
API is used by Hex; no workspace unsafe code is added. Source inspected from the
pinned Cargo registry package and its [upstream repository](https://github.com/fornwall/allocation-counter).
The counter reports total allocations and signed net allocations; a preexisting
object freed in a scope produces a negative net count. Check both total and net
values to detect allocate/free pairs and pure destruction. This does not measure
native API allocations, other threads, locks, blocking or physical deadlines.

Root sensitivity tests pass in debug and release: a black-box Box allocation/free
reports total 1, net 0 and 8 allocated bytes; dropping a previously allocated Box
reports total 0, net −1 and −8 bytes. Empty scopes report zero. Logs:
`artifacts/allocation-probe-calibration-{debug,release}.log`.
The normal desktop dependency tree excludes the allocator. The architecture guard
permits it only as a hex-audio dev dependency; production does not reference it.
Callback workload acceptance is still pending.

Prepared callback allocation source acceptance: five workload tests pass in debug
and release. Root then strengthened and reran the MIDI case in both profiles:
callback PCM matches the headless Session kernel through each of six sounding
scene switches, with no rejected action. The other four cases are unchanged.
This compares the callback boundary to the kernel; it is not an independent MIDI
DSP oracle. Strict feature-enabled all-target audio Clippy, formatting and both
source guards pass. Source/lockfile/compiler/counter provenance is recorded in
`artifacts/callback-allocation-probe-manifest.json`.

Measured callback scopes report zero total allocation count/bytes and zero signed
net count/bytes for these cases:

- 100-track shared and distinct PCM over irregular blocks from 1 to 1,024 frames,
  source exhaustion and scene relaunch, plus 32 executed alternating scene actions.
- A full admitted queue with pre-Stop actions discarded and post-cut launch/Play
  executed; additional admission is refused outside the measured scope.
- Gain/source/tempo graph inheritance with scalar canonical-project PCM checks
  and a genuinely full retirement ring that delays publication until reclaim.
- Priority Stop while the retirement ring is full and a graph waits; output is
  silent and transport paused before control reclamation.
- Fresh empty project installation and shutdown: no callback destruction, old and
  fresh graph Weak references expire only after measured control reclamation.
  External workload/media/revision references are dropped in the lifetime case;
  control reclamation reports negative net allocations/bytes, including at least
  the fixture PCM payload size. This is scoped ownership evidence, not a typed
  allocator report identifying each freed allocation.
- One sounding MIDI track switching between two sources through reserved outgoing
  release storage. This does not cover maximum voice density or all retrigger paths.

The measured closure contains only callback entry/render and a copied error flag;
queue admission, expected output, assertions, status reads and reclamation occur
outside it. No opt-out exclusions are used inside the callback. Zero totals catch
allocate/free pairs, while zero net values catch freeing preexisting buffers.
Allocator instrumentation is separate from the earlier timing profiles.

Exact logs: `artifacts/callback-allocations-test.log`,
`artifacts/callback-allocations-root-release.log`,
`artifacts/callback-allocations-root-midi-{debug,release}.log`, and
`artifacts/callback-allocations-root-final-clippy.log`.
Remaining coverage includes quantized due transitions, maximum MIDI/release density,
permanent candidate rejection/fault paths, and shutdown under ownership backpressure.
The CPAL buffer conversion, OS callbacks, other threads, native API allocation,
locking and physical deadlines remain separate acceptance gates.


## Bounded media conversion checkpoint — 9 October

Source decoding uses 32 KiB byte scratch plus caller-owned stereo windows.
ResamplePlan construction prepares v1 weights once; required_source_range and
render_window perform no Vec allocation by inspection. Identity conversion needs
no kernel; the largest supported downsample table has 1,537 taps × 1,024 phases
× 8 bytes = 12,591,104 bytes, below the 13 MiB cap. Tables currently duplicate
per plan; sharing identical rate pairs is required before many-track integration.

Input/output caps are distinct. An interior 4,096-output-frame window at
192k→8k spans 24×4,095 + 2×768 + 1 = 99,817 source frames before source-edge
clipping: 798,536 bytes of stereo scratch plus 32,768 bytes of output. These are
arithmetic/source bounds, not measured whole-process memory or callback deadlines.
No complete source/output PCM is required by the window APIs.

All 33 media tests and strict Clippy pass. Root independent frozen-v1 comparison
passes 2,700 windows across nine rate pairs in each of debug and release;
first/last and random offsets are included. Sparse 17M-frame WAV tests cover
far reads and conversion. Full Session resident import/history/save/open
regressions remain green. Provenance is in
artifacts/window-conversion-acceptance-manifest.json. No production streaming,
mutable-file snapshot, scheduler/cache bound, 100-long-track physical deadline,
or native input/listening acceptance is implied.


Shared converter preparation now retains one immutable Arc-backed rate-pair
table across explicitly constructed plans. Tests confirm 100 plans share the
same allocation and it is reclaimed only after the final control-side owner.
The largest table payload remains 12,591,104 bytes, rather than 100 copies
(1,259,110,400 bytes). These are table payload counts, excluding per-plan/Arc
metadata and allocator overhead. Existing convenience constructors still prepare
independent kernels; future session/source workers must explicitly reuse them.
All 38 media tests/strict Clippy, 2,700 release frozen-v1 windows, full Session
regressions and desktop build pass. See artifacts/shared-kernel-acceptance-manifest.json.


Verified source access now retains one 64 KiB raw scratch page and32 bytes per raw
page digest (nominal ceil(file_bytes/65536)×32, actual Vec capacity checked against
caller manifest ceiling). Source/output window storage is caller-owned; no whole
source PCM is retained. Creation reads/hashes the whole file off the audio/UI
path; later header/data reads verify whole touched pages before decoding those
same bytes. One-pass scanning does not establish an atomic filesystem image.
These are inspected structural bounds, not allocator/whole-process/deadline or
100-track streaming measurements. All 47 media tests/Clippy and independent Python
SHA-256/24-bit boundary/halo/restored-mtime checks pass in debug/release. See
artifacts/verified-source-acceptance-manifest.json; real cache/worker footprint and
physical/native sustained acceptance remain open.
## Stream delivery resident baseline — 9 October

Before changing clip representation, the existing release Session profile was
rerun with 100 distinct short resident assets at 48 kHz and 128-frame blocks.
Source copies/hashes, compiler and executable identity are retained in
artifacts/stream-delivery-resident-before-manifest.json; full output is in
artifacts/stream-delivery-resident-before-profile.log. The scalar route/source
oracles passed. Active rendering over 4,000 measured blocks had p50 10.667 µs,
p95 14.750 µs, p99 18.750 µs and max 131.792 µs, with no nominal-period exceedance
in this headless run. Cold preparation p50 was 12.542 µs; gain-edit preparation
p50 was 24.542 µs and max 144.708 µs. Preserve outliers when comparing changes.

This resident baseline uses 52,428,800 bytes of PCM with a 3,276,800-byte unique
active window. It is not long-song, disk-streaming, callback or device deadline
evidence. Compare the same profile after the streamed clip/provider changes;
report streamed preparation/queues/cache misses separately from warmed rendering.

After the borrowed-source engine change, the same distinct-source profile passed
its existing PCM/handoff oracles with the same active checksum
2140458349568000. Active render p50/p95/p99/max were
10.625/14.666/19.000/92.834 µs; exhausted max was 99.000 µs versus 71.584 µs
before. These short noisy runs do not establish zero overhead or a regression
budget. Current Clip stride is 72 bytes. An independent scalar indexing/mixing
harness passed debug and release for 12,895 frames each in warm and missing-page
cases, with 20 page lookups per case, source phase, loop seams, resident exhaustion,
mute/balance/master, provider-less refusal and overflow rollback. It uses borrowed
test PCM rather than real worker IO. Evidence:
artifacts/stream-engine-independent-manifest.json and the after-profile log.

## Real worker and 100 long-source tracks — 9 October

The release `hex-stream` example `profile_100_long_sources` now opens 100 separate
same-content sparse PCM16 WAVs with distinct paths and canonical asset IDs, each
17,000,000 frames at 48 kHz (354.17 seconds). Each track has one active clip reading
the far page starting at source frame 16,773,120. All 100 far pages are prepared
and pinned before rendering. The 100 measured runs each render 4,096 frames in
32 chunks of 128 frames; seeks/retriggers, source setup and independent scalar
canonical-order PCM comparisons are outside the timer. No measured run misses.

Whole-window p50/p95/p99/max were 303.292/327.833/370.625/378.833 µs. These are
4,096-frame window timings, not a distribution of individual 128-frame callback
costs. First-source fingerprint preparation took 228.657 ms; the worker verified
all 6,800,004,400 logical source bytes in 22.974262 s. Preparing all 100 far pages
took 42.189 ms. Files are sparse, so this is not a cold-storage throughput test.
The source contents are equal but descriptors and cached PCM buffers are separate.

Measured payloads: 33,554,432-byte pool (1,024 buffers, one reserved staging page,
at most 1,023 cached pages), 3,321,600-byte digest manifests, 6,553,600 bytes of
verified-reader scratch and 65,536-byte worker conversion scratch. Identity-rate
kernels have zero table payload. These figures exclude queues, lookup/catalog
metadata and allocator overhead; no RSS claim follows. All 1,024 buffers were
accounted after joined shutdown. Evidence and hashes are in
artifacts/stream-worker-mutation-and-long-source-manifest.json and
artifacts/stream-100-long-sources.log.

The first catalog-only/one-active-track run is preserved separately in
artifacts/stream-100-catalog-one-active-far-page.log; it does not prove 100 active
tracks. The corrected run establishes bounded long-source admission and warmed
far-page rendering for its 100-track fixture. Sustained advancing disk streaming,
arbitrary overlap, old/new pinned windows, native callbacks/UI, plugins and
first-audible readiness remain open. Separate root debug/release worker checks
compare four converted 44.1→48 kHz pages to frozen v1 and 18,811 engine frames
across phase/fades/loop seams, including resource refusal and restored-mtime edit
rejection with no partial page publication.

## Stream endpoint ownership/allocation checkpoint — 9 October

The split control/pages API retains the same pool and borrowed render path.
Debug and release allocation-counter 0.8.1 probes report zero caller-thread
total allocations/bytes and zero net count/bytes for warmed 31-frame streamed
rendering/page lookup/coalescing and deterministic endpoint operations: successful
requests, failure dequeue/pending deletion/retry, ready install/eviction under
full return-ring pressure, held-return flush and pins/unpin. An explicit alloc/
free calibration establishes that the instrumentation detects temporary allocation.
Construction, worker-thread IO, assertions, shutdown and endpoint destruction are
outside measured closures. These probes do not establish native deadlines or
realtime-safe endpoint Drop.

Root debug/release split real-worker rendering matches 60,452 frames and MIDI
preview against resident wrappers/frozen v1 conversion, including three explicit
loop-boundary seeks. Renderer remains linear: automatic streamed transport loop
scheduling is not established. All eight buffers are accounted at join. Separate
identical-key catalogs recover from wrong pairing and correctly reclaim 16 buffers.
Source/log/binary hashes: `artifacts/stream-control-split-acceptance-manifest.json`.

## Streamed Session performance checkpoint — 9 October

The real-worker debug/release Session oracle matches 110,758 PCM frames against
resident playback and frozen v1 conversion, including actual Arrangement wraps,
immediate/quantized launch, EOF, Seek/Stop and gain-revision handoff. All warmed
miss/request counters remain unchanged; joined shutdown accounts for eight
buffers. A separate 31-frame Session immediate-launch/render page-boundary/EOF
probe reports zero total allocations/bytes and zero signed-net count/bytes.
This is headless caller-thread evidence, not physical callback acceptance.

Current release resident profile: 100 active distinct sources, 128-frame blocks,
4,000 timed samples, p50/p95/p99/max 11.500/15.750/21.000/118.042 µs; exhausted
6.417/7.875/11.625/59.292 µs. PCM checksum is 2140458349568000 (exhausted zero).
A retained earlier binary measured active 10.833/15.334/21.292/134.458 µs and
exhausted 6.500/8.459/12.875/73.208 µs during this run. Its hash does not match
the indexed baseline manifest, so this contextual comparison cannot establish an
exact-parent regression bound. Timing remains provisional, with no device/plugin
claim. Logs: `artifacts/stream-session-{before,after}-resident-profile.log`;
identity limitation: `artifacts/stream-session-before-profile-identity.json`.

## Hybrid-track fixed-demo regression — 9 October 2026

Before/after `cargo run --release -p hex-engine --example profile` on the same
local machine with rustc 1.98.1. This is the existing four-track/eight-source
resident demo, 200 warm-up/4,000 timed blocks per size; background load and power
mode were uncontrolled. It does not benchmark 100 hybrid tracks or devices.

| Block frames | Before p50/p99/max µs | After p50/p99/max µs |
| --- | --- | --- |
| 64 | 0.8 / 0.9 / 55.4 | 0.5 / 0.8 / 79.4 |
| 128 | 1.3 / 1.6 / 50.4 | 0.9 / 1.2 / 33.9 |
| 256 | 2.3 / 7.0 / 44.3 | 1.9 / 7.1 / 50.0 |
| 512 | 4.2 / 9.5 / 55.4 | 4.1 / 10.6 / 64.5 |

Short runs show timing noise; no speedup or sustained deadline claim is made.
Logs: `artifacts/hybrid-profile-before.log` and `hybrid-profile-after.log`.
Independent mixed-source PCM checks pass in debug/release; native listening,
100-track hybrid stress and physical device deadlines remain separate gates.


## waveform cache change — 9 October 2026

Imports now prepare up to 65,536 leaf peaks plus a pairwise hierarchy, replacing
the old 1,024-bin summary. Wider UI range queries are logarithmic; deep zoom
scans at most 64 shared PCM frames per query. Resident counters include all peak
levels, while shared PCM is counted with the existing media bank. This increases
peak memory/import work for detailed rendering; no new render/input timing
benchmark is claimed. Measure dense visible clips and 100-track native edits
before accepting responsiveness. See waveform-display.md.
