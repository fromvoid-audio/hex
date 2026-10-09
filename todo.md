# Hex todo

Target: installed Ableton Live 12 Suite.app — 12.4.6 (2026-09-10_0de5c8fa9a),
macOS arm64 first. See docs/plan.md for milestones and acceptance gates.

## Current implementation focus

Long-project media (9 October): shared conversion kernels and captured-byte source
verification are source accepted. All 47 media tests, strict media Clippy, full
Session regressions, desktop build and workspace checks pass. Independent debug/
release hash, cross-page PCM, halo conversion and edit-refusal checks pass.
Borrowed engine rendering now passes an independent scalar oracle in debug and
release: 12,895 frames each with warmed/missing pages, source phase, page/loop
seams, mix order, one-shot exhaustion and overflow rollback. The same resident
100-distinct-source profile preserves its PCM checksum; short timing differences
remain provisional. The actual stream worker/cache is now headlessly source
accepted. Eleven stream tests, strict Clippy and final root debug/release worker
PCM checks pass. A real 100-track fixture admits separate 17M-frame sparse files,
warms 100 far pages, matches an independent mix oracle and records zero warmed
misses; all 1,024 buffers are accounted at joined shutdown. Source mutation with
restored mtime, resource refusal, rate-pair kernel reuse, distant float NaN,
cancellation and queue-pressure cases pass. Cache/request deletion avoids
accumulating dead entries, with wraparound/turnover regressions. See
docs/page-delivery.md and docs/performance.md for exact workload/limits and
artifact provenance. Canonical Arrangement resident/streamed compilation now
shares one source resolver and the existing clip/MIDI compiler. Five focused
tests, full engine regressions and strict Clippy pass; root actual-worker debug/
release checks match 60,452 resident PCM frames, including three explicitly
scheduled loop-boundary seeks. Native Session/audio allocation regressions and
desktop build pass. Linear Renderer does not automatically loop; compiled loop
metadata and explicit seek continuity are covered, streamed Playback scheduling
remains open. The stream control/page split now passes debug/release tests and
allocator probes. Page operations own no Thread/wake/join; matched shutdown
accounts for buffers, and mismatched pairs return both owners intact. Actual catalog ownership now follows opaque leases and the page endpoint;
control may be released while leased sources remain usable. Page destruction
and final worker joining still belong off realtime after quiescence. Session mixed-source preparation/rendering and automatic
streamed Arrangement loop scheduling are now implemented headlessly: independent
real-worker debug/release checks match 110,758 frames with zero warmed misses
and complete eight-buffer shutdown. Native publication refuses unsupported
streamed graphs before consuming a slot/revision. Audio and stream allocation
probes pass, including streamed Session launch/page-boundary/EOF rendering.
Transactional late-failure, malformed-page, mixed MIDI cold-miss and finite
Playback loop regressions now pass with full engine tests and strict Clippy.
Next: real catalog leases/endpoint retirement and prewarmed native publication.
Sustained advancing streaming remains open.
See docs/media-streaming.md. Native resident limits remain enforced; short-source
microprofiles do not prove long 100-track playback. Native close Cancel/Discard acceptance now passes on the fixed review build;
streamed device/audio/UI acceptance remains open.

Session audio source checkpoint (9 October): independent one-shot sources,
schema 12, worker import/history/save/reopen and frame-anchored mixed playback
are implemented. Root affected-crate suites pass (181 GPUI-support UI tests,
one ignored); strict targeted Clippy, format and architecture/style guards pass.
Fresh-service Save Current/reopen PCM and failed-import rollback pass. Real
retirement-queue backpressure preserves Stop and delayed fresh project ownership.
Compact source controls remain visible while editing; approximately 1 ms nudges
have Alt one-frame precision. Production GPUI event handling also passes
successful-Open/edit-coalescing/failed-Open/admission/new-generation regression
with actual canonical gain changes; native dialog/publication timing is still
unverified. See docs/session-audio.md for evidence and remaining
gates. The documented 100-track shared/distinct audio workloads and scene-index
measurements now pass headlessly. Prepared callback allocation/retirement probes
now pass in debug/release for
active audio, command bursts, edits/backpressure, fresh install and small sounding
MIDI switches. Next: remaining quantized/dense/error callback paths and native audio
input/save/launch/listening and UI coalescing acceptance.
Streaming, warped loops and device chains remain required later; current short
resident-media limits do not satisfy long 100-track projects.

The 100-track fixture and first shared-mode executable are now runnable. Two
root shared/distinct exact-PCM regressions and strict Clippy pass. Preliminary
shared and distinct preparation/edit/route/render timings are retained in
docs/performance.md with outliers, source/binary manifests and reporting limits.
Same-clip retrigger now advances elapsed audio and passes a scalar reset oracle.
Root corrected switch parity and reran both modes after indexing: all 100 switch
samples and the separate retrigger series pass PCM/reset oracles. Final hashes
and logs are recorded in artifacts/session-audio-indexed-manifest.json.
A new 65,536-cell/256-track probe shows a 32-action median
of 1.184 ms versus 0.375 ms with 512 cells before repair. Prepared scene indexing
now reduces the dense burst median to 0.391 ms; full engine tests/strict Clippy
and root audio/Session boundary tests pass. Canonical ordering, interleaved cells,
reordered scenes and empty ranges are covered; maximum index payload is 530,432
bytes. Native input/deadline and worst-case clip/voice gates remain open.
Project-replacement GPUI regressions now live in a focused controller test module;
the extracted test, strict Clippy, formatting and policy guards pass.

Close report follow-up (9 October): retained editor/untyped-window completion
explicitly stops and removes the confirmed window. Two real GPUI test-platform
lifecycle regressions cover unsaved Cancel/Discard after root replacement and
Cmd-Q. Feature-enabled UI suite: 162 passed, one ignored; strict UI/desktop Clippy
and ordinary desktop build pass. Native confirmation remains pending because
the Mac is locked; preserve running apps and their unsaved projects. See
docs/issues.md and artifacts/close-window-lifecycle-regression.log.

Canonical Session inventory and exact precommit PreparedRevision publication are
source accepted. Borrowed per-track Arrangement rendering, common mix stages,
bounded additive Session MIDI accumulation and mixed per-track ownership are
accepted in the engine. Native prepared publication/action queues and compact
canonical Session UI are implemented in source. Physical 100-track
deadlines and native Session interaction remain unverified.

Engine-owned headless SessionPlayback over the accepted revision passes mixed
routes, separate Session/Arrangement clocks, bounded pending actions, outgoing
releases and render rollback acceptance. Periodic loop-aware tempo is accepted
in source. Graph/phase handoff and native publication/retirement are accepted in
source. Capable native Playback now uses the mixed kernel from its initial graph;
legacy finite playback/export stays independent. Canonical Session grid and
owner-aware MIDI editing are integrated; native click-through remains unverified.

Revision handoff accepted (9 October): copied phase/gates, stable IDs, carried
voice cuts, base-clock Seek reset, shared release capacity, releasing gates,
quantized retriggers, source/scene/reorder/rounding and late-chunk rollback pass.
All 135 engine tests, strict all-target Clippy, formatting, 41 Session-layer tests,
desktop build and source guards pass. Gain-edit and baseline 100-track profiles
retain independent PCM/partition checksums; measurements are in performance.md.

Native publication accepted (9 October): explicit unavailable capability,
dual clocks, transactional handoff/retry with one candidate gate, coherent
rejection acknowledgments and control-side shutdown reclamation. All 139 engine,
22 audio and 41 Session-layer tests pass; strict all-target Clippy passes.
Root adds queued scene/StopTrack/return partition PCM parity and deferred Stop
acceptance, plus atomic refusal of out-of-domain legacy clock promotion.
Canonical Session grid, typed UI action queue and owner-aware Session piano roll
are implemented in source (140 UI tests; root acceptance log
artifacts/session-grid-root-tests.log). Native interaction, device recovery of Session route state,
allocator auditing and physical deadlines remain open.

Route/pending telemetry is source accepted: 256 fixed atomic slots, coherent
accepted-revision snapshots and committed engine change stamps; unchanged
control blocks skip slot scans and render commits track local due-change flags.
Root engine/audio/Session tests and all-target Clippy pass. UI indicator
consumption is source accepted, including current/pending same-cell retriggers,
committed Silence markers, bounded view retention and channel-safe fixed buffers.
Root passes 158 UI tests (one ignored), strict Clippy, desktop build and guards.
Ticketed rapid launch/space intent is source
accepted: root passes 149 UI tests (one ignored), including latest-token ordering,
refusal, priority Stop and lifecycle reset. Native interaction remains pending.

Ordered priority Stop is source accepted: protect against full older queues while
preserving successfully admitted Play/launch after Stop. Root passes 18 audio
unit and 16 integration tests, including deterministic captured-prefix races.
Its cutoff contract is in docs/runtime.md; native rapid Stop→Play remains pending.

Compact Immediate/four-quarter launch quantization selector and typed pending
launch cancellation are source accepted, using existing engine semantics. Root
passes 159 UI tests (one ignored), strict Clippy, desktop build and policy guards.
Meter-aware bar quantization and saved/per-clip launch settings remain deferred.
The reverified Live build and planned reference boundary/meter/launch-mode matrix
are in research/observations/session-launch-quantization-001.md; native timing
outcomes remain unobserved.

Latest review bundle: artifacts/hex-session-preview-20261009.app (separate app
identity). It includes accepted Session controls/markers, ordered Stop and close
lifecycle hardening. Native acceptance remains pending; the running development
app and its unsaved project were preserved.

Worker Session workflow accepted (9 October): all 42 Session tests and strict
Clippy pass. Fresh-service save/reopen preserves exact project/IDs/source values
and prepared inventory; mixed render parity crosses loop/one-shot boundaries,
and canceled launches remain canceled past their due frame. Logs:
artifacts/session-workflow-root-{tests,clippy}.log. Native dialogs and output are
separate open gates.

Session keyboard navigation/Enter and a pinned rightmost scene column are source
accepted (170 feature-enabled UI tests, one ignored). Test-platform key dispatch
and geometry at 880/1440 px pass; native input/appearance remains open.
Canonical scene duplicate/reorder is source accepted (99 project/43 Session
tests, strict Clippy), including fresh IDs/independence/history/limits and
worker save/reopen/render parity. Compact scene organization controls and keyboard
dispatch pass source acceptance: Cmd-D duplicates a scene-only selection;
Cmd-Alt-Up/Down reorder it. Worker completion preserves newer selection/mode.
Full feature-enabled UI suite: 176 passed, one ignored. Terminal queued-worker
rejection clears duplicate selection intent. Independently owned Session audio
one-shots are now implemented across model, schema migration, prepared playback/
handoff, worker and compact UI. Root source acceptance is detailed at the top of
this document and in docs/session-audio.md; native acceptance remains open.
Successful Open now installs fresh runtime ownership independently of reused
IDs; normal edits and recovery retain inheritance. Callback same-ID reset,
retirement backpressure, Stop and control-side lifetime tests pass; GPUI event
coalescing is source-tested, and native project replacement/timing remain open.

Session deletion/focus is source accepted: clear MIDI/stop cells via canonical
commands; empty/stale cells cannot fall through to scene deletion; grid/header
clicks restore editor focus, and rejected edits preserve selection. Native
interaction remains pending. See docs/plan.md for the current finite work unit.
The mixed callback tempo/source/gain transfer follow-up passes; all 23 audio
tests pass (native-session-transfer-audio-tests.log).

Periodic-clock acceptance includes checked center-inverse loop anchors, exact
seam lower bounds, borrowed ongoing MIDI conversion, quantized kernel routing,
independent inverse oracle, domain exhaustion and partition checks. Canonical
100-track constant/`--loop-tempo` workloads match an independent finite compiler
PCM oracle; scoped results are in docs/performance.md.

The refreshed inventory-build native editor passes startup acceptance (`project
ready`, `rev 0 applied`, stopped transport, 44.1 kHz stereo); launch, listening and
sustained deadlines remain separate gates. The borrowed render/mix extraction
passes mixed-source PCM and partition regression checks.

Default native canonical editing, background WAV import, prepared playback,
project open/Save Current/create-new Save As, clip editing and background PCM16
project export, clip fades and off-UI canonical startup are implemented in source.
Canonical tempo/meter maps, meter-aligned rulers and opt-in pointer move snapping
are implemented. Canonical MIDI source/start/loop metadata, schema11 persistence with strict
schemas 1–10 migration, and finite looped playback/export pass workspace
tests. Prepared thumbnail/source-control native checks remain tracked below. Original bounded sine
playback/export and transactional prepared native MIDI admission are implemented;
native instrument clips and compact piano-roll controls now pass creation, parameter editing, history and save/reopen acceptance. Direct note drag/resize now passes native save/undo checks; dense/minimum-window
interaction now passes; held-drag Escape remains unverified.
A rebuilt isolated bundle passes fractional-meter and schema9 audio-save acceptance. These checks cover source contracts; physical
device failure/restart, listening and fresh native recovery acceptance remain open.
Fresh native inspection confirms the titlebar reservation and compact mixer.
Add-track, balance/main gain, undo and stopped default-output restart pass in an
empty-audio project; listening and physical failure remain separate acceptance.
Native export revision 1 rendered 507,150 stereo
44.1 kHz PCM16 frames and matched the independent linear render byte-for-byte. Project save
durability failures now distinguish a published file from a pre-publication
failure; the new error path is covered by targeted tests, not native interaction.
Fresh native Save/Save As dialog acceptance and dense-fixture inspection remain
open because interaction with the active app was interrupted. Sustained
device/listening checks also remain open.

Selected-group pitch/gate/velocity buttons and focused octave/group gate keys
are implemented in source; native saved-value acceptance is pending. Continuous
MIDI Draw mode has local insertion/erasure preview, pitch lock, one atomic release
transaction and cancellation; its remaining native saved-value acceptance is
pending. Atomic insertion/mixed commands, multi-selection, group drag/delete
and focused group arrows are accepted prerequisites. See the final active
checklist below and research/observations/midi-draw-native-001.md.

Unsaved-state tracking and ordinary close/Cmd-Q protection are implemented and
the bootstrap close-dialog answer bug is repaired with native Cancel/Discard
acceptance. Preserve
this tracker as the current status; milestone sections below retain historical
check results and the remaining broader roadmap.

## M0 — Setup completed on 8 October 2026

- [x] Inspect workspace; empty directory, no Git checkout or existing work.
- [x] Record installed Live version, architecture, executable hash, and imports.
- [x] Create dependency-free Rust engine and CLI workspace.
- [x] Implement frame timeline, tempo conversion, stereo clips and source loops.
- [x] Implement track gain/mute and play/pause/seek.
- [x] Render a generated demo to WAV without overwriting existing files.
- [x] Document requirements, architecture, research protocol, and staged plan.
- [x] Pass seven engine tests, formatting, and strict workspace Clippy.
- [x] Independently verify demo WAV metadata, silence, stereo channels, and repetition.
- [ ] Manual listening acceptance of artifacts/hex-demo.wav.

## M1 — Immediate research queue

- [x] Install/select Ghidra 12.1.4 and OpenJDK 21.0.12.1; record exact arm64 input/tool provenance.
- [x] Create ignored arm64 analysis database tied to unchanged reference SHA-256.
- [ ] Index executable functions/imports and map a narrow clip-processing path.
- [x] Decompile selected fade-controller callback/target; document addresses, hashes and unresolved control-flow/type warnings.
- [ ] Create user-owned Live CLIP-001 fixture; render unwarped impulse with fades off.
- [ ] Establish export alignment, onset/end frames, and comparison tolerances.
- [ ] Run CLIP-002, MIX-001, TRANSPORT-001, ARRANGE-001 from docs/research/workflow.md.
- [ ] Add reference-backed regression fixtures and update confirmed behavior matrix.
- [ ] Resolve prototype overlap/stop/loop behavior using measured evidence.

## GPUI real-time demo — implemented 8 October 2026

- [x] Choose GPUI and CPAL/CoreAudio native system boundary; pin dependencies.
- [x] Implement a bounded 32-command SPSC channel and priority Stop flag.
- [x] Preload fixed stereo demo sources before callback startup.
- [x] Add GPUI Arrangement/Session views, transport, tempo, mixer, and meters.
- [x] Add bar-quantized two-scene launch and visible device/timing status.
- [x] Test transport, launch boundaries, partition invariance, invalid controls,
      mute, and full-queue Stop. Total: 12 tests across workspace.
- [x] Build desktop and local macOS app; pass format and strict Clippy.
- [x] Run release renderer microbenchmark; record baseline in docs/performance.md.
- [x] Inspect rendered window and exercise Space, Session, scene selection,
      mute/gain/tempo, and actual CoreAudio callback activity.
- [ ] Manual listening acceptance.
- [ ] Ten-minute device/UI stress test and callback allocation instrumentation.
- [ ] Device selection/recovery and disconnect/restart lifecycle acceptance.
- [ ] Add accessible names/roles for GPUI custom controls.

## Next usable milestone — editable audio project

- [x] Canonical project model with stable IDs and asset ownership.
- [x] WAV import with sample-rate handling and missing-asset errors.
- [x] Block-scheduled compiled playback with bounded graph replacement/reclamation.
- [x] Create/delete tracks; place, trim, move, split, and duplicate clips.
- [x] Versioned save/load and undo/redo.
- [x] Cached waveforms, timeline ruler/zoom/seek and configurable loop region.
- [ ] Continuous scrub and drag loop handles; broader native interaction acceptance.
- [x] Canonical linear clip fades and master gain/output peaks.
- [ ] Gain smoothing and final output policy/listening acceptance.
- [ ] Recording, MIDI, automation, routing, devices, and export gates in docs/features.md.

## Later milestone gates — incomplete

- [ ] M3 full acceptance: Session/MIDI/original sine and immediate/four-quarter
      launching are source implemented; recording, broader quantization/launch
      modes, native workflow and reference parity remain open.
- [ ] M4 full acceptance: versioned projects, editing/history and rolling recovery
      are implemented; relinking, automatic crash recovery and automation remain open.
- [ ] Investigate minimal user-authored .als fixtures before any compatibility importer.
- [ ] M5: Routing, latency compensation, resampling, warp, selected DSP devices.
- [ ] Decide external plugin/native-library policy before plugin-hosting work.
- [ ] M6: Expand measured compatibility, packaging, and other platforms.
- [ ] Choose distribution license and source-control/publication policy when requested.

## Known gaps

One selected Live fade-controller callback/target has been decompiled with
unresolved warnings; controlled reference renders and confirmed DSP parity are
still missing. The default desktop uses canonical imported audio and authored
MIDI/Session projects; --demo retains generated media separately. Persisted
tempo/meter maps, finite Arrangement and ongoing Session MIDI playback exist.
Recording, automation, warp, generalized routing/latency compensation and device
hosting remain planned. Source-rate conversion is implemented independently of warp.
Engine tests establish Hex contracts only. Device stress, listening approval, and
Live parity remain open. Native dependency future-compatibility warnings exist
for block 0.1.6 and proc-macro-error2 2.0.1; strict checks of Hex's code pass.

## Product and maintainability rules — 8 October 2026

- [x] Adapt comprehensive AGENTS.md from the supplied PhotoCraft guide.
- [x] Inspect/pin PhotoCraft reference patterns and document audio-specific differences.
- [x] Require lowercase app-owned text and route it through the display helper.
- [x] Embed Ioskeley Mono v2.1.0 with license, provenance, and startup validation.
- [x] Split hex-engine, hex-audio, hex-ui, hex-desktop, and hex-cli responsibilities.
- [x] Add per-crate purpose/dependency/non-goal documents and architecture/style checks.
- [x] Final rebuilt-app typography and interaction acceptance after the split.

## Frame design — Yield appearance and Live density

- [x] Create editable arrangement/session study using Yield's current palette.
- [x] Use embedded Ioskeley Mono and lowercase product text throughout the design.
- [x] Add minimal-dead-space rule to AGENTS.md, UI guide and Frame styleguide.
- [x] Tighten transport, browser rows, track lanes, mixer and device dock.
- [x] Port compact composition/Yield styling into the native demo renderer.
- [ ] Real waveform/media display and device functionality beyond the demo.
- [ ] Adapt resizing/collapsible panels and test small desktop viewports.
- [x] Export portable Frame project and both previews; four-state structural checks and six declared action scenarios pass.

## Frame reference-layout refinement — 8 October 2026

- [x] Create Session and Arrangement concepts close to the two supplied screenshots.
- [x] Use Yield-derived softer charcoal surfaces and a 64-color smoky clip/element library.
- [x] Include browser/groove pool, transport, routing/send/fader widgets, returns,
      right-side Arrangement controls, MIDI editor and device dock.
- [x] Export editable Frame project and both 1900 × 1120 views and color-panel preview; inspect them.
- [x] Check five prototype states and eight declared action scenarios; verify palette uniqueness and label contrast.
- [ ] Native port, full widget interaction, responsive resizing and accessibility acceptance.

## Frame color beauty pass — 8 October 2026

- [x] Review excessive coral repetition, pale waveform glare and muddy/blue chrome trials.
- [x] Preserve earlier trials; r36 keeps neutral charcoal with four vivid core accents: violet, blue, mint and orange.
- [x] Refine 64 coordinated clip/element colors with restrained lightness and richer hues.
- [x] Replace browser dark tag bars with 36 individually outlined compact chips.
- [x] Verify 3,955 non-tag elements preserve r32 geometry and actions; 3,993 total elements.
- [x] Inspect both views and palette; six structural checks and eight actions pass.
- [x] Confirm 64 distinct colors, minimum library label contrast 4.5247:1 and lowercase text.
- [ ] User visual acceptance and native GPUI port of this revised theme.

## Editable project foundation — implemented 8 October 2026

- [x] Add hex-project with a clear control-side responsibility and dependency policy.
- [x] Stable typed track/clip/asset IDs, preserved through undo/redo branches.
- [x] Transactional create/delete/rename tracks, gains/mutes, asset references, place/move/trim/delete clips.
- [x] Bounded snapshot undo/redo; no PCM in history; rejected edits preserve state/redo.
- [x] Compile canonical metadata with exact resolved media into the offline renderer.
- [x] Render explicit trimmed source regions with correct loop/one-shot boundaries.
- [x] Exercise project edits in CLI edit-demo; preserve original demo WAV bytes.
- [x] Pass 22 workspace tests, formatting, strict Clippy, architecture/style guards and desktop build.
- [x] Inspect compact native views; verify playback, meters, scene launch, browser collapse and icon tooltips on initial build.
- [x] Repair mixer clipping and narrow-label layout found by visual inspection.
- [ ] Final rebuilt-app gain/mute/browser and minimum-size interaction acceptance (native automation interrupted).
- [x] Connect canonical editing state to native prepared playback.

Next ordered step: bounded WAV decoding/import and resolved media bank, then
real-time compiled playback publication/reclamation, native clip editing and
versioned persistence. Encountered constraints are tracked in docs/issues.md.

## Modern CPU, scale and latency — requirements added 8 October 2026

- [x] Record measurable large-project/UI targets and modern CPU strategy.
- [x] Document latency propagation, alignment, prepared Play and physical limits.
- [x] Build 100-track / 10,000-clip sparse and active-loop fixtures; profile renderer and history.
- [ ] Measure end-to-end UI feedback and sustained native device deadlines at that scale.
- [ ] Specify bounded real-time workers and independent-path scheduling; preserve
      serial device dependencies and safe graph ownership/reclamation.
- [ ] Implement latency-aware native device contracts and graph compensation,
      including MIDI/automation, racks, sidechains, sends and changing latency.
- [ ] Define/test prepared Play, seek/loop pre-roll, readiness and explicit
      low-latency monitoring without silent bypass or lost first transients.
- [ ] Benchmark 500 lightweight native instances and one 32-device serial chain;
      add reproducible mixed VST3 fixtures after the hosting boundary decision.
- [ ] Measure safe SIMD dispatch/fallbacks on Apple Silicon and x86-64 hardware.
- [ ] Pass sustained audio/UI stress and physical request-to-audible loopback gates.

Requirements are in docs/performance.md and docs/latency.md. None of these new
performance or compensation gates is established by the four-track demo.

## Imported audio and project files — 8 October 2026

- [x] Separate hex-media: bounded PCM16/24/32/float32 WAV decoder and malformed/limit tests.
- [x] Cache bounded stereo waveform peak summaries without modifying source PCM.
- [x] Atomic metadata import command; one-step undo/redo with stable IDs.
- [x] Validate snapshot reconstruction, IDs, limits, media references and intervals.
- [x] Separate hex-storage: schema-2 complete metadata, strict schema-1 migration/future refusal.
- [x] Sync temporary document and publish create-new target without overwriting existing files.
- [x] CLI render-wav, import-project and render-project; bounded resident assets and streamed output.
- [x] Skip inactive clip spans per render block; scalar-oracle partition/seek/mix regression.
- [x] Prepared owned playback with bounded publication/retirement, applied revision and priority Stop tests.
- [x] Native prepared playback adoption with bounded retirement and off-RT disposal.
- [ ] Physical disconnect/stop-failure and sustained device acceptance.
- [x] Canonical application controller and background import/compile integration.
- [x] Native keyboard clip editing, cached waveform views and create-new save controls.
- [x] Worker open-existing workflow, atomic candidate validation and save-as path resolution.
- [x] Native open and full-width waveform inspection.
- [x] Token-checked existing-file Save Current and one rolling recovery generation in storage/session.
- [x] Cmd-S associated-save/untitled-save-as and Cmd-Shift-S create-new Save As UI source.
- [ ] Fresh native Save/Save As interaction; asset relinking and crash recovery remain unimplemented.

The default desktop now uses the canonical project; --demo retains the generated
prototype. Native import, nudge/trim, Play/Stop and save-new were exercised.
The saved file contains the expected edited source/timeline values. Visual review
found collapsed virtualized-row widths; full-width repair passed rebuilt waveform
inspection after schema-1 reopening and mixed-rate import.

## Next clip workflow

- [x] Canonical duplicate clip command, unique IDs and one-step history.
- [x] Cmd-D native dispatch/selection, bounded contiguous repeat intent and render equivalence.
- [x] Rebuilt native repeated contiguous duplication interaction acceptance.

Earlier native automation interruptions were followed by a successful rebuilt
project-open, waveform and duplication run.

## Ordinary close protection

- [x] Worker unsaved state and successful-save/open/failed-operation transitions.
- [x] Shared asynchronous Cmd-Q/window-close cancel/discard guard.
- [x] Protect pending worker/local/duplicate intents and freeze new prompt-time edits.
- [x] Targeted dirty, close-decision and accepted-continuation tests.
- [x] Native Cmd-Q unsaved prompt and cancellation retain edits.
- [x] Native confirmed-discard close of unsaved acceptance-test solo toggles.
- [ ] Recovery journal and OS/forced termination recovery (deferred).

## One-shot clip splitting

- [x] Canonical source-preserving split, stable left/new right IDs, single history step.
- [x] Preserve overlap summation order; renderer/history/persistence tests.
- [x] Cmd-E at playback cursor, returned-ID selection and explicit errors.
- [x] Native one-shot split after acknowledged ruler seek; looping split contract deferred.

## Source-rate conversion — 8 October 2026

- [x] Bounded windowed-sinc v1 conversion with bit-identical shared identity PCM.
- [x] Source/project frame metadata and conversion version in canonical history.
- [x] Strict schema-2 persistence with schema-1 identity migration.
- [x] Worker one-at-a-time preparation and separately bounded resident PCM.
- [x] Mixed-rate save/reopen/render continuity and changed-source failure tests.
- [x] Native 48 kHz WAV import into 44.1 kHz project and schema-2 saving.
- [x] CLI rendering of saved native mixed-rate fixture.
- [ ] Device/project rate conversion, large streaming media and content hashing.
- [ ] Live resampler parity, listening and sustained device stress acceptance.

## Timeline seeking — 8 October 2026

- [x] Actual-bound ruler pointer mapping and bounded ticketed Seek delivery.
- [x] Callback consumption acknowledgment and pending-seek split admission gate.
- [x] Priority Stop behavior, failed-admission ticket and geometry tests.
- [x] Correct single bubble-phase mouse dispatch after acceptance review.
- [x] Native ruler click and post-seek split acceptance on rebuilt app.
- [x] Timeline zoom/pan/fit and viewport-aware waveform mapping.

## Viewport and solo integration — 8 October 2026

- [x] Bounded viewport zoom/pan/fit, clip culling and cropped waveform mapping.
- [x] Fixed-size source interval aggregation; no per-column range vector allocations.
- [x] Canonical exclusive/additive solo, history and compiler gating.
- [x] Schema 3 persists solo; schema 1/2 migration defaults it off.
- [x] Native Up/Down selection and S/Shift-S solo acceptance.
- [x] 124 integrated tests, strict Clippy, guards, formatting and development bundle.
- [x] Repair and inspect track-name width; native button zoom and keyboard pan/fit.
- [x] Native ruler seeking/splitting and schema-3 split/solo save workflow.
- [x] Headless saved render: exact PCM16 source continuity across split, 507150 frames.
- [x] Native Command-Plus/Minus zoom aliases; Command-Equal remains layout-dependent.
- [ ] Listening acceptance.
- [x] Canonical loop region, strict schema-4 persistence and sample-exact wrap.

- [x] Frame r43: reduce clips to 24 choices; replace browns with coral/rose; preserve
  retained cool shades and add separate success/warning/error theme tokens. Six
  visual reviews and eight prototype scenarios pass; native port remains deferred.

- [x] Frame r44: four clip hue families (purple, blue, green, red), seven brightness
  and saturation variations each, eight utilities; brighter semantic signals.
  Six visual reviews and eight prototype actions pass; label contrast remains at least 4.5:1.

- [x] Frame r45: smooth each clip hue into a monotonic brightness gradient, with
  gentle chroma transitions; preserve core shades, UI colors and all geometry.

- [x] Frame r46: shift clip green toward yellow and purple toward red, preserving
  brightness ramps, blue/red shades, utility colors, UI tokens and geometry.

- [x] Frame r47: add vivid dark/bright and desaturated bright clip choices within
  the existing 36-choice palette; preserve core shades and UI geometry.

- [x] Frame r49: ordered vivid/soft gradients per family, further magenta purple
  and acid yellow-green clips; main workspace geometry and UI tokens unchanged.

## Canonical loop region — 8 October 2026

- [x] Validated half-open region, set/toggle/clear, atomic history and snapshot state.
- [x] Strict schema 4 and schema 1/2/3 migration to no region.
- [x] Prepared intro/seek/wrap/pause/Stop and block-partition continuity.
- [x] Stateless period repetition within caller buffer repairs tiny-loop scan cost.
- [x] Native Cmd-L, L, undo, visible wrap, schema-4 save and reopen.
- [x] Linear CLI export matches earlier split/solo render byte-for-byte.
- [x] 134 workspace tests, strict Clippy, guards, formatting and native bundle.
- [ ] Stateful devices/MIDI reset, loop fades, drag handles and musical snapping.

- [x] Frame r51: restore original violet; shift green further yellow and brighten
  it slightly; lift all bright soft swatches while retaining ordered lanes.

- [x] Frame r56: custom color-picker concept with up to 64 personal saved swatches;
  harmonious concept artwork independent of clip distinguishability. Native color
  editing, swatch management and persistence are deferred.

- [x] Frame r56: implement all four user notes; gray/orange core, brighter timing
  and browser indicators, one harmonious clip fill. Layout and actions unchanged.

- [x] Frame r57: adapt the supplied Yield reference colors; preserve all four
  implemented feedback requests and the existing workspace layout.

- [x] Frame r58: slightly brighter warm charcoal surfaces with a very subtle red
  tint; preserve all accent, clip and text colors and workspace geometry.

- [x] Frame r61: green secondary highlights, Yield-style library selections,
  a tiny background lift and darker tag rectangles; native port deferred.

- [x] Frame r62: raspberry tertiary accent replaces violet; preserve green
  secondary, orange primary, brighter backgrounds and dark tag rectangles.

- [x] Frame r63: shift secondary green subtly toward cyan-blue; preserve other
  colors, layout and actions.

## Native offline WAV export — completed 8 October 2026

- [x] Share fixed-block PCM16 WAV rendering and create-new publication through hex-export.
- [x] Add bounded native background export from the current compiled revision.
- [x] Cover finite/RIFF/collision/render failures, job reservation, snapshot stability and worker lifecycle.
- [x] Fresh native bundle exported revision 1: 507,150 frames at 44.1 kHz stereo PCM16.
- [x] Verify the WAV byte-for-byte against the independent linear render (SHA-256 952c8d29e303b10942d47300381b87b074d75828077e1b759f744b29a2bba681).
- [x] Cancel and reopen the native path dialog; confirm no export/dialog reservation remains stuck.
- [ ] Listening quality and sustained export throughput acceptance.
- [ ] Add progress/cancel, selected ranges, stems and overwrite only through separately reviewed contracts.

## Create-new project save durability — completed 8 October 2026

- [x] Distinguish pre-publication errors from post-publication cleanup/directory-sync failures.
- [x] Preserve the complete target and report its path when post-publication durability fails.
- [x] Keep session dirty after that error; test target reloadability with injected post-publication failure.
- [x] Test existing-path collision including dangling symlinks; targeted storage/session/CLI checks and strict Clippy passed.
- [x] Add bounded exact-byte FileToken/load_with_token/check_unchanged for external-edit conflict detection.
- [x] Define validated rolling recovery ownership, one-generation retention and optimistic TOCTOU limits.
- [ ] Recovery journal remains a separate, unimplemented workflow.

- [x] Frame r64: simulated segmented meter levels and peak markers in both views;
  playback-state fills clear on Stop. Native/live metering and animation deferred.

## Exact document tokens — 8 October 2026

- [x] Opaque bounded shared FileToken and single-read load_with_token validation.
- [x] Exact read-only conflict check preserving legacy schema and formatting.
- [x] Three regression tests; targeted storage/session/CLI 39 tests and strict Clippy.
- [x] Copied recovery-backup ownership, one-generation retention and token-checked atomic replacement.
- [x] Session document association and Save Current/Save As worker workflow.
- [x] Native Cmd-S/Cmd-Shift-S routing, create-new path dialog, and bounded save admission in source.
- [ ] Fresh native Save/Save As interaction acceptance.

- [x] Frame r65: opaque meter bars with orange upper levels and red peak caps;
  retain varied stereo levels and playback-state simulation.

- [x] Frame r66: replace strawberry tertiary with ice blue and shift mint
  toward cyan; preserve orange/charcoal and solid meters with peak colors.

## Accepted UI baseline — 8 October 2026

- [x] Save user-approved UI iteration 2 (Frame r67, retaining approved r66) in design/frame/iterations/02,
  including both views, portable project, theme, source and checked artifact hashes.
- [ ] Use iteration 3 as the reference for scoped native GPUI implementation;
  map controls to actual capabilities and verify the rendered native window.

- [x] Refine iteration 2 cool accents: cyan #00e8df, soft violet #9994bd,
  clearer blue #719cce. Preserve original approval snapshot and refreshed hashes.

- [x] Save all five iteration 2 state views with a comparison gallery, matching
  editable states, zero Frame visual issues and eight passing action scenarios.

## Dense native Arrangement regression — current priority

- [x] Generate original muted 100-track/10,000-clip schema-4 fixture.
- [x] Capture main-thread sample after native inspection timeouts/high CPU.
- [ ] Repair bounded native drawing/redraw cost and repeat actual inspection.
- [ ] Retain accurate separation from audio-device and percentile latency gates.

- [x] Frame r68: visual Arrangement time-selection state across three adjacent
  lanes, with cyan tint, range edges and ruler highlight; native interaction deferred.

## Recoverable replacement storage — 8 October 2026

- [x] Token-checked atomic replacement and one validated copied recovery generation.
- [x] Recovery reader, sidecar collision/symlink/size validation and postcommit outcome.
- [x] Restrictive permissions on both outputs and private staging before writes.
- [x] Nine replacement tests, targeted storage Clippy and formatting.
- [x] Native Save Current/session association source after dense redraw repair.
- [ ] Fresh native Save Current/Save As interaction acceptance.

- [x] Frame r69: slightly desaturated cyan #35e8e1, including time-selection
  tint and meter/focus artwork; preserve accepted iteration 2 snapshot.

- [x] Frame r70: further desaturated cyan #70e8e3 across all secondary artwork.

## Accepted UI iteration 3 — 8 October 2026

- [x] Freeze Frame r70 as the current native implementation reference in
  design/frame/iterations/03: softer cyan, six views, time selection, portable
  project, theme, sources, validation and verified SHA-256 hashes.
- [x] Preserve iteration 2; point owning UI documentation to iteration 3.

## Conditional redraw — 8 October 2026

- [x] Preserve bounded polls; invalidate on visible state changes instead of every tick.
- [x] Paused seek/Stop cursor updates, acknowledgments, faults and playback refresh.
- [x] 165 integrated tests, strict workspace Clippy, guards/fmt and native bundle.
- [ ] Fresh dense-fixture native inspection (input interrupted during active app use).
- [x] Session-owned Save Current/Save As backend and native source wiring.
- [ ] Fresh native Save/Save As interaction acceptance.

## Session saving backend — 8 October 2026

- [x] Actor-owned lexical document path and exact file token.
- [x] SaveCurrent and create-new SaveNew association; preserve original media base.
- [x] Conflict/failed Open/Save As collision and later-edit dirty state checks.
- [x] Postcommit warning adopts token while retaining dirty and useful paths.
- [x] Explicit save_complete on success and failure; 26 session tests and Clippy.
- [x] Cmd-S current/untitled routing and Cmd-Shift-S create-new Save As source wiring.
- [ ] Fresh native Save/Save As interaction acceptance (active app input was interrupted).

Integrated Save/Save As result: the current workspace run reports 171 passed tests
across all unit and integration suites; strict Clippy, formatting, architecture/style
guards, workspace check and the local bundle pass. This is source/build evidence,
not native dialog acceptance. The dense-fixture native inspection is also still open.

## Native palette alignment — next bounded UI polish

- [x] Align native theme tokens with accepted Frame iteration 3: warm charcoal,
  off-white, orange, cyan and muted grey clips/waveforms (#c0c4bb). Keep violet and
  blue in the design spec until implemented controls need them. Preserve compact
  layout and real actions.
- [x] Run source build and UI guards; rendered native acceptance remains separate
  while active app input is unavailable.

## Explicit prior-version recovery and palette — 8 October 2026

- [x] Actual CLI recovery to a new path, preserving target and validated sidecar.
- [x] Original media origin, case preservation, missing target/media, malformed backup and no-clobber tests.
- [x] 11 CLI tests, strict UI/CLI Clippy, formatting/guards and development bundle.
- [x] Native source uses accepted iteration 3 charcoal/orange/cyan and grey waveform palette.
- [ ] Fresh bundle palette inspection; current user instance is an older release process.
- [ ] Automatic crash journal and native recovery workflow remain deferred.

## Canonical mixer — source implementation complete, native acceptance open

- [x] Validated balance and master-gain commands, undo/history and compiled rendering.
- [x] Schema-5 persistence with neutral center-balance/unity-master migration from schemas 1–4.
- [x] Stereo/render/history/migration regression coverage; backend targeted suite passed 79 tests.
- [x] Compact 26 px native mixer strip source with balance and main gain controls.
- [x] 178 integrated tests, strict workspace Clippy, guards/fmt and development bundle; later CLI mix-render regression passes (179 total tested).
- [x] Fresh native balance/main gain changes and toolbar undo in an empty-audio track; listening remains open.
- [ ] Measure Live pan coefficients before claiming pan-law parity; smoothing/split pan remain later.

## Native window controls — requested small fix

- [x] Shared 80 px macOS traffic-light reservation in editor and demo transport.
- [x] Keep standard OS close/minimize/fullscreen; other panel padding stays compact.
- [x] Strict UI Clippy, source/style checks and rebuilt bundle.
- [x] Fresh native view confirms the top-bar content starts after the reserved native-control area.
- [ ] Normal traffic-light interaction/fullscreen sizing; OS screen-sharing pill obscured the controls during inspection.

## Default output recovery — source implementation complete, device acceptance open

- [x] Off-GPUI default-device reopen with single-open reservation and safe retirement ordering.
- [x] Reject project/device rate mismatch before stream construction; restore current compiled graph and stopped cursor without project mutation.
- [x] Generation-checked UI restore; reset old tickets, wait for graph and new seek acknowledgment, let Stop override with frame zero.
- [x] Keep editing/saving available; gate Play, seek and split while recovery is pending.
- [x] Targeted lifecycle/controller coverage, 187 integrated tests, strict workspace Clippy/format/guards and rebuilt bundle.
- [ ] Native device failure/restart, rate-mismatch interaction, disconnect and listening acceptance; the prior release instance was preserved.
- [x] Move initial output startup off GPUI with an explicit bootstrap that keeps the project's initial rate bound to the actual device.

## Asynchronous canonical startup — current bootstrap unit

- [x] Add bounded initial open with unconstrained device rate through the existing lifecycle worker.
- [x] Present loading window, replace root on completion, bind service to actual rate or failed-open offline48k.
- [x] Preserve new-root close guard, weak/non-owning pending lifetime and safe late output disposal.
- [x] Targeted policy/lifecycle checks, 189 integrated tests, strict Clippy/format/guards and rebuilt bundle.
- [x] Fresh native launch reaches empty editor at actual 44,100 Hz with top-bar reservation and compact main mixer.
- [ ] Measure startup latency and exercise close-during-open; verify normal traffic lights without the OS screen-sharing pill.

## Toolbar routing repair — 8 October 2026

- [x] Route canonical action icons through their owning Window instead of the globally active window.
- [x] Strict UI Clippy, formatting and rebuilt bundle.
- [x] Native add-track/undo and stopped default-output restart preserve canonical state.

## Canonical clip dragging — current work unit

- [x] Source pointer move within/across tracks using one canonical MoveClip at drop.
- [x] Preserve visible grab offset, clamp zero, reject invalid/overflow/stale drops and cancel without history.
- [x] Compact preview/target feedback, canonical selection reconciliation and targeted source checks.
- [x] 196 workspace tests, strict workspace Clippy, formatting, guards and rebuilt bundle.
- [ ] Fresh native drag/undo/cancel/resize acceptance; native control interruption preserves the current app/project.
- [x] Musical pointer-move snap grid and edge trim handles.
- [ ] Later: drag autoscroll and musical snap for trims/keyboard edits.

## Canonical clip fades — current work unit

- [x] Validated fixed fade domain, typed edits, trim policy and split/duplicate preservation.
- [x] Schema-6 round trips and explicit neutral migration from schemas 1–5.
- [x] Prepared envelopes with neutral bypass; boundary/split/source-loop/project-loop/partition regressions.
- [x] Compact selected-clip controls and clipped visualization with exact inherited breakpoints.
- [x] 212 workspace tests, strict Clippy, guards/fmt, desktop build and rebuilt development bundle.
- [x] Legacy neutral export stays byte-identical; paired release profiles recorded with remaining uncertainty.
- [x] Isolated native muted fixture: inherited indicator, 100→105 ms edit, Save metadata, undo/redo, second Save and clean quit.
- [ ] Listening, minimum-window/dense/fault interaction and measured Live curve study.
- [ ] Later: adjustable fade curves and automatic crossfades.

## Pointer trim and loop source phase — current work unit

- [x] Validated source phase, canonical trim bounds/preview and undoable edge commands.
- [x] Loop splits retain PCM and fade phase; strict schema-7 migration from schemas 1–6.
- [x] Prepared renderer and waveform mapping share the source phase.
- [x] Compact native edge handles, bounded preview, cancellation/stale-state checks and one-edit release.
- [x] 227 workspace tests, strict Clippy, guards/fmt, rebuilt bundle and byte-identical legacy export.
- [x] Isolated native left/right trim, saved source phase, undo/redo, cross-track move and off-track cancellation.
- [ ] Escape/resize/dense native acceptance and compiled metadata performance follow-up.

## Native titlebar spacing follow-up — 8 October 2026

- [x] Confirm the running toolbar starts after the shared 80 logical pixel reservation.
- [x] Rechecked the main native bundle: toolbar begins at x=80 logical pixels; macOS exposes native close, minimize and fullscreen controls. Sharing chrome still covers their visual region during automation.
- [x] Require the reservation in agent rules and guard loading/editor/demo top rows.
- [ ] Normal traffic-light interaction remains unverified while macOS sharing chrome occupies the region.

## Compiled clip metadata layout — 8 October 2026

- [x] Compact optional phase/fade metadata, control-side builder reclamation and zero-phase span dispatch.
- [x] Phase reset/fade retention regression, 228 workspace tests, strict Clippy, guards/fmt and rebuilt bundle.
- [x] Alternating neutral/phase/fade profiles with exact digests; legacy export byte-identical.
- [ ] Sustained device/dense acceptance and combined fade/phase performance investigation.

## Canonical musical timebase — current work unit

- [x] Validated fixed-point beat units, bounded tempo/meter maps and deterministic conversions.
- [x] Transactional point edits/history and strict schema-8 migration preserving frame audio.
- [x] Compact native tempo/meter workflow, bounded beat/bar ruler and opt-in snap.
- [x] Integrated math/render/storage checks; native tempo/meter save, reopened maps, quarter snap/move/undo/redo and snap-off acceptance.
- [x] Fix clipped ruler labels and cap attempted ticks even when fractional meter offsets reject labels.
- [ ] Native held-drag snapped-preview/stale cancellation, dense/minimum-window interaction and meter-aligned ruler subdivisions for fractional map offsets.
- [ ] Later: explicit musical clip anchors, tempo ramps, MIDI/recording event compilation and audio warping.

## Musical anchors and MIDI — current design unit

- [x] Accept canonical clip/note ownership, musical anchoring/duration and bounded event contracts (docs/midi.md).
- [x] Implement validated MIDI note metadata, typed edits/history and strict schema9 persistence without changing frame audio.
- [x] Compile one-shot musical note edges outside the callback, with off-before-on ordering and a 131,072-edge cap.
- [x] Consume prepared events in a shared native/export synth runtime with discontinuity and voice-capacity rules.
- [x] Original instrument playback and compact note editor vertical slice.
- [ ] MIDI controller/expression/chance/MPE, imported MIDI, recording and external I/O remain required backlog.
- [x] Meter-aligned ruler subdivisions with bounded visible enumeration; 63 UI tests and strict checks pass.
- [x] Fresh native fractional 6/8 subdivisions, zoom and schema9 audio-save migration acceptance.
- [ ] Dense/minimum-window subdivision checks.

MIDI model acceptance: workspace tests and strict checks pass. Independent saved
mixed-project CLI test resolves a tempo-crossing note to frames 24,000–80,000
and confirms export refusal publishes no WAV. Worker admission/Open tests retain
the old project, revision, history and audio graph on unsupported instrument data.
Duplicate MIDI clip overflow and wrong-kind moves are atomic; exact maximum event
count and same-pitch overlapping notes are tested. Legacy audio export remains
byte-identical. Prepared MIDI edges are not yet played; native editing remains
withheld until a real instrument is integrated. Main running app preserved.

## Original instrument playback — active 8 October 2026

- [x] Read-only architecture review: retain absolute-frame renderer and analytic note chase.
- [x] Add hex-devices original sine/envelope DSP and bounded engine interval compilation.
- [x] Verify partition, seek, pause/Stop, loop and graph replacement behavior.
- [x] Validate playable edits before committing history; include MIDI/release export horizon.
- [x] Replace unsupported MIDI tests with playable native-worker/CLI acceptance.
- [x] Preserve legacy audio export bytes; measure stated 100-track synth workload.
- [x] Inspect separate MIDI native Open/Play/Stop/export; native/CLI WAV bytes match.
- [ ] Listening and sustained physical-device stress remain separate acceptance gates.
- [x] Expose compact playable clip/note creation and piano-roll editing.

Instrument acceptance repair: corrected future-median interval pruning.
Independent four-note isolated/combined and CLI onset tests pass; regenerated
native/CLI exports contain all four notes and match exactly. Workspace tests,
targeted post-repair engine/session/CLI tests, strict workspace Clippy, formatting,
architecture/style checks and desktop build pass. See issues.md/performance.md
for the repair, corrected benchmark and remaining native note-editor work.

## Native MIDI editor — active 8 October 2026

- [x] Draw musical arrangement clips and note previews without changing audio layout.
- [x] Reconcile audio/MIDI clip and note selections across edits/undo/open.
- [x] Add compact instrument/clip creation and an actual piano-roll editing surface.
- [x] Route MIDI clip move/duplicate/delete and note parameter edits through canonical commands.
- [x] Verify bounded geometry, stale/pending edit guards and numerical limits.
- [x] Native create/edit/undo/save/reopen acceptance in an isolated bundle.
- [x] Complete visible-note Canvas and note drag/resize gestures implemented and native-checked.

Native MIDI supporting checks: controller author/update/delete/undo/duplicate/
move/save/reopen integration passes. Reproducible 100-track/6,400-note release
control-edit baseline is recorded in performance.md; native workflow acceptance
now passes as recorded below. Dense paint/interaction timing remains unmeasured.

Native MIDI acceptance: isolated final-check bundle reopened the authored note,
changed pitch 60→61, onset 0→0.25, gate 0.25→0.5 and velocity 100→92; deleted/
undid the note; moved the clip to beat 1, duplicated to beat 5, deleted/undid the
copy; saved and reopened. Independent schema9 inspection confirms distinct IDs
and exact values in artifacts/native-midi-editor-edited-20261008.hex. Keys/grid
now share body height; octave labels match Live and the complete parameter
readout fits. 67 UI tests, strict UI Clippy, desktop build and guards pass.
- [ ] Native minimum-window/dense interaction, rapid pending gestures and listening.

## Direct MIDI note gestures — active

- [x] Body move with snapped local onset, row pitch and preserved gate/grab offset.
- [x] Both note-edge resizes with fixed opposite edge and minimum gate.
- [x] Bounded preview; one release-time command; no-op/escape/stale cancellation.
- [x] Geometry/lifecycle checks and native move/resize/undo/save acceptance.

Native pointer acceptance passes body displacement, both gate edges, off-roll
cancellation, one-step undo and exact saved normal/short-note metadata. Source
review and native checks repaired phantom off-grid edits, short-note admission,
cropped handles and a relative-position override. All 76 UI tests and strict checks
pass. Main bundle rebuilt; running earlier acceptance projects preserved.
- [ ] Held-drag preview/Escape, resize/stale-window and minimum/dense native checks.
- [ ] Dense virtualization/navigation, multi-selection and flexible grid controls.

## Complete dense MIDI editor — active

- [x] Cached all-visible Canvas scene and pitch-row hit index; no 512-note omissions.
- [x] Deterministic overlap reachability with stable IDs.
- [x] Compact horizontal zoom/fit and bounded clip-local pan.
- [x] Preserve direct gestures and selected/preview geometry across spans.
- [x] Late-note/cache/overlap/span checks and release geometry/hit measurements.
- [x] Native 600-note/overlap and minimum-window interaction acceptance.

Dense editor native acceptance found and repaired a blank Canvas in the isolated
new build. Native all-visible painting and exact edit/save checks now pass; unit
tests alone were insufficient to catch the sizing error. Older running projects
are preserved.

Dense editor acceptance progress: repaired zero-sized GPUI Canvas with absolute
fill inside the sized roll. Fresh native 600-note painting and late-note selection
pass; Save As and independent full-project comparison prove only note 553 changed
velocity 80→72, retaining onset/gate/pitch/channel and all 599 other notes. Native
zoom-in redraw passes without a project revision. Six-overlap cycling, full zoom/
fit/pan gestures and minimum-window acceptance remain open. 85 UI tests and strict
checks pass; release geometry benchmark is recorded in performance.md.

Native dense-editor follow-up: six identical pitch/time notes cycle through
velocities 69, 68, 67, 66, 65, 64 without a revision. At a two-beat span, a body
drag moves note 8 from onset 1/pitch 60 to onset 1.25/pitch 61 in one revision,
retaining gate 0.25, channel 0 and velocity 69. At 880 × 540 logical pixels,
all piano controls/readout and 24 pitch rows remain visible; fit and local pan
redraw correctly without history edits. A new clip at arrangement beat 8 opens
at local zero and accepts note 10 there. Independent schema-9 assertions verify
these exact edits and five unchanged overlapping notes in
`artifacts/native-midi-overlap-minimum-accepted-20261008.hex`.
Top-bar output status clipping at minimum width is a remaining layout issue.
Dense-check instance now has unsaved user changes: preserve it; this follow-up
used a fresh overlap-check instance instead. Zoomed edge-resize, held Escape
and sustained input/paint measurements remain separate acceptance.

- [x] Native six-overlap cycle, zoomed body move, fit/pan and 880 × 540 piano layout.
- [x] New arrangement-beat-8 clip opens at local zero; author/save exact note.
- [x] Repair minimum-width transport output truncation using verified GPUI bounds.
- [x] Native zoomed edge resize; exact saved onset/gate and opposite-edge preservation.
- [ ] Held-drag Escape and measured input/paint timing.

Transport status repair accepted: use a bounded flexible one-line clamp with
ellipsis rather than no-wrap truncation; complete output metadata is available
in a lowercase Ioskeley Mono tooltip. Native final build at 1240 × 760 and
880 × 540 shows ellipsis inside the window; the narrow-width tooltip shows
`coreaudio · 44100 hz · 2 channels`. Main source behavior is repaired; preserved
user-running dense-check bundle is not overwritten. Desktop build, strict UI
Clippy, formatting and style/architecture guards pass.

## Configurable MIDI grid — active

- [x] Shared fixed/free grid contract across insert, parameters and gestures.
- [x] Compact control, matching bounded grid and stale-gesture cancellation.
- [x] Grid/off/span/edge/domain checks and existing UI regressions.
- [x] Native grid switching, author/move/resize/save at compact size.

- [x] Native both-edge resize at two-beat span/880 × 540; off-roll release cancels.
- [ ] Held-drag Escape/stale-window cancellation: source checks pass; native proof open.

Zoomed edge fixture `artifacts/native-midi-zoomed-edges-accepted-20261008.hex`
confirms first note onset 0/gate 1 beat, pitch 61, velocity 92, channel 0; full
project comparison proves the second clip and other metadata unchanged. Each
edge commits once; off-roll body release preserves revision 3 and exact values.

Configurable MIDI grid source now passes 89 UI tests (one diagnostic benchmark
ignored), strict UI Clippy, formatting, architecture/style checks and desktop
build. Default 1/16 preserves quarter-beat steps; off uses exact pointer timing,
a quarter-beat insertion/nudge fallback and positive raw-unit minimum resize.
Offsets, opposite resize edge and expression remain intact. Native grid/save
acceptance is next; exact Live magnetic thresholds and Cmd inversion stay open.

## Temporary MIDI snap inversion — active

- [x] Platform primary modifier inverts fixed/free snapping for insertion and gestures.
- [x] Raw pointer anchors preserve grab offsets across mid-drag modifier changes.
- [x] Move/release modifier, both edges, no-op and domain regression checks.
- [x] Native fixed/free authoring and both Cmd insertion inversion directions; saved values.
- [ ] Native held-Cmd drag/resize and mid-drag modifier changes.

Fresh grid fixture Open passed at revision 1. Native coordinate input then failed
to locate the live window; see docs/issues.md. Keep grid interaction acceptance
open and preserve the running projects while source implementation progresses.
The same fixture loads and exports successfully through `hex-cli render-project`
to `artifacts/native-midi-grid-baseline-20261008.wav`; this proves its renderable
schema/model baseline, not mouse-grid behavior.

Modifier source passes 92 UI tests (one diagnostic benchmark ignored), strict
Clippy, formatting, style/architecture guards and desktop build. Primary source
acceptance confirms release uses its current modifier and fixed/free candidates
derive from the same original note/raw anchor. Native fullscreen activation works;
the grid-check app subsequently received user edits (unsaved revision 11) and must
be preserved. The running app predates modifier implementation; do not overwrite
its bundle or interpret those edits as controlled modifier acceptance.

Fresh `hex-midi-snap-check.app` acceptance passes every selector division and off,
Cmd fixed→free/off→1/16 insertion, snapped body move, free body move/both edges,
and the compact 880 × 540 layout. Authoring fixture
`artifacts/native-midi-grid-authored-20261008.hex` contains seven expected notes;
full-project comparison preserves the second clip and all other metadata. Gesture
fixture `artifacts/native-midi-grid-gestures-accepted-20261008.hex` differs only in
note 12's exact onset 1→1.5/pitch 58→59 and note 9's free movement/resizes.
Free pointer calculations agree within 4096 Q32.32 raw units (under one millionth
of a beat); expression and the five other authored notes remain exact. Each
gesture commits once. The resulting fixture also exports successfully through
hex-cli. Held-modifier drag native proof remains open: the documented automation
API has an atomic drag without a modifier argument.

## Atomic multi-note edits — active prerequisite for multi-selection

- [x] Narrow clip-scoped update/removal commands with bounded unique note IDs.
- [x] Full validation before mutation, stable ordering/IDs and one undo transaction.
- [x] Whole-project undo/redo, late-invalid rollback and scheduling rejection checks.
- [x] Targeted project/session checks, strict lints and desktop build.
- [ ] Follow with native selection, group gestures and exact save/history acceptance.

Public `MidiNoteUpdate`, `UpdateMidiNotes` and `RemoveMidiNotes` now provide one
clip-scoped atomic operation capped by the existing 65,536 `MAX_NOTES` limit.
Every ID/value validates before mutation; removal uses stable linear compaction.
Whole-project tests cover one-step undo/redo, preserved order and untouched notes,
empty/oversized/duplicate/foreign/missing/late-invalid/overflow batches and redo
preservation. A real session service test opens 129 spaced playable notes, moves
them to one onset in a batch, and confirms onset-density compile rejection without
project/revision/dirty/redo change or replacement playback. Project/session tests,
workspace strict Clippy, fmt, guards and desktop build pass. Primary acceptance
reviewed command dispatch, validation, stable compaction and compile-before-history
integration. Native multi-selection is the next unit, not completed by this API.

## Native multi-selection — active

- [x] Normal selection/draw-mode separation, compact pencil/B and double-click authoring.
- [x] Shift toggles/additive marquee, select-all and focused piano shortcuts.
- [x] Clip/revision/project selection reconciliation and complete selected-note painting.
- [x] Shared bounded group body preview and one atomic move/delete transaction.
- [x] Source selection/group/cancellation regressions and strict checks.
- [ ] Native normal/draw/selection/group history/save and compact layout acceptance.
- [ ] Continuous draw strokes and group edge/parameter editing in following units.

Source checks: 100 hex-ui tests pass, one existing benchmark ignored; strict workspace Clippy, fmt, architecture/style guards and desktop build pass. Native Shift toggle, marquee, select-all/Escape, normal double-click and B draw insertion pass in the isolated selection-check bundle. Saved group move preserves channels/velocities/gates and all other project metadata exactly; group deletion removes only IDs 6/7. One-step undo is observed; the final restored file preserves all musical data, with next_id advancing from 13 to 16 after three undone insertion probes. Native additive marquee, group redo/no-op/off-roll, focused arrow routing and minimum-window acceptance remain open.

## Group keyboard movement — accepted checkpoint

- [x] Native single-note arrow routing and exact whole-project saved-value acceptance.
- [x] Shared group clamp/payload helper for pointer and keyboard movement.
- [x] Piano-focused group onset/pitch keys with one atomic update and zero-delta no-op.
- [x] Boundary/expression/history tests, strict checks and desktop build.
- [x] Native group arrow/save/undo acceptance.

Native selection follow-up: focused single-note Up/Right passes exact full-project
comparison (`native-midi-selection-keyboard-20261008.hex`). Group no-op and
off-roll release produce an identical project (`native-midi-selection-canceled-20261008.hex`).
Group deletion undo/redo runs as single actions; saved redo removes exactly IDs
6/7 and preserves every other value (`native-midi-selection-redo-20261008.hex`).
Native additive marquee and minimum-size layout remain open. The documented
automation drag has no modifier parameter, so Shift-marquee has source evidence
only. Held-preview Escape remains separate from selection-clearing Escape.

103 UI tests pass (one existing benchmark ignored), workspace strict Clippy, fmt,
architecture/style guards and desktop build pass. Independent saved comparison
of `native-midi-group-keyboard-accepted-20261008.hex` confirms only notes 6/7 move
+1/4 beat/+1 semitone, retaining gates, spacing, channels, velocity and every other
field. Native one-step undo/redo passes. All-note Right at a clip-ending note
leaves revision 6 unchanged; redo remains available, and
`native-midi-group-keyboard-redo-20261008.hex` exactly equals the earlier accepted
project. Minimum 880×540 native layout now passes with seven selected notes and
all compact controls/grid visible. Shift-marquee and held-preview Escape still
need native manual evidence.

## Next MIDI authoring unit

- [x] Define bounded atomic multi-note insertion for one undoable draw stroke.
- [x] Add validation/admission/history tests before continuous pointer authoring.
- [x] Implement continuous drawing source without per-pointer project edits; native acceptance is tracked below.
- [ ] Keep group edge/parameter edits, broader selection gestures and recording tracked separately.

## Atomic note insertion — accepted prerequisite

- [x] Public bounded clip-scoped insertion records/command and Copy created-ID range.
- [x] Complete validation/storage/allocator preflight before mutation.
- [x] Order/expression/unique IDs, one-step history and rejected-candidate rollback.
- [x] Actual session compile admission/rejection and targeted checks/lints/build.
- [x] Use accepted insertion contract through the mixed edit for continuous drawing.

Project MIDI tests (10), session admission tests (5), strict workspace Clippy,
desktop build, fmt and architecture/style guards pass. Primary source acceptance
reviewed dispatch/public range, complete validation/global cap, fallible reserve
and checked range allocation, ordering, history and real onset-density refusal.
Tests independently cover every invalid field, missing/audio clips, exact cap
and other-clip/project cap, allocator exhaustion and redo preservation. No native
stroke command is exposed yet; no native drawing acceptance is implied.

## Mixed drawing transaction — accepted prerequisite

- [x] Unique clip-owned removals plus new note records admitted as one transaction.
- [x] Final-size reservation/global cap and rollback/history/session compilation checks.
- [x] Use mixed edit for local continuous draw previews and one release commit.

Mixed edit acceptance requires independent ordering/expression/history, invalid
removal and late-insert rollback, global-cap replacement, allocator exhaustion,
and actual service admission/refusal checks before draw UI integration.

`EditMidiNotes` shares admission/compaction/append through midi_batch.rs with
AddMidiNotes and RemoveMidiNotes. Project MIDI tests (13), session admission (6),
strict workspace Clippy, desktop build, fmt and architecture/style guards pass.
Primary reviewed shared validation, final-size reserve, checked allocation before
compaction, stable ordering, complete project/allocator/history rollback and actual
service mixed admission/refusal. Erase-only consumes no IDs; insertions return a
contiguous range. Native draw strokes remain the next dependent unit.

## Unsaved-project close repair — 8 October 2026

- [x] Harden repeated Discard handling: retain the editor through prompt completion, explicitly remove the confirmed window and expose lifecycle update failures. Four targeted tests, strict UI Clippy, desktop build and source guards pass (9 October).
- [x] Verify the repeated report against `artifacts/hex-discard-review-20261009.app`: native close → Cancel preserved the disposable unsaved project; repeat close → Discard exited the process. Reopened the corrected build clean for the user; older running apps remain untouched. Six close-related tests pass again; identity is in `artifacts/discard-review-20261009-manifest.json`.

- [x] Consume close/quit prompt answers after bootstrap root replacement using a retained strong editor entity and untyped window handle.
- [x] Native close → Cancel preserves the editor; repeat close → Discard exits.
- [x] Native Cmd+Q → Cancel preserves the editor; repeat Cmd+Q → Discard exits in the final build.
- [x] Targeted close test, UI strict Clippy, desktop build, formatting, architecture and style guards pass; remove temporary diagnostics and rebuild the inactive development bundle.
- [x] Refresh the inactive development bundle after the repeated close report (9 October); targeted close test/build and source guards pass again.
- [x] Recheck the repeated Discard report: all four close-related tests pass; refreshed bundle matches the current desktop binary (see docs/issues.md for hash/logs).
- [ ] Repeat native close/Cancel/Discard on the refreshed bundle when the Mac is unlocked; current automation was blocked by the lock screen.

## Continuous MIDI drawing — active integration

- [x] Stage pure bounded stroke resolver and mixed command payload with targeted tests.
- [x] Review and wire Draw down/move/up with local insertion/erasure preview and one release command.
- [x] Integrate pitch lock, context/focus/Escape cancellation and matching-clip created-range selection.
- [x] Source-check fixed-grid clip-end snapping and clicked overlap targeting; keep universal interpolation invariance unproven.
- [ ] Source checks and native saved stroke/erase/history/cancel/compact-layout acceptance.
- [ ] Installed-Live experiments for detailed sweep, overlap, Off spacing and modifier-transition parity.

Source checkpoint: 116 UI tests pass, one benchmark ignored; workspace strict
Clippy, desktop build, formatting and guards pass. Native initial locked stroke
produces five exact initial notes and one revision; complete saved/history checks
remain pending because further edits entered that instance, and a fresh loaded
acceptance app currently rejects automated pointer input. Preserve both instances.

## Selected-group parameter editing — active implementation

- [x] Enable compact pitch/gate/velocity buttons for atomic relative group edits.
- [x] Preserve IDs/channel/relative differences; refuse the entire edit at any member's boundary.
- [x] Cover missing selection, unequal expression/gates and source checks; inspect rendered-context guards.
- [ ] Native saved-value one-step history and minimum-layout acceptance.

Group edge dragging, channel controls and Live boundary-policy parity remain
separate work. The existing native drawing process is still alive; Raise and
clicks now return successfully but selection/dialog input has no visible effect.

Source checkpoint: 121 UI tests pass (one existing benchmark ignored), strict
UI Clippy, formatting, architecture/style guards and desktop build pass. Primary
reviewed capture, uniform checked deltas, six compact controls, stale-context
guards and one batch/history. Development bundle updated while confirmed inactive.
The isolated group-parameter app starts; initial screenshot is blank, then an
exposed zoom action redraws the ready editor. Cmd+O still produces no dialog;
native saved-value/layout acceptance remains pending. Preserve that process.

## MIDI authoring keyboard follow-up — active implementation

- [x] Piano-focused Shift+Up/Down octave transposition, single and group.
- [x] Shift+Left/Right length editing through the accepted group transaction.
- [x] Interval/expression/history preservation, bounds/no-op checks and targeted source validation.
- [ ] Native focused-key saved-value acceptance, including unaffected Arrangement shortcuts.

GPUI lifecycle source trace found no demonstrated initial-paint/input defect;
OS-menu AX actions work while simulated Escape does not. Keyboard delivery and
initial redraw remain independent native evidence gaps. Pinned GPUI custom
accessibility semantics remain a dependency capability gap.

124 UI tests pass (one existing benchmark ignored), strict UI Clippy/fmt and
desktop build pass. Primary reviewed MIDI-context bindings, explicit focus gates,
single/group delta dispatch and unchanged Arrangement trims. Drawing native
zoom repaints the existing process, now showing revision 2 with "project opened";
the intervening revision was not observed. Preserve it and don't assume pristine
fixture content from the earlier cached capture.

The main development bundle was updated only after confirming it is inactive;
its executable hash matches the successfully built desktop binary. Existing
running native acceptance processes retain their earlier binaries and state.

## Local MIDI clip looping — active contract work

- [x] Define separate source region/start/loop brace and Arrangement duration.
- [x] Resolve bounded occurrences, seams/gates/releases and tempo-crossing compilation.
- [x] Canonical commands/history, strict migration and source-oriented edit validation.
- [x] Playback/export/seek partition tests, atomic dense refusal and legacy one-shot preservation.
- [ ] Compact native loop controls and independent saved/audible/reference acceptance.
- [ ] Prepared-template path for ongoing Session launching; finite expansion alone is insufficient.


Local-loop first prerequisite in progress: validated source projection and bounded
per-note occurrence plans, shared by existing one-shot edge/span compilers.
Canonical loop metadata, schema migration, commands and native loop controls are
still required; this prerequisite alone does not enable looped project playback.


- [x] Source projection prerequisite: separate validated source/start/loop/duration descriptor with constant-time per-note count and stable occurrences.
- [x] Both compiler consumers share projection/frame conversion and whole-project occurrence preflight; one-shot regression checks pass.
- [x] Independent tiny-domain segment-walk oracle, huge-count/overflow cases, strict checks and desktop build.
- [x] Connect canonical clip source metadata, atomic commands/history and strict schema migration; source-aware bounds are integrated, compact loop controls remain pending.

Acceptance: 7 project projection tests, 40 engine tests and 34 session tests pass;
strict project/engine Clippy, fmt and architecture/style guards pass. No looped
saved project, native local-loop controls or ongoing Session playback is verified.


## canonical local MIDI source integration — active

- [x] Per-clip source settings and one atomic edit with history/ID preservation.
- [x] Strict schema-10 writer/reader and explicit one-shot schema-9 migration.
- [x] Actual projected loop playback/export and bounded refusal regressions.
- [x] Source-aware piano-roll bounds, draw/parameter edits and same-clip viewport reconciliation.
- [ ] Loop-aware Arrangement thumbnail, compact native source controls and native save/playback checks next.


Source-oriented UI bounds are integrated and 124 UI tests pass (one existing
benchmark ignored). Initial test compilation ran out of disk; safely recovering
only Hex incremental compilation cache allowed the rerun to pass. Canonical
model/schema/playback acceptance remains in progress with the existing executor;
do not infer native local-loop completion from source bounds or build results.


Canonical source integration acceptance:358 workspace tests pass,1 existing UI
benchmark ignored; strict workspace Clippy,fmt,guards and desktop build pass.
The suite corrected one stale writer-version assertion while preserving legacy
fixtures. Runtime/native controls/listening/reference and ongoing Session gates
remain open. Existing running apps retain their earlier binaries and state.

## prepared MIDI Arrangement thumbnails — source accepted

- [x] Bounded control-prepared frame summaries and sorted clip lookup.
- [x] Matching immutable summary/project/revision publication and unchanged-state reuse.
- [x] Thin UI summary consumer and row-level shape budget replacing raw source-note thumbnails.
- [x] Offset/intro/repeat/tempo/partial/sample/revision/ownership source checks; atomic prepared undo/redo and import publication.
- [ ] Native cropped/minimum-layout and dense sampling acceptance; source marker editing/save/playback checks.

## compact MIDI source controls — source accepted, native gates open

- [x] Separate arrangement length, source extent/start, local loop start/end and enabled controls.
- [x] Grid steps, atomic full-command publication, invalid/stale/no-op guards and source tests.
- [ ] Native compact layout, marker edit/undo/redo/save/reopen and hidden-note preservation.

Source-control acceptance follow-up — 8 October 2026: 132 UI tests pass (one
existing benchmark ignored), strict UI Clippy/fmt/style/build pass. Native
Arrangement/source/start/loop A/loop B increments committed independently; loop
toggle, Undo and Redo preserve retained markers. Create-new Save As equals the
entire baseline plus five intended fields, with notes/IDs/tempo unchanged.
Reopen displays those saved markers. Final source-fit/reopen note interaction
was interrupted and remains unverified; minimum layout and listening stay open.

## prepared Session tempo conversion — active prerequisite

- [x] Graph-owned immutable timing contract reusing canonical Q32 conversion.
- [x] Checked borrowed block/event conversion calls, seam/inverse/limit tests.
- [x] Dense release conversion microbenchmark; no callback integration claim.

Development bundle refreshed after confirming `org.portalsurfer.hex.dev` inactive:
`artifacts/hex.app` executable SHA-256
`9ea0313fee5c989554f2846a51dd466a848597c72664fdfaa3f7ea90fdd52fe0`.
This includes the verified source-control button identity repair and close fix.
The running isolated acceptance app is preserved.

Prepared timing source acceptance: 51 engine tests pass; strict engine Clippy,
formatting and architecture/style guards pass. Release 1024-point profile records
1,228,800 conversions in 43.689 ms (35.6 ns/op), checksum
15572411654917454540. This excludes map/query preparation and measures mixed
conversion throughput including loop/checksum overhead, not realtime deadlines.
Next: bounded ongoing edge templates with independently proven admission across
quantized seams; canonical Session slots/scenes and runtime remain unimplemented.

## ongoing MIDI edge template prerequisites — active

- [x] Exact canonical frame-to-beat range lower bound, including one-past-domain result.
- [x] Prepared delegation and independent brute-force/minimality/dense-map tests.
- [x] Immutable ongoing intro/repeat source edge template and bounded seekable queries.
- [x] Conservative density/quantization admission, finite oracle/partition/seek regressions.
- [ ] Runtime voice/discontinuity and aggregate admission contract, then Session model/UI.

Exact range-query acceptance: full project/engine suites passed before final test
strengthening; root then ran the final seven canonical tempo and eight prepared
engine tests successfully. Strict project/engine Clippy and fmt pass. Logs:
`artifacts/tempo-lower-bound-project-tests.log` and
`artifacts/tempo-lower-bound-engine-tests.log`. Ongoing templates are now the
active coherent implementation; no audio/runtime/Session capability is implied.

Ongoing template source acceptance — 8 October 2026: full engine suite passes
63 tests. Final root acceptance passes 8 query integration and 3 independent
density-oracle tests, strict engine Clippy, fmt/guards and desktop build. Queries
cap 256 frames, 256 edges, 128 onsets and 260 period windows with fixed caller
storage; exact range lookup and binary phases avoid historical replay/source
scans. Faster/different-rate query maps and beyond-domain starts are refused.
Admission is conservative at the intro/tail seam and for exact-one-frame gates.
Release dense one-shot benchmark: 10,000 queries × 255 actual edges in 32.933 ms,
3.293 µs/query; setup/100 warmups excluded. No callbacks, active voices, Session
schema/UI, aggregate 100-track load or device deadlines are implied.

Development bundle refreshed after verifying the main app is inactive:
`artifacts/hex.app` executable SHA-256
`93cb94c8a338d597bfb0d9a3ec3fefc86eb2b710642bccf251831739220e6698`.
Existing native acceptance app state is preserved. No template UI/runtime
integration is claimed by this build update.

## ongoing instrument rendering — source accepted

- [x] Reconstruct sounding gate/release spans at arbitrary playback frame with bounded storage/work.
- [x] Admit at most 64 simultaneous sounding voices, including source-seam release tails.
- [x] Reuse prepared hex-devices DSP and compare finite/ongoing audio across partitions and seeks.
- [ ] Define Pause/Stop/launch/graph-replacement ownership and aggregate track work before runtime wiring.
- [x] Measure dense admitted render queries; preserve current Arrangement and Session backlog.

8 October source acceptance: 74 engine tests pass through the executor, with
final root acceptance of 7 instrument integration tests and 3 density oracles.
Strict engine Clippy/fmt, architecture/style guards and desktop build pass.
The interval-tree median pruning and post-seam release admission findings are
repaired with regressions. Queries cap 256 frames, 192 spans, 326 period windows
and 16,384 node visits; immutable preparation remains control-side.

Optimized held-voice microbenchmark: 64 voices/source, 64 frames at 48 kHz;
1,000 single-source renders take 8.554 ms and 100 serial 100-source renders take
81.521 ms. This does not establish physical deadlines or graph-wide admission.
See performance.md and artifacts/midi-spans-*.log. Main app was verified inactive
before bundle refresh; executable SHA-256
`7d88893dbcf585b6e1226de0e473af51bbb48a1358c5c6c03b38ddb4ce81fb1e`.
The retained native acceptance app remains untouched.

Next finite unit: define aggregate project admission and canonical Session
slot/launch ownership before wiring ongoing templates into the existing runtime.
Track/master mixing, Pause/Stop/launch/graph transitions and outgoing-tail policy
need explicit acceptance; the existing finite Arrangement renderer is unchanged.

## canonical Session ownership — source accepted; aggregate/runtime work open

- [x] Define canonical scene/slot/source identity and edit/history ownership without a second project model.
- [x] Preserve schemas 1–10 and complete Session state round trips in schema 11.
- [ ] Separate prepared source inventory from finite Arrangement compilation and runtime launch state.
- [ ] Bound aggregate tracks, sources, spans, onsets and render work; verify meaningful many-track fixtures.
- [x] Measure serial aggregate kernel distributions for sparse, dense and loop-seam workloads; record deadline-budget exceedances without a device claim.
- [ ] Specify launch quantization, per-track replacement, empty slots, cancellation, Pause/Stop/seek and graph changes.
- [x] Implement the selected first coherent unit, with refusal/history/persistence tests and targeted checks.
- [ ] Integrate compact canonical Session UI and native playback after ownership/admission gates pass.

The previous goal turn made verified source progress: ongoing stateless instrument
queries/rendering and dense 100-source evidence are accepted. This design step
must advance real canonical Session functionality; the two-scene demo remains
a separate prototype and must not acquire another editable project model.

10,000-block distributions now show dense 6,400-voice kernels exceeding the
nominal 64-frame budget in some blocks; averages are insufficient for admission.
Source-start-0 32-note loop over-refusal is recorded for a tighter seam proof.
See performance.md and artifacts/midi-aggregate-profile.log. Canonical Session
source ownership/schema implementation is active in project/storage.

8 October source acceptance: shared MidiSource values, independently owned
Session MIDI clips, globally allocated scenes and sparse typed cells are
integrated in project/storage. Existing note commands/batches resolve both
owners, copies allocate new IDs, cross-track moves retain IDs, and scene/track
removal cleans cells. Format caps and shared global clip/note limits are enforced.
Stop is stored separately from absent cells without a launch parity assumption.
The engine retains the Arrangement preparation wrapper and adds prepare_source.

Workspace tests (one existing ignored), strict workspace Clippy, desktop build,
fmt and guards pass. Final root acceptance passes 9 Session model tests,
8 storage persistence tests including literal schemas 1–10, and one independently
owned Session source-template test. Logs: artifacts/session-ownership-*-tests.log.
Root updated the affected session profiling Snapshot and saved-version assertion.
Native Session UI/playback is not integrated or accepted by this milestone.

Main app was rechecked inactive before the final build refresh; bundle executable
SHA-256 `3b9a150de1b73a398742fe5d4f93d4ca235a23ddf490fb8c7659e28cd645b647`.
Existing native acceptance app state remains preserved.

- [x] Tighten first-tail admission: valid source-start-0 32-note loop prepares and matches finite PCM across first/later release seams; delayed-tail overload remains refused.
- [x] Prepare and transactionally publish Session source inventory with structural per-track maxima, exact revision artifacts and full controller/worker admission.
- [ ] Implement quantized per-track/scene launch, pending cancellation and bounded release/retirement.
- [ ] Integrate compact canonical Session grid and native launch/edit/save/reopen acceptance.

Current execution: engine first-tail overlap proof/regressions and a read-only
prepared Session publication/runtime contract review. The previous goal turn
changed authoritative canonical ownership/persistence and measured aggregate
timing distributions; it was progress, not a wait or completion of the DAW goal.

8 October first-tail acceptance: 8 ongoing-instrument integration tests and 4
density tests pass (artifacts/midi-first-tail-*-tests.log). The one-sided sweep
enumerates bounded nonnegative occurrences over the intro release horizon.
Source-start-0 aggregate profiles now include the actual first seam; dense blocks
still exceed the nominal period. Desktop rebuild failed during linking with
ENOSPC; the earlier verified close-fix/schema-11 bundle remains intact.

Current execution: immutable prepared Session inventory and exact transactional
revision artifact, with internal shared-source COW. No Session launch/runtime/UI
activation until aggregate work and transition contracts are accepted.

- [ ] Implement per-track Session override/silence/Back to Arrangement, retaining
  Arrangement playback on non-overridden tracks; do not use global exclusive
  playback modes as the parity endpoint.
- [ ] Execute SESSION-ROUTING-001 disposable launch/stop/continuation fixture;
  current installed UI/menu observation and manual contract do not prove timing.

- [x] Add canonical 100-track/200-cell heterogeneous Session preparation/edit
  benchmark; release execution and targeted strict example Clippy pass. Source
  reuse is measured; worker/UI and callback responsiveness remain unproven.

Inventory implementation is active: COW and immutable source inventory core lib
tests passed in the executor's preliminary check. PreparedRevision history/import
and service Open/Restore integration still needs final source/root acceptance.

- [x] Extend the canonical Session benchmark with complete PreparedRevision
  production inside Editor candidate admission; source reuse and strict example
  Clippy pass. Empty Arrangement fixture excludes full-song edit performance.

Prepared Session integration acceptance in progress: desktop rebuild now passes
after cache recovery; root Arrangement regressions pass 17 tests and both guards
pass. Legacy controller mutation/history APIs share Session inventory validation.
Discriminating mixed-scene maxima and populated worker Open/Restore rejection
coverage remain the final source acceptance gate for this inventory unit.

Prepared Session source acceptance — 8 October: COW source identity, immutable
inventory, compatible template reuse, disjoint per-track work maxima and exact
PreparedRevision products are integrated. Modern and legacy mutations/history
share candidate preparation; service initial/edit/import/open/restore use the
same artifact. Export remains finite Arrangement. Source acceptance includes
populated grid save/open/history, dense edit/Open refusal and Restore Arc identity.

Executor engine/project/session/storage tests and strict all-target Clippy pass.
Root acceptance passes 15 focused COW/inventory/controller/worker tests, 17 finite
Arrangement regressions and 8 audio boundary tests. Desktop build, fmt and both
guards pass. Logs: artifacts/session-prepared-*.log. Development main app was
confirmed inactive before binary refresh; SHA-256
`7a9ee19decff01e36077ef780f8f65c1e4f25076f663783e1cfc6f8c3119a9b1`.
Existing running apps/project state were preserved. Native Session remains open.

## Borrowed track-source render and common mixing — 8 October 2026

- [x] Extract allocation-free finite track accumulation and reusable prepared track/master stages; preserve per-contribution operation order.
- [x] Check overflow before destination mutation; preserve pause/empty/transport behavior and compiled mute/solo.
- [x] Root targeted acceptance: 29 track/mix, finite MIDI, fades, independent scalar spans and loop tests pass (`artifacts/track-mix-acceptance-tests.log`).
- [x] Full engine tests, strict engine all-target Clippy, formatting, architecture/UI guards and desktop build pass (`artifacts/track-mix-desktop-build.log`).
- [x] Independent active 100-track release fixture retains historical PCM digest (`artifacts/track-mix-active-profile.log`); no sustained deadline claim.
- [x] Add bounded additive ongoing MIDI spans through shared mix coefficients, with query-failure and per-voice rounding checks.
- [ ] Implement mixed ownership, quantized launch/stop/return commands, aggregate admission and bounded voice continuity/retirement before Session UI.

## Additive ongoing MIDI rendering — 8 October 2026

- [x] Add borrowed launch/timing/table context and per-voice accumulation through the accepted prepared track mix, without master gain or runtime allocation.
- [x] Refused queries preserve existing destination samples and clear reused span storage; muted queries still validate.
- [x] Preserve the standalone overwrite API, including failure zeroing and original PCM.
- [x] Root full template suite passes 21 tests; executor full engine tests, strict all-target Clippy, formatting and architecture/UI guards pass (`artifacts/midi-additive-template-tests.log`).
- [x] Validate finite/ongoing scaled accumulation into a nonzero destination before release profiling (`artifacts/midi-additive-profile.log`); record dense timing misses in performance.md.
- [x] Implement per-track route state and bounded quantized commands in a headless kernel; complete native integration only after the remaining timing/handoff gates.
- [ ] Complete periodic tempo, revision continuity, aggregate admission and device acceptance before native canonical Session playback claims.

## Headless mixed-route Session playback — 8 October 2026

- [x] Build SessionPlayback over exact PreparedRevision with fallible control-side state allocation, separate construction/command/render modules and one selected source per track.
- [x] Sparse scene no-op/Stop, per-track quantized replacement, explicit cancellation, Back to Arrangement and distinct Stop All/transport policies.
- [x] Keep performance time monotonic across Arrangement wraps and preserve <=256-frame source queries in arbitrary caller buffers.
- [x] Retain outgoing phase/release anchors in a bounded Copy bank with checked stable capture order; refuse capacity/arithmetic errors without losing prior state.
- [x] Whole-block rollback after a due transition and later query refusal; full output zeroing and successful retry PCM comparison.
- [x] Root 16 kernel tests plus 38 source/mix/compile/loop regressions pass; full engine tests/strict Clippy, desktop build, formatting and guards pass (`artifacts/session-kernel-final-kernel-tests.log`, `artifacts/session-kernel-final-acceptance-tests.log`, `artifacts/session-kernel-desktop-build.log`).
- [x] Profile a canonical 100-track mixed kernel with partition comparison and documented exclusions (`artifacts/session-kernel-final-profile.log`).
- [x] Prepared periodic tempo clock, including changes inside loops and ignoring markers strictly beyond the loop end; canonical shared-frame endpoint semantics are explicit.
- [ ] Active-voice tempo/graph handoff, aggregate structural/physical admission and existing publication/retirement integration.
- [ ] Native compact canonical Session UI, reference launches and device/listening acceptance; current native app remains on Arrangement playback.

## Periodic Session tempo — 8 October 2026

- [x] Prepare Copy cycle metadata in the exact inventory and borrow its canonical map at event/block boundaries.
- [x] Repeat the loop tempo segment on monotonic Session time; canonical shared-frame endpoint selection and seam lower-bound minimality are explicit.
- [x] Admit/reuse sources against effective rounded periodic rate, including offset one-frame loops.
- [x] Retain typed unavailable-clock errors without rejecting finite Arrangement revisions.
- [x] Test inverse/forward oracles, interior roundtrips, shared start/end markers, large cycles, domain exhaustion and quantized launches; all 109 engine tests and strict Clippy pass.
- [x] Root 41 Session-layer regressions, desktop build, formatting and architecture/style guards pass.
- [x] Constant and repeating-tempo 100-track workloads match independent finite PCM and irregular partitions; logs and exclusions are in performance.md.
- [ ] Implement active source/tempo/graph continuation and bounded publication refusal/retry before native Session activation; keep source/track removal, tails and pending launch intent explicit.


## Prepared callback allocation acceptance — 9 October

- [x] Add pinned dev-only allocation-counter 0.8.1 and opt-in allocation-probe feature; normal desktop dependencies exclude it.
- [x] Architecture guard rejects allocator-counter use outside hex-audio dev dependencies; no workspace unsafe code added.
- [x] Calibrate debug/release detection of allocate/free pairs and freeing memory allocated before the scope.
- [x] Instrument prepared callback entry/render under 100-track shared/distinct audio and exact independent PCM.
- [x] Cover admitted bursts, priority Stop, graph inheritance/fresh install, actual retirement saturation and shutdown without masked final ownership.
- [x] Verify sounding MIDI scene-switch/release coverage against headless PCM; maximum density, same-clip/quantized paths and physical/native gates remain deferred.
- [x] Accept five debug/release cases, strengthened MIDI reruns and strict audio all-target Clippy; inspect measured scopes and record source/lockfile/compiler provenance.


## Long-project media — 9 October 2026

- [x] Bounded seekable WAV reader with shared parser/decoder, source/header/window limits and descriptor mutation checks.
- [x] Supported format bit parity, random/irregular windows, failure zeroing, topology validation and sparse far-offset reads; 25 media tests and strict Clippy pass.
- [x] Full Session resident import/open/history/save regressions pass after parser refactor.
- [x] Incremental project-rate conversion with exact global phase/source halos; 33 media tests, strict Clippy and 2,700 frozen-v1 windows per debug/release build pass.
- [x] Share immutable rate-pair kernels: 100-plan allocation/lifetime coverage and 38 media tests pass.
- [x] Verify raw source pages with bounded cancellable SHA-256 manifests before decode/conversion; 47 media tests and independent debug/release hash/PCM/halo/edit acceptance pass.
- [ ] Implement docs/page-delivery.md: actual hex-stream worker/page queues, borrowed engine rendering, 32 MiB initial pool and source-generation ownership; then native publication/lifecycle.
- [ ] Bounded worker waveform building, immutable source/page identity, scheduling and cache ownership.
- [ ] Callback-safe prepared page access integrated into Arrangement, Session and export.
- [ ] Sustained native 100-long-track mixed-rate playback/edit/seek/launch acceptance.

## Native stream ownership — accepted; paused

- [x] Retain actual worker/catalog owner through opaque leases and callback pages.
- [x] Reject busy shutdown without mutating or losing either half; retry after graph retirement.
- [x] Verify premature control/endpoint release, generation/fingerprint coverage, pair collision and final off-render join; debug/release stream suites, allocation probes and independent real-worker checks pass.
- [ ] Integrate graph/export leases and bounded callback endpoint retirement into native audio lifecycle.
- [ ] Add separate control prewarm queue and installed-page readiness; protect first transients on Play/Seek/Session launch.
- [ ] Run sustained advancing 100-track native streaming and device-fault acceptance before lifting resident import limits.

Implementation paused at user request after catalog ownership acceptance. Await
user priorities for faster practical usefulness before selecting further work.
The full roadmap remains retained; no prewarm/native streaming work started.

## User-selected usability: left browser — implemented; user acceptance pending

- [x] Default-open compact Live-style browser in Arrangement and Session.
- [x] Bounded background folder/WAV listing, current-folder search and native folder picker.
- [x] Double-click sample import and actual available instrument/project actions.
- [x] Collapse/expand, resize and correct remaining workspace geometry.
- [x] Targeted library/UI checks: 3 library and 188 UI tests passed (1 existing ignored); strict Clippy, fmt, architecture/style guards and desktop build passed.
- [x] Native startup/layout and final search-placeholder contrast inspected in an isolated review build.
- [x] User accepted the browser checklist except right-edge resizing; X-button collapse works.
- [ ] User retest right-edge resizing after the full-height separator fix (automated pointer regression passes).

User requested to own native interaction testing while present. Review build:
`artifacts/hex-browser-review-20261009.app`. The running main development app
was preserved because it contains unsaved user work; it has the initial browser
build, while the review bundle includes the final placeholder contrast fix.
Preview audio, drag-to-timeline, recursive indexing and persisted places remain
deferred. The broader roadmap remains paused.

## User-selected hybrid tracks — implemented; user acceptance pending

- [x] Unified canonical audio/MIDI contents and validated edit/history commands.
- [x] Versioned persistence and lossless legacy track migration.
- [x] Mixed Arrangement rendering and audio/MIDI Session switching/admission.
- [x] Source-based compact UI: audio import and MIDI creation on every track.
- [x] Targeted tests/lints/build/render and allocation acceptance.
- [ ] User native acceptance checklist and feedback.

Source checks and independent debug/release PCM/persistence acceptance pass.
GPUI support: 190 passed, one existing ignored. Mixed-source callback probes
pass in debug/release with zero allocation/deallocation activity. Separate
review build: `artifacts/hex-hybrid-tracks-20261009.app`; includes browser resize
fix. Native checklist is in docs/hybrid-tracks.md. Broader roadmap remains paused.

## Source control — 9 October 2026

User authorized Git initialization and pushing the existing project to a private
`PORTALSURFER/hex` GitHub repository on `main`. Source, tests, docs, original
design studies and attributed fonts are included; build artifacts and private
reference analysis remain excluded. No distribution license selected.

## Arrangement landing preview and MIDI dragging — implemented; native acceptance pending

- [x] Replace floating drag ghost with exact snapped/clipped target-track preview.
- [x] Enable same/cross-track MIDI clip moves using canonical beat timing/history.
- [x] Verify preview/drop agreement, original grab offset and cancellation/stale guards.
- [x] Targeted UI tests/lints, desktop build, source guards and separate review app.
- [ ] User native audio/MIDI drag, undo and cancellation acceptance.

UI suite: 192 passed, one existing ignored. Final snap-off repair: musical-time
8/8 and drag 11/11; strict UI Clippy, desktop build, fmt, architecture/style
guards pass. Review: `artifacts/hex-clip-drag-20261009.app`; native checklist in
docs/clip-drag.md. Existing running hybrid review app preserved. No commit/push
for this change; broader roadmap remains paused.

## Next-build repairs and browser persistence — source accepted; user retest pending

- [x] Repair reported audio/MIDI clip pickup regression; exercise rendered GPUI dispatch.
- [x] Persist added browser folder paths outside project metadata, with bounded background I/O.
- [x] Restore saved places on startup and preserve unavailable folders/casing/order.
- [x] Verify persistence and malformed/unwritable failure preservation; targeted checks/build.
- [x] Separate next build; user confirms browser persistence and audio/MIDI dragging work (9 October).

Previous clip-drag source checks did not establish native pickup: user reports
no drag begins. Its native acceptance remains failed/pending repair.

8 preferences tests and 197 UI tests pass (one existing ignored), including
rendered cross-row preview/drop dispatch, simulated worker-completion re-pickup,
painted-generation scroll eligibility and background place restore/save. Strict
affected lints/build/guards/fmt/diff checks pass. Combined review bundle:
`artifacts/hex-browser-places-20261009.app`. User restart and drag acceptance
remain open. Broader roadmap paused; no commit/push for this change.

## Browser WAV dragging and conditional clip ghost — source accepted; native check pending

- [x] Browser WAV becomes a cursor-attached floating clip outside track lanes.
- [x] Valid track hover replaces floating ghost with exact snapped landing preview.
- [x] Valid release imports onto hovered hybrid track/start through canonical worker.
- [x] Existing audio/MIDI clip drags show floating preview outside lanes and locked landing inside.
- [x] Background bounded waveform/duration preparation, cancellation/stale guards and rendered regressions.
- [x] Establish outbound boundary: pinned GPUI has no native outbound file drag; separate integration deferred.
- [x] Affected checks/build and separate review app.
- [ ] User native interaction/listening acceptance (checklist in `docs/clip-overwrite.md`).

200 UI tests pass (one ignored), preferences/session suites and strict affected
lints/build/fmt/guards/diff checks pass. Separate review build:
`artifacts/hex-browser-drag-20261009.app`. User checklist in docs/browser-drag.md.
In-window dragging only; no native cross-app file drag or held-source pinning.
Earlier accepted browser persistence and clip drag remain covered. No commit/push.

User acceptance — 9 October: browser WAV free/locked dragging works. User authorized committing and pushing this completed milestone before track-insertion work.

## Sample drop creates an inserted track — implemented; native acceptance pending

- [x] Canonical atomic prepared import at stable before-track/end destination and clip start.
- [x] Browser hover between tracks: full-width insertion line, no floating ghost.
- [x] Empty/bottom Arrangement sample drop creates bottom hybrid track and clip.
- [x] Preserve existing track-body import, cancellation and stale/painted geometry guards.
- [x] Model/session order/history/persistence/failure and rendered interaction tests.
- [x] Affected checks/build and separate review app.
- [ ] User native interaction/listening acceptance (checklist in `docs/clip-overwrite.md`).

Accepted browser/clip/preferences source was committed and pushed to main as
1d892dc. New insertion work remains uncommitted until authorized.

Source acceptance: full project/session suites and 205 UI tests pass (one
existing ignored), strict affected lints, desktop build, fmt, both guards and
diff check pass. Rendered gap/bottom/empty/scroll cases and ghost suppression
are covered. Review build: `artifacts/hex-track-insert-20261009.app`. New feature
remains uncommitted; earlier accepted changes are on origin/main at 1d892dc.

## Track-creation drag follow-up — implemented; native retest pending

- [x] Bottom/empty append keeps floating ghost and end-line; dividers show line only.
- [x] Fix bottom-hover workspace jump with rendered geometry regression.
- [x] Fix insertion timeline origin so sample lands under pointer with current snapping.
- [x] Atomic audio/MIDI move into a newly inserted/appended hybrid track; retain clip contents/IDs/source track.
- [x] Model/history/persistence/failure and actual rendered pointer regression checks.
- [ ] Separate repaired build and user native acceptance.

209 UI tests pass (one existing ignored), focused project import/MIDI 30 and
Session service 18 pass; independent full project/session suites pass. Strict
affected Clippy, desktop build, fmt, guards and diff checks pass. New review:
`artifacts/hex-track-drag-fix-20261009.app`. User retest remains open; source
track is retained and existing clip IDs/contents are moved without copying.
No commit/push of the insertion feature or these corrections.

## Drop-area highlight and overwrite — source implemented; native review pending

- [x] Zero-track body highlight with ghost and no cursor-following line.
- [x] Blank append area highlight plus fixed bottom line and floating ghost.
- [x] Explicit MIDI fragmentation/rounding/source identity contract (`docs/clip-overwrite.md`).
- [x] Drop-specific atomic overwrite across audio/MIDI collections, browser imports and existing moves.
- [x] Fragment source/timing, failure bounds, undo/persistence and rendered tests.
- [x] Affected checks/build and separate review app.
- [ ] User native interaction/listening acceptance (checklist in `docs/clip-overwrite.md`).

209 UI tests pass/one ignored; independent full project/session suites and five
audio split PCM regressions pass. Strict affected lint/build, guards/fmt/diff
checks pass. Review: `artifacts/hex-overwrite-drop-20261009.app`; evidence in
`overwrite-drop-manifest.json` and associated logs. Native review remains open;
no commit/push.


## divider highlight correction — 9 October 2026

Native user report: between-track hover highlighted every row below the divider.
The append overlay was gated only by insertion-line presence; it now also
requires a current new-track destination with no before-track anchor. Divider
hover remains line-only for browser samples and existing audio/MIDI clips;
zero-track and bottom empty-area highlighting retain their prior behavior.
All three new rendered divider assertions failed before the fix; the fixed
full UI suite passes 209 tests (one existing ignored). Strict UI Clippy, desktop
build, guards/fmt/diff pass. Review: `artifacts/hex-divider-highlight-20261009.app`;
native retest remains pending. No project/audio model or overwrite behavior change.


## arrangement grid and pointer navigation — active, 9 October 2026

User-selected next workflow: adaptive musical grid in Arrangement, wheel zoom
in/out anchored at the pointer, middle-button drag pan horizontally and
vertically. Arrangement only; retain browser/editor scroll behavior and clip
gestures. Grid should remain compact, visible behind clips, aligned with ruler
through tempo/meter changes, and bounded to visible content. Reuse virtualized
track scrolling and window-level gesture capture; no audio/schema changes.

Acceptance: useful adaptive density at zoom extremes; stable cursor anchor and
clamped time/scroll limits; both pan axes and release outside the workspace;
meaningful maths/rendered event tests, strict UI lint/build and style guards.
Native interaction acceptance belongs to the user. New work follows published
checkpoint `b873931930fef57f49b60cfc5005faa406b555f2`; keep it uncommitted for review.

- [x] Draw bounded adaptive musical grid behind Arrangement clips and empty space.
- [x] Cursor-anchored wheel zoom scoped to Arrangement/ruler.
- [x] Captured middle-button horizontal/vertical pan with bounded list scrolling.
- [x] Maths and rendered interaction checks, strict UI lint/build and guards.
- [x] Fresh separate review bundle.
- [ ] Native user acceptance: `docs/arrangement-navigation.md` checklist.

Contract/checklist: `docs/arrangement-navigation.md`.

Navigation source milestone: 216 UI tests pass/one ignored, strict UI lint,
desktop build and guards/fmt/diff pass. Review app
`artifacts/hex-arrangement-navigation-20261009.app`; native input/feel review
remains pending. Drag/drop checkpoint b873931 pushed and remote verified; new
navigation work remains uncommitted.


## adaptive snapping and prospective track preview — active, 9 October

Latest user correction keeps grid in blank space and zero-track Arrangement,
enables matching snapping there, and requests a locked clip preview on the
prospective appended row. Command temporarily inverts snap per official Live12
manual; dynamic preview/release modifiers must agree. Track-only rendering is
superseded. Finite scope, reference confidence and native checklist are in
`docs/grid-snapping.md`. New source remains uncommitted for native review.

Adaptive snapping/append preview source milestone complete: shared visual/snap
policy defaults to Adaptive, Command temporarily inverts it, stationary changes
refresh previews, and actual release modifiers determine the committed edit.
Browser/audio/MIDI append previews lock to the prospective row and clip to the
body. Grid stays in blank/zero-track space per latest correction; dividers remain
line-only. 220 UI tests pass/one ignored; strict UI lint/build and guards/fmt/diff
pass. Review `artifacts/hex-grid-snapping-20261009.app`; checklist/reference limits
in `docs/grid-snapping.md`. Native acceptance open; no commit/push.

- [x] Shared adaptive/fixed/off paint and placement policy, Adaptive default.
- [x] Command inversion at pickup, stationary modifier changes and release.
- [x] Prospective appended-row preview for browser/audio/MIDI, including no tracks.
- [x] Targeted/rendered/full UI checks, lint/build and separate review app.
- [ ] Native grid snapping, Command and blank-area preview acceptance.


## grid appearance, native input and overview — active, 9 October 2026

User reports snapping functionality looks good, but Command hides the grid,
wheel zoom direction is inverted and native middle-button pan fails. Keep
grid painting independent of temporary snap modifiers; add alternating subtle
bands every four adaptive units anchored to musical time. User cancelled the
scrollbar/handle and instead requested an always-fit overview above the ruler.
Use a bounded cached 128×16 audio/MIDI occupancy thumbnail, aggregating dense
tracks, with viewport outline and click-to-center. Preserve compact layout.

Pinned GPUI0.2.2 lacks native otherMouseDragged callback and scroll inversion
metadata. Primary accepted a narrow external dependency repair, after readonly
architecture review: native callbacks/flag inside excluded upstream GPUI only,
no unsafe Hex or registry-cache edit. Provenance/license/check policy is in
`docs/native-input.md`. Physical positive-Y up zooms in, negative down zooms out;
browser/panel raw scrolling retains OS preference behavior.

Acceptance: Command grid remains visible while movement is free; band phase
remains stable under pan/zoom/tempo/meter; cached overview includes every track
and both clip kinds without whole-project per-paint scans; click/view geometry
and bounds; wheel Lines/Pixels both inversion states; native target compiles;
prior snapping/preview/navigation regressions pass. User owns native input
and visual verdict. No scrollbar, audio/schema change, commit or push.


## overview and native-input review milestone — 9 October 2026

Implemented the compact always-fit Arrangement overview above the ruler,
revision-cached audio/MIDI raster, viewport outline and click-to-center. Details
and native checklist: `docs/arrangement-overview.md`. Command no longer hides
the grid; four adaptive-unit alternating bands stay anchored to musical time.
Physical wheel deltas are normalized independently of macOS Natural Scrolling.
The pinned external GPUI patch registers native middle-drag delivery; source
and simulated event tests cannot establish actual physical delivery.

Review build: `artifacts/hex-overview-input-20261009.app`. Source/binary identity:
`artifacts/overview-input-manifest.json`. Native visual, physical wheel (both
Natural Scrolling settings) and middle-button pan acceptance remain open.
No scrollbar, audio/schema change, commit or push. Broader roadmap stays paused.

Verification: 228 UI tests pass, one existing ignored. Strict UI Clippy, desktop
build, formatting, architecture/UI-style/GPUI-patch guards and diff checks pass.
The final band candidate budget is bounded even when conversions yield no
painted bands. Existing upstream dependency compiler warnings remain; no Hex
warning or check failure. Native interaction/appearance verdict remains open.


## minimal append feedback — 9 October 2026

The user superseded empty-area background highlighting after accepting clip
snapping. Remove the full-area tint for browser samples and existing audio/MIDI
clips. Retain the bottom line of the last track and the snapped clip preview.
With no tracks, show only the prospective clip preview; there is no last-track
line. Preserve grid shading, snapping and drop destinations. Native review
remains the user's responsibility.

Source complete: 228 UI tests pass, one ignored; strict UI Clippy, desktop
build, format and architecture/style/GPUI patch guards pass. Rendered tests
verify no empty-area tint while the bottom line and clip remain. Review build:
`artifacts/hex-minimal-append-20261009.app`. Native verdict pending; uncommitted.


## live clip resize — active, 9 October 2026

Finite scope: thin edge handles at all meaningful zoom levels, a preserved
clip-body move target with detection priority, actual in-track geometry/waveform
preview during trimming, existing adaptive/fixed snapping and Command inversion
including stationary modifier/release changes. Commit one validated canonical
edit at release; cancellation/stale/no-op gestures preserve project/history.
Audio trim source/fade limits remain canonical. MIDI Arrangement trim policy
requires source-contract review before extending this work.

Acceptance: both edges at narrow and wide rendered clip widths; move versus
resize hit targets; live preview with unchanged project before release; grid/
free candidate and final edit agreement; cancellation and stale state; source
tests, strict UI lint and native build. User owns native UI checks. No schema,
audio callback, commit or push; broader roadmap stays paused.

The user explicitly included MIDI on 9 October 2026. Added the canonical
beat-based `TrimMidiClipEdge` and O(1) `MidiClipTrim` preview: retain authored
notes/IDs/source bounds, adjust source marker for left edits, preserve loop
phase by the documented original policy. Right extension can expose silence
for a one-shot. No schema fields changed. Source policy: docs/midi.md; visual
contract/checklist: docs/clip-resizing.md. UI integration and native verdict
remain pending. This supersedes the MIDI deferral in this work-unit heading.


## audio/midi resize source milestone complete — 9 October 2026

Thin bottom-corner handles (up to 4×6px; narrow widths derived from the rendered
3px audio/4px MIDI minimum) preserve the upper move target. In-lane previews
redraw waveform/note geometry immediately; original hit geometry remains fixed
and transparent. Existing grid/Command inversion updates during motion,
stationary modifier changes and authoritative mouse-up. Release commits one
validated edit; cancellation/no-op/stale state preserves canonical data.
MIDI metadata-only previews retain authored notes and source/loop markers.

233 UI tests pass, one ignored after the final narrow-width repair. Workspace
integration: 695 tests pass, one ignored before that UI-only repair. Project/
storage/session/UI strict lint, desktop build, fmt and architecture/UI-style/
GPUI-patch guards pass. Logs and source/binary identity are in
`artifacts/clip-resize-manifest.json`. Existing upstream compiler warnings remain.

Review `artifacts/hex-clip-resize-20261009.app`; native audio/MIDI left/right
corner usability, real-time appearance, Command release agreement and Undo
acceptance remain open. Checklist: docs/clip-resizing.md. No commit/push or
automated native interaction; broader roadmap stays paused.


## invisible full-height clip edges — 9 October 2026

The user superseded bottom-corner resize handles. Audio and MIDI clips now use
invisible full-height left/right edge strips with the native horizontal resize
cursor. Remove orange rectangles; derive narrow hit widths from rendered clip
width while preserving a central full-height move region. Live previews, grid
snapping, Command override and canonical release semantics remain unchanged.
Native top/middle/bottom edge hover and resizing acceptance remains pending.

Full-height edge source checks pass: 233 UI tests/one ignored, strict UI lint,
desktop build, fmt and architecture/style/GPUI patch guards. Review build:
`artifacts/hex-clip-edges-20261009.app`; identity in
`artifacts/clip-edges-manifest.json`. User native cursor/edge usability check
pending. Uncommitted; prior apps and unsaved work preserved.


## bracket cursors and flush clips — active, 9 October 2026

User accepts full-height resize behavior; requests distinct bracket cursors,
flush full-track clip height and a one-pixel black border replacing the left
accent stripe. Finite scope: original left/right bracket shapes using the
existing external native cursor boundary; cache/retain native objects, preserve
edge hotspots/body movement, no per-hover allocation. Apply flush clip geometry
and border consistently to audio/MIDI, live resize and landing previews.
No project/audio/schema changes. Source checks/rendered geometry and native
build first; user owns physical cursor/appearance acceptance.

Waveform scope added by the user: replace 48 blue amplitude bars with black
filled signed per-channel silhouettes and zoom-dependent detail; paint the
existing musical grid through clips. Control-worker peak caches now use up to
65,536 leaves, a bounded extrema hierarchy and shared immutable PCM for exact
small-range queries. Accounting includes hierarchy bins. Media/session tests
and strict lint pass; UI integration/native appearance pending. Contract:
`docs/waveform-display.md`. No callback/schema changes or broader roadmap work.


## bracket/clip/waveform source milestone complete — 9 October 2026

Original bracket cursor variants, two retained macOS cursor objects, directional
fallbacks elsewhere, and a narrow test-only rendered-cursor request readout.
Verified original GPUI baseline and exact 12-file input/cursor delta; no unsafe
Hex code, proprietary assets or registry changes. Cursor delivery/scaling/hotspot
appearance remains native acceptance.

Audio/MIDI clips and prepared previews fill their 44px rows with a one-pixel
black border and warm fills. Shared black signed stereo waveform paths draw
zoom-adaptive detail from one query per column, with a bounded 4,096-column cap
and real sample traces at deep zoom. Grid lines remain aligned through fills
and beneath the waveform; empty-area feedback remains line/clip only.

235 UI tests pass, one ignored; workspace tests 700 pass, 1 ignored.
Media/session/UI strict lint, desktop build, formatting, architecture/style and
pinned GPUI provenance guards pass. Native paint timing remains unmeasured.
Review: `artifacts/hex-clip-visuals-20261009.app`; source/binary/log identity in
`artifacts/clip-visuals-manifest.json`. Check both bracket hovers, waveform zoom
and source detail, flush borders/grid continuity, and resize/drop previews.
User owns native checks; prior running apps/unsaved projects preserved.
No commit/push; broader roadmap remains paused.

## waveform paint correction and track context menu — 9 October 2026

The user reported a blank waveform in the clip-visuals review build. The shared
canvas had no intrinsic height, so its paint callback received zero-height
bounds and returned. Pinning the canvas to its parent fixes that layout path.
Removed the old 3.5-percent viewport-width waveform cutoff for normal and
resize-preview clips, and removed a crossing closure segment in the silhouette.
A rendered-layout regression checks full canvas bounds on 8px and 260px clips.
Native waveform appearance must be rechecked by the user; previous source checks
did not establish visible rendering.

Added a reusable compact rectangular popup with a fine border/drop shadow,
lowercase text, keyboard navigation/Enter, Escape/outside-click dismissal and
window-edge clamping. Arrangement row right-click offers only `insert track`,
inserting a hybrid track below the clicked row; empty-area right-click appends.
The new canonical `InsertTrack` command uses a stable optional insertion anchor
and one undo step. Generation/anchor checks refuse stale menu edits. Popup focus
restores on dismissal and timeline wheel/pan/seek capture respects the popup.
No submenus, extra track types, or broader roadmap work were added.

Tests cover rendered selection/dismissal/keyboard routing, popup clamping,
track order, append, missing-anchor rejection and undo/redo. Native rendering,
menu appearance and user workflow acceptance remain pending. No commit/push.

Verification: 747 workspace tests passed, 1 ignored; 239 UI tests
passed, 1 ignored. Strict UI/project lint, desktop build, formatting and
architecture/UI-style/GPUI guards passed. Separate review bundle:
`artifacts/hex-waveform-menu-20261009.app`; source/binary/check identity:
`artifacts/waveform-menu-manifest.json`. User checks: visible waveform while
zooming/resizing; right-click track/empty space; insert position; Undo/Redo;
Escape and outside-click dismissal. Running apps and unsaved work preserved.

## arrangement edit cursor and Command-E — 9 October 2026

Finite work unit: track clicks place a grid-aware pulsing one-device-pixel edit
cursor with steady top/bottom markers; Command-E splits the containing audio or
MIDI clip without reading/waiting for the audio playhead. Keep clip drag/trim
priority, transient cursor lifecycle, canonical validation/history and separate
transport behavior. Contract: `docs/edit-cursor.md`. Native acceptance pending.
MIDI split retains the left authored source and materializes the finite right
region into a one-shot; carry notes retrigger, and right loop/source settings
reset. Expansion is bounded and errors leave project/history unchanged. No
claim of exact Live split or seamless DSP parity.

Verification: 757 workspace tests passed, 1 ignored; strict UI and
project/session/storage lint, desktop build and source guards pass. Fresh review
app: `artifacts/hex-edit-cursor-20261009.app`; identity and logs:
`artifacts/edit-cursor-manifest.json`. Native click/pulse/markers, Command-E,
Undo/Redo and continued clip drag/resize checks remain with the user.

## next guided workflow: unified clip view — 9 October 2026

User requested a shared toggleable audio/MIDI clip detail view based on the
provided screenshots, with a compact left parameter section and a waveform or
piano roll on the right. Track it as separate targeted PRs after the edit-cursor
PR. First: visibility/selection retention, cached audio detail and existing
MIDI/source edits, plus canonical audio loop control. Then real per-clip gain,
pitch/playback-rate policy, warp/time-stretch modes and detail gestures/numeric
entry. The complete request remains open; do not claim full Live clip-view
parity or display unsupported controls as if functional. Plan and acceptance:
`docs/clip-view.md`.

First clip-view panel is implemented: shared toggle, retained selection/focus,
compact MIDI left controls and Arrangement audio source waveform/ruler/markers.
Real audio position/length/source/play-start/loop controls dispatch canonical
undoable edits. 715 workspace tests passed (1 ignored); feature-enabled GPUI
248 passed (1 ignored); affected strict lint, build and source guards pass.
Review: `artifacts/hex-clip-view-20261009.app` and
`artifacts/clip-view-manifest.json`. Native acceptance and the remaining gain,
pitch, warp, marker gestures, zoom and numeric entry are open.
