# eq eight response measurement tooling — pulse2251 / pr198

This is an isolated control-side measurement/comparison unit. It changes no EQ
DSP, codec, descriptor, ramp, history, capacity, registry or UI. No measured Live
dataset or approved adaptive-Q law is supplied. Pulse2251 remains incomplete;
its scope includes all eight response shapes, adaptive Q, migration, runtime
checks and native listening. Synthetic tests only validate this tool.

## actual prepared-response export

Run inside the exact clean canonical checkout:

```sh
cargo run -p hex-devices --example eq8_response --locked -- \
  48000 stereo stereo bell 1000 6 0.70710677 1 100 1000 10000 \
  > artifacts/eq8-response.csv
```

Arguments are rate, mode, selected bank, shape, authored frequency/gain/Q/output
gain, then one to8192 strictly increasing query frequencies. This example chooses
an original Hex test case, not measured reference settings. Modes/banks: stereo/
stereo, lr/left or right, ms/mid or side. Shapes: bell, low_shelf, high_shelf,
low_cut_12, low_cut_48, high_cut_12, high_cut_48, notch. Rate is8000..192000;
queries are positive and below Nyquist. Authored fields use the real descriptors;
provider preparation can refuse otherwise in-range combinations. Exactly band0
of the selected bank is enabled, all other bands remain disabled, output gain
is explicit. LR/MS outputs are working lanes, not physical L/R measurements.

The example calls the actual EQ definition/state preparation and
PreparedNativeProcessor::frequency_response_at_hz for every query. No filter math
is reproduced. The current API consumes/returns f32, while existing coefficient
preparation is f64. Metadata records state version, Git source/dirty status and
actual f32-authored configuration (including signed zero). The CSV contains
query_hz,lane0_linear,lane1_linear. These are steady-state magnitudes, not phase,
latency, impulse, runtime automation, listening or hardware acceptance. Preserve
Cargo/toolchain/platform/command/log identity alongside the export; the metadata
is attributed provenance, not authenticated execution proof.

## measured manifest and comparison

Copy research/observations/eq8-response-2251/manifest-template.json into ignored
artifacts/scratch and fill EVERY null from actual observations. The unfilled
tracked template deliberately fails validation and is not a reference dataset.
One manifest describes one case, one bounded CSV capture and its provenance:
producer/version/executable SHA256, method, exact case and raw capture SHA256.
Retain original capture assets, settings, measurement calibration, repetitions
and derivation of uncertainty next to the manifest. A method must identify lane
conversion, excitation, settling/window/grid and uncertainty calculation; this
string is a declaration, not proof the procedure ran. Reference app inventory
must be rechecked natively: docs/research/reference.md's Live12.4.6/hash is only
historical evidence. This Linux tool cannot establish the installed reference.

Both measured and Hex CSVs must have the exact three-column header, one to8192
finite nonnegative magnitude rows, and identical increasing frequency grids.
No interpolation, resampling or dB-floor substitution occurs. Configure the
reference using the same f32-authored values and query grid. Configuration fields
are compared by f32 bits; grids are compared by their parsed numeric values.
Absolute/relative linear uncertainty and tolerances must be finite in0..1, chosen
and justified before comparison. Unknown/duplicate fields, future schemas,
nonfinite/out-of-domain inputs, hashes, missing files, oversized files (>1MiB),
escaping capture paths, dirty Hex source and incompatible cases refuse. File
hashes establish package integrity, not measurement authenticity.

```sh
python3 scripts/eq8-response-compare.py \
  artifacts/reference-case/manifest.json artifacts/eq8-response.csv \
  > artifacts/eq8-comparison.json
```

For each lane/query, error E=abs(hex-reference), uncertainty
U=absolute_linear+relative_linear*reference and tolerance T use the same formula.
Within tolerance requires E+U<=T; outside requires E-U>T; otherwise the point is
inconclusive. Any outside point makes the case outside; otherwise any inconclusive
point makes the case inconclusive. Exit0 means within the DECLARED tolerance,
exit1 outside, exit3 inconclusive, exit2 malformed/missing/unsupported input.
Reports retain manifest/export hashes, configuration, source, producer provenance,
per-point values and explicit parity_claim=false. f32 API quantization and capture
uncertainty must be considered when justifying tolerances. Zero magnitude is
compared in linear units without log-of-zero errors.

kind=synthetic is retained conspicuously in reports; it must never be relabeled as
native/reference evidence. Current Hex has no adaptive-Q toggle/law: adaptive-on
captures may be recorded, but this comparator refuses to compare them to legacy
Hex or assert coverage. A within-tolerance baseline sample does not establish all
shapes/rates/settings or define an adaptive law.

## remaining whole-issue work and checks

A native owner must capture version/hash-bound reference data across all eight
shapes, frequency/gain/Q ranges, extreme rates, modes and adaptive settings.
Specify meaningful measured tolerance/uncertainty and resolve differences from
legacy cookbook behavior. Approve the adaptive law/toggle scope and versioned
legacy-state/history behavior before production edits. Codec-version coordination
with oversampling2252 remains explicit. No public toggle or vendor law is guessed.

Focused tool tests cover real prepared peak/inactive-lane export, signed zero,
input/refusal, independent three-outcome uncertainty boundaries, wrong lanes,
grid/count/hash/missing/malformed/provenance/schema/path failures, dirty source
and unsupported adaptive comparison. Strict affected Clippy and required full
local CI must pass on final exact source. These checks prove tooling only; no
callback/native/reference acceptance is inferred. Native staged review artifacts
and actual correctness/listening review remain separate, with no Hex deployment,
merge, release or Done authorization.
