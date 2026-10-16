# Project-wide selection timings — 16 October 2026

The complete native capture passed: 360 rows, including 40 discarded warmups and
320 measured samples, with zero failures. Each operation/track-count case has
five warmups and 40 measured samples. Times below are milliseconds; cells show
median / p95 / maximum. p95 is nearest rank 38/40; p99 is 40/40 and equals the
reported maximum. This small sample does not estimate rare-event tails reliably.

Measured source: `bc8da4876c63964706d8bde9b4588d58141fd97b` ([PR258](https://github.com/PORTALSURFER/hex/pull/258));
canonical feature [PR31](https://github.com/PORTALSURFER/hex/pull/31) remains unchanged.
Full 20-step local CI passed on this exact clean source with identical start/end
content digest and `stale=false`. The later report commit adds evidence only and
must not be substituted for the executable source. Exact binary, fixture, toolkit,
CI and source hashes are in [manifest.json](manifest.json).

## Native paint observations

| Tracks | Operation | Selection paint | Edit feedback paint | UI completion paint |
| ---: | --- | ---: | ---: | ---: |
| 1 | copy | 7.08 / 9.20 / 9.53 | 5.41 / 18.34 / 19.21 | 35.25 / 42.97 / 43.57 |
| 1 | cut | 6.97 / 8.77 / 15.01 | 5.34 / 17.32 / 18.46 | 32.10 / 37.48 / 43.49 |
| 1 | replace-paste | 7.06 / 8.85 / 9.65 | 5.65 / 9.84 / 22.43 | 28.03 / 37.47 / 39.67 |
| 1 | ripple-insert | 7.07 / 9.26 / 10.12 | 5.07 / 6.37 / 7.54 | 28.35 / 40.03 / 43.79 |
| 100 | copy | 32.46 / 35.29 / 40.92 | 25.82 / 30.57 / 49.38 | 75.29 / 82.34 / 87.54 |
| 100 | cut | 32.65 / 38.57 / 44.88 | 26.55 / 30.39 / 34.22 | 62.31 / 69.75 / 76.26 |
| 100 | replace-paste | 32.73 / 36.61 / 38.21 | 24.98 / 27.48 / 28.46 | 75.92 / 83.22 / 88.25 |
| 100 | ripple-insert | 32.75 / 37.58 / 39.84 | 24.99 / 26.91 / 32.37 | 76.30 / 82.64 / 99.78 |

## Following-frame observation bounds

Callbacks registered from each paint are observed at the following frame boundary,
after submission of that scene. These are separate bounds, not GPU completion,
compositor completion or visible pixels.

| Tracks | Operation | Selection following frame | Feedback following frame | Completion following frame |
| ---: | --- | ---: | ---: | ---: |
| 1 | copy | 9.19 / 11.98 / 12.41 | 9.05 / 19.87 / 20.78 | 36.59 / 44.37 / 45.04 |
| 1 | cut | 8.99 / 11.43 / 16.74 | 9.12 / 18.71 / 19.78 | 33.17 / 38.53 / 44.65 |
| 1 | replace-paste | 8.97 / 12.47 / 15.38 | 9.30 / 13.08 / 24.56 | 29.77 / 39.06 / 41.53 |
| 1 | ripple-insert | 9.26 / 11.10 / 11.88 | 9.01 / 11.27 / 12.17 | 29.88 / 41.80 / 45.35 |
| 100 | copy | 38.67 / 42.88 / 46.16 | 31.49 / 35.84 / 56.41 | 80.74 / 88.96 / 94.53 |
| 100 | cut | 39.13 / 45.87 / 52.98 | 31.87 / 37.00 / 41.08 | 66.43 / 73.34 / 81.87 |
| 100 | replace-paste | 39.30 / 43.89 / 44.63 | 30.37 / 32.96 / 34.75 | 81.50 / 88.88 / 94.78 |
| 100 | ripple-insert | 39.39 / 43.56 / 51.41 | 30.52 / 32.84 / 41.22 | 81.82 / 88.44 / 106.45 |

## Interpretation and limits

One-track median selection paint is 6.97–7.08 ms; 100-track median is
32.46–32.75 ms (about 4.6×). Edit-feedback medians are 5.07–5.65 ms versus
24.98–26.55 ms. UI-observed completion medians are 28.03–35.25 ms versus
62.31–76.30 ms; the worst observed completion paint is 99.78 ms. Completion
includes the normal 16 ms UI polling of worker results. Work and latency are
not independent of project size. A roughly 33 ms median selection paint at this
density does not establish 60 Hz responsiveness or the subjective smoothness goal.
Dense rendering/focus/accessibility acceptance remains separate Pulse #29 work.

Captured on Apple M5 Pro (15 cores, 24 GiB), macOS 26.6.2/25G83, arm64, AC power,
with a 1240×760-point native window. Desktop load was uncontrolled: Live was
active, another Hex audio test was visible before capture and Clippy after it.
Load averages changed from 9.02/7.49/7.33 to 7.11/7.13/7.20. These are point-in-time
measurements under that load, not an idle-host performance budget or comparison
with Live. No physical audio device, plugins, OS-input latency or display scanout
is measured. Previous functional user acceptance is preserved; this run adds
numeric evidence, not a new subjective acceptance claim.

The fixture has 50 audio and 50 MIDI clips per track, eight notes per MIDI clip,
shared 48000 Hz stereo PCM, gaps and partial selection boundaries. Both sizes use
identical per-track content: 100/10000 clips and 400/40000 authored notes.
Untimed paste setup uses a validated snapped main cursor; the measured drag is
unsnapped. The existing unsnapped stationary-cursor projection defect is tracked
separately as Pulse #2647 / [PR263](https://github.com/PORTALSURFER/hex/pull/263).
No editing behavior was changed to obtain these measurements.

Method and reproduction: [time-selection-profile.md](../../time-selection-profile.md).
Raw complete samples: [raw.csv](raw.csv). Machine-readable statistics:
[summary.json](summary.json). Recompute without native execution:

```sh
python3 docs/evidence/time-selection-2026-10-16/analyze.py docs/evidence/time-selection-2026-10-16/raw.csv /tmp/time-selection-summary.json
```

Earlier failed captures are retained locally and excluded; this report uses one
uninterrupted complete run, with no sample splicing or outlier removal.
