# project spectrum

Pulse #2738 / canonical PR #349 adds a read-only analyzer below the top toolbar
and above the workspace. Master remains the baseline with zero selected tracks;
each selected canonical track adds its own curve in its assigned color. All
selected tracks are independent: no selected-track sum, selection-order palette,
array-index identity or hidden prefix cap. The current project bound is 256 tracks
plus Master. Master paints last with contrasting theme color. The panel uses the
same 4096-frame Hann FFT, 96 logarithmic bands, stereo-power reduction, interpolated
narrow bands, immediate attack, 48db/s capture-driven decay and −96..0dbfs display
as the improved EQ analyzer. This does not increase FFT resolving power.

## Signal contract

Track signals are actual direct post-device/post-fader/balance output, before
shared Return processing and master gain. Retained release/device tails belonging
to that canonical track contribute to its layer. Master is the actual project mix
including Returns and master gain, before monitor-only metronome/browser preview.
Nonlinear shared processing prevents any claim that displayed tracks sum to
Master. Muted or idle valid owners produce actual rendered silence; a missing
canonical owner or unsupported runtime refuses the whole project batch. Audio
rendering and existing offline-capture admission rules remain independent.

The engine forwards synchronous borrowed spans through the existing rendering
transaction, compensated path, stopped-keyboard path and streamed chunk-offset
adapter. Capture is speculative until final render/output validity, generation,
rate, canonical revision floor and exact selection token are rechecked. A changed
selection, removed owner, project replacement or restarted output cannot reuse an
old curve. Queue loss consumes a sequence and clears all rolling windows rather
than splicing samples across a gap. Nonfinite samples and mismatched rates refuse.

## Work, storage and lifecycle bounds

Native construction adds project capture to the existing analyzer worker, reader,
shutdown receipt and lifecycle reaper. There is no per-track worker and no new
join path on the callback. Native factory identity includes both actual project
packet rings; foreign capture/reader endpoints refuse construction. Payload costs
are charged by the same native resource accounting, with a separate 32MiB known
project-payload ceiling. Opaque allocator/channel implementation metadata is
reported as resource groups rather than invented byte counts.

A fixed 33-owner packet pool feeds a 32-slot forward ring and a 33-slot return ring.
Each packet has at most 257 layers × 256 stereo frames. The callback zeroes only
the selected prefix and adds real borrowed contributions. At most 263168 stereo
contribution frames are accepted per callback, including retained tails. Budget
exhaustion rejects the complete batch without dropping a selected layer or
changing audio. Queue-full paths retain or return existing owners: no callback
allocation, lock, logging, final owner destruction or FFT.

One aligned 4096-frame window covers every layer. Project work services at most two
capture packets and eight stereo FFT layers per worker turn, then yields to the
existing EQ analyzer. A multi-turn analysis freezes its one rolling window while
the bounded packet queue absorbs incoming capture. Analysis is capped at 30 Hz for
up to 32 selected tracks, 10 Hz for 33–128 and 5 Hz for 129–256. Freshness limits are
100/250/400 ms respectively; stale curves clear rather than being refreshed by an
idle timer. Existing EQ cadence and 100 ms freshness remain unchanged. These are
software bounds, not a hardware deadline or 256-track playback performance claim.

Collapse clears selection and displayed results, preventing further project
capture/FFT work after already queued work drains. The fixed pool remains owned
until the audio lifecycle retires. Expand starts a new token/window. Height is
view-only, bounded to 72–300 px and available window space; resizing commits no
project/history edit. Outside release completes it; Escape, focus/context loss or
window resize restores the previous height. At constrained heights with
populated details, the expanded panel uses a 22px
header containing a live miniature plot of every layer. The preferred graph
height returns when space permits; this is separate from manual collapse, which
stops capture. Full detail controls retain their ordinary unclipped layout when
they fit. Otherwise a bounded vertical viewport keeps at least one 44px track row
usable; empty detail space yields to at least 60px of timeline. No selected layer
is omitted in compact mode. No project-format or persistence field is added.

## Verification and native acceptance

Source tests cover actual engine PCM with independent track gains/balance and
Returns across loop boundaries, simultaneous actual callback EQ/project capture,
final context cancellation, stereo anti-phase energy, independent tone/level
layers, queue loss/recovery, full 256-track storage and allocation bounds, custody
identity refusal, incremental worker fairness and rendered selection/color,
collapse/resize/focus/layout behavior. The ignored optimized
`profile_project_spectrum_maximum` test is run explicitly to measure the actual
capture/worker at 0/32/128/256 selected tracks; timings are evidence, not assertions.

An optimized Linux source profile (Rust 1.94.0, five available CPUs, concurrent
layout CI) measured the actual capture/worker with all selected layers enabled:

| selected tracks | layers | capture median µs | capture max µs | worker median ms | worker max ms | maximum worker turn ms |
| --- | --- | --- | --- | --- | --- | --- |
| 0 | 1 | 0.911 | 2.167 | 0.402 | 0.658 | 0.594 |
| 32 | 33 | 20.589 | 122.965 | 15.565 | 33.197 | 24.357 |
| 128 | 129 | 105.044 | 28862.068 | 47.131 | 66.148 | 28.459 |
| 256 | 257 | 169.508 | 639.471 | 101.237 | 114.196 | 20.710 |

The known project payload was 26319848 bytes at every selection size because the
pool/window capacity is fixed. Two warmups preceded five measured trials; each
trial captured sixteen 256-frame packets and analyzed one complete aligned window.
Timings include observed scheduling outliers, without attributing their cause.
They exclude the driver, graph DSP and native paint/input. The 32-track 30 Hz and
256-track 5 Hz rates are upper caps, not guaranteed update frequencies. Retained
provenance is `artifacts/project-spectrum-profile/actual-worker.json` with the
dirty-source report/digest and raw output. The same report's initial UI step ran
zero tests; it supplies no rendered-regression evidence. Correctly enabled UI
checks are retained separately and required again in final full CI.

Exact-source full local CI is required before merge. Linux source tests do not
establish macOS GPU/input/listening acceptance. The coordinated native checklist
must verify master/individual frequencies, Return semantics, selection/color and
project/output replacement, silence/rates, minimum-window scrolling, outside
resize/Escape, collapse lifecycle, and EQ responsiveness with 256 selected tracks.
Only the coordinator schedules a serial Mac follow-up; the launcher owns the one
combined signed application delivery and safe save/quit/replacement procedure.
