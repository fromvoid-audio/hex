# EQ quality review cycle 1

Pulse2784 / canonical PR390, 17 October 2026. The user authorized cloud
implementation after the earlier planning-only request. Review source begins at
actual merged393 `394f6c130bb87fe5c8c49cfd82f4c52cb6049533`, tree
`c8e93e2c431f0ac21de8cbb6a3258fc34dd3ac72`. Installed app remains earlierdfa,
build3751944026; this source is not claimed installed or natively accepted.

All six original EQ source PRs342/343/347/344/348/349 are merged. Their immutable
source receipts remain at `b0713bd6fbf61cebbcc85e6e3fbde1e868f69c83`.
Gesture2729 is user accepted; it is not reopened. Later production spectrum
stack bug2786/392 and engine test-fixture bug2787/393 are independently repaired
and merged. Both have exact-head full20 cloud passes and independently verified
actual merge-tree equivalence. Factory392 additionally has actual default-stack
Mac debug/release/allocation regression passes. Full Mac20 also passed on
exact98/treec8e93e2c, default RUST_MIN_STACK unset, native summary
`55cae340d5b9548961eb769129ace85ca15f8778c6c0894e496b97e32d636bdd`
(run20261017T194705Z-93e5ce6a), authenticPulse2787note20935. Native raw
files are retained by the coordinator; cloud did not inspect those raw files. Final source/negative evidence is immutable
`83925658f2d5c1dbbf94018601653fa357161678`. These are different source/platform
receipts, not retrospective replacement of the original327Mac failure.

## Reference identity

FabFilter Pro-Q4 official online help, inspected2026-10-17. Moving documentation,
not an installed reference executable/version measurement. No cloud access to a
licensed installed reference, physical audio, native screenshots or direct A/B.
The following links are the exact inspected pages. Interpretations below are
paraphrases, no proprietary code/assets were used.

- https://www.fabfilter.com/help/pro-q/using/eqdisplay
- https://www.fabfilter.com/help/pro-q/using/analyzer
- https://www.fabfilter.com/help/pro-q/using/overview
- https://www.fabfilter.com/help/pro-q/using/bandcontrols
- https://www.fabfilter.com/help/pro-q/using/dynamic-eq
- https://www.fabfilter.com/help/pro-q/using/spectral-dynamics
- https://www.fabfilter.com/help/pro-q/using/processingmode

Workflow reference supports multi-band selection/editing, fine and constrained
gestures, direct values and adjustable graph scales. Analyzer documentation offers
pre/post/external traces and adjustable range, resolution, decay and tilt. Its
band guide explicitly warns that matching numerical Q between EQs need not match
their response. Therefore any later legally available A/B must match measured
response, phase and level, not merely identical knob numbers.

## Comparison matrix

“source verified” refers to identified existing tests/history, not current full
suite/native acceptance. “unverified” means additional measurement is needed,
not an asserted defect. Keep the original Hex theme and shared native widgets.

| Area | Hex source inventory / evidence | Classification and next action |
| --- | --- | --- |
| Bell/edge-cut creation | eq_panel.rs, docs/eq.md;2729/342 stable-ID preview/release/cancel tests | source verified; user accepted gestures preserved |
| Single-node selection/drag | EqPanel selected Option<NodeId>, ResponseGraph stable-ID events | source verified; native task timing pending |
| Multiple-node selection/group edits | single selected-node owner in current EQ panel | missing capability; propose later, no implied expansion |
| Fine movement | ResponseGraph Shift precision and arrow keys; widget tests | source verified; native text/focus/precision check pending |
| Axis-constrained modifiers/wheel-Q | graph has Shift fine movement and arrows; Alt/Control have selection/listen responsibilities, no graph wheel-Q/axis law | missing workflow; defer separate input-contract design to preserve accepted2729 |
| Frequency/gain/Q direct values | shared canonical native controls/descriptors; graph authored curve | implemented; finite native entry/focus check pending |
| Filter shapes | bell, low/high shelf, low/high cut, notch | source verified on the504-query PCM matrix below; broader stimuli/listening pending |
| Slopes | cuts12/24/36/48db/oct; source finite/attenuation tests | deliberate bounded capability; fractional/brickwall/other-shape slopes missing |
| Banks/modes | stereo/L/R/M/S banks, global stereo/LR/MS mode | implemented; independent channel/mid-side stimulus audit pending |
| Capacity |64 stored nodes incl disabled across banks;64lanes/160points/64points-per-lane | existing decision retained; no silent cap/budget changes |
| Per-band bypass/delete | canonical enable/remove; node+automation atomic removal | source verified; native focus isolation pending |
| Solo/listen | stable-ID transient audition,256frame fades; actual callback fixture | source verified; level/discontinuity/listening checks pending |
| Undo/Redo/identity | monotonic node IDs/high-water allocator and atomic history | source verified; current-format disposable save/reopen pending |
| Automation | content clock,64sample coefficient cadence/ramps; partition/retarget/loop tests | source verified within existing contract; zipper metric/listening pending |
| Response graph | explicitly authored response; cache by state/rate;96-point sampling hid high-Q notch depth | confirmed display defect, selected cycle1 repair below; automated-response animation remains a deliberate difference |
| Static transfer | original actual-cut impulse regression retained; additional504queries across shapes/rates/extremes | source verified matrix below; no universal sonic/reference parity |
| Input/output traces | actual entry pre/post including trims;2730/343 | source verified; native calibration/bypass/Return checks pending |
| Analyzer power |4096Hann/96logbands, stereo power, immediateattack/48db/sdecay, -96..0dbfs | source verified; settings/tilt/freeze/range variation missing or deliberate fixed choices |
| Project spectra | actualMaster plus each selected canonical track up to256, direct postdevice/postfader/balance | source repaired in2786/392; all257 layers retained; physical native performance remains open |
| Analysis freshness/lifecycle |100msEQ, project100/250/400ms, generation/token/sequence refusal | source verified; physical native churn/collapse/driver behavior pending |
| Compact layout | shared intrinsic text actions/frequency dial/separate automation row;2731/344 | rendered source tests; native880x540 and1500x650 pending |
| A/B state slots | no current EQ-owned A/B workflow found; Undo is not A/B | missing capability; propose later scoped decision |
| Auto level matching | output trim exists; no measured auto-match contract found | missing capability; manual calibrated matching for audit only |
| Phase modes | zero algorithmic latency IIR; no Natural/linear phase modes | deliberate current boundary; later architecture/cost proposal |
| Dynamic/spectral EQ | static band/automation model; no detector/spectral processing contract | explicit missing capability; separate native roadmap scope, not polish changes |
| EQ Sketch/Grab/Match | no current authored equivalents | missing capability; later product proposals |
| External/instance spectra | project layer panel exists; not cross-instance external analyzer routing | deliberate distinction; external/instance workflows missing |
| Advanced shapes/channel formats | no bandpass/tilt/allpass or immersive mode in sparseEQ | explicit gaps; no implied parity |

## Prioritized findings and reproducible evidence

1. P1 native factory overflow: original327 Mac and Linux default-stack failures
   preserved; independent2786/392 now merged with focused actual Mac passes,
   exact cloud full20 and merge-tree proof. No duplicate source fix in390.
2. P1 native qualification fixture overflows: independent2787/393 corrected both
   engine fixture construction causes with unchanged assertions/production.
   Default engine527pass/release2pass, cloud full20 and actual merge-tree proof;
   coordinator full Mac20 now passed on98/treec8e93 with defaultstack unset. No native deadline guarantee.
3. P2 narrow-notch display defect: confirmed and selected for this source repair.
   The old line hid actual notch depth. Three focused model tests now pass,
   including the before-failing case; the independent504-query PCM audit passes.
   This addresses the limited previous numerical coverage without asserting
   reference-product sonic parity.
4. P2 evidence gap: new analyzer controls/workflow still require the finite native
   compact-layout/focus/level/listening session. Do not substitute cloud renders
   or old gesture acceptance for those results.
5. Feature gaps above are proposals, not bugs or selected implementation by fiat.
   Multi-select, A/B, axis/Q workflow and clear analyzer scales are candidates for
   coordinator prioritization after concrete numerical/native findings.

## Selected finding: narrow-band response display (P2)

Reproducer:48kHz, one enabled1kHz notch, Q18. The actual registered coefficient
response is below1e-5 at center, but the old96log-point displayed model reaches
only normalized y0.36088595 (−6.6775dB in the ±24dB plot). Its nearest sample is
1014.596Hz. The deterministic test exercises the actual registered response and
the production graph-model data; it fails before repair. This is a misleading
display, not evidence that the processor fails to notch the sound.

The selected repair samples enabled band centers and two Q-scaled shoulders,
plus at least64 logarithmic background samples. It never exceeds the existing
256query/graph limit: min(96,256−3×enabled_count) background points, then at most
3×64 anchors, sorted/deduplicated. All five banks contribute anchors so switching
the displayed channel does not silently lose another audible bank's feature.
High authored frequencies use the actual0.49×rate DSP clamp. Each point's x uses
its actual logarithmic frequency, rather than assuming uniform array indices.
All64 centers fit; no band cap or graph API/DSP/budget change.

Both channel curves are cached by effective authored state and rate. Sampling,
preparation and logarithms occur only on invalidation; steady repaint clones the
bounded cached curves for the existing shared widget. Persistent cache payload
is at most2×256×8=4096bytes, versus the previous96×8=768byte magnitude cache;
returned widget models have their own same-size curve payload. This is explicit
control/UI memory, not a callback or global-memory claim. The finite curve still
interpolates between samples: this repair does not promise a uniform error bound
for arbitrary overlapping filters or all frequency intervals.

Regression covers original notch depth/location, all64band centers across five
rates/banks, sorted/deduplicated valid shared models, disabled flat response and
rate-clamped near-Nyquist features. Native pixels/resize visibility remain in the
finite coordinator checklist. No accepted2729 gesture semantics change.

## Measurement protocol fixed before the selected display repair

- Independent PCM response: deterministic impulse and coherent/noncoherent tone
  projections, six supported shapes, all cut slopes, both gain signs, Q extremes,
  low/mid/high frequencies, supported rates8/44.1/48/96/192k. Refuse queries at or
  above Nyquist explicitly; distinguish authoredfrequency from rate-clamped DSP.
- Tolerance:0.05dB relative magnitude error where oracle>-80dB; below
  that use absolute linearerror<=1e-5. Predefine finite impulse truncation/tail
  residual criterion; a not-yet-decayed high-Q response is inconclusive, not a
  reason to widen tolerance. Stereo lane/crosstalk and opposite-polarity probes.
- Analyzer calibration: bin and nonbin unity/half-amplitude tones, bass/noise/
  transient/silence, pre/post trims and bypass. Keep existing kernel calibration
  and decay tolerances; distinguish band energy/leakage from tone peak level.
- Automation: partition invariance, rapid authored revisions, loop/seek clocks,
  stable IDs/history and export equivalence; retain maximum sample-step/RMS and
  spectral sideband metrics with input/transition settings. No physical zipper or
  click verdict without listening and reference conditions.
- Cost: existing64node+64lane allocation and release density fixtures, all project
  layer cases0/32/128/256; record hardware, rate/buffer, warmups/trials/outliers,
  memory accounting and callback total/net allocations. No timing assertion or
  claim these prove driver/GPU deadlines.

## Finite native review session, later coordinator serial lane only

1. Bind exact corrected optimized source/tree/executable/build and original CI
   receipts; safe current app state before any authorized replacement. Record
   unavailable reference instead of requesting a purchase or changing ownership.
2. Disposable current-format project, original theme: bell/edge cuts, fine drag,
   exact values and Q/shape/slopes; record task time/misclicks and bounds.
3. Numeric/text focus, graph navigation, bypass/listen/delete and Undo/Redo;
   switch track/device midgesture, Escape/focus loss, confirm stable identity.
4. Input/output calibrated tones, bypass/Return/anti-phase/silence/stale cases;
   distinguish filter response scale from spectrum dBFS scale.
5. Compact880x540/expanded1500x650, long numeric values and automation row;
   project-panel resize/outside release/cancel/collapse with every selected layer.
6. Save/reopen disposable project; compare authored values, automation IDs and
   history behavior within the documented current format. No legacy migration.
7. Level-matched generated fixtures (sweep, bass, noise, drum-like transient),
   disclose phase/gain/Q laws; legally available installed Pro-Q4 only if present.
8. Raw release64band and all256layer profiles plus sustained callback/GPU evidence
   if instrumentation exists; missing physical telemetry stays UNRUN with owner.

Cycle1 remains open until comparison/baseline and selected findings have concrete
verified candidates or explicit actionable remaining blockers. Cycle2/2785/391
must use an independent reviewer after cycle1's candidate; not started here.

## Current verification ledger and remaining scope

A focused processed-impulse audit extends the original cut regression (which is
retained with its original settings, three queries and1e-5absolute tolerance). It audits all six supported
shapes, all four cut slopes, rates8/44.1/48/96/192kHz, Q0.1/18, frequency20Hz,
1kHz and high-band settings, positive/negative gains and opposite-polarity stereo.
It compares actual rendered PCM complex projections against the coefficient
response at center and two off-center queries. Tolerances were fixed before the
repair:0.05dB above−80dB, absolute linear1e-5 below, settled last256samples <1e-8.
The first131072sample window was insufficient for the192k/20Hz bell; that failed
oracle-settling result is retained as inconclusive. The20Hz window is now1048576
samples, other windows131072, with every tolerance unchanged. No DSP/source/test
limit was relaxed and no failure is relabeled as a sonic defect or a pass.

The three response-model regressions pass, including the original before-failing
notch case. The PCM matrix passes504queries with maximum relative error
0.00074261dB above−80dB and settled tails/opposite-polarity preservation; its
standalone unoptimized test took36.31s on this cloud executor. These are scoped
working-source results; final committed full-CI receipts will follow. Original maximum64band runtime/storage measurements remain
in docs/performance.md, sparse eq17October (source238bcb85, actualM5Mac); these
historical isolated mean measurements do not qualify current physical deadlines.
Full allocation/automation/history/analyzer tests are part of required final CI.
Bass/noise/transient/sweep, rapid-edit sideband metrics, native task times,
misclicks/screenshots, physical callback/GPU behavior and licensed Pro-Q4 A/B
remain explicit native or follow-up measurements, owned by this open review and
coordinator lane. No external plugin binary is available here; sonic parity is
not claimed. Advanced feature rows are deferred proposals, not source bugs.

The focused source repair follows the repository's normal exact-check merge
policy. The review ticket remains open for the finite optimized native session
and the explicitly owned measurements above; a source merge does not complete
those criteria. Cycle2 remains queued for an independent reviewer after this
candidate; this author does not perform its independent acceptance review.
