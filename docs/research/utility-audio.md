# utility response fixtures

Pulse 18 and 31 require audible/numerical evidence separately from the original
filter implementation and the bounded static control-path study. These original
signals contain no third-party audio. Research Python is tooling only; the app
remains Rust/GPUI.

Generate a fresh directory at each matching reference rate:

```sh
python3 research/fixtures/utility_audio.py research/private/utility-response/48000 --rate 48000
python3 research/fixtures/utility_audio.py research/private/utility-response/44100 --rate 44100
python3 -m unittest discover -s research/fixtures -p test_utility_audio.py
```

Create the parent directory first. The generator refuses existing output
directories and unsupported rates. Outputs are stereo float32 WAVs with frame
counts, generator/output SHA-256 hashes and exact marker positions in a manifest.
Sine rounding may differ across math-library versions; use the actual hashes,
not a claim of universal bit reproducibility. A partial failed generation stays
in its newly created directory for inspection, without overwriting older evidence.

## signals and measurements

| signal | purpose | measurement window |
| --- | --- | --- |
| four-second asymmetric impulse | channel mapping, alignment and impulse response | marker at 0.25 seconds through the end |
| four-second asymmetric dc step | dc rejection, settling and release | step from 0.5 to 2 seconds, then two seconds of silence |
| sixteen-second mid/side tones | bass-mono side rejection and audition isolation | eight two-second segments; analyze 0.75–1.5 seconds of each segment |

Tone segments have side frequencies 10, 25, 50, 100, 200, 500, 1000 and 2000 Hz;
the independent mid probe is 3000 Hz. Signals start at 0.25 seconds and stop at
1.75 seconds of each segment. Derive mid=(left+right)/2 and
side=(left-right)/2 before measuring each probe frequency; broadband RMS mixes
those two probes and cannot establish the side response alone.

Recheck the installed reference identity following
[the research workflow](workflow.md), using a minimal user-authored set. Record
its hash and exact sample rate, export settings and actions. Match the sample
rate, disable warp, normalization/dither, fades and unrelated processing, and
export the full input length plus any explicitly recorded additional tail. Keep
private sets, captures and manifests under ignored `research/private/`.

Capture a neutral baseline first, then separate DC enabled, bass mono at
50/200/500 Hz and bass audition cases. Also capture an off-to-on DC transition
with the exact automation/event frame retained; static inference is not a reset
specification. Observe controls and mode interaction independently.

Before interpreting results, require matching frame counts, finite PCM and
unchanged impulse marker position: no automatic shifting or normalization.
Neutral tolerance is maximum per-sample absolute error <=1e-6 for float32 export.
For wet comparisons, retain raw impulse/DC residuals and independently measured
mid/side magnitude and phase. Initial discrepancy thresholds are 0.1 dB at the
stated tone frequencies and one sample of equivalent phase delay; exceeding them
means investigate, not silently realign or relax limits. Near-zero response must
report an absolute noise floor instead of taking a logarithm of zero. These
thresholds guide comparison; they do not assert that current original filters
match Live or establish a complete device specification.

Source tests verify WAV framing, asymmetric impulse/DC boundaries, independent
mid/side probes, output hashes and refusal to overwrite. Actual Live captures,
comparisons, filter laws, smoothing and native listening remain open. See the
[static study](../../research/observations/utility-controls-001.md) and
[original filter limits](../utility-filters.md).
