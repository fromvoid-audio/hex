# smooth clip editing — 9 October 2026

User reported intermittent flicker while changing position/size in the clip panel
and slow clip resizing. This finite optimization unit targets the repeated UI
work without changing canonical edits, undo, compilation or callback ownership.

## requirements and acceptance

- Reuse waveform paint geometry across root view rebuilds when source, mapping,
  visible interval, bounds and display scale are unchanged. Bound cache entries
  and total stored column work; retain source identity safely and invalidate
  actual source/geometry changes. Preserve black stereo extrema, constant-signal
  tracing, source looping and clipped views.
- Preserve original clip hit geometry during resizing, but avoid preparing the
  invisible original waveform under the live preview.
- Repeated pointer input resolving to the same snapped trim candidate must not
  request another root redraw. Modifier changes, free resizing, cancellation,
  authoritative release and one-step Undo must retain their contracts.
- Keep parameter colors stable during worker handoff; pending/stale edit gates
  still prevent invalid writes. Accepted state remains canonical, not optimistic
  project mutation.
- Run targeted regression checks and a release waveform-preparation comparison
  with fixture, workload, hardware, warm-up and timing reported. Deliver a fresh
  review app. User native checks establish actual flicker/smoothness; source
  tests and microbenchmarks cannot establish physical input/paint latency.

## investigation and deferred work

Source trace found root notifications rebuild Arrangement, clip detail and mixer.
Waveform painting re-queries extrema and tessellates stereo paths per paint,
including a fully transparent original clip while trimming. Static detail
waveforms also repeat work during unrelated pointer motion. Clip-panel controls
switch to muted colors during pending work, causing a visible color pulse.
These are concrete repeated paths; exact native flicker attribution is pending.

MIDI handles retain an Arc<Project>, not a deep project copy. The initial trace's
clone-cost suspicion was incorrect; replacing that with a deep tempo-map copy
would not demonstrate an improvement.

Overview invalidates on every project revision and scans all clips; visible
track rows scan their clip collections; canonical worker edits clone/validate
and prepare a revision before committing. Those remain profile-driven follow-ups.
Do not remove precommit validation, bypass history, publish incomplete graphs,
or broaden this PR into a speculative scheduler/project-model rewrite. Full
100-track input-to-visible latency and sustained playback performance remain
open gates in performance.md.

User clarified the flicker is details text and bottom status text. This unit also
keeps pending/revision status in stable-width slots so worker/admission updates
cannot repeatedly shift adjacent status text. Pending/applied audio state and
errors must remain truthful and visible; stabilization is not permission to hide
failed or delayed edits.

## retained rendering boundary

The editor owns the shared geometry cache. It retains at most 128 entries and
65,536 aggregate device-pixel columns, with at most 4,096 columns for one waveform.
Entries retain source Arcs and use pointer equality; bounds, display scale,
visible interval and clip/source mapping are part of the key. LRU eviction and
project replacement release cached ownership on the UI/control side. No cache
or new work is introduced in the audio callback. Cached paths still require
GPUI vertex cloning/scaling, scene submission and GPU drawing each paint; a
preparation cache hit is not a measured frame or input-to-visible deadline.

Resize overlays remain display-only. Identical validated preview candidates
skip root notifications; free movement and changed snap/modifier candidates
still redraw. Release remains one authoritative canonical edit. The original
clip keeps its hit shell while hidden grid, waveform and fade paint children
are omitted beneath the overlay. Accepted controller state and audio publication
remain separate and retain their existing error/backpressure contracts.

## verified milestone and native review

253 feature-enabled UI tests passed, 2 ignored. New regressions cover source/
mapping/bounds cache invalidation, a 13-waveform working set, entry/column
budgets, identical trim candidates, omission of hidden resize waveforms, and
stable status layout. Existing gesture and canonical dispatch checks still pass.
Strict UI lint, architecture/UI-style/GPUI guards, formatting and the optimized
desktop build pass. Three release component samples and their limits are recorded
in performance.md; no full-frame or device deadline claim follows from them.

Review app: `artifacts/hex-smooth-clip-20261009.app` (release build), identity and
logs: `artifacts/smooth-clip-manifest.json`. Running older apps were preserved.
User native checks:

1. Repeatedly adjust audio position/length/source values; details should keep
   their colors and neighboring bottom status text should not move sideways.
2. Resize audio and MIDI clips with grid snapping, then with Command held;
   compare responsiveness and confirm the committed edge matches the preview.
3. Resize/zoom a detailed waveform, cancel with Escape, Undo/Redo, and open a
   different project; check no stale waveform remains.

Native flicker/smoothness verdict remains pending. Overview invalidation,
per-track visible-clip indexing, broad view invalidation and control-side edit
preparation remain future measured optimization units.
