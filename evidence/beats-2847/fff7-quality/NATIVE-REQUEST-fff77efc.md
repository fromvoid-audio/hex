# Native request — frozen Beats449 fff77efc

Coordinator-owned serial Mac lane ONLY; this is a request, not execution evidence.
No app launch/replacement/quit, device/backend action, MTS action or user-data edit.
Keep Pulse2847 In Progress / PR449 draft pending the remaining acceptance.

## Source and runner contract

- Canonical PR449, branch `pulse/task-2847-g1-dfd4b8b077c1`.
- EXACT head `fff77efcdbe22c67e574e2e6d16bf3f2454d943e`.
- EXACT tree `3142d295a4522ca5d36e850c614e1d43c85810e2`.
- Algorithm11. Same one-billion work and32MiB scratch ceilings.
- Confirm source/tree and unchanged baseline using the coordinator's existing
  read-only preflight. Use its private target/temp locations and pinned Rust1.94.
  No environment reset, shared target, source editing, fallback or head chase.
- These are optimized `--release` tests, not optimized app/backend evidence.
- Run the following FOUR commands serially, fail fast. Preserve raw outputs,
  Cargo exit code, per-binary counts, compiler/host identity, source/tree and
  original failure if any. Do not retry or substitute an OR filter. Counts below
  are test functions; nested cases do not inflate the unique test count.
- Require EXACT45 distinct passes total /0failed /0ignored, every command Cargo0.
  Any zero-test green, skipped/ignored case or count mismatch is a failure to
  qualify this packet. Do not infer completion from a parent script exit alone.

## 1 — private analysis/plan/search, expected4 tests

```sh
cargo test --release --locked -p hex-warp --lib beats:: -- --nocapture
```

Exactly:

1. `beats::plan::tests::reflected_schedule_finishes_native_pass_excludes_guard_and_has_no_compression_reverse`
2. `beats::onsets::tests::spectral_physical_onsets_detect_quiet_antiphase_percussion_above_bass_without_grid_attacks`
3. `beats::synthesis::search_tests::exhaustive_and_pruned_search_select_identical_complete_matches`
4. `beats::synthesis::search_tests::screened_finalists_match_full_search_quality_on_shared_stereo_overlaps`

Nested evidence:480 SAME-sequence complete-Match bit comparisons,20 screened/full
stereo quality comparisons, positive midpoint/count condition; onset rates
16k/44.1k/48k/96k. Require the actual480 diagnostic and20 quality-case diagnostics.
FFT and generic Tones/Texture period tests are deliberately excluded by `beats::`.

## 2 — Beats algorithm/control/cancel/work/edges, expected37 tests in11 binaries

```sh
cargo test --release --locked -p hex-warp --test beats --test beats_alternating --test beats_bass_band --test beats_envelope --test beats_groups --test beats_plan --test beats_quality_followup --test beats_seams --test beats_short --test beats_stop --test beats_wsola -- --nocapture
```

Each named integration target runs all its test functions, without a substring
filter. Expected per-target counts (no overlap with command1):

| Target | Count |
| --- | ---: |
| beats |4|
| beats_alternating |2|
| beats_bass_band |1|
| beats_envelope |2|
| beats_groups |7|
| beats_plan |2|
| beats_quality_followup |5|
| beats_seams |6|
| beats_short |4|
| beats_stop |1|
| beats_wsola |3|

Includes selected-group native-relative first pass, compression omission/FIR
exclusion, coincident grids, piecewise/fractional timing, transient/loop/short
edges, exact Stop gaps, per-logical-group envelope, stereo/DC/noise level,
Forward/Alternating pitch and turn continuity, deterministic repeated output,
actual cancellation and work/scratch charging. Bass test prints66 valid-case
metrics and checks six compensated-ratio Invalid outcomes; those are NOT ignored
or unsupported skips. Require the16k/27.5/0.5/0 metric line and actual test pass.

## 3 — actual Engine preparation/cache/retained revisions, expected3 tests

```sh
RUST_MIN_STACK=2097152 cargo test --release --locked -p hex-engine --lib audio_transform::tests::beats_ -- --nocapture
```

Exactly:

1. `audio_transform::tests::beats_actual_preparation_joins_cache_parameters_version_and_undo_match_rendered_playback`
2. `audio_transform::tests::beats_wsola_default_preparation_locks_drums_and_keeps_bass_continuous`
3. `audio_transform::tests::beats_alternating_preparation_reverses_safe_chirp_tail_and_undo_preserves_revisions`

Require the three actual-execution diagnostics. These exercise canonical setting
changes, real preparation/cache hits and invalidation including algorithm-version
poisoning, old retained revisions and Undo/Redo, actual PCM across partitions/seek,
drums+bass and Alternating chirp behavior. Fixtures construct owned in-memory
media; there is no public skip/early-return or supported-platform shortcut here.
This is control-side/cache/history lifecycle evidence, NOT full native reaper,
whole graph admission, physical callback or backend qualification.

## 4 — actual linear export consumer, expected1 exact test

```sh
RUST_MIN_STACK=2097152 cargo test --release --locked -p hex-export --test return_routes transformed_warp_root_gain_lane_and_playable_routing_match_native_linear_and_export -- --exact --nocapture
```

This one test executes BOTH its existing Texture and Beats Transients/Forward
branches. It compares actual prepared/render/export paths with source phase,
looping root, fractional pitch, gain/envelopes/fades and routing. It uses isolated
TestDir temporary files, not user media. No claim of transport-loop/Session export
or physical native playback follows. No UI compile/runtime repetition requested:
this449 changes no UI; previous434 UI evidence remains separately scoped.

## Raw27.5Hz metric law and preserved before/after

Source8000 stereo frames at16k, left0.5*sin(2*pi*27.5*i/16000+0.4), right=-0.7*left;
output4000 frames (0.5x duration /2:1 compression), Transients, Forward100, pitch0,
no loop/fade/normalisation. Measure LEFT-channel frames[500,3840),3340 samples.
Fit least squares `x[i]=C+A*sin(2*pi*27.5*i/16000)+B*cos(...)` with centered sums.
Residual is `sqrt(max(0,Sxx-A*Sxs-B*Sxc)/n)`; signal RMS is `sqrt(Sxx/n)` after DC
removal. Free phase avoids falsely calling a constant phase offset distortion.
Original test law is residual <0.1*signalRMS, signalRMS in[0.30,0.40), stereo
relation within1e-6. No test threshold was relaxed.

| Actual source/raw result | Residual RMS | DC-removed signal RMS | residual/signal |
| --- | ---: | ---: | ---: |
| c9f before rawF32 |0.27902720027807687|0.3343521313970514|0.8345309453006753|
| fff final rawF32 |0.00014780187872284555|0.3487701937278495|0.0004237801319632764|

Before unit-test log has0.27902720027812783 /0.3343521313974698; the tiny last-digit
difference from the evidence input's separately constructed F32 source is not a
threshold or algorithm change. The final rawF32 and final test metric agree.
Before source c9f and source9c of the older real-recording comparison MUST NOT be
conflated. These numbers measure the stated interior tone fit, not all join
neighborhoods, whole music or sonic acceptance. Separate tests cover joins.

## Existing audio intake / finite listening checklist

Use the already-published ARCHIVES.json/README in this same folder. Reassemble ALL
listed parts in order, including phrase part03, then verify full SHA before
extracting. Per-file manifests, exact gains/settings, construction recipe and CC0
rights/credits accompany the WAVs. Do not regenerate the54 recording matrix for
status or rewrite original before bytes.

1. At one comfortable playback gain, compare each of the six originals and its
   `2x-Forward100-before-fixed` / `after-fixed` pair. Check attack timing/count,
   doubled or lost attacks, new clicks, body/sustain stutter, bass pitch/colour,
   noise texture and stereo/mono image. Listen to whole files AND all joins/ends.
2. Compare the additional matched versions separately, using their recorded
   whole-file gains. Do not substitute matching for the fixed-gain comparison or
   hide a peak/energy change. Note preference and any defect/timecode/settings.
3. Play the generated120BPM bass/snare phrase at each of the9 existing
   ratio×tail cases. Default Forward/Alternating100 should be useful without
   reducing the envelope to hide seams. Distinguish intentional Stop gaps from
   objectionable sustain gating. Compare the same source attacks/rhythm, not raw
   absolute phase after a deliberate chosen-group or reflected-tail change.
4. Compare sub-bass source/before/after fixed, then matched; check pitch, roughness,
   level and pumping. The improved fitted residual is evidence to listen to,
   not proof that it sounds better or that every sample/musical mix improved.
5. Record PASS/FAIL per material/settings plus audible defects/timecodes. Further
   live app control/seek/play/export listening follows only in root's separately
   authorized optimized delivery/app workflow. No acceptance question to the
   user is implied before the coordinator actually hears/tests these artifacts.

## Explicit remaining limits

- No audio has been listened to in this cloud; no licensed matched Live render or
  proprietary parity evidence exists. Automated continuity/RMS/PCM tests do not
  establish absence of objectionable pumping, stutter or clicks in real music.
- The six acoustic recordings and constructed phrase are a useful partial corpus,
  not complete commercial mixed/polyphonic music, vocals, piano or all user media.
  Source recording BPMs are UNKNOWN; only the generated phrase is authored120BPM.
- Finite tests cover stated rates/ratios/pitches/maps, not every permitted input.
  Dense polyphonic sustain, detector choices and short/crowded attacks remain
  practical quality risks. Compression cannot preserve every overlapping decay;
  bounded chosen-group omission is intentional, not proof of perfect musicality.
- The20 screened/full comparisons bound those fixtures' score/correlation deficit,
  not universal equality with exhaustive search. Version11 selection changes PCM.
- Full-score exact pruning preserves winner/PCM WHEN BOTH renders complete; work
  totals and cancellation polling differ.54 current recording cases pass, but
  this does not prove every old near-cap request completes or general throughput.
- This packet qualifies headless optimized tests only. Full prospective-tree local
  CI/source review belongs to the sole merger, and actual optimized/native app,
  backend/listening and user musical acceptance remain OPEN. No Sampler395 or
  registry/admission completion is claimed.
