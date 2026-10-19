# Session audio implementation contract

Status: implementation in progress, 9 October 2026. This is Hex's chosen contract,
not measured Live parity. Source acceptance and native acceptance are separate.
Reference provenance and unwarped/warped distinction:
[SESSION-AUDIO-001](../research/observations/session-audio-001.md).

Model/storage checkpoint: root independently passes 102 project and 23 storage
tests plus strict all-target Clippy. New coverage verifies bounded audio-track
sources, asset reference protection, fresh scene/cell-copy IDs, independent source
editing/history, malformed asset/range/ID snapshots and schema-12 round-trip.
The initial schema-11 migration fixture contains Arrangement data; preservation
of existing MIDI Session contents across that migration is still required.
Additional audio-specific invalid setter/history, generic cell lifecycle and
limit cases remain under review. Logs:
artifacts/session-audio-model-storage-root-{tests,clippy}.log.
Prepared engine/worker/UI integration and application build remain in progress;
these model/storage checks do not establish a usable audio-cell workflow.

## Console browser-drop source checkpoint (19 October 2026)

Console (the Session clip grid) accepts one existing browser audio payload on a
cell body, replacing only that cell after successful media preparation. Narrow
four-logical-pixel row edges insert before/after the actual scene; the explicit
bottom row appends a scene. Right-clicking a scene label inserts before that
stable SceneId; the bottom row offers Add Scene even with no tracks or scenes.
Existing toolbar Add Scene still appends. Targets capture TrackId/SceneId and
project generation/revision, never current selection or a guessed newest ID.

All browser placements use the existing AudioBatchImport/AudioBatchControl.
Decode/preparation precedes a single candidate/batch, one graph preparation and
one Undo entry including optional scene creation and occupied-cell replacement.
Resident and VerifiedStreaming use the actual typed prepared Session clip to
resolve its asset binding. Limits, retained old/new media budgets, validation and
refusal remain in their existing owners. Undo restores authored contents and
asset references while deliberately retaining the canonical ID allocator high-
water mark; Redo restores the same clip and scene IDs. Failure preserves the old
cell, model, media and history. No hidden Arrangement clip is created.

Browser UI cleanup is payload-identity scoped: drop/rejection/outside release,
Escape, view changes and canonical revision/project changes retire the matching
ghost, target and scroll state. Root release cleanup is deferred until actual
child drop dispatch has consumed the payload, and never clears Trim, clip or
native-device captures. The worker control owns pending import cancellation;
a retained drag object grants no admission authority. Console moves do not use
retained Arrangement geometry. Arrangement clip payloads keep only the existing
same-track, empty-existing-cell copy contract; cross-track, occupied or inserted-
scene clip-copy payloads refuse explicitly rather than adding a new copy model.

Focused worker/model and rendered-event checks are source evidence. Native
component verification, actual audio hardware/listening, physical drag/context
acceptance and optimized installed delivery remain separate. Earlier dated
contracts below are historical; current tracks are hybrid and historical-alpha
migration work is not part of this change.

## Canonical source and persistence

A Session audio clip owns a globally unique ClipId, its own name, an AssetId and
a half-open source frame interval. Frames are in the project's converted playback
rate, not the original file rate or musical beats. Require
`0 <= source_start < source_end <= asset.playback_frames`. Only audio tracks may
contain audio cells; MIDI cells retain their instrument-track restriction.

Share immutable asset PCM; do not reference an Arrangement clip as the Session
source or create a hidden Arrangement placement. Copying Arrangement audio into
Session creates a fresh clip ID and independent source metadata over the same
asset. Changing either source later must not change the other. Occupied targets,
missing/stale sources/assets and invalid ranges refuse atomically.

Extend existing generic cell/scene move, duplicate, clear, remove, global clip/ID
limits, asset-reference protection, history and snapshot validation. Scene copies
preserve audio sources with fresh clip IDs. Asset deletion must refuse if either
view still references it. No source ID may collide across either view.

Write strict schema 12 once the audio representation is integrated; retain
decoding/migration of schemas 1–11 without changing prior audio/MIDI semantics.
Do not emit an audio cell under schema 11 or silently drop unknown source data.
Storage remains metadata-only. Resident PCM belongs to the Session/media layer.

## Prepared ownership and render

PreparedRevision preparation already receives the authoritative asset media bank.
Compile an immutable Session audio inventory containing track/scene/clip/asset
identity, validated bounds and shared project-rate PCM. Check media rate/length
against canonical asset metadata before committing the candidate. Reuse prepared
audio only when source metadata and PCM identity both match.

Use typed launch anchors: MIDI keeps its existing beat anchor; unwarped audio
uses an absolute performance-frame anchor. Set the audio anchor at actual route
application, including quantized due transitions. Never convert an audio source
cursor through the tempo map. Beat quantization schedules when ownership changes;
it does not stretch or transpose the unwarped sample.

For elapsed output frames `e`, render PCM at `source_start + e` while inside the
validated interval, then exact silence. Maintain Session ownership after the
one-shot ends. A new launch restarts it; StopTrack, explicit Stop cells and return
to Arrangement use the existing route commands. No automatic Arrangement return.
This post-end ownership choice requires later installed-reference verification.

Pause/resume preserves source position. Seek retains the selected audio owner
but reanchors it at the requested performance frame, restarting at source_start.
Transport Stop resets the clocks and audio anchor to zero, matching the existing
restart shape for MIDI. These are explicit Hex discontinuity rules; no reference
timing equivalence is claimed.

Candidate handoff for the same stable clip, track and content kind preserves
elapsed audio frames across tempo edits, AssetId replacement and window changes.
If the new window is shorter than elapsed time, retain the owner and emit silence
until relaunch. Removed/mismatched clip or track cancels ownership to Silence.
Pending actions continue to use existing prepared beat-grid scheduling and
generation/revision checks. Failed candidate publication retains the old graph
and audible state. No optimistic UI ownership.

The callback reads only prepared PCM and fixed metadata. No new allocations,
locks, I/O, waits, logging or final asset drops. Shared old/new media lifetime
must remain under prepared-revision retirement/reclamation on the control side.
Reuse does not transfer asset ownership into a disposable callback-local object.

Ownership inspection during implementation: PreparedRevision owns its Session
inventory strongly; prepared audio retains a cloned Audio handle over shared PCM.
In hex-audio/src/prepared.rs, accepted/rejected/replaced publications go through
`return_graph`, including a retained slot under retirement backpressure.
`Control::reclaim` performs publication destruction outside the callback, and
shutdown returns active/deferred/queued publications before teardown. Keep the
new media exclusively under that ownership path. This is source inspection;
audio-specific callback retention/reclamation tests and allocator instrumentation
remain acceptance work, not inferred from existing graph tests alone.

Project replacement is a separate publication contract: successful Open must
carry an explicit runtime-reset policy so a new file cannot inherit old Session
ownership from coincidentally reused IDs. A fresh runtime starts paused at frame
zero with Arrangement ownership; existing retirement and priority Stop rules
still apply. Normal edits, imports, history and same-project recovery inherit
runtime state. Failed Open cannot reset the active project. Source review found
an unconditional inheritance defect; the implemented InstallFreshRuntime policy
skips inheritance for caller-prepared fresh graphs. Same-ID replacement and
full-retirement-queue/priority-Stop regressions pass. Native project replacement
remains unverified under the issue documented in docs/issues.md.
Preserve reset intent while coalescing newer candidates of the same project
generation under publication backpressure. Once admitted, the publication gate
orders subsequent normal edits behind the fresh installation. Capture project
generation for audio import path prompts and reject stale dialog completions;
target-ID existence alone is not an identity check across files.

## Worker and compact UI

Use the canonical control worker for decode/resample/import, edit validation,
history, waveform caching, complete preparation and publication. A failed import
or missing/mismatched asset preserves the old project; resident-frame limits
apply before candidate commit. Reuse resident assets for Arrangement-to-Session
copy instead of decoding the same file again.

Select audio cells without opening the MIDI piano roll or resolving their IDs as
Arrangement selections. Extend generic Enter/clip/scene launch, route/pending
markers, cell delete and copy admission with stable IDs and generation guards.
Reuse compact rows, cached waveforms and small toolbar icons; source controls use
time/frame units and theme::text. Native waveform/focus/layout acceptance remains
required at minimum and normal window sizes. No new decorative playback model.

## Acceptance sequence

1. Model/history: audio-track restriction, bounds/assets, globally unique IDs,
   count/exhaustion refusal, independently editable copies and generic scene/cell
   operations; failed edits preserve undo/redo and current metadata.
2. Storage: schema-12 round-trip and schema-1–11 migrations, malformed source and
   missing asset rejection, exact source/rate/conversion metadata preservation.
3. Prepared inventory: authoritative PCM rate/length checks, stable lookups,
   metadata/PCM reuse, precommit refusal, retirement lifetime inspection.
4. Engine: independent direct-frame PCM oracle, irregular block partitions,
   exact start/end/due samples, tempo independence, pause/Seek/Stop, scene/Stop
   cells, cancellation/return and mixed Arrangement/MIDI ownership.
5. Handoff/native backend source: active source/window/asset edits and removal,
   transactional candidate refusal, fixed bounded callback work and reclamation.
6. Worker: original WAV import/copy, Save As/Save Current/fresh-service reopen,
   metadata/inventory/PCM parity, missing media and resident-limit rollback.
7. UI: production GPUI action/worker tests for selection, focus, typed copy,
   deletion and truthful offline launch gating; actual native input/appearance
   and physical listening/deadlines remain separate gates.

Warped beat-synchronized playback/loops, pitch, fades, envelopes, legato/follow
actions, recording/capture, streaming and full Live parity remain required later.
Unwarped one-shots are a real workflow prerequisite, not a substitute for those
features. Do not mark this implementation accepted from model tests alone.

## Source acceptance checkpoint — 9 October 2026

Root's integrated affected-crate run passes, including 181 GPUI-support UI tests
and one ignored benchmark. Engine tests compare direct frame samples, exact
quantized onset, mixed sounding MIDI/audio/Arrangement routes and source/asset
handoff. Model checks cover wrong-track snapshots, stale IDs, global ID exhaustion
and combined clip limits. The schema-11 fixture preserves MIDI notes and Stop.
Worker SaveNew/SaveCurrent and a fresh Service reopen retain the exact project,
paths, casing and IDs, and render the edited half-open source interval followed
by exact silence. Occupied, missing-media and resident-budget refusals retain
revision/history/document state and playable resident PCM.

The real retirement-ring backpressure regression verifies Stop while fresh
replacement waits, Arrangement ownership after reclamation, and old/new graph
lifetime until control-side drops. UI source nudges clamp the nonempty range,
use approximately 1 ms at the project rate, and Alt selects a one-frame nudge.
Stale generation/revision/selection/source and mode guards protect callbacks;
the source row remains visible at at most 38px during pending edits.

Logs: `artifacts/session-audio-integrated-root-tests.log`,
`artifacts/session-audio-worker-storage-root-tests.log`,
`artifacts/session-audio-reset-backpressure-root-tests.log`,
`artifacts/session-audio-panel-root-tests.log` and
`artifacts/session-audio-final-root-clippy.log`. Targeted strict all-target Clippy,
formatting and architecture/style guards pass. This is source and test-platform
evidence. Native dialogs, visual/input/listening, full UI publication-coalescing
interaction, allocator instrumentation, large audio workloads and physical
deadlines remain open.

UI replacement follow-up: a GPUI event-sequence test exercises the production
receive handler with actual canonical gain edits around a successful Open,
failed Open, control-queue admission and a second Open. It verifies that the
latest prepared candidate retains fresh installation until admission, later
edits inherit, and project generations stay correct. Targeted test and strict
feature-enabled UI Clippy pass in
`artifacts/project-open-coalescing-ui-root-{tests,clippy}.log`. Actual native
dialog/publication timing remains unverified.

## Headless streamed sources — 9 October

PreparedRevision can now resolve the same mixed resident/streamed source bank
for Arrangement and Session. Session one-shots expose `source()` and optional
`resident_audio()`; streamed clips carry immutable metadata rather than fake
resident PCM. Metadata value equality (including generation and fingerprint),
asset/track and interval determine reuse; gain-only revision handoff preserves
the launch-frame phase. Source replacements retain the existing phase contract.

SessionPlayback and Playback accept a borrowed MediaPages provider, sharing
Arrangement page-span mixing and the existing transport loop scheduler. Audio
remains an unwarped frame-domain one-shot. Missing/short/wrong-index pages
request only unavailable spans, leave other tracks progressing and silence the
unavailable contribution. A playing streamed graph without a provider refuses
before output/state mutation; paused and empty-block semantics are preserved.

Native Control rejects these graphs until callback endpoint ownership, catalog
lifetime, readiness and retirement are integrated. These headless APIs do not
lift import limits or establish native streaming/device acceptance. Independent
real-worker PCM and allocation evidence is recorded in docs/performance.md;
additional failure-path acceptance is currently under review.

Streamed Session source acceptance completed: full engine suites (70 unit tests
plus integrations), strict engine/audio/stream Clippy and workspace formatting
pass. Added coverage verifies short/wrong-index pages, cold misses preserving
audible resident MIDI/audio, finite Playback loops, and later-chunk refusal after
a page request: the full block zeros while routes/clocks/releases roll back.
The existing actual-worker oracle, allocation probes and desktop build pass.
Provenance: `artifacts/stream-session-acceptance-manifest.json`. Native source
lifetime/readiness/publication, persisted fingerprints and sustained advancing
streaming remain open. The next implementation unit must attach catalog lifetime
to accepted graphs, provide bounded control-side prewarming, and return callback
endpoints after quiescence before lifting native streamed-admission refusal.
