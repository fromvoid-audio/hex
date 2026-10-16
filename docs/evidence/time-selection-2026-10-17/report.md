# native project-wide selection timing — 17 october 2026

The optimized candidate `17ce787dc7ef892b8d9876f811d70663ca2b4f68` meets the selection budget in this capture: all 160 measured 100-track input-to-paint samples are below 16.67 ms; the maximum is 9.847 ms. All 360 protocol rows are valid (320 measured, 40 warmup), with zero failures. This establishes the measured native paint budget, not physical display or GPU deadlines.

The original 100-track selection medians were 32.461–32.746 ms. The final medians are 4.723–4.937 ms, with p95 6.208–9.466 ms.

## source and checks

- Source: `17ce787dc7ef892b8d9876f811d70663ca2b4f68`; tree: `a8edfe53050213a8526a79806f5681a4e1c80dc8`; binary SHA256: `7f6a474d6ae636f24927f0a72822255cfcdc7e5dfb48fc5643978eaed7edf570`.
- Full local CI: all 20 steps passed on that exact clean source; [retained summary](ci-summary.json). No hosted CI.
- PR267 fixes a key captured before lane prepaint while content was built with the newer geometry. Its regression fails before the assignment (two builds instead of one), passes afterward, and verifies current selection, geometry and trim capture without extra settling.
- PR264 supplies bounded clip-content caching, current-draw epochs, ordered GPUI replay and the bounded waveform working set. The original issue14 remains linked to merged PR31.
- The native harness is unchanged: blob `0c9ddaea2bbefb865ed7a70d15cca81103461a67`. All original fixture and actual WAV hashes match [the baseline](../time-selection-2026-10-16/report.md).

## identical protocol and documented projection correction

The original 1/100-track hybrid fixtures, shared 48 kHz stereo PCM, normal controller worker, offline audio, 5 warmup plus 40 measured samples per case, four operations, alternating track counts and 7–17 ms setup phases are unchanged. There are no additional settling frames or sample exclusions. Each track has 50 audio and 50 MIDI clips, with eight notes per MIDI clip.

The independently merged cursor fix PR263 now projects the whole main-row gesture through the main lane. The same physical input yields the uniform frame interval `[529092, 1537596]`; the old buggy projection ended at `1539570`. The fixture and harness were not adjusted to hide this difference. Timing comparisons therefore retain the small intended musical-range correction. All current runs are checked for one consistent interval.

## all six timing distributions

Units are milliseconds. Each cell is **median / p95 / maximum** over 40 measured samples. P95 is nearest rank 38/40; p99 equals the observed maximum and does not establish rare-event tails. Baseline columns use the original native capture. UI completion includes normal 16 ms worker polling.

### selection input → first paint

| tracks | operation | baseline | final |
| ---: | --- | ---: | ---: |
| 1 | copy | 7.077 / 9.199 / 9.525 | 3.726 / 6.804 / 7.812 |
| 1 | cut | 6.970 / 8.768 / 15.013 | 4.195 / 7.007 / 7.617 |
| 1 | replace-paste | 7.062 / 8.848 / 9.655 | 4.190 / 7.115 / 7.710 |
| 1 | ripple-insert | 7.071 / 9.256 / 10.117 | 3.571 / 6.974 / 7.821 |
| 100 | copy | 32.461 / 35.286 / 40.920 | 4.742 / 6.208 / 7.153 |
| 100 | cut | 32.650 / 38.573 / 44.885 | 4.789 / 6.991 / 9.232 |
| 100 | replace-paste | 32.735 / 36.607 / 38.212 | 4.723 / 9.466 / 9.847 |
| 100 | ripple-insert | 32.746 / 37.582 / 39.837 | 4.937 / 6.948 / 9.564 |

### selection input → following-frame observation

| tracks | operation | baseline | final |
| ---: | --- | ---: | ---: |
| 1 | copy | 9.186 / 11.980 / 12.407 | 10.460 / 14.163 / 14.548 |
| 1 | cut | 8.988 / 11.433 / 16.744 | 11.197 / 14.177 / 15.419 |
| 1 | replace-paste | 8.967 / 12.471 / 15.383 | 10.610 / 13.871 / 14.432 |
| 1 | ripple-insert | 9.261 / 11.103 / 11.878 | 10.064 / 14.341 / 14.548 |
| 100 | copy | 38.673 / 42.877 / 46.158 | 9.805 / 11.947 / 15.589 |
| 100 | cut | 39.132 / 45.874 / 52.984 | 9.640 / 13.668 / 14.231 |
| 100 | replace-paste | 39.303 / 43.886 / 44.629 | 10.053 / 14.553 / 15.922 |
| 100 | ripple-insert | 39.387 / 43.558 / 51.407 | 9.641 / 14.243 / 15.095 |

### operation input → first feedback paint

| tracks | operation | baseline | final |
| ---: | --- | ---: | ---: |
| 1 | copy | 5.412 / 18.335 / 19.214 | 5.597 / 19.007 / 21.059 |
| 1 | cut | 5.341 / 17.319 / 18.456 | 5.741 / 16.071 / 23.503 |
| 1 | replace-paste | 5.653 / 9.844 / 22.428 | 4.377 / 9.197 / 11.533 |
| 1 | ripple-insert | 5.068 / 6.373 / 7.537 | 4.521 / 9.003 / 10.892 |
| 100 | copy | 25.823 / 30.573 / 49.384 | 19.849 / 21.760 / 38.343 |
| 100 | cut | 26.547 / 30.385 / 34.224 | 19.696 / 21.555 / 31.011 |
| 100 | replace-paste | 24.978 / 27.476 / 28.456 | 20.119 / 23.338 / 31.864 |
| 100 | ripple-insert | 24.991 / 26.908 / 32.374 | 20.161 / 24.007 / 24.456 |

### operation input → feedback following frame

| tracks | operation | baseline | final |
| ---: | --- | ---: | ---: |
| 1 | copy | 9.048 / 19.866 / 20.783 | 10.523 / 20.132 / 22.163 |
| 1 | cut | 9.116 / 18.706 / 19.777 | 10.353 / 17.040 / 24.752 |
| 1 | replace-paste | 9.305 / 13.081 / 24.560 | 9.282 / 13.940 / 16.187 |
| 1 | ripple-insert | 9.010 / 11.267 / 12.169 | 9.387 / 13.553 / 14.455 |
| 100 | copy | 31.490 / 35.837 / 56.410 | 24.605 / 26.978 / 44.211 |
| 100 | cut | 31.869 / 36.997 / 41.082 | 24.422 / 26.310 / 39.581 |
| 100 | replace-paste | 30.374 / 32.957 / 34.749 | 24.785 / 28.418 / 38.886 |
| 100 | ripple-insert | 30.519 / 32.838 / 41.223 | 24.878 / 28.892 / 29.837 |

### operation input → UI-observed completion paint

| tracks | operation | baseline | final |
| ---: | --- | ---: | ---: |
| 1 | copy | 35.251 / 42.967 / 43.568 | 25.684 / 37.585 / 38.905 |
| 1 | cut | 32.099 / 37.483 / 43.492 | 24.442 / 34.662 / 37.360 |
| 1 | replace-paste | 28.028 / 37.470 / 39.671 | 26.294 / 32.832 / 33.545 |
| 1 | ripple-insert | 28.348 / 40.031 / 43.788 | 27.221 / 37.242 / 39.332 |
| 100 | copy | 75.291 / 82.342 / 87.539 | 60.801 / 63.428 / 68.443 |
| 100 | cut | 62.311 / 69.755 / 76.263 | 50.752 / 53.084 / 78.257 |
| 100 | replace-paste | 75.923 / 83.219 / 88.247 | 54.564 / 59.385 / 84.680 |
| 100 | ripple-insert | 76.296 / 82.642 / 99.777 | 54.816 / 60.582 / 63.925 |

### operation input → completion following frame

| tracks | operation | baseline | final |
| ---: | --- | ---: | ---: |
| 1 | copy | 36.593 / 44.371 / 45.038 | 26.617 / 38.995 / 39.816 |
| 1 | cut | 33.168 / 38.528 / 44.653 | 25.293 / 35.598 / 38.350 |
| 1 | replace-paste | 29.773 / 39.065 / 41.527 | 27.573 / 34.625 / 37.623 |
| 1 | ripple-insert | 29.883 / 41.799 / 45.346 | 29.752 / 38.174 / 40.611 |
| 100 | copy | 80.738 / 88.963 / 94.528 | 65.831 / 68.530 / 76.388 |
| 100 | cut | 66.426 / 73.336 / 81.866 | 54.036 / 56.689 / 82.875 |
| 100 | replace-paste | 81.501 / 88.877 / 94.778 | 59.427 / 64.008 / 91.713 |
| 100 | ripple-insert | 81.816 / 88.436 / 106.451 | 59.928 / 65.909 / 69.279 |

## retained intermediate runs and sustained work

The earlier frozen `cbc33ff0` run is retained as historical evidence, not relabeled as merged-source acceptance. It passed p95 but had a 43.279 ms maximum. The exact merged `5607b14c` run is also retained: cut-case p95 was 28.229 ms, so that run failed the target. All nine selection outliers across those runs occurred at 7–12 ms setup phases; none occurred at 13–17 ms. This supported investigating the redundant next-frame rebuild, without proving every stall had that cause.

The final regression demonstrates one correct cold build followed by reuse on immediate selection; it does not add benchmark warmup. The separate optimized headless diagnostic has 40 consecutive growing selection draws and zero post-warmup clip builds: median 6.805 / p95 8.765 / maximum 10.700 ms. It uses GPUI TestPlatform and NoopTextSystem, and excludes native font shaping, GPU, presentation and physical input latency.

Host load is uncontrolled and differs across runs; Ableton Live and other applications remained open. The final native run used AC power. Exact timestamps, load observations, hardware, source, artifact, fixture and toolkit identities are in [manifest.json](manifest.json). No subjective native review, physical audio deadlines or Windows acceptance is claimed. No development app was installed by this capture.

## reproducibility

Raw rows, full numerical distributions and baseline comparisons are retained alongside this report. From the repository root, use new output destinations:

```sh
python3 docs/evidence/time-selection-2026-10-17/analyze.py docs/evidence/time-selection-2026-10-17/raw.csv /tmp/hex-selection-new-summary.json
python3 docs/evidence/time-selection-2026-10-17/compare.py docs/evidence/time-selection-2026-10-17/raw.csv /tmp/hex-selection-new-comparison
```

The analyzer validates the unchanged native schema and protocol. The comparison records the main-lane projection correction only for runs whose interval changed. Human tables round half up to 0.001 ms; JSON retains the microsecond values. No outlier removal is performed.
