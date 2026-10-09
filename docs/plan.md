# Project plan

## Goal and target

Reconstruct the selected Ableton Live workflows in an independently implemented
Rust DAW, using public documentation, experiments against the installed app,
and focused binary analysis where behavior is ambiguous. Reference version:
Live 12 Suite 12.4.6 (2026-09-10_0de5c8fa9a), macOS arm64 first.
Full Suite equivalence is a long-term program. M0 is complete and an initial
GPUI/CPAL demo slice of M2 is implemented; its full acceptance gate is still open.

Current source also includes canonical editable Arrangement, Session MIDI and
unwarped Session audio one-shots, schema-12 persistence/history, prepared mixed
playback and route-aware Session UI.
The dated work-unit sections below retain historical scope and results; the latest
finite unit and acceptance are at the end of this document and in todo.md's
current implementation focus. Recording, automation, routing/device chains,
plugin hosting, broad Suite features and measured Live parity remain incomplete.

Ableton's [Live concepts](https://www.ableton.com/en/live-manual/12/live-concepts/),
[Arrangement](https://www.ableton.com/en/manual/arrangement-view/), and
[Session](https://www.ableton.com/en/live-manual/12/session-view/) documentation
provide the initial vocabulary. Arrangement organizes clips on a linear timeline;
Session supports interactive launching. These descriptions do not establish
internal algorithms or sample-level behavior.

## Modern performance requirements — added 8 October 2026

Use modern Apple Silicon and x86-64 effectively through measured SIMD,
cache-friendly DSP and parallel independent graph paths. Treat 100 active tracks,
dense clip editing and hundreds of lightweight native device instances as routine
design workloads. External plugin capacity depends on the actual device/preset
and serial critical path; validate representative VST3 workloads rather than
promising an arbitrary device count. Keep UI feedback immediate under audio load.

Mandatory budgets and fixtures live in [performance.md](performance.md). Device
latency propagation, sample alignment, immediate prepared transport and explicit
monitoring tradeoffs live in [latency.md](latency.md). These requirements apply
throughout implementation, not as an optimization pass after feature parity.

Sequence the work within the existing milestones:

1. M2: safe compiled graph publication, active-clip scheduling, prepared playback
   and instrumentation; benchmark the 100-track audio/editing fixture early.
2. M3/M4: MIDI timing, dense UI virtualization and bounded incremental edits;
   measure undo/history and visible feedback while audio runs.
3. M5: native device latency API, serial/parallel graph timing, compensation and
   monitoring/startup contracts; then VST3 hosting after its boundary decision.
4. M5/M6: SIMD and bounded multicore execution based on bottleneck measurements;
   validate ARM64/x86-64 fallbacks, long chains and mixed-plugin stress fixtures.

Deferred dependencies: arbitrary native playback, routing/devices, plugin policy,
reference latency fixtures and representative x86-64 test hardware. Immediate
transport must not wait for compilation; exact first audible output through
arbitrary lookahead/live-input devices remains constrained by processing latency.

## Requirements for the initial setup

1. Record the actual installed reference and reproducible research entrypoint.
2. Establish a Rust workspace with an independently testable engine.
3. Implement a useful offline vertical slice: clip timing, looping, mixing,
   transport, and export.
4. Write architecture, evidence protocol, phased plan, and ordered todo.md.
5. Pass format, engine tests, strict Clippy, and inspect a generated WAV.

Deferred dependencies: a decompiler, controlled Live reference renders, real-time
backend decisions, UI choice, project format, and device/plugin contracts.
No release or source-control publication is part of this setup.

## M0 — Foundation (implemented)

Deliver engine and CLI with no external dependencies. Acceptance: deterministic
rendering across block sizes; exact clip boundaries; transport/mute/gain tests;
invalid inputs rejected; independently verified WAV header, length, and silence.
This milestone establishes Hex behavior only, not Live equivalence.

## M1 — Reference experiments and targeted decompilation

Pin reference hash; choose a macOS arm64 decompiler; keep its project private.
Create small user-owned Live sets with impulses, short tones, and known MIDI.
First experiments: unwarped one-shot timing, loop endpoints, gain scaling,
transport stop/restart, and overlapping arrangement clips. Record render settings
including warp, fades, normalization, dithering, sample rate, and device latency.

Map relevant call paths using imports, cross-references, and controlled behavior;
select one small function at a time. Translate observations into a specification
with confidence and tests before writing Rust. Do not guess semantics from names.
Acceptance: at least five reproducible experiments, one traced/decompiled function
with a documented behavioral hypothesis, and golden comparisons that distinguish
confirmed behavior from assumptions. A selected fade-controller callback has
been decompiled with unresolved research warnings; the complete five-experiment
and golden-comparison gate remains open (see todo.md and docs/research/reference.md).

## M2 — Minimal real-time audio DAW

Decide the OS boundary policy and backend. Add device output, bounded control
commands, metering, and a basic track/transport interface. Preload audio; move
allocation and destruction off the callback. Acceptance: deadline measurements,
underrun reporting, start/stop/seek under load, device disconnect/recovery, and a
manual audible session. Offline passing tests alone cannot satisfy this gate.

## M3 — Session clips and MIDI

Add clip slots/scenes, launch quantization, Session/Arrangement ownership,
MIDI note scheduling, and a small original Rust instrument. Measure ownership,
stop commands, and exact-boundary launches against the reference first.
Acceptance: deterministic launch traces across block sizes; no stuck notes;
manual recording and scene-launch workflow.

## M4 — Editing and persistence

Introduce stable project IDs, an owned versioned project format, undo/redo,
asset resolution, arrangement edits, and automation. Acceptance: round-trip and
migration tests, missing-asset recovery, undo consistency, automation timing.
Investigate `.als` separately using minimal user-authored files; preserve unknown
fields in any later importer and report unsupported features explicitly.

## M5 — Routing and devices

Add sends/returns, audio/MIDI routing, latency compensation, resampling, warp,
and selected original DSP devices. Prioritize according to measured user needs.
Plugin hosting, Max for Live, Push, Link, bundled content, and Suite instruments
are separate compatibility projects, not implied M0 dependencies.
Acceptance: graph/cycle policy, measured latency, DSP reference fixtures and
explicit numerical tolerances, stress tests, and host/listening acceptance.

## M6 — Compatibility expansion

Maintain a versioned behavior matrix. Expand Live-set import and selected device
matches only when evidence justifies them. Evaluate packaging and cross-platform
support after usable macOS workflows exist. Do not label the product a replacement
until an explicitly selected end-to-end workflow passes reference acceptance.

## Current implementation milestone — GPUI runtime demo

Requirements: native GPUI UI; Arrangement/Session demo views; real output;
transport/tempo/mute/gain; bar-quantized scenes; bounded UI/audio communication;
visible device failures and callback timings; tests, build, and UI interaction check.
This slice deliberately has fixed generated media. It is not M2 completion.

The next usable editing milestone adds a canonical editable project with stable
IDs, WAV import, compiled block playback, clip placement/trim, save/load, and
undo/redo. Recording and MIDI follow once that model owns asset lifetime.
See [features.md](features.md) for the full fundamentals and acceptance matrix.
Reference experiments and targeted decompilation remain a separate ongoing
research gate; original Hex functionality need not wait on decompiling Live.

## Active implementation sequence — 8 October 2026

The product target is feature-for-feature Live functionality, not just visual
resemblance. Work proceeds through complete workflows with explicitly recorded
unsupported features and measured compatibility gates.

The current finite work unit is the first editable-project prerequisite plus
native UI composition. Acceptance:

1. A separate `hex-project` crate owns stable typed IDs, bounded metadata history
   and transactional track/asset/clip commands; invalid edits preserve state.
2. `hex-engine` compiles a canonical snapshot with resolved PCM into the offline
   renderer, supporting trimmed source regions without copying PCM.
3. CLI demo output preserves its previous bytes; an edited demo exercises trim,
   gain, undo/redo and compilation. Source/timing/history tests pass.
4. Native GPUI uses Yield tokens and Live's compact composition with icon-sized
   transport controls, real demo tracks, mixer, inspector and collapsible browser.
   Inspect playback, scene launch, mute/gain, browser collapse and resizing.
5. Record encountered issues in `docs/issues.md`, update architecture and todo,
   pass relevant source guards, formatting, workspace tests and strict Clippy.

Deferred from this work unit: WAV decoding/import, graph publication/reclamation,
interactive clip editing, versioned persistence, MIDI, recording and Live fixtures.
The project model is the canonical editing direction; the desktop demo does not
adopt it until the playback boundary can safely own arbitrary media. This unit
is a prerequisite, not completion of M2 or M4.

Next ordered work: (1) bounded WAV importer and media bank; (2) compiled playback
handoff with explicit lifetime/backpressure; (3) track/clip editing in the UI;
(4) save/load and history integration; then recording/MIDI workflows. Continue
reference experiments alongside implementation when behavior needs confirmation.

Work-unit result: project foundation and CLI compilation implemented; 22 workspace
tests, formatting, strict Clippy, architecture/style guards and development bundle
build pass. Original CLI WAV output is unchanged byte-for-byte. Initial native
playback/scene/browser/tooltip checks passed; inspection repairs were made for
mixer clipping and narrow labels. Final rebuilt-app gain/mute/browser/minimum-size
interaction acceptance remains open because native control was repeatedly
interrupted. Next implementation starts with WAV import and media resolution.

## In progress — imported audio and prepared playback

Size: large; risk: high for the playback ownership seam, moderate for WAV import.
Work in tested units: (1) a dedicated bounded control-side media decoder plus CLI
import/render workflow; (2) owned compiled playback with bounded publication and
retirement; (3) native canonical project adoption and compact clip interaction.
Acceptance for import: exact mono/stereo PCM16/24/32 and float32 decoding, strict
malformed/unsupported/limit errors, source path preservation, no implicit
resampling, streamed CLI export and existing demo regression. Acceptance for
publication: no callback destruction/allocation, explicit backpressure and applied
revision, Stop under queue pressure, replacement/seek/partition tests. Persistence,
recording and device graphs remain subsequent units; retain the current usable
native demo while those prerequisites are verified.

## Native canonical editing slice — in progress

The headless import/persistence and prepared handoff prerequisites now pass
focused checks. Proceed to a default native canonical editor, retaining the older
fixed demo as --demo. A dedicated hex-session control worker owns project state;
GPUI sends bounded commands and renders read-only updates. Import/decode, history,
compilation and save stay off the UI/audio threads. Native acceptance: import a
rate-matching WAV, see actual waveform/clip, play/stop, move/trim/delete with
undo/redo, gain/mute, add/remove tracks and create a complete new project file.
Graph revision/status must distinguish edited/pending/applied audio. Preserve
compact lowercase/Ioskeley styling and no unsupported placeholder controls.

Deferred: asset relinking, resampling, drag editing, MIDI/recording and general
Session/device graphs. The lifecycle
worker retains/retries the one stream when pause fails; hardware failure recovery
and sustained CPU/UI gates are still acceptance work, not claimed capabilities.

## Native integration evidence and next unit

Workspace tests, strict Clippy, policy/style guards, formatting and the development
bundle passed for the first canonical editor. Native Cmd-I import, quarter-second
nudge/trim, Play/Stop and Cmd-S save-new were exercised on a 44.1 kHz original
fixture. The saved schema contains start 11025 and duration/source length 165375,
matching those edits. Initial blank launch capture was transient; subsequent
rendering exposed a real virtualized-row width collapse hiding clips. Explicit
list/row width constraints are being rebuilt and visually checked. Audible and
sustained timing acceptance remain separate.

Next finite unit: native Cmd-O project opening. Load/resolve/validate/compile a fresh
session on the worker before replacing existing state; reject missing or mismatched
media without changing the current project/playback. Guard intentional discard and
queued edits across project replacement. Test relative paths, rate/frame/budget
failures, preserved IDs and empty loaded history. Existing-file saving is now
implemented through a separate storage/session workflow; native acceptance remains
distinct from these project-open checks. Asset relinking remains deferred.

Project-open unit result: successful candidates preserve IDs, resolve media beside
the document, use empty history and compile before commit. Missing/mismatched or
over-budget assets preserve the existing session. Save-as converts relative
references in a derived snapshot against the original document directory; tests
include unreferenced assets and saving/reopening from another directory. Opening
submission failures no longer leave editing permanently paused. Successful native
replacement stops old transport and clears pending playback intent/selection.
Workspace tests, strict Clippy, guards, formatting and bundle pass. Native dialog
opened, but selection/navigation was interrupted twice, so final opened-waveform
and discard interaction acceptance remain pending.

Next finite unit: duplicate selected audio clip through a validated canonical
command and Cmd-D, with stable fresh IDs, unchanged source references, one-step
undo/redo, overflow/limit refusal and renderer equivalence. Recording/MIDI,
asset relinking and resampling remain subsequent workflows.

Native-save verification: CLI rendering of artifacts/native-editor-check.hex
produced 176400 frames at 44.1 kHz, an exact silent 11025-frame lead and 165375
source frames matching the original PCM16 samples with zero observed LSB
difference. This checks persistence-to-render continuity, not audible output.

Duplicate unit result: validated canonical commands preserve source regions and
share media, returning fresh IDs for one-step undo/redo. Overflow/clip-limit
failures preserve project/history. Native Cmd-D selection uses returned IDs;
bounded repeat intent serializes rapid presses into contiguous copies, cancels
on explicit selection/open/delete/failure and reports admission refusal. Project,
service render and pure UI intent tests pass; rebuilt native interaction remains
pending.

Next reliability unit: track unsaved state and protect ordinary native close/Cmd-Q
from silently discarding edits, including pending imports/edits/saves. Check GPUI's
close interception separately from non-cancellable OS quit/forced termination;
do not claim full crash recovery without a recovery journal.

## Ordinary close protection — in progress

Size medium, risk moderate. Requirements: read-only unsaved state from the worker;
model changes mark dirty, successful save/open mark clean, failed operations
preserve dirty state. Undo may conservatively remain dirty even when returning
to saved content. UI close decisions also include accepted/local pending requests
and duplicate repeat intent. Cmd-Q and native window close share a coalesced
asynchronous lowercase cancel/discard prompt, defaulting to cancel. Freeze edit
admission during the prompt, stop output before confirmed discard, and avoid any
UI-thread join. Tests cover dirty transitions and pending/confirmation decisions.

Deferred: recovery journal, automatic restore and forced/OS quit protection.
Pinned GPUI on_app_quit cannot cancel shutdown and has a 100 ms
completion window; ordinary close guards do not establish crash recovery.

Close-unit result: dirty state and ordinary close/Cmd-Q cancellation/discard are
implemented. Already-accepted duplicate continuations bypass only the new-edit
close-prompt admission gate, so canceling close does not lose accepted presses.
Opening still blocks continuations; queue overflow stays explicit. Integrated
tests, strict Clippy, guards and formatting pass. Native/CLI default resident
ceilings now match at 8,000,000 stereo frames. Rebuilt native close acceptance
and OS/forced termination recovery remain open.

## One-shot clip splitting — in progress

Split selected non-looping audio at the playback cursor with Cmd-E. Preserve
the original left ID and create a fresh right ID in one history transaction.
Partition source range/duration exactly, preserve the arrangement end, share
PCM, and insert the right segment adjacent to the left to retain floating-point
summation order with overlapping clips. Reject boundaries, exhausted sources,
missing IDs, overflow, clip caps and looping clips atomically. Verify block-size
render equivalence, trimmed sources, history and save/load. UI selects returned
IDs and reports cursor-outside errors; no hidden midpoint fallback.

Loop phase splitting needs a distinct contract and remains deferred rather than
changing the repeated source interval or silently guessing its phase.

Split-unit result: one-shot source-preserving split and Cmd-E are implemented.
Exact rendered equivalence includes trimmed sources and overlapping clips over
block sizes 1, 3, 7 and 16. Invalid bounds/loops/capacity/ID exhaustion preserve
state; history and persistence retain both IDs and source intervals. Workspace
tests, strict Clippy, guards, formatting and desktop build pass. Native split
acceptance remains separate and pending.

## Source-rate conversion and schema 2 — in progress

Size large, risk high across media, project metadata, storage and control-worker
loading. Convert imported WAVs to the project rate before publication; waveform
summaries and clip source intervals use project frames. Preserve original source
rate/count separately from prepared playback length. Schema 2 records converter
version (identity 0, windowed-sinc 1); strict schema-1 reads migrate to identity
metadata. Reject unknown versions and changed source shape without replacing the
current project. Identity conversion shares PCM without changing sample bits.

Acceptance: deterministic checked duration rounding (nearest, positive half-up,
minimum one frame), bounded source/output allocations, anti-alias numerical tests,
metadata/history/strict persistence round trips, mixed-rate import/save/reopen/render
continuity and failure atomicity. Decode at most 64 MiB / 8,000,000 source frames
per file; prepared resident PCM is separately capped at 8,000,000 frames. Prepare
one asset at a time on the control worker, never UI/audio threads.

Deferred: project-rate/device-rate conversion, device configuration, streaming
large assets, source-content hashing, warp/tempo conversion and Live resampler
parity. The v1 filter's taper and zero-padded boundary treatment are independent
Hex choices, documented in hex-media; they can alter boundary transients. A saved
converter version makes future algorithm changes an explicit migration decision.

Rate-conversion result: source/playback metadata, strict schema 2/schema-1 migration,
background preparation and CLI rendering are implemented. All 104 workspace tests,
strict Clippy, style/architecture guards, formatting and desktop bundle pass.
Native schema-1 opening shows waveforms in the repaired full-width layout;
48 kHz source import into a 44.1 kHz project, repeated contiguous duplicates,
close cancellation and create-new schema-2 saving passed. The saved native
fixture is artifacts/native-mixed-rate-check.hex; headless export succeeded.
Numerical filter tests establish selected signal/boundary behavior, not Live
resampler equivalence or audible/device stress acceptance. Parallel tests exposed
a timestamp-only temporary-directory collision; deterministic per-process
sequence names repaired the harness.

## Native timeline seek — in progress

Size medium, risk moderate. Click the ruler using its actual rendered timeline bounds to seek without changing
project metadata/history. Exclude the track-header area, validate geometry, clamp
to the visible horizon and deliver through the bounded playback queue. Preserve
playing/paused state and priority Stop. Refuse seek during close/open admission
pauses, and prevent Cmd-E from reading a stale pre-seek callback position. Keep
feedback compact and report queue refusal. Validate mapping boundaries and native
click/transport/split behavior. Drag scrubbing, zoom and snap grids remain separate.

Seek result: actual ruler bounds, checked pointer mapping, bounded ticketed seek,
priority Stop and cursor-dependent split admission are implemented. All 110
workspace tests, strict Clippy, guards, formatting and the bundle pass. Acceptance
review corrected a GPUI mouse listener handling both capture and bubble phases;
the action now runs only on bubble. Saved schema-2 mixed-rate native reopening
passed. Native seek input was interrupted by app-state refresh guards, so click,
post-seek split and confirmed discard remain pending, not inferred from unit tests.

## Timeline viewport and cropped waveforms — in progress

Size medium, risk moderate. Add view-only zoom-in/out, horizontal pan and fit with
checked project-frame bounds; reset on project replacement. Anchor zoom to an
in-view cursor, otherwise the midpoint. Keep actual-bound ruler seeking, clip
positions and the playhead on the same viewport. Cull offscreen clips and preserve
compact rows/tooltips. No changes to dirty state, project metadata or callback DSP.

A cropped clip must show its visible source interval, not stretch its whole source
across the cropped rectangle. One-shot exhausted tails stay silent; loop waveform
columns honor phase/wrap. Aggregate cached peaks into at most 48 columns; document
that cached extrema do not provide sample-level detail at extreme zoom. Tests cover
viewport bounds/overflow/anchors and visible source mapping. Deferred: drag scrubbing,
scrollbar/trackpad gestures, musical snapping, editable loop region and detailed
sample rendering. Native zoom/seek/split acceptance follows the rebuilt bundle.

## Straight-track solo — in progress

Size medium, risk moderate. While viewport work stays in the UI, add canonical
exclusive/additive solo for the existing straight Arrangement tracks. Toggles
resolve on canonical state: exclusive enabling clears other solos in the same
history transaction; additive enabling preserves them; disabling affects only
the selected track. Mute remains effective on a soloed track as an explicit Hex
edge policy. The compiler precomputes effective muted tracks without callback
allocation or extra per-frame solo traversal. Default output remains unchanged.

Persist solo in schema 3; strict schema-1/2 migrations set solo false and preserve
all source conversion metadata, IDs and paths. Test repeated toggles, invalid
IDs, history, removal/restoration, rendered mix/partition behavior and strict
persistence. Native UI adds compact solo controls and selected-track keyboard
access after viewport changes finish; no overlapping UI writers. Reorganize
44-pixel track headers into name/control rows if needed to preserve names.

Reference documentation and unmeasured edges are in
research/observations/mixer-solo-001.md. Groups/returns/routing/cueing, pan law,
master gain, solo preference persistence and native Live mute precedence remain
deferred. Do not describe straight-track solo tests as full mixer parity.

Viewport/solo integration result: all 124 workspace tests, strict Clippy, guards,
formatting and development bundle pass. Native opening, Up/Down track selection,
exclusive S, additive Shift-S and confirmed discard of only unsaved test toggles
passed. Native review found track names collapsed to an ellipsis and viewport
actions without a visible effect; these are being resolved before visual acceptance.
An old process had remained open across the rebuild; quitting and observing its
new process identity resolved the stale-build confusion. No audible or Live solo
parity acceptance is inferred from these checks.

## Canonical Arrangement loop region — next implementation unit

Size large, risk high: validated metadata/history, persistence, compiled playback
and native UI cross existing subsystem boundaries. Add an optional half-open
project-frame region [start,end), with enabled state. Require start < end, permit
one-frame loops, validate all restored documents. Set/toggle/clear are canonical
atomic edits and undoable; legacy projects migrate to no region. Save as schema 4
with strict version-specific migration from 1/2/3; no overwrite-save expansion.

Prepared Playback honors the compiled region. Offline Renderer/CLI export remains
a linear arrangement export (region metadata is not an instruction for infinite
export). Play before loop start renders the intro; reaching end wraps exactly to
start. Play/seek at or beyond end normalizes into the region at the next playing
render; Pause preserves position and emits silence, Stop returns to zero. Region
changes arrive with graph replacement and inherit playing/position before applying
new bounds. No callback allocation, ownership drop, waiting or graph compilation.

Bound wrap processing by output length; tests compare frames/position across
partitions including empty output, tiny loops, huge seek and edited region changes.
A one-frame loop can increase span-dispatch cost: measure before any deadline claim.
No stateful device/MIDI reset or crossfade is implemented; document these deferred
contracts rather than pretending source-loop repetition establishes device parity.

Native controls: Cmd-L sets/enables the region from selected clip bounds; L toggles
an existing region with an explicit no-region error. Compact loop indicator and
ruler band reflect canonical state and existing pending audio revision. Region
editing does not change clip source-loop flags. Deferred: drag handles, musical
snap grids, tempo map, fades, external devices and Live-specific loop parity.
Acceptance: targeted canonical/history/storage/render continuity tests; integrated
checks; native set/toggle/undo/save/open and visible wrap when feasible.

Native follow-up: explicit header width restored readable names at 44 px rows.
Direct viewport button listeners and unique IDs repaired button zoom; Alt-Right
pan, Cmd-0 fit, ruler seek, Cmd-E split and schema-3 saving passed. Cmd-equal
zoom shortcut remains pending. Independent saved-render comparison showed exact
PCM16 continuity across the split and solo gating (507150 stereo frames, 44100 Hz).

Loop result: canonical/history/schema-4/compiled/native integration passes all 134
workspace tests, strict Clippy, guards, formatting and the development bundle.
Native set/toggle/undo, visible wrap, save/reopen and Command-Plus/Minus zoom passed.
Saved region is [11025,88127), enabled, in artifacts/native-loop-check.hex. Linear
export matches the prior saved split/solo render byte-for-byte. A measured one-frame
span dispatch regression was repaired by exact stateless period copies inside the
caller output buffer; methodology and before/after results are in performance.md.
Stateful device/MIDI wrap and audible acceptance remain deferred.

## Native offline export — completed 8 October 2026

Shared hex-export now owns linear stereo PCM16 WAV rendering and create-new
publication for CLI and native export. It renders fixed 256-frame blocks, rejects
non-finite output and RIFF overflow, writes an exclusive same-directory temporary,
synchronizes it, and publishes without replacing an existing path. Empty ranges
remain supported by the shared CLI contract; native export refuses empty projects.

The native worker accepts one reserved export at a time. It captures an immutable
compiled project Arc and revision only when current, then renders from frame zero
through the latest clip end. Global loop metadata does not repeat. Completion is
separate from project view/graph events, so a late result cannot replace newer
state or alter dirty/transport status. Closing with an export pending warns that it
may not finish; the UI does not join the worker. Progress, cancellation, overwrite,
stems, selected ranges, float WAV, normalization/dither, plugin tails and streaming
remain deferred.

Acceptance: renderer/publication tests cover headers, samples, ranges, finite and
RIFF validation, collisions, failed-render preservation, empty CLI output and long
filenames. Session tests cover snapshot stability during later edits, busy admission,
empty-project refusal and worker disposal. The fresh native bundle exported revision
1 to artifacts/native-export-check.wav: 507,150 frames, stereo 44.1 kHz PCM16. The
path-dialog cancel/reopen also passed; no export reservation or dialog-open flag
remained stuck. It matched artifacts/native-loop-linear-render.wav byte-for-byte (SHA-256
952c8d29e303b10942d47300381b87b074d75828077e1b759f744b29a2bba681). No listening
or sustained export performance acceptance was performed. The last full workspace
run passed 145 tests; one later export test and two save-durability tests passed in
focused checks (148 tests across the full and targeted runs). Strict Clippy, style
and architecture guards, formatting, and the development bundle passed.

## Create-new project save durability reporting — completed 8 October 2026

Size small, risk moderate. hex-storage now distinguishes failure before
publication from cleanup or parent-directory synchronization failure after the
create-new link commits. PublishedButNotDurable includes destination path and source
error; the complete target remains loadable. Session preserves unsaved state and
reports the published path rather than implying no file exists. Existing paths,
including dangling symlinks, are refused before writing; hard-link publication is
the authoritative race-safe no-clobber step.

A deterministic post-publication failure test verifies the complete target remains
loadable and the returned error identifies its path/operation. Collision, round-trip
and cleanup cases pass. Targeted storage/session/CLI checks (36 tests) and strict
Clippy passed; the new failure message was not exercised in the native app. Existing
file overwrite, recovery journals and permission changes are out of scope.

## Exact document-content tokens — completed 8 October 2026

Size small, risk moderate. Opaque shared FileToken retains at most 8 MiB from the
same bounded read that load_with_token validates. check_unchanged performs no
writes and reports Conflict with the selected path when exact bytes differ.
Legacy schema/formatting stays in the token; byte-identical file replacement is
accepted and even formatting-only edits conflict. Debug output shows only length.
Three tests cover original versus migrated bytes, equivalent JSON formatting,
byte-identical replacement, clones, invalid JSON and bounded oversized reads.
Targeted storage/session/CLI checks pass 39 tests and strict Clippy; guards and
formatting pass. Together with the prior full run and targeted additions, 151
tests have passed. This source-only API has no new native interaction.

Next: settle persistent copied-backup ownership and finite retention, then atomic
replacement and session-owned document association. These tokens are content
checks, not locks or overwrite permission; an external noncooperating writer can
still race a later publication. This historical token milestone preceded the
completed replacement/session work below.

## Recoverable existing-document replacement — current storage unit

Size medium, risk high. Extend hex-storage only; retain existing native behavior
until the storage contract passes targeted checks. Require an exact FileToken,
refuse direct target symlinks and changed content, prepare/sync new document and
copied old bytes, recheck immediately before publication, then atomically rename
on supported macOS. Retain one rolling same-directory recovery sidecar with a
strict versioned envelope, target basename binding and a bounded exact prior
JSON payload. Only a validated recognized sidecar can roll forward; preserve
unknown files, symlinks and malformed sidecars and refuse the save. Provide a
validated recovery reader. No unrelated cleanup or unbounded backup generations.

Before project commit, failure preserves the old target; after commit, structured
outcome includes the new content token, destination/recovery path and durability
warning so a later actor can adopt the published version without false conflicts.
A copied backup cannot be altered through an old hard-link alias. Recheck narrows
but does not close the race with noncooperating writers; do not claim filesystem
CAS. Preserve lexical path casing; target hard-link aliases retain the prior inode.
Basic permissions and nonportable ACL/xattr behavior need an explicit supported
policy; no changes to unrelated files or machine permissions are authorized.

Acceptance: exact replacement/recovery roundtrip, repeated one-generation rolls,
external-edit conflict, malformed/oversized inputs, unknown sidecar preservation,
symlink refusal, injected pre/postcommit failures, bounded allocation and targeted
storage/dependent checks. Storage and session association are implemented; native
Save Current/Save As interaction remains a separate acceptance gate.

## Native dense Arrangement inspection — parallel acceptance unit

Open an original generated schema-4 fixture with 100 tracks and 10,000 clips
sharing one existing test WAV. Keep every track muted and transport stopped.
Inspect bounded row scrolling/selection, zoom/pan, clip edits and undo on the
actual app; save a separate test document if needed. This tests native composition
and control behavior, not audible/device deadlines or input-latency percentiles.
No plugin/device workload or full 100-track performance gate is claimed.

Replacement storage result: the API and validated recovery reader pass eight
new replacement tests plus a later ninth basic-permission regression test.
Targeted storage/session/CLI passed 47 tests before the permission addition;
all nine replacement tests and storage Clippy passed after the repair. The copied
recovery and replacement both inherit the original basic permissions; changes to
those bits during staging refuse publication. ACL/xattr fidelity and optimistic
concurrent-writer races remain explicit limitations. Native Save Current and
session association were completed later; fresh Save/Save As interaction is still
unaccepted. Dense native inspection revealed a separate drawing regression;
prioritize its repair before enabling more editor workflows. See issues.md for
sampled evidence.

Final replacement review also moved inherited permission application before the
first staging write and creates Unix temps initially at 0600. All nine replacement
tests, storage Clippy and formatting pass after this repair. Current evidence totals
160 tests across the prior full workspace run and later targeted additions. No
native Save Current interaction was accepted by this storage-only milestone.

## Conditional editor refresh — current regression repair

Keep 16 ms bounded queue/worker/audio polling, but invalidate GPUI only when
visible state changes. Preserve immediate input notifications, request/export
completion, graph/seek acknowledgment, faults and playing transport animation.
Refresh idle diagnostics at a bounded slower cadence rather than painting every
callback. No project clones/string allocations on idle ticks. Test the pure refresh
policy, rebuild, relaunch, and repeat dense native inspection/idle CPU observation.
This is an idle-redraw fix first; interactive/playing cost remains to be measured.

Conditional refresh source result: 165 integrated workspace tests and strict
workspace Clippy pass, with formatting/style/architecture guards and native bundle.
Paused/stopped cursor changes and seek acknowledgment are covered alongside idle,
playback and diagnostic refresh policy. Fresh native idle process was around 1.4%
CPU with an empty project, not the dense fixture; UI input was interrupted during
active app use, so dense-native acceptance remains open and the app was preserved.

## Session-owned saving — backend complete

Size medium, risk high. Associate an opened or successfully saved project with its
lexical absolute path and exact file token on the canonical worker. Add SaveCurrent
with explicit untitled refusal; retain create-new SaveNew as Save As. Obtain tokens
from the bytes actually published, not an uncontrolled reread. Open association
changes only after media validation and compilation succeed. Save Current uses
the validated rolling-recovery replacement; adopt its new token after a committed
write even if a durability warning keeps the project conservatively dirty.
Failures/conflicts preserve association, revision and compiled playback. Saves
stay on the bounded actor and apply to its revision at processing time; later
queued edits mark dirty again. Preserve original relative-media resolution when
saving to a different directory without silently changing the live canonical model.

Acceptance: opened/untitled/new association, ordinary repeated saves, conflict
preservation, Save As collision and failure, later queued edits, relative media
across moved save paths and failed Open preservation. These are covered by
session/storage tests; UI/native acceptance is tracked separately below.

Session saving source result: targeted storage/session/CLI passed 50 tests and
Clippy; workspace check/fmt/guards pass. Open/Save As association and existing-file
saving are implemented with two new backend workflow tests and expanded existing
media/collision cases. A focused postcommit warning-adoption regression and
explicit save completion events were added before UI wiring.

## Native Save / Save As — source complete; acceptance pending

Size medium, risk moderate. Cmd-S saves the associated document through the actor,
or opens create-new Save As while untitled. Cmd-Shift-S always selects a new path.
One native save-path dialog and one pending save per view; explicit worker save
completion clears admission state even on errors. Do not drop rapid edits or
infer save completion from generic status strings. Keep compact existing icon,
lowercase hints/errors and bounded paths. Conflict/failure retains unsaved state;
UI does not perform file replacement or own the authoritative file token.
Native acceptance uses only original disposable project fixtures, with separate
copies for replacement/recovery checks. Active user app use may defer native
interaction without blocking independent source work.

Backend completion follow-up: Event.save_complete explicitly acknowledges every
processed save request, including refusal/conflict, and remains false for ordinary
edits. A focused committed-warning test proves the actor adopts the published token,
keeps dirty true, identifies target/recovery paths, and can subsequently save without
a false external-change conflict. All 26 session tests, session Clippy and formatting
pass. Native Cmd-S/Cmd-Shift-S routing, create-new dialog, bounded path validation,
single-dialog/job admission, completion clearing, status hint and UI README are
implemented in source. The integrated workspace run logged 171 passing tests;
strict Clippy, workspace check, formatting, guards and local bundle passed. Fresh
native Save/Save As dialog interaction remains open because active app input was
interrupted.

## Native theme palette alignment — next bounded UI task

Size small, risk low. Align `hex-ui` theme tokens with accepted Frame iteration 3:
warm charcoal, off-white, orange, cyan, and muted grey clips/waveforms (`#c0c4bb`).
Preserve the compact layout and keep every control mapped to existing
functionality. Leave violet and blue as design-specification roles until an
implemented control needs them; do not add unused runtime colors. Build and run
UI style/architecture guards; rendered native inspection remains a separate gate.

## Explicit CLI recovery — current user recovery unit

Size small, risk moderate. `recover-project <target.hex> <new-project.hex>` validates
that target's retained recovery sidecar and writes the prior canonical project to
an absent new path. Resolve any relative prior media paths against the original
target directory before writing elsewhere. Preserve current target and backup,
refuse unknown/malformed sidecars and destination collisions, and do not decode
or invent missing media. Test the actual CLI in a separate working directory,
including missing target with retained recovery, case preservation, no-clobber
and malformed input. This is explicit prior-version recovery, not a crash journal.

CLI recovery result: two actual-binary regression tests pass, including a different
working directory, retained backup after target deletion, missing media and mixed
case paths, destination collision, and malformed sidecar preservation. All 11 CLI
tests, strict CLI Clippy, formatting and architecture/style guards pass. Recovery
is explicit to a new file; no crash journal or native recovery dialog is claimed.

Theme alignment source result: warm charcoal surfaces, dark separators, grey clips,
off-white waveform/text and orange/cyan accents are applied through shared tokens.
Strict UI/CLI Clippy, formatting/guards and the development bundle pass. Existing
rows/layout and functional controls are preserved; violet/blue remain design tokens
until real controls need them. The current running instance is an older release
process, so fresh bundle visual acceptance remains open.

## Canonical stereo balance and master gain — current backend milestone

Size large, risk high; first finite unit covers canonical state, validated commands,
undo/redo, shared renderer and strict schema-5 persistence with neutral migration
from schemas 1–4. Track balance [-1,1] uses the explicitly original attenuating
linear law in research/observations/mixer-balance-001.md; master gain [0,2] multiplies
the summed stereo output. Default center/unity must preserve prior render bytes.
Tests cover asymmetric stereo endpoints/intermediate balance, master-after-sum,
mute/solo and block/loop compatibility, invalid inputs, history and strict migrations.
No claim of measured Live pan law, split stereo pan, smoothing or routing/device
processing. Compact native controls follow the tested backend contracts, using one
26 px mixer strip while preserving existing 44 px track rows. Reserve native visual,
workflow and audible acceptance separately.

User-requested titlebar repair: the transparent titlebar's native controls have an
80-logical-pixel leading reservation shared by the editor and demo. Strict UI Clippy,
formatting/style checks and the rebuilt bundle pass. No custom duplicate traffic
lights were introduced. Native refresh was interrupted twice after rebinding; leave
fresh rendered acceptance open and preserve the user's running release process.

Mixer result: 178 integrated workspace tests, strict workspace Clippy,
formatting/architecture/style guards and the rebuilt bundle pass. A subsequent
actual-CLI non-neutral balance/master render test also passes (12 CLI tests;
179 total tested across these runs). Control-side prepared channel coefficients
and unity-master bypass avoid unnecessary neutral per-frame work; paired benchmark
results and limitations are recorded in performance.md. The prior schema-4 native
loop render remains byte-identical at centered/unity defaults. The current native
release process has not accepted the new mixer, palette or titlebar changes;
repeated control interruption leaves these visual/workflow checks open. No listening
or Live numeric pan-law parity is claimed.

## Default output recovery — source implementation complete, native acceptance open

The bounded backend reopen service and canonical `RestorePlayback` path are
implemented. Recovery validates the default output against the project rate before
stream construction, holds one open reservation through result consumption or late
retirement, and never constructs CPAL streams on GPUI or the callback. The session
restores the current compiled graph without changing project/history/dirty/path
state. The native state machine uses generation IDs, clears old counters and intents,
stays stopped, waits for the latest graph and a newly consumed cursor seek, and
allows Stop to replace the pending cursor with zero. Editing/saving remain available;
Play/seek/split are gated while pending. Device selection, automatic reconnection,
recording and rate conversion remain deferred.

Recovery result: targeted lifecycle/controller coverage and the integrated workspace
run of 187 tests pass; strict workspace Clippy, formatting, architecture/UI guards
and a rebuilt bundle also pass. The physical device failure/restart, rate-mismatch
interaction, disconnect and listening paths were not tested in the native app; the
older release instance remained active after user-control interruption. At this recovery milestone, initial startup was still synchronous; the following
bootstrap unit removes that limitation while preserving actual-device rate binding.

## Asynchronous canonical startup — current bootstrap unit

Size medium, risk moderate. Present the native loading window immediately with
shared font/theme/titlebar rules, while an injected bounded initial-output handle
opens the default device on the lifecycle worker without forcing a project rate.
After completion, create the canonical service at the actual device rate; only a
failed output open retains the existing 48 kHz offline fallback. Replace the
bootstrap root through pinned GPUI window APIs outside Render, attach close
protection to the new EditorView entity, and preserve one last-window quit observer.
A weak bootstrap entity and non-owning window handle must not keep a closed window
or pending output alive; canceled/late outputs retire through existing RAII.
No blocking receive, new UI framework/crate, implicit device-rate adoption for
existing projects, or demo startup redesign. Tests verify unconstrained worker
requests and rate-selection/failure policy; compile and lifecycle checks verify
handoff. Native startup timing, close-during-open and actual-device behavior remain
separate acceptance, subject to the current native control limitation.

Bootstrap result: 189 workspace tests pass, alongside strict workspace Clippy,
formatting, architecture/UI guards and the rebuilt development bundle. A newly
launched native app reached an empty canonical editor at 44,100 Hz; the top bar
starts after the 80 px native-control reservation and the compact main mixer is
visible. This does not measure startup latency or exercise close during opening.
The OS screen-sharing pill occupies the native-control area in the captured view;
normal traffic-light interaction/fullscreen sizing remain open acceptance.

Native follow-up: repaired toolbar action dispatch to use its owning Window rather
than the globally active window. Targeted strict UI Clippy, formatting and rebuilt
bundle pass. Add-track, balance r10, main 110%, undo back to main unity and stopped
default-output restart were exercised in a one-track empty-audio project, with
canonical revisions and unsaved state preserved as appropriate. Physical failure
and listening after restart remain deferred.

## Canonical Arrangement clip dragging — current work unit

Size medium, risk high for native input/lifetime behavior. Reuse canonical MoveClip
for pointer movement within/across audio tracks, preserve grab offset when the clip
is cropped by the viewport, clamp before frame zero and reject overflow. No PCM
copies, temporary canonical edits during pointer motion or second project model.
One accepted drop produces one transactional history edit; no-op, cancel and
rejected drops do not create edits. Pending open/close disables drag admission;
resolve IDs against the current project and refuse stale payloads after replacement
or conflicting edits. Give compact preview/target feedback with existing tokens.
Acceptance: numerical boundary tests, history/cancellation behavior, targeted UI
checks, workspace integration and actual drag/undo where native control permits.
Snap grid, trim handles and automatic timeline/track scrolling are later extensions
of the same interaction, not claimed implemented by this unit.

Dense native follow-up: read-only tracing confirms vertical virtualization and
that the 1%-width fixture clips skip waveform bars. Per-visible-track clip scans
and element construction repeat on redraw and measurement; clip vectors are not
time-sorted, so binary-search culling would be invalid. The reported high CPU still
requires a confirmed loaded-fixture profile; native control was interrupted again.
Do not label an unmeasured source hypothesis as a performance fix.

Research track result: Ghidra 12.1.4/OpenJDK 21.0.12.1 are installed through
Homebrew. The unchanged Live arm64 slice was imported into an ignored private
project without whole-program analysis. FADE-CONTROLLER-001 records one selected
callback branch and target decompile, addresses, output hashes and unresolved
warnings. This removes the missing-decompiler prerequisite, not the measured
reference-fixture or behavior-parity gates. The original Java research helper is
separate from the Rust application runtime.

Clip-drag result: metadata-only native payload, cached lane geometry and one typed
MoveClip request on valid drop are implemented. Revision/generation/viewport and
quiescent-worker checks reject stale or pending work; valid no-ops do nothing.
Selection reconciles with canonical clip ownership through move/undo/redo, and
the geometry cache is pruned to current tracks. Escape cancels first through its
own action; explicit Stop cancels dragging and always stops audio. 196 workspace
tests, strict workspace Clippy, formatting, architecture/UI guards and rebuilt
bundle pass. Fresh native drag/undo/cancel/resize checks remain open after native
control interruption; no dense-project responsiveness or Live drag parity is
claimed from these tests.

## Canonical clip fades — current work unit

Size large, risk high across editing, persistence and rendering. Add an explicit
frame-domain linear envelope with span, offset, fade-in and fade-out lengths.
Splits retain the original domain and advance the right offset so recursive
splits preserve every sample's gain. Moves and duplicates retain the envelope;
setting fades reanchors to the current duration. Trimming deliberately reanchors
and clamps both lengths, and can reshape an inherited partial fade. Zero lengths
mean unity; lengths of two or more include both ramp endpoint samples. One-frame
fade-in is unity and one-frame fade-out is zero. Overlap multiplies the ramps.
Validate checked domain bounds and allow overlapping ramps. This is an original
Hex policy, not a confirmed Live curve.

Acceptance: transactional edits and undo/redo; strict schema-6 persistence with
neutral migration from schemas 1–5; allocation-free prepared rendering with a
neutral bypass; boundary, overlap, split, block partition, seek and loop tests;
unchanged legacy neutral export; before/after release profiling. Then expose
compact selected-clip controls and a clipped envelope display through the same
commands. Arbitrary curves, automatic crossfades and measured Live fade parity
remain deferred. Native acceptance uses a separate development instance when
possible: the user's main instance now contains unsaved edits and must remain
untouched.

Fade result: 212 workspace tests, strict workspace Clippy, formatting and source
guards, desktop build and rebuilt development bundle pass. Saved CLI fixtures
cover numerical PCM endpoints, overlap/reset and recursively split exports with
undo/redo. The legacy 507,150-frame 44.1 kHz export remains byte-identical.
Prepared neutral clips avoid envelope allocations and sample-loop branching;
paired CPU profiles and remaining differences are in `performance.md`.

Native acceptance subsequently ran after the main app had independently closed.
Only the isolated bundle loaded the original muted fixture: inherited 100 ms
readouts and the clipped envelope were visible; increasing fade-in gave 105 ms
and reanchored the domain. Save wrote schema 6 with span 22,050, offset zero,
fade-in 4,631 and fade-out 4,410 frames. Undo restored the inherited shape/marker,
redo restored the edited envelope, and a second Save cleared the conservative
dirty state before clean quit. No audio was auditioned. Minimum-window sizing,
dense interaction, device stress and Live curve matching stay open.

Next implementation focus: extend canonical Arrangement editing with pointer
trim handles, retaining validated source bounds, one transactional history edit
per accepted gesture and the documented fade trim policy. Musical snap grids
depend on a canonical tempo/timebase decision; do not infer tempo from the demo.

## Pointer trim and loop source phase — current work unit

Size large, risk high across model, persistence, rendering and native gestures.
Add source phase in project-rate frames relative to the source interval: loops
require phase < source length; one-shots require zero. Schema 7 explicitly
migrates schemas 1–6 to zero phase. Loop lookup and waveform mapping apply the
same modular phase. Loop splits retain the source interval and advance the right
phase, preserving both PCM and the fixed fade envelope without copying audio.

One typed TrimClipEdge command owns bounds and clamping. Left uses an inclusive
start, right an exclusive end. One-shot trims shift timeline/source bounds
together, retaining their existing silent-tail difference and at least one source
and timeline frame. Extensions stop at source/timeline bounds. Loop left trims
advance or retreat phase modulo the interval; loop right trims change duration
only. Actual trims reanchor/clamp fades; unchanged boundaries are true no-ops.
The metadata-only preview must use these model rules without cloning a whole
project or copying PCM. Each accepted native release creates one undo step;
stale, canceled, busy or off-track gestures create none. Handles are compact,
visible only at real edges, with clear resize feedback and no added panel.

Acceptance: checked endpoint/modular arithmetic, one-shot silent tails, loop
wrap/extension, recursive split PCM continuity, fade trim/history, strict schema
roundtrip/migration, waveform phase alignment and neutral export/profile checks;
native trim/undo/cancel and existing move regression. Snap grid and autoscroll
remain deferred; this phase prerequisite does not implement recording, MIDI or
general Session/device workflows.

Implemented and integrated: source phase, metadata-only edge preview, canonical
trims, loop splits, strict schema 7, phase-aware rendering/waveforms and compact
handles. 227 workspace tests, strict Clippy, guards/fmt and rebuilt bundle pass.
The saved original three-sample fixture checks independent PCM through trims,
extensions, history and recursive splits. An additional engine regression checks
split phase/fade continuity through project loops, seeks and render partitions.
The previous legacy export remains byte-identical; profiles and remaining metadata
layout costs are recorded in `performance.md`. Contract: `clip-trim.md`.

Native testing caught and repaired threshold-crossing pointer capture: use the
initial MouseDown retained in one view-owned generation/clip/edge-keyed slot,
not the later GPUI on_drag offset. Separate bundle/fixture checks confirmed full
right-edge displacement, left-edge phase advancement, Save and undo/redo. The
saved left trim has start 33,666, duration 66,233 and phase 11,616; its right edge
and source interval remain fixed. A cross-track move retains clip ID 3, phase,
source interval and fades, commits one edit and follows selection through history.
Off-track trim release leaves revision 7 unchanged and creates no undo step.
The isolated test was saved and quit; the user-modified acceptance app was left
untouched. No listening or device-stress acceptance is implied.

Next: investigate the recurring neutral render median increase with a focused
compiled-metadata layout profile, and complete cropped/resize/Escape gesture
acceptance. Continue toward the canonical musical timebase needed for snapping,
tempo and MIDI after this editing/performance gate. Recording, streaming, routing
and device chains remain required fundamental workflows on the larger roadmap.

## Compiled clip metadata layout — current work unit

Size medium, risk moderate. Preserve the current public clip builder/compiler
contracts, canonical schema and exact neutral/phase/fade PCM. Compare the current
inline-phase release executable with a compact clip whose optional control-owned
metadata contains source phase and non-neutral fade. Resolve metadata once per
active span; neutral clips must not allocate metadata or look it up per sample.
All allocation/reclamation remains outside render. Check phase reset and fade
retention on builders, split/seek/loop partition tests, saved CLI continuity,
strict checks and paired neutral/phase/fade profiles. Record measured stride and
timings; retain the change only if evidence supports this layout direction.

Completed source integration: 72-byte compiled clips on macOS arm64, optional
phase/fade metadata and zero-phase span dispatch. A reconstructed prior field
layout measures 80 bytes. Four alternating paired profiles support the compact
neutral/phase direction; combined fade/phase is mixed and remains tracked. All
digests and the legacy saved export match. 228 workspace tests, strict checks and
rebuilt bundle pass. The phase reset regression covers repeated builder updates
and fade retention. No canonical/public API or schema change was required.

Next implementation design: canonical musical timebase and tempo/meter ownership
needed for snapping, beat/bar display and MIDI. Preserve existing frame-based
audio playback and schema migration explicitly while designing musical anchors;
do not interpret demo BPM as canonical project tempo.

## Canonical musical timebase — current work unit

Size large, risk high across project math, snapshots/storage and editor gestures.
Implement within hex-project's std-only canonical ownership: nonnegative Q32.32
quarter-note beats, integer micro-BPM, bounded piecewise-constant TempoMap and
separate MeterMap, both beginning at beat zero. New/migrated Hex defaults are
120 BPM and 4/4. Validation policy: 1–1,000 BPM, numerators 1–99, power-of-two
denominators up to 64, at most 1,024 points per map. These are original Hex
contracts, not claims of measured Live ranges.

Build sorted fractional-frame Q32 tempo prefixes with checked u128 arithmetic on
the control side. Round once to nearest frame (half ties later) at conversion
boundaries. Inverse conversion uses the same prefixes; define its quantization
and boundary laws explicitly. Meter changes may occur at any beat and truncate the previous bar. Continue
bar numbering by the ceiling of the prior section's bar count; expose fragmentary
bar ends. This Hex numerical policy is not yet measured against Live. Snap is optional,
deterministic, and resolves exact ties later; visible ruler enumeration is capped.

Project commands insert/replace validated tempo and meter points transactionally
with normal history. Schema 8 persists maps and explicitly migrates schemas 1–7
without changing any clip, loop, source phase or fade frame values. Existing
audio remains frame-anchored; changing maps does not warp/move its PCM. This
prerequisite does not replace the later explicit musical-anchor/duration policy
needed for MIDI, audio warping, tempo automation ramps and recording.

First native workflow: compact tempo/meter controls, beat/bar ruler and opt-in
snap choice using one canonical map; frame viewport/hit-testing/playback remain
intact. Snapped moves still submit existing canonical frame edits and retain
original pointer capture. Preview and release must agree and cancel if map,
project/revision, viewport or snap state changes. No dead-space panel or second
UI scheduler. UI integration follows the backend contract, not duplicate math.

Acceptance: multi-rate conversion, fractional beats/half ties, segment boundaries,
inverse quantization/monotonicity, caps and overflow; meter/bar transitions, snap
ties; transactional failure/history; strict schema-8 roundtrip and all legacy
migrations; byte-identical frame audio; visible bounded ruler and native tempo,
meter, snap/move/undo checks. Callback timing work is still compiled frame data;
the new conversion routines must not run per sample or allocate in render.

Musical-time result: schema 8, transactional tempo/meter edits, bounded integer
conversion and compact editor workflow are integrated. 241 workspace tests and
two additional dense-map tests pass, including inverse monotonicity at 1,024
fractional boundaries and a rational frame-prefix golden. Workspace strict
Clippy passed; the later ruler layout correction and added tests pass targeted
strict checks, UI tests and desktop build. Architecture/style/format guards pass.
Legacy schema-4 export remains byte-identical (SHA-256
952c8d29e303b10942d47300381b87b074d75828077e1b759f744b29a2bba681).

Native isolated acceptance saved/reopened 121 bpm and 3/4, moved a clip with
quarter snapping to frame 87,471, and verified undo to 75,154 and redo. Snap-off
admitted frame 100,747; undo returned to the snapped position. Source phase,
source interval, duration and fades remained unchanged through those measured
moves. Corrected ruler labels are visibly contained within the existing 24 px
row. The earlier fixture interval included additional edits whose cause was not
verified; do not attribute them to this acceptance sequence.

Review repairs: count attempted ruler ticks independently of accepted labels,
retain meter markers in sub-quarter viewports, and give the ruler cells a real
height. Remaining UI gates: held-drag preview/stale cancellation, dense/minimum
window interaction and meter-aligned subdivisions for fractional marker offsets.
No listening/device-stress or exact Live timing parity claim. The running main
app was preserved; the rebuilt acceptance bundle is separate.

Next: define canonical musical clip anchors/durations and original MIDI note
data, then compile bounded frame events before callback consumption. This is
required to progress from audio-only arrangement editing toward fundamental
MIDI/instrument workflow parity. Recording, streaming, Session, routing, devices
and plugins remain required later milestones.

## Musical anchors and MIDI model — next implementation unit

Size large, risk high: define canonical beat-domain clip/note ownership before
introducing MIDI scheduling or an instrument. Preserve current frame audio,
media references, fade/source phase contracts and exact legacy exports. Read
research/observations/midi-model-001.md for the limited public reference evidence.

Finite acceptance: validated beat interval/note values and stable IDs; typed
create/edit/remove commands with atomic failure, no-op and undo/redo semantics;
explicit audio-versus-musical duration/anchor policy; strict next-schema migration
and full round trips; tempo-map edits recompile beat content without retiming
frame audio. Before rendering notes, require bounded control-side event preparation,
fixed voice/event capacities, deterministic same-frame ordering, partition-invariant
output and explicit seek/stop/loop/graph-replacement cleanup. An audible original
instrument and compact native note editor are required next, not implied by a
standalone note-data helper. Architecture review is in progress before editing
these subsystem boundaries.

Parallel bounded UI repair: make ruler subdivisions follow denominator beats and
fractional meter anchors, retaining marker priority, compact density and a strict
attempt bound. Validate half-beat meters, fractional offsets, huge and sub-quarter
viewports, tempo changes and quantized-frame collision/deduplication.

MIDI architecture accepted: see docs/midi.md. Keep tagged track contents behind
compatible audio accessors; MIDI commands and note IDs are explicit. Schema 9
migrates all old tracks as audio. First one-shot musical clips and sorted prepared
event edges are a prerequisite; instrument playback remains explicitly refused
until the immediately subsequent shared native/export synth runtime is integrated.
Do not expose nonplayable MIDI edit controls or silently omit instrument audio.

Meter-ruler source repair is complete: denominator-beat landmarks start at each
meter marker, visible sections share a fixed candidate budget, and deduplication
retains the latest marker on quantized frames. 63 UI tests cover 6/8, fractional
3/4 offsets, huge/sub-quarter viewports, tempo changes and map caps. Targeted
strict checks pass; native repair acceptance remains separate.

MIDI prerequisite result: canonical one-shot beat clips/notes, typed edits and
strict schema9 are implemented. Project MIDI edit logic lives in its own cohesive
module; no new crate or UI-owned scheduling was introduced. Schema1–8 audio and
source/fade/phase contracts remain intact. Control-side note edges are bounded
and stable; zero-frame gates fail explicitly. Workspace tests and strict checks
pass, including exact edge cap, same-pitch overlap, global IDs/history, atomic
duplicate overflow/wrong-kind errors and strict persistence cases.

Independent saved MIDI/CLI and worker tests verify tempo-crossing event frames
24,000–80,000, preservation through save/reopen, refusal without output publication,
and unchanged working sessions on unsupported edits/Open. Session admission has
a temporary explicit MIDI guard before history; remove it only when shared
instrument playback/export is integrated. A metadata-capacity issue remains:
65,536 canonical notes do not necessarily fit the existing 8 MiB storage ceiling.
Measure/tune this with large-project acceptance instead of silently raising limits.

Native isolated ruler acceptance at 44.1 kHz shows meter-aligned 6/8 half-beats
after a beat 1.5 marker and 3/4 after beat4; zoom gives readable subdivisions
within the existing compact row. Saving the original schema8 fixture produces
schema9 audio contents with clip start87,471, source phase4,909 and all meter
points preserved. The legacy saved audio render still hashes to
952c8d29e303b10942d47300381b87b074d75828077e1b759f744b29a2bba681.
Running main app preserved; only a separate acceptance bundle was rebuilt.

Next required implementation: an original instrument with bounded active voices
and prepared event consumption shared by native playback and offline export.
Define construction/reclamation, note ordering and seek/pause/Stop/global-loop/
graph-replacement semantics before touching the callback. Then expose compact
MIDI clip creation and note editing backed by these commands. One-shot clips are
a starting source policy; local looping, controllers/expression/chance/MPE, external
I/O and recording remain required. No audible MIDI or Live parity claim yet.

## Original instrument runtime — active implementation unit

Size large, risk high. Architecture review selects stateless absolute-frame note
spans so native Playback and offline Renderer share synthesis, including seek
chase and deterministic global-loop replay. hex-devices owns independently
testable original sine/envelope DSP; engine owns tempo compilation, interval
indexing and mixing. This is not a claim about future plugin reset behavior.

Finite acceptance: 5 ms attack/50 ms release, linear velocity and explicit 0.1
voice gain; checked gate/release endpoints, finite output and explicit Nyquist
refusal; 64 overlapping voices including tails and 128 onsets per sliding
256-frame window per track; bounded indexed queries and fixed stack scratch,
no callback allocation or trigonometry; block/seek/loop invariance; common
mute/solo/pan/master semantics; legacy audio PCM unchanged. Session edits must
validate compile feasibility before changing canonical history or revision.
Native and CLI export horizons must include instrument release tails. Replace
previous unsupported-instrument acceptance tests with playable round trips and
atomic invalid-candidate tests. Measure a stated 100-track workload and inspect
a separately bundled native fixture. Compact piano-roll interaction follows
this runtime; local MIDI loops, general devices/plugins and recording remain
required subsequent work.

Original instrument result: hex-devices DSP and engine interval compilation are
integrated; native and CLI exports include MIDI/release horizons. Transactional
prepared edits preserve history/IDs/redo on compile refusal and publish the exact
prepared graph. Workspace tests pass, saved mixed/MIDI-only tests produce notes
and unchanged legacy audio bytes. Isolated native Open/Play/Stop and native
export passed at44.1 kHz; native/CLI export bytes match. 100-track release baseline
is recorded in performance.md, with device/listening acceptance still separate.
Instrument lanes remain blank because existing UI draws audio clips only. Next
unit: draw/select beat clips and note previews, then compact native MIDI clip/note
creation and piano-roll edits through canonical commands. Preserve bounded UI
work, project revisions/IDs, undo and lowercase/font/compact rules.

## Native MIDI editor — active implementation unit

Size large, risk moderate. Reuse canonical schema9 note commands and prepared
worker admission. Finite workflow: create an instrument track and empty musical
clip; draw beat-anchored arrangement clips with note previews; select a clip to
open a compact piano roll; add/select/delete notes and adjust pitch, onset, gate
length and velocity through validated commands; move/duplicate/remove MIDI clips;
Save/reopen and undo/redo. Retain audio drag/trim/fade semantics, compact rows,
shared titlebar inset, lowercase text and bundled typography.

Selection must reconcile audio and musical IDs after edits and history; note
selection must clear when its owner disappears. Pending worker edits must not
use stale note geometry to enqueue conflicting commands. Rendering/hit testing
needs an explicit bound and useful behavior for notes outside the visible beat
and pitch ranges. Verify coordinate mapping and tempo-crossing clip previews,
wrong-kind command routing, history selection, note parameter limits and native
creation/save/reopen interaction. Drag gestures, local clip looping, keyboard
recording, MPE/controller lanes and dense-editor virtualization remain subsequent
required work rather than implied capabilities.

Native MIDI editor supporting evidence: worker integration tests now exercise
note authoring/update/delete/undo, duplicate/move, Save New and reopen with IDs,
pitch/velocity and compiled playback intact. A separate release control benchmark
measures 100 tracks/6,400 notes/32 history slots; see performance.md. This proves
controller contracts and measured preparation cost, not native pointer/focus or
paint acceptance. Those gates remain with the active UI implementation.

Native MIDI editor acceptance — 8 October 2026: creation, grid insertion and
native saved pitch 60 alignment passed in the isolated layout-check bundle.
Final-check reopened that fixture and exercised pitch/gate/velocity buttons,
keyboard onset, note delete/undo, clip move/duplicate/delete/undo, Save As and
confirmed replacement/reopen. Independent schema9 inspection confirms clips at
beats 1/5 with distinct note IDs, pitch 61, velocity 92, local onset 0.25 and gate 0.5.
Source geometry uses one shared body height; octave labels follow Live's MIDI 60=C3
convention (Ableton Live12 release notes). Compact readout uses p/t/g/v for pitch,
local onset, gate in quarter-note beats and velocity. 67 UI tests, strict UI
Clippy, desktop build and architecture/style/fmt checks pass. Full parity remains
open. Next unit: direct note move/resize gestures with preview, stale/pending
cancellation and one canonical history transaction, followed by minimum-window
and dense-editor inspection. Retain caps and selected-note visibility.

## Direct piano-roll note gestures — active unit

Size medium, risk moderate. Implement body move and both gate-edge resize with
bounded visual preview, one release-time canonical update, no-op/escape cancellation,
stale revision/generation/owner/view checks and preserved velocity/channel.
Acceptance: finite/edge geometry tests; canonical command/history compatibility;
native body move, both resizes, undo and save inspection; cancellation outside the
roll and Escape; no unrelated audio-drag regression. Dense/minimum-window stress
and modifier/free-grid/multi-selection remain explicit required follow-up.


Direct note gesture result: body move and both edge resizes passed in isolated
org.portalsurfer.hex.midigesturecheck. Original p61/onset0.25/gate0.5 became
p63/onset1.25/gate0.5 after a full one-beat/two-row drag; right resize reached
end 2.25, left resize retained that end with onset 1/gate 1.25. Each release advanced
one revision, off-roll release advanced none, and one undo restored the previous
gate. Native Save As independently verified pitch 63/start 1/gate 1.25/v92/channel0
with duplicate untouched. A short off-grid note clicked without dirty/revision
change, then moved onset0.125→0.625 retaining exact gate0.125 and expression
metadata. Fixtures: artifacts/native-midi-gesture-accepted-20261008.hex and
native-midi-gesture-short-accepted-20261008.hex.

Acceptance repaired sub-quarter admission, phantom click snapping and cropped
edge hit zones, plus native positioning (.relative overriding .absolute). All 76
UI tests (nine focused gesture regressions), session compound undo/redo, strict
UI/session Clippy, fmt/guards and desktop build pass. Held-preview/Escape, window
resize/stale geometry and minimum-window/dense native interaction remain open.
Next unit: dense-editor navigation/rendering and interaction verification, retaining
one canonical note history and bounded preview. Multi-selection, modifier/free-grid,
audition, local musical loops and MIDI expression/recording remain required work.

## Complete dense MIDI editor — active unit

Size large, risk moderate. Replace the 512-note visibility cap and per-note GPUI
layout with a revision-keyed immutable normalized Canvas scene owned by hex-ui.
Keep canonical data/history unchanged. Cache key includes project generation,
revision, selected clip, beat window/span and pitch range. Retain all visible
notes up to MAX_NOTES 65,536, with pitch-row hit buckets and O(1) Arc event capture.
Selected/gesture overlays reuse the scene during motion. Notes are insertion
ordered: no binary search assuming onset order.

Add deterministic overlap selection and compact bounded horizontal zoom/fit/pan
so density does not make canonical notes inaccessible. Preserve current move/
resize/release/no-op contracts across spans. Verify late notes beyond 512, overlap
IDs, cache reuse/invalidation, clipped/preview geometry, dynamic-span drag and
release performance. Native 600-note and six-overlap fixtures are authored and
valid; minimum-window interaction is an acceptance gate. 65k quads are bounded but
not automatically fast; measure geometry/hit costs separately from GPU/native
interaction and report gaps honestly. No new subsystem crate or audio redesign.

Cache/navigation acceptance also includes creating a clip at a nonzero arrangement
cursor: initial roll start must be local zero, not the placement beat. Native600
fixture successfully exports through hex-cli; it is a playable workflow fixture,
not just a synthetic unrenderable geometry test.

Dense MIDI unit progress: Canvas/cache/index/overlap/zoom source implementation
and 85 UI tests pass. Native acceptance caught and repaired a zero-sized Canvas.
The corrected isolated app paints 600 notes; note 553 is selectable/editable and
saves with exact unchanged timing, pitch and channel. Whole-project comparison
confirms the velocity edit is the only difference. Native zoom-in redraw passes.
Continue with six-overlap cycling, zoom/fit/pan direct gestures, nonzero-placement
clip creation and minimum-window acceptance before closing this unit.

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

Minimum-width transport follow-up is repaired and native-accepted. Bounded
one-line output status ellipsizes correctly at default/minimum window sizes;
full device metadata remains readable in a themed lowercase tooltip. Continue
with zoomed edge-resize/native cancellation checks, then the next documented
MIDI editing work; source tests alone do not close input/paint performance gates.


## Configurable MIDI editing grid — active next unit

Size medium, risk moderate. Add a transient piano-roll grid control for 1/4, 1/8,
1/16, 1/32 of a whole note and off; default 1/16 preserves the existing quarter-
beat step. Use one clear grid contract for insertion, initial gate, parameter
increments, body movement and edge resizing. Free mode must retain pointer-local
timing without quantization while still creating valid positive gates. Preserve
no-op exactness, grab offsets, expression and one release-time history edit. Grid
changes invalidate gestures. Keep adaptive drawing bounded and compact controls
usable at 880 × 540. No canonical schema, audio or device changes in this unit.
Verify every step/free mode, dynamic spans, both edges, gate/domain boundaries
and existing regression tests; native author/move/resize/save checks remain
required. Multi-selection, modifier gestures, audition and recording follow.

Dense/zoomed native follow-up: at two-beat span and 880 × 540, trailing edge
changes gate 0.5→0.75 with onset 0.25 fixed; leading edge then changes onset
0.25→0 and gate 0.75→1 while retaining end 1. Off-roll body release cancels
without revision. Whole-project saved comparison confirms exact metadata and
unchanged duplicate. Held-drag Escape cannot be independently exercised with
the current atomic-drag automation API; keep that native proof open alongside
stale-window checks rather than claiming the source guards are native evidence.

Grid source implementation completed in midi.rs/midi_drag.rs/mod.rs: fixed
whole-note divisions map to 1, 1/2, 1/4 and 1/8 quarter-note beats. Off keeps
exact Q32.32 pointer timing and quarter-beat default gates/nudges; resizing has
a positive one-raw-unit minimum. Grid is transient editor state and part of the
gesture context; switching cancels preview. Tests cover all divisions/free mode,
insert coordinates, both edges, no-op preservation and domain limits. 89 UI
tests and strict checks/build pass. Native grid acceptance uses the empty first
clip in artifacts/native-midi-grid-start-20261008.hex; its second clip remains
a preservation baseline. Do not claim native acceptance or exact Live magnetic
snapping before those gates are verified.

## Temporary MIDI snap inversion — active implementation

Size small, risk moderate. Use the platform primary modifier during insertion,
body dragging and both resize edges to invert fixed/free snapping temporarily.
An off grid temporarily uses the default 1/16 division; a fixed grid temporarily
uses free timing. Keep the configured selector unchanged, preserve raw pointer
anchors when the modifier changes mid-gesture, and apply the release event's
modifier to the final candidate. Preserve channel/velocity, the opposite resize
edge, positive gates, clip bounds and exactly one changed release transaction.
Verify toggling, no-op and boundary behavior in pure tests. Native fixed/free
authoring, modifier gestures and saved-value checks remain required. Exact Live
magnetic thresholds stay a separate reference/implementation unit.

Grid native fixture Open is confirmed at revision 1, but coordinate input is
currently failing in the automation tool despite a live process and readable
window. See issues.md; do not close its native acceptance gate from unit tests.

Snap inversion source is implemented: insertion samples MouseDown's platform
modifier, and gestures sample MouseMove and MouseUp. Pure tests now pass 92 cases
with one diagnostic benchmark ignored; strict checks/build pass. Primary source
acceptance reviewed raw anchors, immutable configured grid, release-time modifier
and one canonical update. Native grid/modifier saved-value checks remain open;
the grid-check instance now has user-authored unsaved revision 11 and is preserved.

Native grid acceptance now passes in a fresh snap-check bundle: all divisions/off,
both Cmd insertion inversion directions, compact snapped/free movement and free
both-edge resizing. Saved full-project comparisons and headless export pass; see
todo.md for fixture names and values. Held-modifier drag and mid-drag toggles remain
native-open because the current atomic drag API cannot hold modifiers.

## Atomic multi-note edits — active prerequisite

Size medium, risk moderate. Add narrow clip-scoped multi-note update/removal
commands before implementing native multi-selection. Reject empty/oversized,
duplicate, missing and wrong-clip note IDs, invalid note domains and overflow
before any mutation. Preserve ordering, stable IDs and untouched data; one command
is one undo/redo transaction. Keep existing session candidate compilation before
history commit, including rejection of a batch that exceeds real scheduling
budgets without changing revision/dirty/redo/project. No schema, dependency or
audio-thread changes. Require whole-project undo/redo/rollback tests, targeted
project/session checks, strict lints and desktop build. Follow this prerequisite
with native additive/marquee selection, group editing and saved-value acceptance;
backend commands alone do not establish the multi-selection workflow.

Native selection follow-up must distinguish normal editing from draw mode:
normal empty click selects time, empty drag selects notes, double-click inserts;
Shift supports additive/toggle selection and Cmd+A selects all. A compact draw
toggle/B shortcut preserves quick authoring using explicit drawing behavior.
Use public manual sections 10.4.1/10.5.2 as the initial specification, retaining
installed-app comparison as a separate reference gate. Do not preserve single-
click insertion in normal mode at the cost of Live-style selection composition.

Atomic group command prerequisite is source-accepted. Commands cap batches at
`MAX_NOTES`, validate unique same-clip targets and all fields, preserve note order,
and integrate through existing session candidate compilation. Whole-project
history/rollback and real onset-budget rejection tests pass alongside targeted
project/session checks, workspace strict Clippy, guards/fmt and desktop build.
Native additive/marquee selection and group gestures remain the active next unit.

## Native multi-note selection and group move/delete — active

Size large, risk high. First coherent UI milestone uses the accepted atomic group
commands. Normal mode starts enabled: empty click places a local insert marker,
empty drag selects a note rectangle, Shift adds/toggles selection and Cmd+A selects
all notes in the selected clip. Normal double-click inserts/deletes a note. Add a
compact pencil/B toggle for click authoring; continuous draw strokes remain a
subsequent drawing unit. Separate piano key focus/context so note selection and
deletion cannot accidentally move/delete Arrangement clips or change tracks.

Group body movement applies one shared time/pitch delta to immutable originals,
clamped to the intersection of every member's clip/pitch domain. Preserve relative
spacing, pitch intervals, gates, channels and velocities, including off-grid/free
timing. One changed valid release submits one atomic update; group deletion is
one atomic removal. Escape first cancels a gesture, then clears focused piano
selection; normal transport Stop remains available. Stale project generation,
revision, ownership, selection, viewport, grid or pending work cancels preview.

Reconcile selection after project/clip changes, history and removed IDs. Keep all
selected notes visible through the cached Canvas and show a compact count. Avoid
per-note layout nodes and full selection clones per render. Split cohesive helpers
within hex-ui to keep existing MIDI rendering/controller modules maintainable.
Single-note edge/parameter editing remains usable; group edge/parameter actions
must be clearly deferred or disabled, never silently edit just one group member.

Required source checks cover selection/toggles/marquee/select-all/reconciliation,
shared group clamps/spacing, free/fixed/modifier movement, no-op/cancellation and
complete batch payloads. Run UI tests, strict workspace lints, guards/fmt and
desktop build. Native gates: normal empty click versus double-click/draw toggle,
Shift/marquee/all-note selection, one-step group move/delete undo/redo, exact saved
metadata and minimum-window clarity. Continuous drawing, group edge/parameter
edits, audition, stretching, note overwrite and full Live selection parity remain
explicit subsequent units; this milestone does not close them.

Selection milestone checkpoint: implementation and source checks pass (100 UI
tests, one existing benchmark ignored; strict workspace Clippy, guards, fmt,
desktop build). Isolated native selection-check acceptance confirms Shift toggle,
marquee, all-note selection/Escape, normal/draw insertion and shared move/delete.
Fixtures `native-midi-selection-moved-20261008.hex` and
`native-midi-selection-group-deleted-20261008.hex` compare exactly against
independently transformed complete projects. `native-midi-selection-undo-20261008.hex`
restores the earlier single-delete state exactly. Group-delete one-step undo is
visually observed; final `native-midi-selection-restored-20261008.hex` restores
all musical/project data with monotonic next_id 16 after undone authoring probes.
Keep native additive marquee, redo/no-op/off-roll, focused arrows and minimum
layout unchecked. The capture showed content at half the returned canvas size;
the resize attempt did not establish minimum sizing. Preserve this limitation
without attributing it to application rendering before a repeat.

## Group keyboard note movement — active

Size medium, risk moderate. Extend piano-focused Left/Right and Up/Down to all
selected notes. Use current grid movement_step or one semitone; derive a common
delta from every member’s clip/pitch bounds and preserve spacing, short/off-grid
gates, expression and IDs. One changed key action is one atomic update/history
step; a common zero delta enqueues nothing and preserves redo. Refuse stale,
missing/foreign selection or pending edits, cancel previews and retain selection
after successful updates. Keep single-note behavior and Arrangement routing.
Extract shared pure clamp/payload helpers from group dragging rather than duplicate
math. Group gate/parameter/Shift-octave editing, duplicate/split routing and draw
strokes are deferred. Accept targeted source boundary/payload/history tests, strict
workspace Clippy, fmt/guards/build, then native focused group movement/save/undo.

Selection acceptance update: native single-note Up then Right changes only note 6
by +1 semitone/+1/4 beat. Complete saved metadata in
`native-midi-selection-keyboard-20261008.hex` equals the prior restored project
with exactly those edits; both Arrangement clip positions remain unchanged.

Native selection follow-up: focused single-note Up/Right passes exact full-project
comparison (`native-midi-selection-keyboard-20261008.hex`). Group no-op and
off-roll release produce an identical project (`native-midi-selection-canceled-20261008.hex`).
Group deletion undo/redo runs as single actions; saved redo removes exactly IDs
6/7 and preserves every other value (`native-midi-selection-redo-20261008.hex`).
Native additive marquee and minimum-size layout remain open. The documented
automation drag has no modifier parameter, so Shift-marquee has source evidence
only. Held-preview Escape remains separate from selection-clearing Escape.

Group keyboard checkpoint accepted: shared pointer/keyboard clamp and payload
builder, current-clip selection admission and one changed batch per key. 103 UI
tests pass (one existing benchmark ignored), strict workspace Clippy, fmt/guards
and desktop build pass. Native Right/Up preserves complete metadata outside the
selected pair. One-step undo/redo and a clip-end-clamped no-op preserving redo
pass, with exact saved comparison. Fresh keyboard-check bundle provides consistent
capture and accepts 880×540 layout with seven selected notes; the previous capture
limitation is resolved for this gate without a display or source workaround.
Native Shift-marquee and held-preview Escape remain open.

Next authoring dependency: a bounded atomic multi-note insertion command, needed
before continuous drawing can commit an entire stroke in one history step. Validate
all notes before IDs/mutation, enforce current note cap and scheduling admission,
preserve allocator/history on rejected candidates and keep callbacks unchanged.
Do not implement drawing by issuing one project command for every pointer move.

## Atomic note insertion — active prerequisite for continuous drawing

Size medium, risk moderate. Add `MidiNoteInsert` and clip-scoped `AddMidiNotes`,
nonempty and capped by both the batch/project 65,536-note limit. Validate all
fields, note ends, clip ownership and allocator range before mutation; reserve
storage before allocation/append. Preserve input order and expression, permitting
duplicate musical values as single insertion does. Return a Copy created-note
range (clip, inclusive first/last ID) without heap ownership in service events.
One batch creates one history step; undo retains allocator high-water and redo
restores the same IDs. Compile candidate before canonical commit; late validation,
capacity, ID exhaustion or scheduling rejection preserves project/revision/dirty/
redo/allocator and publishes no replacement playback. No schema/engine/callback
changes. Source gates: boundary/order/IDs/history/rollback tests and actual service
valid spaced acceptance plus 129-simultaneous-onset rejection, targeted tests,
workspace strict Clippy, guards/fmt and desktop build. Continuous drawing UI and
its native acceptance depend on this contract and follow separately.

Drawing reference correction: public §10.4.1 also requires melodic erasure and
pitch-locked backtracking/Option inversion. After AddMidiNotes, add mixed atomic
insert/remove semantics before UI strokes. Sparse pointer interpolation, retracing
and overlap order need explicit contracts and experiment evidence. Insertion-only
painting is not the requested final behavior. See the dated gesture observation
and docs/issues.md.

Mixed drawing edit direction (next dependency, not implemented): one clip-scoped
command carries unique existing removal IDs and new note records. Either list may
be empty; both empty is rejected. Validate all membership/fields/allocator and
final global count before mutation. Stable-compaction removal followed by ordered
append preserves untouched note order and produces one history entry/publication.
Return the created range when insertion occurs, otherwise no created IDs. Reserve
for final clip size rather than old size plus every inserted note, since erasure
can free capacity. Reuse insertion admission instead of duplicating validation.
Pitch-lock backtracking should remove pending local preview notes before release,
not submit controller undo commands. Exact pointer sweep/overwrite behavior still
requires reference experiments; this transaction is a model prerequisite only.

Atomic insertion prerequisite accepted: public record/command/created range and
full global-cap/field/storage/allocator admission implemented. Ten project MIDI
tests and five session admission tests pass, including exact-cap acceptance with
notes elsewhere, separate late-invalid fields, absent/audio targets, exhausted
ID range, ordered expression/history and actual onset-density refusal preserving
working graph/project/revision/dirty/redo/allocator. Workspace strict Clippy,
desktop build, fmt and guards pass. Primary read dispatch, append/range preflight
and revised regression/service tests; no Git metadata exists for diff review.
No UI stroke authoring or native acceptance is claimed. Next: mixed insertion/
removal admission before continuous drawing, as specified above.

## Mixed note edit — active drawing prerequisite

Size medium, risk moderate. Implement `EditMidiNotes { clip, remove, insert }`
using unique clip-owned removal IDs and existing MidiNoteInsert records. Either
list may be empty; both empty rejects. Cap each list, validate all targets/fields,
final global count and entire ID range before mutation. Reserve final clip size,
retain untouched notes in one pass then append new records in input order. Return
the created range only for insertions; erase-only consumes no IDs. Share admission
and compaction/append helpers with existing batch commands. One changed edit is
one history/compiled publication; preparation failure preserves original notes,
allocator/revision/dirty/redo and working graph. Source gates cover mixed/order/
expression/history, list-only variants, each invalid field/target, full-cap replace
versus increase, allocator partial-range failure and actual dense mixed refusal.
No schema/callback/DSP/UI changes. Native drawing remains the next dependent unit.

Mixed prerequisite accepted: `EditMidiNotes` is backed by dedicated midi_batch.rs,
shared with insertion/removal commands. Thirteen project MIDI and six session
admission tests pass. Source review confirms unique owned removals, all-field
insertion validation, final global cap, net-growth reservation, checked allocator
range before note mutation, stable retain then ordered append. Tests cover list-only
variants, full-cap replacement, invalid membership/each field, allocator exhaustion
and preserved redo. Actual service admits a mixed edit and proves dense mixed
refusal preserves original notes, allocator/revision/dirty/redo and publishes no
replacement graph. Strict workspace Clippy, desktop build, fmt and guards pass.
No schema, callback or UI stroke behavior changed. Continuous drawing is next.

## Unsaved-project close repair — 8 October 2026

The native startup window retains its bootstrap handle type after the editor
replaces the root view. The old close prompt downcast that handle to EditorView;
on failure it returned after displaying the sheet, never consuming Cancel or
Discard. The prompt now captures an untyped window handle and updates the editor
through its weak entity. Closed windows/entities remain harmless failed updates;
Cancel preserves the project, and Discard stops playback before close/quit.

Native isolated acceptance: add a disposable instrument track, close, Cancel,
close again, Discard; the test process exits. Cmd+Q, Cancel, Cmd+Q, Discard also
exits in the final build. Observed GPUI native responses are zero-based 0/1;
no response-code workaround remains. Targeted close test, UI strict Clippy,
desktop build, formatting and architecture/style guards pass. Temporary
response diagnostics were removed. The existing development bundle is rebuilt
only after confirming it is inactive; no user's unsaved project was discarded.

## Continuous MIDI drawing — active native integration

Size large, risk high. The pure stroke resolver is staged in hex-ui; integrate it
into the existing piano roll without DSP or schema changes. Pointer-down captures
a bounded immutable baseline once, moves update only local insert/erase previews,
and an inside release commits one mixed transaction. Preserve normal selection,
double-click, single/group editing, channels/velocity of untouched notes and exact
clicked-note erasure. Fixed-grid anchors stay on the final valid grid cell;
Off keeps the raw first onset and quarter-beat gates/spacing anchored to it.
Pitch lock defaults on with a compact toggle; Option inverses it at stroke start.
Modifier changes mid-stroke cancel until reference evidence defines transitions.
Off spacing and lowest-ID overlap resolution are original Hex policies, not
established Live parity. Keep these reference uncertainties explicit.

Acceptance: preview additions and erased baseline notes; sparse pointer segments;
locked backtracking/reversal; melodic and vertical strokes; dwell deduplication;
complete cap refusal; one-step saved undo/redo; erase-only and mixed strokes;
clip-end snapping; no project commands on motion; cancellation on Escape, focus,
revision/project/clip/grid/view/bounds/mode/pending changes and outside release.
Select created ranges only in the matching currently selected clip. Share scene/
preview data rather than copying a complete project or stroke on every render.
Targeted source checks and independent native saved-value acceptance must both
pass. Held-modifier/held-mouse behavior unavailable through the automation API
remains a separate manual gate, never claimed by source checks alone.

Continuous drawing source checkpoint: focused midi_draw bridge, pure midi_stroke
resolver, scene preview geometry, Canvas overlay/events, lifecycle subscriptions
and matching-clip Created::Notes selection are integrated. Primary source review
caught and repaired clip-tail anchor and short off-grid clicked-note issues.
116 UI tests pass (one benchmark ignored), strict workspace Clippy, desktop build,
fmt and guards pass. Native initial locked drag produced the expected five notes
and one revision. Its saved file contains the exact initial IDs 6–10 plus unrelated
later edits, so it is not a pristine whole-project acceptance artifact. Those
edits and the instance were preserved. A fresh acceptance process loaded the
original fixture, but repeated pointer actions failed in the native automation
window lookup while read-only state/screenshot and process inventory succeeded.
No app crash was established. Rebind to the same unique process did not restore
input. Saved undo/redo, erase/mixed, off-roll and minimum-layout gates stay open;
resume native acceptance on the existing process when input becomes available.
This evidence limitation does not block independent project implementation.

## Selected-group parameter editing — active implementation

Size medium, risk moderate. Enable the existing compact pitch, gate and velocity
buttons for a selected group. Capture current clip-owned originals once and submit
one `UpdateMidiNotes` transaction, retaining IDs, order, channel and all other
fields. Pitch changes by one semitone, gate by the current grid movement step,
velocity by eight. Apply one uniform relative delta; refuse the whole edit if any
member would exceed pitch 0–127, velocity 1–127, positive gate or the clip end.
This boundary policy is original Hex behavior pending Live reference evidence.
Keep existing single-note and group arrow/body movement behavior unchanged.

Acceptance: unequal gates/velocities and nonzero channels retain their relative
differences and untouched fields; whole-group boundary refusal, stale/missing
selection rejection and pending-work guards; one batch and one history action;
UI tests, strict Clippy, formatting/style/boundary guards. Native saved-value and
minimum-layout checks remain separate if the existing automation route cannot
deliver input. Group edge dragging, channel controls, recording and expression
lanes are deferred to their own contracts.

Source checkpoint: group controls, bounded capture and checked parameter payloads
are integrated in hex-ui. 121 UI tests pass, one existing benchmark ignored;
strict UI Clippy, fmt, guards and desktop build pass. Primary reviewed immutable
capture, rendered-context admission, exact deltas, refusal and one batch/history.
The inactive development bundle contains the new build. Isolated native startup
initially captures a blank canvas; exposed zoom triggers ready-editor painting.
A one-second sample shows the main thread predominantly in the AppKit event loop,
with editor update tasks running, not a demonstrated deadlock. Keyboard open still
does not show a dialog. Preserve this process and keep the explicit saved-value/
minimum-layout protocol pending; investigate startup/redraw evidence separately.

## MIDI authoring keyboard follow-up — active implementation

Size small, risk moderate. Add piano-focused Shift+Up/Down octave transposition
and route Shift+Left/Right through the accepted selected-group gate transaction.
The [Live 12 manual](https://www.ableton.com/en/live-manual/12/editing-midi/#changing-note-length)
documents note length keys and octave transposition in sections 10.5.5/10.5.9.1.
Reuse the existing canonical commands and immutable group resolver; keep existing
single-note and Arrangement shortcuts, fields, history, DSP and toolbar unchanged.
Group octave movement uses one common clamped pitch delta; group gate buttons/keys
refuse an entire boundary-crossing edit. These boundary policies need reference
measurement and must not be described as verified Live behavior.

Acceptance: group octave offsets preserve intervals/timing/gates/channel/velocity,
common boundary clamp and no-op, one history action and untouched notes; empty/
pending/focus guards and existing Shift-trim routing; UI tests, strict UI Clippy,
formatting/build/guards. Native key delivery remains an independent open gate.

Source checkpoint: the keyboard follow-up is integrated with no toolbar expansion
or schema/DSP changes. 124 UI tests pass, one existing benchmark ignored; strict
UI Clippy/fmt and desktop build pass. Primary reviewed focused action dispatch and
existing audio trim branches. Native gesture/save checks stay pending, not inferred
from the batch/history tests. The source investigation did not justify a renderer
or shortcut workaround for the separate native input/redraw issue.

## Local MIDI clip looping — contract and compiler work

Size large, risk high. This is the next documented fundamental source/playback
gap, beyond the accepted one-shot note editor. Define independent source start,
source extent, loop brace and finite Arrangement length before extending model,
persistence and compilation. Preserve notes outside the playable region and all
existing one-shot project behavior; do not duplicate canonical note IDs per repeat.

The [Live 12 clip-view manual](https://www.ableton.com/en/live-manual/12/clip-view/#looping-clips)
establishes a distinct start marker, adjustable loop region, and an intro before
the loop. It does not establish numerical gate, chase, release-tail or seam rules.
Those require bounded original semantics with explicit parity experiments.

Acceptance design must cover strict version migration, atomic invalid edit/compile
refusal, intro/partial first and final iterations, notes outside loop, checked beat
and frame endpoints through tempo changes, deterministic repeated occurrence
ordering, gate and release behavior at seams/clip end, existing density/voice
budgets, partition invariance, seek/Stop/global-loop interaction and source-side
editing. Finite Arrangement compilation is a stage, not the full Session looping
requirement; potentially unbounded Session playback needs prepared templates and
bounded work per callback. Review that path before locking public contracts.


### source projection prerequisite — active implementation

Implement the shared checked beat-domain source projection in hex-project and
reuse it in both engine edge and instrument-span compilation. This first stage
keeps canonical clips/schema 9 one-shot; it must not be called audible local-loop
support. The next dependent stage adds source metadata, commands/history and
strict migration, then connects compact source controls and native acceptance.

The validated descriptor separates finite Arrangement duration, source extent,
start marker and optional loop brace. The first pass is `[source_start, loop_end)`;
subsequent passes are `[loop_start, loop_end)`. Start may precede loop start, so
intro notes sound once. Notes before the start marker are not onset-chased; notes
inside the loop but before the start marker first sound on a later pass. Gates
are cut at source/loop/end boundaries with normal instrument release afterward.
This seam policy is original Hex behavior pending reference measurement.

Count occurrences in constant time per authored note, including huge durations
and tiny loop spans; never scan empty iterations. Admit the aggregate against an
independent expanded-occurrence budget before reserving/emitting. Convert every
absolute onset/off independently through the tempo map and preserve the existing
density/voice limits. Current one-shot conversion, deterministic ordering and
render samples must remain unchanged. Test intro/partial/repeated cases, hidden
notes, overflow/count refusal and existing tempo/render regressions; inspect
both compiler consumers, then run project/engine checks, strict Clippy and guards.


Next integration review: schema 9 puts `loop_policy` on instrument track contents;
source controls are per clip, so version 10 must retain the strict version-9 DTO
and explicitly migrate its one-shot defaults rather than weakening the old
reader. `MidiClipSnapshot`, place/duplicate construction and note validation
must carry source metadata together. Duplicate keeps source settings and creates
new canonical IDs; move changes only Arrangement placement. Reject source-extent
changes that would invalidate authored notes rather than deleting hidden notes.

Audit all MIDI editor length consumers when separating source and Arrangement
length: piano canvas fit/pan, scene cache key, drawing context and parameter/gate
validation currently use `clip.length()`. These become source-length consumers;
timeline placement/duplication/audible end remain Arrangement-length consumers.
Do not enable source edits until that distinction is integrated, or a shorter
Arrangement would incorrectly make valid source notes inaccessible. The project
compiler must admit the whole-project occurrence count before any per-track span
allocation, not only impose independent caps on each track.


Source projection checkpoint: validated `MidiSourcePlayback` and allocation-free
per-note plans are implemented, and both edge/span compilers consume the shared
projection/frame conversion. Project-wide 65,536 occurrence admission precedes
buffer reservation; event capacity remains 131,072. Stable iteration ordering
is prepared without canonical ID duplication. Review confirmed intro/start-after-
loop behavior, final clipping and the overflow-avoiding first-repeat comparison.
Seven project projection tests, including an independent exhaustive small-domain
segment-walk oracle, pass. All 40 engine tests and 34 session tests pass; strict
project/engine Clippy, fmt, architecture/style and desktop build pass. Existing
one-shot tempo/density/partition/seek/refusal regressions pass. No new native loop
behavior is claimed; schema 9 and canonical clips remain one-shot.


## canonical local MIDI source integration — active implementation

Size large, risk high. Connect the accepted projection to canonical per-clip
source length/start/loop markers/enabled state, retaining `length()` for finite
Arrangement duration. One typed atomic source-settings edit admits all fields
and notes before mutation; source shrink refuses invalid authored notes, while
notes outside playback start/loop remain intact. Move only changes placement;
duplicate copies source settings and allocates new clip/note IDs.

Schema 10 writes required clip-local source metadata, retains strict schema-9
DTO/default migration, and preserves schemas 1–8 audio migration. Piano editing,
gestures, drawing, parameter bounds and fit/pan use source length; timeline range
still uses Arrangement duration. Reconcile same-clip source changes into viewport
bounds as well as selection changes. Source controls and bounded loop-aware
Arrangement preview are the next UI integration step; the raw source-note
thumbnail currently cannot represent repetitions or offsets correctly.

Acceptance: model validation/atomic refusal/no-op/redo, hidden notes, duplicate/
move, exact schema-10 roundtrip and malformed/missing fields, literal schema-9
migration, legacy audio tests, repeated event/span timing through tempo changes,
seam/release/final-partial behavior, partition/seek/global-loop invariance and
whole-project expanded-limit refusal. Session refusal must preserve revision,
dirty/history/redo and working graph. Targeted affected crates, strict Clippy,
formatting, desktop build and policy guards; native saved/audible/reference
loop checks remain independent and ongoing Session templates remain required.


Thumbnail architecture decision: prepare a compact per-clip frame-domain summary
on the existing engine/session control-worker path, publish it with the matching
read-only project/revision, and let UI lookup/draw only bounded samples. Do not
rebuild projection by scanning notes on a UI cache miss. Summary construction
can reuse admitted occurrence traversal, selecting a small deterministic sample
across initial/repeated passes; the full spans still drive playback. Use a sorted
clip index, checked global/per-clip budgets and fallible allocation. Keep summary
ownership separate from callback graph lifetime and source piano geometry.
Accepted/refused compile paths must retain matching graph/project/summary;
save/path/dirty-only state updates reuse the same immutable preview. The next
milestone tests offset/intro/repeated/partial/tempo samples, replacement/refusal
revision consistency and bounded UI shape work, then adds compact source controls.


Source-aware UI checkpoint: piano bounds, parameter/nudge edits, drawing contexts,
fit/pan and selected/same-clip viewport reconciliation use source length; timeline
range remains Arrangement length. 124 UI tests pass, one existing benchmark
ignored. The first UI build failed due disk exhaustion; after safe generated-cache
recovery the same checks passed with command-local incremental caching disabled.
Canonical model/storage work and repeated-playback/session tests are still under
validation; native source controls/loop thumbnails remain the next dependency.


Canonical integration source acceptance: model fields, `SetMidiClipSource`, history,
snapshots, strict schema 10 and explicit schema-9 one-shot defaults are integrated.
Existing schemas 1–8 remain strict audio migrations. Source-aware UI bounds and
same-clip viewport reconciliation are integrated. Primary reviewed seam/tempo
expected frames, independent source/Arrangement lengths, hidden-note preservation,
checked pre-allocation admission and strict old/new readers. A large render-test
fixture was moved to heap storage; finite release-tail output is asserted before
global-loop comparisons. The workspace suite found one stale emitted-version
assertion, repaired without changing legacy fixtures. All 358 workspace tests
pass, one existing UI benchmark ignored; desktop build, fmt and policy guards
pass. Native loop controls/thumbnail/listening acceptance remain incomplete.


Strict workspace Clippy also passes for the accepted canonical source integration.
Logs: `artifacts/midi-source-workspace-tests.log` and
`artifacts/midi-source-clippy.log`. Existing running apps were not replaced.

## prepared MIDI Arrangement thumbnails — active implementation

Size medium-to-large, risk high because graph/view revision ownership crosses
engine and session. Existing coherent executor owns the prepared frame summary
and publication path; primary owns the thin UI consumer. No new worker/crate.
Generate bounded deterministic samples during admitted control-side occurrence
compilation, retain authored and projected counts, and publish a sorted immutable
clip index with the matching accepted project/revision. Reuse prior summaries on
save/path/dirty-only and refused events. UI performs lookup and bounded shape
construction, never note or occurrence compilation/scans during paint.

Acceptance: offsets/intros/repeats/partial gates and tempo boundaries, first/last
sample coverage, per-clip/global budgets, empty clips and deterministic one-shot
positions; accepted edit/open/history/refusal consistency and summary reuse;
existing graph retirement/callback invariants and no new callback preparation.
Then replace raw-source Arrangement thumbnails with bounded per-row drawing and
truthful sample counts, run affected tests/build/guards and native resize checks.
Compact loop source controls follow the accepted thumbnail path.


Expanded thumbnail acceptance: compile-before-history restores are required to
keep project/graph/summary coherent on refusal. Add narrow prepared history APIs
with candidate high-water and storage admission before commit; retain existing
headless bool APIs. Primary authorizes this scoped project/session extension after
read-only architecture review. Test deliberately refused preparers, unchanged
history/IDs/redo, then exact accepted artifacts/revisions. Import publication is
being inspected for the same invariant before accepting the lifecycle milestone.

UI consumer source checkpoint: source-note scans and per-paint tempo conversion
are removed from track thumbnails. Prepared lookup, viewport-cropped frame
geometry, per-clip sample bound and 128 note-shapes per row are integrated; compact
shown/total badge and tooltip expose sampling. 125 UI tests pass, one existing
benchmark ignored; native layout acceptance and complete lifecycle checks pending.

## prepared thumbnail acceptance and next source controls — 8 October 2026

364 workspace tests pass, one existing UI benchmark ignored; strict workspace
Clippy and desktop build pass. Engine compilation emits bounded samples and
session publishes the exact graph/preview pair. Prepared undo/redo and import
commit only after graph/preview preparation succeeds. Source refusal tests retain
history, IDs, project/media and published state. UI bounds note shapes to 32 per
clip and 128 per row; clip containers are not yet globally virtualized.

Native isolated acceptance opened the original loop fixture at the active
44.1 kHz device rate, displayed four projected occurrences and fit all four
authored notes across the eight-beat source. The preserved 48 kHz baseline was
correctly refused by the active-device sample-rate contract. Cropped/minimum and
dense layout, saved marker edits and listening acceptance remain open.

Next finite unit: compact selected-clip controls for arrangement duration, source
extent/start, loop brace and enabled state. Use SetMidiClipSource atomically,
keep source/arrangement units independent, preserve hidden notes and disabled
markers, reject invalid bounds/overflow and stale revision/generation/selection,
avoid no-op history and fit the 880×540 layout. Run resolver/UI checks and native
edit/undo/redo/create-new save/reopen checks before claiming acceptance.

Native thumbnail follow-up: zoom/pan crop shows two remaining gates and `2/4`
badge. Play/Stop transport passes visually. Minimum 880×540 capture is
inconclusive because canvas appeared clipped after resize; keep this gate open.

## ongoing Session design dependency — 8 October 2026

Read-only architecture review confirms finite occurrence expansion cannot serve
as an ongoing Session scheduler. Next backend prerequisite after source controls:
compile bounded beat-domain source templates once, then test bounded sample-exact
edges across indefinite wraps, tempo changes, partitioning, launch boundaries and
Stop/seek. Callback ownership must use IDs/indices into retired immutable graphs,
fixed active/pending state and admitted event/voice work. Existing control-only
tempo conversion cannot be silently moved to per-sample callback work. Choose
and verify a bounded prepared timing conversion/horizon contract before runtime
integration; define overflow/short-loop refusal and stale launch rejection.

Canonical slots/scenes and shared source/template identity require an explicit
versioned project/command design after that engine contract. Current MidiClip
combines source and finite Arrangement placement; do not make it implicitly
unbounded. Proposed shared template IDs remain a design recommendation, not an
implemented model. Recording/capture to Arrangement, audio Session clips,
controllers/expression, plugins/follow actions and reference parity stay required
later dependencies. No demo-launcher substitution or finite-loop parity claim.

Scheduler timing direction from architecture review: prefer graph-owned immutable
piecewise tempo prefixes with checked allocation-free conversions at block/event
boundaries, never per sample. A worker-refilled event horizon can run dry on a
control-worker stall and delay prepared Play. The first engine contract must
match canonical Q32 rounding/inverse boundaries for up to 1024 tempo segments,
report checked conversion faults without allocation, and measure dense admitted
work before callback integration. Template density admission must bound both
onsets and note-offs across seams and tempo quantization; a shortest-period
conservative proof requires independent boundary tests before adoption. No
callback timing policy is changed by this design note.

## source-control native progress and prepared timing unit — 8 October 2026

Compact source controls are integrated with metadata-only paint, bounded labels,
checked grid steps and atomic source commands. Explicit unique button IDs repair
the observed +/- identity collision. Native five-field edits and loop toggle
Undo/Redo pass. Create-new Save As preserves every baseline field except the five
intended numeric edits; reopen visibly restores markers. Final fit interaction
was interrupted, so source-note reopening/minimum layout/listening remain gates.

Next backend executor owns hex-engine's prepared immutable tempo contract only:
reuse canonical rounding/inverse code, validate sample-rate agreement, test up to
1024 tempo points and checked query failures, and measure prepared dense queries
in release. This is the timing prerequisite for ongoing templates, not Session
playback or callback integration. Canonical Session schema/commands and scheduler
remain required subsequent work.

Prepared timing prerequisite accepted in source: engine-only `PreparedTempoMap`
owns the canonical map, validates expected rate/point cap, and delegates borrowed
checked conversions with typed static errors. No copied rounding kernel, Arc
churn or callback integration. 51 engine tests, strict engine Clippy, fmt and
guards pass. Release 1024-point mixed conversion profile: 43.689 ms for 1,228,800
queries, 35.6 ns/query; no realtime deadline claim. Next ongoing template edge
contract must prove query/admission bounds at quantized seams and partial periods,
then implement independent finite-projection/partition/seek oracle checks before
Session schema/runtime integration.

## ongoing MIDI edge query contract — implementation sequence

The adopted first scheduler unit is immutable engine templates plus stateless
half-open next-window edge queries, using current clip/note IDs. It does not
change canonical Session schema or finite Arrangement semantics. Templates read
source/start/loop metadata; an explicitly ongoing template ignores Arrangement
duration. Query launch position is a beat, query ranges are at most 256 frames,
and a caller-owned buffer admits at most 256 edges (128 onsets).

First prerequisite now in implementation (small size, moderate numerical risk):
canonical `TempoMap::beat_lower_bound_for_frame` returns the first raw beat whose
forward rounded frame reaches the request, or the one-past-u64 raw-beat sentinel.
For positive frame f the fixed threshold is `(f << 32) - 2^31`; cached prefixes
locate a segment and checked ceil division resolves the first raw beat. Frame 0
returns raw 0. PreparedTempoMap delegates to this kernel. Independent raw-domain
brute force, predecessor minimality, coincident boundaries, max-domain sentinel,
1024-point/multi-rate checks and strict project/engine checks are required.

The later query candidate interval is `[lower_bound(start), lower_bound(end))`;
forward-convert and filter each candidate edge. Intro notes play once from the
source start to loop end; repeated notes use loop phases and seam-clipped gates.
An off at period end belongs to its original iteration. Sort bounded output by
frame, off before on, stable source identity and iteration. Querying after a seek
must use arithmetic period indices, never replay all previous loops.

Admission design: use fastest map tempo and a conservative beat-window width
`ceil(258 * 2^32 * max_micro_bpm / (60000000 * sample_rate))`. Prove this covers
rounding and prefix truncation before implementation acceptance. Bound intro,
cyclic periodic density and intro/tail seam windows independently; the former
full-period shortcut is insufficient. Reject collapsed periods/gates explicitly
and admit a dense spread-out valid fixture. Compare query results independently
with finite canonical projection over bounded horizons, varying block sizes,
seeks, tempo seams and simultaneous edges.

Active voices, chase, release/stateful DSP, aggregate multitrack admission, launch
quantization, graph replacement and canonical slots/scenes remain subsequent
runtime contracts. A successful edge-query primitive alone is not Session parity.

Exact lower-bound prerequisite source accepted: canonical/prepared APIs are
integrated with explicit one-past-domain sentinel. Final focused tests pass
(7 canonical tempo, 8 prepared engine); strict checks pass and earlier full
project/engine suites pass. Active executor now owns immutable source edge
templates, control-side density preparation and fixed-storage queries. Timing
map compatibility is part of admission: faster or different-rate query maps must
be refused, rather than silently invalidating the prepared event bound.

## Ongoing source edge templates — source acceptance

PreparedMidiSourceTemplate and fixed MidiEdgeBuffer are implemented in hex-engine.
The source snapshot ignores finite Arrangement duration explicitly. Intro/repeat
edges are control-prepared; query candidates use exact canonical lower bounds,
binary phase lookup and arithmetic period indices. A 260-window hard guard and
256-edge/128-onset output caps enforce bounded work; failures expose zero buffer
length. Timing rate/tempo compatibility and beat-domain exhaustion are checked.

Control-only density preparation is a separate private module. Independent tiny
closed-window/cyclic/seam oracles test the conservative counting bound, including
residual-zero coincident phases. Finite canonical projection, varied partitions,
seek order, intro-before-loop and source-start-after-loop, seam cuts and positive
same-frame quantized events pass. Short-gate/period refusal names describe the
proof limitation; exact-one-frame and some seam-dense valid patterns may be
conservatively refused. Full engine 63 tests pass; final root 8 query + 3 density tests,
strict Clippy/fmt/guards and desktop build pass. Benchmark methodology/results
are in performance.md.

Next required contract: admitted active voice reconstruction/ownership and
discontinuities for real ongoing playback, including simultaneous release tails,
seek/pause/Stop/launch and graph replacement; aggregate track/event bounds and
measured device deadlines. Then connect canonical Session slots/scenes, versioned
commands/history and compact UI. This engine primitive alone is not Session
playback or feature parity.

## Ongoing instrument span/render contract — active implementation

Use stateless source spans beside the accepted edge template. Retain gate
association in the existing source-preparation pass, prepare original
hex-devices voices off the render path, and query indexed intro/repeat intervals
into fixed 192-span storage (64 sounding plus 128 new onsets). Release lasts 50 ms
at the project rate and continues across loop seams. Exact absolute onset/gate
frames drive the existing oscillator/envelope, preserving seek and partition
behavior without prior-event replay. Generic MIDI template validation must not
acquire instrument-specific Nyquist restrictions; unplayed source notes remain
preserved.

Preparation must prove 64 simultaneous gate-plus-release voices, separately from
the accepted edge count. A conservative raw-beat release horizon covers rounding
and prefix truncation; periodic sounding-span density uses half-open interval
overlap, unlike closed edge-density arcs. Queries need independent fixed period
and interval-tree traversal bounds. Verify a finite canonical PCM oracle, active
release tails, before/after loop starts, tempo/shared-frame boundaries, arbitrary
seeks and varied/one-frame partitions. Caller storage/output must be empty or
silent on explicit query/preparation errors.

This medium-size/high-risk backend unit owns no CPAL/Session/controller/UI or
stateful device lifecycle changes. Pause/Stop/launch replacement/graph replacement
and outgoing-tail retention remain a subsequent integration contract. Measure
100-source work separately, then define aggregate project admission; one source
being bounded does not establish 100-track device deadlines.

Source acceptance — 8 October: the stateless span/render prerequisite is
implemented in hex-engine with a separate control-side density module. Queries
use release-extended indexed intervals and direct exact frame lower bounds.
Preparation admits 64 simultaneous raw gate/release intervals; the 192-hit
bound, balanced height-17 trees and at most 327 trees bound traversal below its
16,384 guard. A separate 326-repeat-window admission bounds period work.
Source seam admission deliberately overestimates separated intro/repeat voices.

74 engine tests pass; final root 7 instrument and 3 density checks, strict engine
Clippy/fmt, guards and desktop build pass. Finite PCM comparisons include true
intro/repeat seams, tempo change, one-frame/31-frame/256-frame partitions and
late seek reconstruction. The 100-source held-voice microbenchmark is recorded
in performance.md. It does not satisfy aggregate runtime admission, listening,
physical callback deadlines or Session workflow parity.

Next contract: bound aggregate graph work and define canonical Session slot,
launch/replacement/cancellation and transport ownership before runtime wiring.
Reuse prepared immutable publication/retirement, preserve finite Arrangement
semantics, and specify removed-source release-tail behavior explicitly.

## Canonical Session ownership — active implementation

First coherent unit: independently owned MIDI Session scenes/cells, source
editing/history and schema-11 persistence in the existing project/storage crates.
Factor reusable source metadata and notes into MidiSource. Finite MidiClip keeps
Arrangement placement; SessionMidiClip owns its own clip ID, name and source
without timeline start/length. Scenes use globally allocated SceneIds. Sparse
cells are keyed by track/scene and distinguish MIDI clips from explicit stop
cells; absent cells remain empty. Empty/stop launch semantics require the
installed-Live experiment and are not inferred from the stored representation.

Create, copy, move, duplicate, clear, scene/name and source commands participate
in ordinary transactional history. Copying allocates independent clip/note IDs;
moving preserves them. Occupied destinations are refused until explicitly
cleared or replaced by a documented operation. Existing MIDI note commands and
batch edits resolve sources under either ownership. Removing a scene/track
removes its owned cells; Undo restores content without reusing consumed IDs.
Shared project clip/note limits cover both ownership sets. Scene/cell caps are
format/control safety limits, not callback performance promises.

Snapshot validation must cover global IDs, ownership, unique cell keys,
instrument-track restrictions, source/note bounds and all caps. Schema 11 stores
Session separately while retaining Arrangement field shapes and defaults;
schemas 1–10 migrate to an empty grid. Acceptance includes independent copy
edits/deletion, generic note batches, refusal/history/allocator invariants,
round trips, literal migrations and malformed strict-schema cases. A narrow
source-oriented engine preparation adapter allows Session reuse while preserving
the Arrangement wrapper and finite compiler behavior.

This unit does not make Session playable. Prepared Session inventory/publication,
aggregate admission, launch quantization, transport/source retirement and compact
canonical UI remain required next stages. Do not extend the demo runtime or
publish an unprepared Session graph on the audio callback.

Source acceptance — 8 October: the independent ownership unit is implemented,
including shared source values, cross-owner notes/batches, sparse cells, stable
scene IDs, transactional commands/history and schema 11. Combined clip/note
limits and global ID validation cover both locations. Schemas 1–10 migrate an
empty Session grid. PreparedMidiSourceTemplate keeps its Arrangement wrapper and
adds prepare_source for sources without finite placement. Workspace tests,
strict Clippy, desktop build, fmt/guards pass; root acceptance passes 9 model,
8 persistence and one source-adapter test. Native Session/runtime remains open.

Next required work is tighter first-tail voice admission, then prepared Session
inventory/publication and aggregate work metadata. Quantized launch/transport and
bounded outgoing sine releases need explicit sample-exact tests before UI wiring.
Per-scene maxima alone cannot bound arbitrary mixed per-track launches; aggregate
admission must cover their reachable combinations and replacement tails.

## First-tail voice admission and prepared Session publication — active

Repair the remaining polyphonic loop refusal with a bounded first-tail sweep.
During the intro tail, only nonnegative repeat iterations exist; the bi-infinite
periodic maximum must not be added to intro voices as though earlier repeated
tails were already present. Keep separate intro and steady-periodic maxima,
and compute overlap across the entire finite intro-release horizon. Independent
tiny-domain sweeps and the earlier delayed-onset overload regression are required.
The source-start-0 32-note four-beat chord must prepare and render across first
and later seams with exact finite PCM agreement. Control-side occurrences and
allocation need a mechanical bound, not replaying unbounded history.

Next publication contract must prepare Session inventory off-callback and before
canonical command/Open/Undo/Redo commit, preserving the exact project/graph pair
on refusal. Aggregate work must cover independently mixed per-track launches.
For the original stateless sine, investigate a fixed Copy-voice outgoing bank so
cut notes retain sample-exact release without holding entire retired graphs;
stateful plugin lifetime remains a separate requirement. Launch/transport UI
must wait for that prepared inventory, fixed runtime storage and transition tests.

First-tail source acceptance — 8 October: separate intro and steady-periodic
maxima plus a one-sided nonnegative first-tail sweep replace the invalid sum of
intro tails and bi-infinite repeats. The sweep is bounded by admitted periodic
voices and the finite release horizon. Root acceptance: 8 ongoing-instrument
tests, 4 density tests including independent tiny-domain oracle and work-bound
refusal. A 32-note source-start-0 loop matches finite PCM through first/later
seams, including one-frame boundary windows and 31/256 partitions.

The next finite unit prepares one immutable Session inventory and an exact
PreparedRevision before canonical candidate/history/Open commits. Internally
shared MidiSource values use COW to preserve unchanged identity without copying
all notes on unrelated edits; authored snapshot/schema/API behavior stays intact.
Inventory stores project-order tracks, scenes, sparse typed cells, sorted prepared
sources, a stable original-sine kernel, timing and structural work metadata.
Aggregate metadata sums each track's maximum across its independently launchable
cells. Service publication uses the same accepted artifact; Arrangement rendering
and exports remain unchanged. Acceptance covers refusal rollback, source identity,
mixed cell maxima, worker history/open/restore revision coherence and Arrangement
PCM. Session activation/launch UI remains the next dependency.

Future tempo-edit continuity needs frame-anchored active voices, not source pointer
identity alone. Preserve original onset phase anchors; keep already-releasing
voices' gate/end anchors; remap held voices' musical gate through new timing and
clamp at the cut if past. Suppress remapped duplicate occurrences until cached
voices end. This policy still needs bounded storage and sample-exact transition
tests before runtime integration.

Session reference contract correction — 8 October: playback ownership must be
per track, allowing Arrangement and Session sources on different tracks together.
A Session launch overrides only its track; clip stop keeps it silent until an
explicit Back to Arrangement operation. A global exclusive playback mode is not
the final parity contract. Sparse no-op and explicit-stop cells remain distinct.
See research/observations/session-routing-001.md for manual provenance, installed
UI evidence and unperformed timing/launch experiments. Inventory-only work is
unaffected; runtime design/aggregate work must include mixed ownership.

Session inventory benchmark prerequisite — 8 October: a canonical heterogeneous
100-track/200-cell fixture measures cold preparation and real Editor gain/note
transactions plus prepared inventory. Source reuse checks pass; p50 values are
326.833 / 19.792 / 21.500 µs. See performance.md for exact scope and exclusions.
The worker artifact and end-to-end/native latency are not covered by this check.

Later mixed-route runtime prerequisite: extract a borrowed per-track finite
Arrangement rendering seam and share track/master mix stages. Render exactly one
selected source per track; do not render underlying Arrangement then subtract it.
Admission takes each track's maximum between Arrangement and independently
launchable Session cells, plus routing/mixing and outgoing releases. Stop All
Clips changes ownership separately from transport Stop; preserving ownership and
phase on the next Play still needs the disposable reference experiment.

Prepared Session inventory acceptance — 8 October: internal COW and owning
source-identity tokens, immutable prepared cell inventory, timing/kernel-safe
reuse, structural per-track envelopes and exact precommit PreparedRevision are
implemented. All controller mutations/history, including legacy signatures, and
worker initial/import/Edit/Undo/Redo/Open/Restore use full preparation. Export
retains the Arrangement graph; Playback retains the complete accepted artifact
without activating Session. Existing bounded Publication retirement moves the
whole boxed Playback, so inventory ownership follows the established control-side
reclamation path; no CPAL runtime behavior was changed.

Root discriminating inventory/worker/controller/COW acceptance passes 15 tests;
finite Arrangement regressions pass 17, audio boundary tests 8. Executor full
engine/project/session/storage tests and strict Clippy pass. Desktop build, fmt
and both architecture/UI guards pass. Source acceptance does not prove native
Session playback, mixed-route deadlines, stateful plugin lifetime or launch parity.
Next prerequisite is a shared borrowed per-track Arrangement render/mix seam,
followed by explicit ownership/quantization/continuation/retirement runtime tests.

Native inventory-build startup acceptance — 8 October: the refreshed development
bundle opens the canonical editor with `project ready`, `rev 0 applied`, stopped
transport and the actual 44.1 kHz stereo output. No edits or close/discard actions
were performed; retained acceptance-app and Live project states were preserved.
This is startup acceptance only, not Session launch or sustained device evidence.

Current finite render/mix prerequisite: extract borrowed finite track-source
rendering, prepared channel mixing and final master mixing without per-block
allocation. Preserve sample operation order, fades/phase/overlaps, finite MIDI
span order, compiled mute/solo, pause and transactional overflow behavior. Test
mixed audio/MIDI and irregular partitions; compare the independent active-track
scalar fixture, run engine tests/strict Clippy and rebuild desktop. No launch,
route state, CPAL, schema or UI behavior is added in this prerequisite.

Render/mix prerequisite accepted — 8 October: borrowed finite track accumulation,
prepared per-contribution stereo mixing and prepared post-sum master stage are
implemented. Root targeted acceptance passes 29 tests covering track selection,
mix controls, fades/phase, independent scalar clip spans, finite MIDI and global
loops. Executor full engine tests, strict all-target Clippy and formatting pass;
desktop rebuild and both guards pass. The active 100-track scalar fixture retains
digest `a7db7404bf70aac8`; see performance.md for scope and timings. Main native
app remains running and its bundle was not overwritten by this source-only unit.
Next: integrate a bounded additive ongoing MIDI span path with the shared mix
policy before per-track route/launch runtime, preserving the standalone template
render contract and testing independent per-voice mixing and refusal behavior.

Additive ongoing MIDI acceptance checklist: query the whole bounded window before
changing a destination; a refusal clears reused query storage but preserves all
existing destination samples. Add each voice through prepared track coefficients
in span order, with master gain left to the complete mix. Successful muted sources
remain no-ops but still validate their queries. Retain the standalone overwritten
mono/stereo render's current PCM and failed-query zeroing behavior. Verify against
finite Arrangement sources with matching gain/balance, a nonzero destination,
rounding-sensitive overlapping voices, loop/tempo seams and irregular <=256-frame
partitions. Refusal coverage includes width, overflow, rate and tempo admission.
No runtime route/launch, device I/O, schema or UI state is part of this unit.

Additive MIDI source acceptance — 8 October: the implementation and all checklist
items above pass source checks. Root full template suite passes 21 tests, including
independent finite scheduling across tempo/loop seams, nonzero destinations and
an actual overlap-window discriminator for sum-then-scale rounding. Executor full
engine tests/strict Clippy, formatting and both guards pass. Release profiling
also checks a scaled 64-voice ongoing source against finite scheduling before
timing; one of 100 dense replicated blocks exceeds its period. See performance.md
for methodology. Next is explicit per-track routing and bounded launch scheduling,
with whole-block refusal, structural mixed-route admission, voice continuity and
retirement evidence before native Session integration.

Mixed-route runtime design constraints — 8 October: keep the existing wrapped
Arrangement cursor API compatible while tracking an explicit unwrapped Session
performance clock; do not copy the whole mixed output across Arrangement loops.
Commands resolve stable ownership against the exact prepared revision and use
control-allocated fixed route/pending storage. Sparse scenes leave absent tracks
unchanged. Clip Stop/Stop All and Back to Arrangement are separate ownership
operations. Whole-output refusal must preserve transport/routes/pending state,
even when a later chunk fails; query width remains <=256 per source. Account for
any two-pass preflight cost in measurements. Graph edits must not silently discard
active Session routes or rely on source pointer identity for tempo-map phase;
bounded continuation/retirement is an integration gate. Unmeasured loop/Stop/launch
coupling remains in the reference tracker, rather than being labeled Live parity.

Next executable milestone: add an engine-owned `SessionPlayback` kernel over
PreparedRevision, for headless acceptance before existing Playback/native
delegation. Separate monotonic performance time from the wrapped Arrangement
cursor; render mixed routes in <=256-frame queries and retain outgoing voices in
a bounded Copy release bank. No duplicated editable project or demo runtime.
Commands and rendering use construction-time route/pending/scratch storage;
later-chunk refusal zeros requested output and commits no runtime state. A full
release bank refuses a transition rather than dropping voices. Held outgoing
voices release at the cut; already-releasing voices keep their existing anchors.

Initial kernel scheduling is explicit Hex policy: immediate or four-quarter
quantization, exact-boundary application allowed, one pending action per track
with newer requests replacing that track's pending action. A scene updates its
represented tracks atomically; absent cells leave ownership and pending actions
alone. Paused selection must not start transport. This extends the fixed demo's
scene-only pending convention and is not measured Live launch/cancel parity.
Graph-revision/tempo-edit continuity remains the next dependent acceptance gate;
keep current native Playback/publication/priority Stop unchanged until it passes.

Clock review refinement: the canonical PreparedTempoMap is nonperiodic, so a
second sample counter alone cannot repeat tempo changes inside an Arrangement
loop. Treat active-loop/nonconstant-map combinations as an explicit unsupported
kernel case until a prepared loop-aware SessionTempoClock exists. The follow-up
must map intro once and cyclic tempo segments onto monotonic musical beats,
generalize the bounded MIDI conversion seam and prove boundary/rounding behavior.
Include an internal loop tempo change and an outside-loop future tempo point in
tests. This is a required remaining part of the full runtime, not a narrower
completion target or a claim of Live loop-tempo parity.

Headless SessionPlayback acceptance — 8 October: mixed route selection, sparse
scenes, immediate/four-quarter pending actions, distinct transport/clip ownership
commands, monotonic Session time, wrapped Arrangement cursor, bounded outgoing
voice release and one-pass whole-block rollback are implemented. Code is split
into construction/views, commands and rendering modules. Root passes 16 kernel
tests plus 38 related regressions. Full engine tests, strict all-target Clippy,
desktop build, formatting and both guards pass. Final canonical 100-track release
profile retains partition equality/checksum and has zero observed period misses
in its scoped 4,000-block run; no native/deadline/complete-DAW claim follows.
No running native bundle or project state was overwritten. Next implement the
periodic timing seam and tempo/graph-phase handoff before integrating the kernel
into existing Playback/retirement and compact Session UI.

Periodic Session timing implementation — active: prepare checked loop beat/frame
anchors off the callback and expose a borrowed conversion seam to ongoing MIDI
queries. The intro occurs once; the loop's tempo segment repeats on monotonic
Session time, ignoring future tempo points outside that segment. Use center
inverse anchors, not lower-bound anchors that bias each period by half a sample.
The inverse lower bound must include prior-cycle beats that quantize to the next
cycle's first frame. Preserve explicit beat-domain exhaustion and all canonical
finite conversions; no silent approximation of unrepresentable boundaries.

Acceptance requires an independent forward/binary-search inverse oracle,
non-beat-aligned frame boundaries, shared quantized markers, one-frame loops,
large cycle counts and checked overflow. Ongoing notes/gates/releases and
quantized route transitions must remain partition invariant across tempo seams;
whole-block failures preserve state. Retain the constant-tempo profile and add
`profile_session_playback --loop-tempo` for a canonical 100-track workload with
an internal change and a future outside-loop marker. No native Playback, CPAL,
UI or graph-revision handoff is changed in this prerequisite. Repeating tempo
automation for Session is Hex's shared-clock design; native Live coupling remains
an open reference experiment, not established parity.

Frame-anchor boundary policy: canonical latest-boundary `frame_to_beat` selects
both loop beat endpoints. A tempo marker that quantizes exactly onto the end
frame therefore participates in endpoint selection; only markers whose
quantized frames are strictly beyond the end are ignored by the repeating
segment. Shared-frame marker selection is intentional canonical compatibility,
not an assertion of Live tempo-automation behavior.

Next dependent unit: transactional Session revision transfer. Candidate runtime
storage must be prepared off-thread; the callback copies value state from the
old graph and either accepts the whole handoff or keeps the old graph active.
Stable track/clip/note IDs drive remapping. An unchanged source and clock retain
their launch anchors without duplicate voice capture. Timing/source changes
need explicit phase-preserving continuation and suppression of the matching
template occurrence; held musical gates may be remapped, already-releasing
voices retain their frame anchors. Removed/altered active notes release rather
than retrigger; disappearing clips select silence until an explicit launch.

Track removal/reordering also requires ID-based tail transfer, with copied old
mix coefficients for a removed track, not a reference to the retired graph.
Capacity refusal must preserve both current playback and pending publication;
old/candidate graphs must never be finally dropped on the callback. Pending
launch intent and beat targets need a deliberate edit-time policy before coding
the transfer, including sparse scene edits and removed clips. These are proposed
Hex contracts awaiting source tests and reference evidence, not implemented
native behavior. Keep existing Arrangement/export and priority Stop contracts.

Periodic timing accepted — 8 October: the inventory owns prepared Copy cycle
metadata and admits/reuses sources against the effective rounded periodic rate.
The kernel uses borrowed canonical/periodic timing at event boundaries. Shared
start/end markers, offset loops, one-frame loops, inverse minimality/roundtrips,
large cycles, domain exhaustion and periodic quantized launches pass. Valid maps
cannot overflow forward frames within the u64 Q32 beat domain: even 192 kHz at
1 bpm stays below 5e16 frames. Unrepresentable loop endpoints preserve the
Arrangement revision and return a precise deferred Session construction error.

Executor passes all 109 engine tests, strict all-target Clippy and formatting.
Root passes 41 Session-layer tests, desktop rebuild and both source guards.
Constant/changing-tempo canonical 100-track release workloads match independent
finite PCM and irregular partitions; scoped metrics are in performance.md.
Logs: artifacts/session-loop-session-tests.log, session-loop-desktop-build.log,
session-loop-final-constant-profile.log and session-loop-final-tempo-profile.log.
No native bundle or user project was modified by this timing unit. Next implement
the transactional phase/voice handoff above, then publication and native Session
UI integration. Reference parity and physical deadlines remain separate gates.

Active revision-handoff requirements: gain/track-order edits must preserve exact
Session phase without capturing duplicate notes. Tempo/loop edits must preserve
the musical position at the cut while using the new tempo for future onsets and
held gates; keeping the sample counter alone is insufficient. Releasing voices
keep their existing sample envelope. Source edits reconcile stable note IDs and
properties, release removed/altered notes, and avoid chasing newly inserted past
onsets. Clip/track removal must preserve bounded outgoing tails independently of
old graph ownership. Pending actions retain the already-requested per-track
target snapshot and musical beat; removing the target cancels that intent, while
editing a scene does not retrospectively change its queued per-track choices.

Candidate transfer must be all-or-nothing and allocation-free, with prepared
storage and explicit capacity backpressure. Test unchanged/reordered sources,
slower/faster tempo, held/releasing cuts, repeated edits, source and track removal,
copied fresh IDs, pending launch removal/remapping, capacity refusal/retry, and
partition PCM. The first implementation remains engine-owned/headless until its
public handoff contract is accepted; production publication/retirement tests
remain necessary before native activation. Clock anchoring and pre-cut event
representation are under design review rather than hidden approximations.

Handoff control acceptance follow-up — 9 October: stop/retrigger must reconcile
copied held voices as well as queryable current-template voices. Backward Seek
is an explicit discontinuity: validate the base clock first, then clear the edit
anchor, historical onset floor, pending actions and tails atomically. Carry and
route-cut releases share one prepared global bounded bank; track deletion must
not shrink its capacity below retained orphan tails. Test capacity refusal and
retry without either runtime committing partial state.

At a shared quantized frame, retain a due pending action's requested beat and cut
frame, but anchor the actual source phase to the clock's inverse at application.
Do not clamp the shared forward clock or compress historical event plateaus.
This explicit Hex policy requires a rounding-boundary regression and retains
the separate Live reference gate. The gain-handoff workload now also supports
`--loop-tempo`, cutting after the first repeating tempo cycle; compare each
post-edit block with the independent finite compiler before reporting timings.

After headless handoff acceptance, integrate it into the existing native
Playback/publication boundary. The callback must retain a refused candidate for
retry, continue rendering the old revision and processing priority Stop, and
advance applied_revision only after a successful transfer. Reclaim both active
and deferred candidates through the existing control-side retirement path during
shutdown; queue fullness and failed transfer must not cause final graph drops in
the callback. Report transfer refusal distinctly from render/device failure.
Preserve finite Arrangement-only construction and export semantics. Add queue,
retry, Stop and shutdown ownership regressions before exposing compact canonical
Session controls. This is the next dependent unit, not current native behavior.

Revision transfer source acceptance — 9 October: the engine passes 135 tests
(61 unit, 74 integration), strict all-target Clippy and formatting. Root accepts
25 focused handoff tests, all 41 Session-layer tests, desktop build and source
guards. Coverage includes carried phase/held and releasing gates, repeated
occurrences beyond a source seam, fresh IDs/history filtering, moved/deleted
clips/tracks, copied old mix, queued scene snapshots, shared-frame due launches,
paused/Stop/Seek behavior, capacity refusal/retry and whole-block late refusal.
The rollback fixture proves an audible first chunk, complete 512-frame output
zeroing/state preservation after later capacity refusal and exact twin retry.

Gain-edit and unchanged playback profiles retain exact independent finite PCM
and irregular partition digests in both constant/repeating-tempo workloads.
Logs: artifacts/session-handoff-engine-final-tests.log,
session-handoff-engine-final-clippy.log, session-handoff-root-tests.log,
session-handoff-session-tests.log, session-handoff-desktop-build.log and the
four final profile logs documented in performance.md. No native bundle was
replaced by this engine acceptance unit; the separate close-fix refresh remains
the installed development artifact. Continue with the native publication gate
above; complete Live parity, device chains and physical performance remain open.

Native integration active — 9 October: preserve legacy finite Arrangement
construction/export while preparing Session runtime storage on the control side.
A valid Arrangement revision can retain an unrepresentable periodic Session
clock; constructor/capability policy must expose this distinction rather than
silently losing edits or selected Session routes. Resolve that policy before
wiring fallible handoff into publication. The bounded command/Stop path is now
independent of defensive held-retirement backpressure; 15 audio tests and strict
all-target Clippy pass, including forced retention/reclamation coverage.

Native contract selected: keep legacy finite constructors, introduce explicit
prepared Session capability with visible Arrangement fallback, and use the
mixed kernel for capable native graphs after parity acceptance. Preserve dual
transport counters through capability transitions. Refuse capability loss when
active Session owners/pending/tails would be lost. One publication gate bounds
queued/deferred candidates; transient capacity refusal retries, permanent
refusal is retired with rejected-revision acknowledgment, and failed actions
have rejected-ticket acknowledgment. UI waits must resolve on either acceptance
or rejection, so a corrected revision can publish. No Session grid UI changes
belong to this publication unit.

Acceptance: legacy/all-Arrangement PCM over wraps and partitions; native queue
mixed launches/stops/return and tempo/source transfer; explicit unavailable
Session actions alongside valid Arrangement edits/playback; dual counters;
queue/full-gate refusal and retry; command rejection/Stop ordering; shutdown
weak-identity ownership/reclamation. Run engine/audio/Session tests, strict owned
crate Clippy, formatting, source guards and desktop build. Native interaction,
allocator instrumentation and physical deadlines remain subsequent gates.

Native publication source accepted — 9 October 2026: the worker prepares Session
storage off the callback, retaining valid finite Arrangement playback with an
explicit capability reason if Session construction is unavailable. Capable
graphs use the mixed kernel from the outset; legacy/export constructors retain
finite rendering. Publication admits one queued/deferred candidate, retries
release-capacity refusal once per block, retires permanent refusals with coherent
revision acknowledgment, and never reports them as applied. Stop works while a
candidate is deferred or retirement is backpressured. Shutdown returns all graph
ownership to control-side reclamation. UI consumes failures and resets telemetry
high-water marks for a recovered control channel.

Root acceptance: 139 engine tests, 22 audio tests and 41 Session-layer tests,
strict all-target Clippy (engine/audio/session/UI), both source guards. Callback
scene/track-stop/Arrangement-return PCM agrees with the headless kernel across
127-frame partitions and loop wraps, including an untouched Arrangement-only
companion track while the first track owns Session MIDI; independent finite-vs-capable Arrangement
and benchmark PCM oracles remain separate evidence. Legacy monotonic exhaustion
is explicit, and promotion validates the Session beat domain before mutation.
Logs: artifacts/native-session-root-acceptance-tests.log and
artifacts/native-session-root-clippy.log. Fresh native interaction is blocked by
the locked Mac; source acceptance does not establish physical deadlines.

Next coherent UI unit: a compact canonical Session grid with stable scene/track
cell selection, scene creation, MIDI create/copy, typed bounded launch/stop/return
actions and owner-aware shared piano-roll editing. Preserve distinct empty/stop
cells and unsupported audio sources. Render visible rows/columns only, preserve
Arrangement shortcuts and retained scroll state, and guard captured IDs against
project generation changes. Do not infer active routes from a button click or a
consumed ticket; applied-route telemetry is a subsequent contract. Accept source
editing/history/save paths and queue admission tests before native click-through.

Native transfer follow-up — 9 October: callback replacement now has an explicit
tempo/source/gain edit regression in an active mixed project. The callback
matches a reconciled headless kernel byte-for-byte after each edit, including
113-frame device-like partitions, loop wraps and preserved performance time.
All 23 audio tests pass (artifacts/native-session-transfer-audio-tests.log);
the focused regression log is native-session-transfer-callback-tests.log.

Applied-route telemetry — accepted design, implementation active: the engine
exposes copyable stable-ID owner/launch/pending values and an opaque committed
route-change stamp. The stamp changes for committed route/pending changes only;
failed transactional render cannot publish a change. Audio owns fixed atomic
storage for up to 256 tracks, allocated when the channel is created. Its bounded
three-attempt snapshot reader returns a list tagged with the accepted audio
revision, so UI cannot pair old route IDs with a newer desired project revision.
Snapshot publication scans/writes slots only when that revision/stamp changes;
unchanged blocks stay on scalar checks. Accepted graph replacement forces a
new coherent snapshot; deferred/rejected candidates remain invisible, fresh
recovery channels begin empty, and legacy/unavailable states explicitly clear
old route lists. Use all-atomic odd/even sequencing, no unsafe ordinary-memory
seqlock, waiting or callback allocation.

Acceptance before UI consumption: immediate and quantized pending routes,
exact due transition, failed-render rollback, Stop/Seek/Cancel, reorder/removal,
deferred and rejected graphs, recovery, concurrent coherent reads, max-track
bounds and unchanged-block instrumentation. Measure change and unchanged paths
separately and state fixed storage footprint. Session UI remains with its current
executor; telemetry source is a separate engine/audio unit and must pass acceptance
before adding active/queued clip indicators.

Launch/start contract correction: scene/cell launch from stopped transport must
start playback, but two queued commands cannot make failure atomic. Use one
`PlaybackAction::LaunchAndPlay` that accepts only clip/scene launch, applies the
route change and starts transport only when it succeeds. Existing transport and
headless route-selection commands keep their semantics. Cover unknown IDs,
nonlaunch payload, unavailable capability and successful stopped launch through
the callback; UI retains one action under bounded backpressure.

Session grid and route telemetry source accepted — 9 October: canonical mode
switching, paged track columns/virtualized scene rows, stable cell/scene IDs,
MIDI create/copy, explicit stop cells and shared Session piano-roll/source editing
are implemented. Copy preserves the canonical same-track contract; unsupported
audio Session clips remain unavailable. One atomic launch/start action passes
callback rejection/success tests. UI queued transport survives unrelated edits;
Session actions remain generation/revision gated, coalesced revision accounting
loads rejection before acquiring applied state, and bounded Session ticket
feedback resolves on consumption/rejection/Stop/open/recovery/fault.

Root evidence: 140 UI tests (one ignored) in session-grid-root-tests.log;
140 engine, 29 audio and 41 Session-layer tests in session-routes-root-tests.log;
strict all-target Clippy, both policy guards and desktop build logs. The engine
now exposes stable route values and committed change stamps; native status uses
256 channel-allocated atomic slots, three-attempt coherent snapshots and accepted
audio revision headers. Failed/deferred publication retains the old snapshot,
shutdown clears to revision 0, and unchanged blocks avoid snapshot slot scans.
No-allocation is inspected, not allocator-instrumented. Read performance.md for
100/256-track route-control timing and the retained earlier 12.372 ms outlier.
Native interaction/appearance/minimum-layout acceptance remains unavailable on
the locked Mac, and the existing app bundle is preserved.

Next UI work: ticketed launch/play intent must make rapid launch-then-space
ordering correct without letting failed/stale launch intent erase newer transport
intent. Then consume accepted-route snapshots for truthful active/queued cells.
Associate each accepted audio revision with its canonical project view; never
pair the route list from an old audible graph with a newer desired project view.
Retain only bounded read-only snapshots, clear on recovery/open generation
changes, and test rejection/coalescing/reorder/removal. Track labels and queued
indicators must come from matching stable IDs and revisions. Native saved-value,
launch/listening, natural horizontal scrolling and stress work remain gates.

Ticketed play intent — active UI unit: a private pure tracker retains the latest
successfully admitted state-changing action (Play/Pause/LaunchAndPlay) with a
checked UI identity token, desired state and optional callback ticket. Queue
reservation and revision/generation checks precede admission. Dispatch attaches
the ticket only to the matching latest token; stale or rejected earlier requests
cannot clear a newer Pause/Play choice. Consumed high-water resolves a ticketed
latest intent and subsequent toggle uses actual audio state. Unsent latest
intent remains until dispatched or cancelled. Priority Stop clears queued work
and supersedes ordinary intent independently of token exhaustion; open/recovery
and faults clear the tracker. This is UI intent ordering, not an optimistic
active-route indicator. Preserve the existing independent priority Stop policy.

Acceptance: launch→space→space yields launch/Pause/Play; failed/stale launch
preserves later transport intent; earlier consumption cannot clear newer unsent
intent; latest failure returns toggling to actual state; full-queue refusal does
not change intent; Stop/reset/generation transitions remain correct. Keep this
in a focused UI module and test it before consuming route visuals.

Ticketed play intent source accepted — 9 October: root inspected admission,
dispatch and acquired callback reconciliation; all 149 UI tests pass with one
ignored (`artifacts/play-intent-root-tests.log`). Latest matching tokens preserve
rapid launch/Pause/Play order, earlier stale or consumed launches cannot erase a
newer unsent choice, refusal leaves intent unchanged, and Stop works without a
token. Strict UI Clippy, formatting, both guards and desktop build pass. Native
Space/launch acceptance remains pending, alongside the repeated close report.

Active UI unit: revision-matched route indicators. Retain only bounded read-only
canonical snapshots captured with their prepared graph, one audible view and
publication candidates. Consume the fixed route buffer without treating a failed
snapshot attempt as new data. Render selected owners/pending targets only against
matching displayed canonical revision/generation and stable track/clip IDs;
deferred/rejected publication must not become visible. Clear channel/generation
state on open/recovery and distinguish ownership from actual playing. Accept
coalescing, mismatch, reorder/removal, pending/apply/cancel and lifecycle tests
before native visual claims. The audio callback contract remains unchanged.

Ordered priority Stop — active audio unit, separate from route visuals: replace
the independent boolean with a single atomic cutoff marker (0 means no request,
otherwise last admitted ticket + 1). Reserve the largest ordinary ticket for
representable Stop at exhaustion. Publish admitted high-water only after a
successful queue push; callback acquires an entry cut before consuming the marker,
applies Stop first, discards its older prefix, and applies later actions in FIFO
order. A safe rtrb peek leaves commands admitted beyond the captured cut for the
next block. Keep the exact 32-slot/32-pop bound and ownership retirement unchanged.
Repeated Stop after a marker swap remains a fresh request, including the same
cutoff. Consumption acknowledgments remain consumption-only.

Acceptance: older transport/Session requests discarded; Stop→Play/Pause/launch
preserves successful admission order; invalid launch remains stopped; full-queue
Stop and post-drain later commands work; captured-prefix and mid-drain races stay
bounded; refusal does not advance admission high-water; exhausted ordinary
tickets still permit Stop; repeated Stop, shutdown and held-retirement paths
remain safe. No UI optimism, new audio worker, waiting or callback allocation.

Ordered Stop source accepted — 9 October: root review and independent audio
tests pass (18 unit, 16 integration; `artifacts/ordered-stop-root-tests.log`).
The production captured-cut processor is exercised after deterministic mid-block
admissions, proving a post-cut Play remains queued while Stop remains pending,
then applies after Stop at the next actual block. Older Session launch is
discarded, valid later launch starts, and invalid later launch stays stopped with
its rejection ticket. Queue-full/exhaustion, repeated Stop, shutdown and retained
ownership tests pass; strict audio Clippy passes. Native rapid Stop→Play remains
unverified. Preserve the distinction from the fixed demo's older Stop policy.

Accepted-route UI source acceptance — 9 October: exact Event-associated read-only
views are retained with a bounded 32-candidate publication queue plus one audible
view. Failed/deferred graphs do not acquire route graphics. Two fixed 256-entry
scratch/last-good buffers isolate failed reads; channel identity, exact audio/
canonical revisions and project generation gate display. Canonical IDs are
validated once per route stamp, not per cell. Open replacement/recovery/fault/
shutdown clear stale route state; failed open preserves the existing mapping.
Markers distinguish current playing/nonplaying source, queued launch/stop/return
and committed Silence; same-cell retrigger shows current and pending separately.
Root passes 158 UI tests (one ignored), strict UI/desktop Clippy, desktop build,
formatting and both guards. Logs: artifacts/session-route-view-final-root-*.log.
Native appearance, minimum layout, Session listening/save/reopen and repeated
Discard acceptance still require an unlocked Mac. The running app is preserved.

Active next UI unit: expose existing Immediate/FourQuarter launch quantization
with a compact toolbar selector, preserving Immediate as the current default.
Label the fixed musical grid truthfully as four quarter notes, not one bar under
arbitrary meter. Capture the selected setting in admitted clip/scene launch,
track/all-clip stop and Arrangement-return actions; later selection changes do
not alter queued actions. Add typed CancelPending for all tracks; it cancels
queued transitions without stopping the current source. Markers follow accepted
callback snapshots. This selection is currently local UI state, not saved project
metadata. Global/per-clip options, meter-aware bar quantization and exact Live
behavior remain required future work. Test action capture, stale/refused actions
and cancellation, then run normal UI source acceptance before native review.

Reference prerequisite: [SESSION-LAUNCH-001](../research/observations/session-launch-quantization-001.md)
records the reverified installed 12.4.6 hash, public-manual evidence and planned
transport/boundary/meter/loop/override/retrigger/launch-mode experiment matrix.
Native outcomes remain unobserved. Separate known Hex contract from Live parity
before introducing meter-aware or persisted per-clip launch semantics.

Quantization/cancel UI source accepted — 9 October: toolbar cycles none/four
quarter notes. Current selection is sampled at click/admission and copied into
launch/stop/return commands; later changes leave admitted actions intact.
CancelPending uses the same stale/capability/queue checks and leaves current
ownership unchanged. Root passes 159 UI tests (one ignored), strict UI/desktop
Clippy, desktop build, formatting and both guards; logs:
artifacts/session-quantization-root-{tests,clippy,build}.log.
Setting persistence, bar/subdivision/per-clip timing and launch modes remain
future work. Native click-through/listening and the reference experiment remain
pending. A separate `artifacts/hex-session-preview-20261009.app` contains this
source milestone plus the close lifecycle hardening; the running development app
and unsaved state are preserved. Its executable matches the captured desktop
build; artifact identity is in session-preview-20261009-manifest.json. Later
source/build changes do not refresh this preserved bundle.

Completed UI correctness unit: Session Delete/focus targeting. Stop cells have no
selected clip ID, so the old Delete handler skips them; cell/header clicks also
need explicit editor focus after piano use. Resolve occupied-cell deletion via
ClearSessionCell independently of clip selection; empty cells are no-ops.
Scene-header selection clears old cell/clip/note context and Delete resolves to
RemoveScene. Piano-focused Delete remains selected-note deletion, Arrangement
behavior stays intact, and rejected admission preserves selection. Use fresh
canonical stable IDs and existing editing/dialog gates. Accept MIDI/Stop/empty/
scene-only/stale/focus/refusal tests and normal UI checks before native claims.
Scene duplication/moving and clipboard behavior remain separate work.

Delete/focus source acceptance — 9 October: occupied MIDI/Stop cells resolve to
ClearSessionCell; empty/stale cell selection never falls through to scene deletion.
Scene selection clears stale cell/clip/note context; pointer selection focuses the
editor and rejected edit admission preserves selection. Root's complete UI suite
passes 162 tests with the two close lifecycle tests enabled (one ignored); strict
UI/desktop Clippy, build, formatting and guards pass. Native focus/Delete remains
unverified while the Mac is locked.

Next UI unit — Session keyboard navigation and selection launch. Arrow keys move
by canonical track/scene order with clamped boundaries, keeping the selected cell
visible across horizontal pages and scene scrolling. Scene-only selection remains
scene-only for vertical movement. The scene launch column belongs at the right
of the track grid; Right from the final track enters that scene column, Left
returns to the final track, and the outermost boundaries clamp. Empty/stale
IDs must not dispatch Arrangement edits. Piano focus retains note editing and
Arrangement shortcuts retain their existing behavior. Enter launches the selected
MIDI/Stop cell or scene using existing generation/revision/capability/queue gates
and the selected quantization; an empty cell is a no-op. No new engine/schema or
launch semantics. Accept boundaries, stale IDs, empty grid, focus dispatch and
admission/refusal checks before native interaction claims. Scene duplicate/move,
clipboard and full quantization parity remain separate units.

Close lifecycle follow-up — 9 October: finite acceptance covers retained editor
ownership, untyped bootstrap-window completion, Cancel retaining unsaved state,
Discard removing the window, duplicate-close suppression and Cmd-Q. Two GPUI
test-platform lifecycle regressions and the full feature-enabled UI suite pass
(162 tests, one ignored), with strict Clippy, desktop build and source guards.
Native Cancel/Discard remains an open gate while the Mac is locked. No running
app was replaced and no user's project was discarded.

Worker Session persistence workflow accepted — 9 October: authored independent
Session MIDI sources/notes, Stop cell and Arrangement companion pass canonical
worker deletion/history, Save As, Save Current and fresh-service reopen. Exact
project/IDs/source fields and prepared inventory survive; explicit relaunch PCM
matches across irregular partitions through 120,001 looping frames and 180,001
one-shot frames. A bounded cancel test advances beyond the original quantized
deadline and retains the old owner/PCM. Root passes all 42 Session-layer tests
and strict all-target Clippy; logs: artifacts/session-workflow-root-{tests,
clippy}.log. This closes the headless worker workflow gap, not native save dialogs,
physical callback admission/listening or Live reference parity.

Next canonical Session organization unit: atomic scene duplicate/reorder. Add
DuplicateScene using a stable source ID, inserting the complete copy immediately
after its source with a fresh scene/clip/note ID set; preserve names/source values,
track association, explicit Stop cells and structural empty slots. Copies remain
independently editable. Add MoveScene using stable scene and before-scene IDs
(None appends), preserving cells/IDs and rejecting missing targets; moving before
self is a no-op. Both use the existing candidate/precommit compile/history path,
bounded scene/cell/clip/note limits and schema-11 representation. No new schema,
runtime ownership, active launch, clipboard or capture behavior. Accept rollback,
limits/ID exhaustion, full history and worker compile/save/reopen cases before UI
integration. The [Live Session manual](https://www.ableton.com/en/live-manual/12/session-view/)
confirms scene duplicate/reorder workflows; exact
installed-reference interaction and timing remain separate unobserved gates.

Keyboard/scene-column source acceptance — 9 October: Session arrows and Enter
use production handlers; stale/empty selection cannot dispatch Arrangement edits,
and piano focus retains note handling. The scene column is pinned at the right
edge, with fixed compact track widths and bounded paging. GPUI test-platform
geometry passes at 880 and 1440 logical pixels; real key dispatch covers navigation
and truthful offline launch refusal. Root passes 170 UI tests (one ignored),
strict relevant Clippy and both guards. Logs: artifacts/session-keyboard-root-tests.log
and artifacts/session-organization-root-clippy.log. Native visual/input/listening
acceptance remains pending.

Canonical scene organization source accepted — 9 October: DuplicateScene copies
all currently supported MIDI note/source fields, Stop cells and structural empties
with fresh IDs and independent sources; MoveScene preserves IDs/cells and uses
stable insertion targets. All model limits and ID exhaustion are preflighted.
History, missing-target refusal/redo preservation, prepared order, save/fresh
reopen and partitioned original/copy PCM parity pass. Root passes 99 project and
43 Session tests plus strict Clippy; logs: artifacts/scene-organization-root-tests.log,
scene-organization-refusal-tests.log and session-organization-root-clippy.log.
MIDI per-note expression is not yet represented, so no expression-parity claim.

Scene UI unit: expose accepted scene duplicate/reorder through existing command
admission. Cmd-D duplicates a scene-only selection in Session focus; selected
cells retain their explicit unsupported duplication path for this unit. Compact
toolbar icons and Cmd-Alt-Up/Down move a selected scene one neighbor using stable
before IDs. Do not reorder during piano focus or Arrangement mode. Reject stale
IDs/dialog/pending edits visibly; preserve selection on refusal. Created-scene
events clear stale cell/clip/note context only while Session is still the active
mode, and reveal the accepted scene. No optimistic audio ownership, clipboard,
cell overwrite, naming editor or capture behavior. Accept source planning,
production GPUI action dispatch, refusal, history integration and normal checks
before native claims.

Scene UI source acceptance — 9 October: compact icons and production key bindings
dispatch scene-only duplication/reordering through the canonical worker. A bounded
duplicate-selection intent selects the copy only if its source remains selected;
newer navigation and mode changes retain their selection. Worker failure consumes
the intent so later scene creation remains independent. Reordering reveals the
same scene ID after accepted state arrives. Full feature-enabled UI suite passes
176 tests (one ignored), strict Clippy/build/format/guards pass. Root independently
passes 13 navigation/resolver/GPUI tests in artifacts/scene-ui-root-acceptance.log.
Terminal queued-worker Busy/Closed rejection clears the corresponding duplicate
intent. The cleanup helper has focused regression coverage; no deterministic
closed-Service integration seam is exposed. Native
visual/input/listening acceptance remains open.

Session audio implementation unit: the previously accepted Session cells contain
MIDI or explicit Stop only. Trace the owning model, persistence migration,
resident media preparation, compiled inventory, mixed render/handoff and UI
selection contracts, then specify independently owned unwarped audio sources.
Require model/history/migration/save/reopen coverage and deterministic launch,
one-shot/Stop/return-to-Arrangement PCM across partitions before UI admission.
The public Clip View manual requires Warp for audio looping; do not substitute
frame looping for that workflow. Record this constraint and native experiments
in research/observations/session-audio-001.md. Warped looping, clip envelopes
and reference timing parity remain separate requirements;
do not encode Arrangement placement as a second Session clip model.

Session audio contract review — 9 October: read-only owning-source review is
complete. Chosen model, typed frame-versus-beat launch anchors, post-end ownership,
Seek/Stop/handoff semantics, prepared PCM lifetime, schema-12 migration and full
worker/UI acceptance are in [session-audio.md](session-audio.md). One coherent
code executor is implementing this across the existing crates. No Session audio
source acceptance or native parity is claimed yet; verify every staged gate
before marking the feature accepted.

Implementation checkpoint: root passes 102 project and 23 storage tests plus
strict Clippy for audio-source metadata/history and schema-12 persistence.
Prepared engine integration is implemented and the executor's engine suites
pass; root review requires exact admitted quantized onset and sounding mixed
MIDI/audio oracle cases before engine acceptance. Worker/UI work is ongoing.
The original native/reference fixture is generated and independently validated
under artifacts/session-audio-reference-fixture.*; no native capture is claimed.

Integrated root checkpoint — 9 October: all affected project/storage/engine/audio/
Session/UI suites pass with GPUI test support (181 UI passed, one ignored).
Exact admitted quantized onset, sounding additive MIDI/audio/Arrangement oracles,
frame-domain source edits and PCM replacement, and the source row remaining
visible during pending edits are covered. Source implementation now includes
fresh-runtime publication on successful Open and guarded source callbacks.
Additional full-retirement-queue/Stop regression passes without forced handoff
refusal. Fresh-service worker audio save/reopen PCM, Save Current and failed-import
rollback acceptance now pass. Source nudges
now use approximately 1 ms by default and Alt for one-frame precision, with
boundary clamping; four panel tests and strict targeted Clippy pass. Native input,
visual/listening, allocator instrumentation, streaming and 100-track physical
deadlines remain open. Existing short resident-media limits still apply.

Performance prerequisite in progress: the 100-track original shared/distinct-PCM
fixture is authored, with two independent audio sources and an Arrangement
companion per track plus Stop cells. Root review distinguishes its active source
working set from resident bytes and requires advancing active render samples,
correct relaunch-relative oracle anchors and fixture-scoped Arrangement checks.
Shared/distinct preparation, edit, route and render profiles now pass independent
PCM checks, with timings and preserved outliers in performance.md. These remain
offline short-source measurements. A near-limit scene grid exposes full-cell-scan
scaling (32 actions: 1.184 ms median). The control-side scene index now preserves
canonical ordering and sparse/empty behavior and reduces the identical dense burst
to 0.391 ms median; engine and root audio/Session boundary regressions pass. Physical callbacks, native interaction and long-song streaming remain
required; the small fixture does not discharge those gates.


Next finite acceptance unit — prepared callback allocation instrumentation:
use a pinned dev-only per-thread counter through safe APIs, retaining the workspace
unsafe prohibition. Require zero total and signed-net counts/bytes for actual
callback entry/render scopes, independent active PCM and command/retirement lifetime
checks. Root calibration passes debug/release, including pure preexisting-object
destruction; workload acceptance is pending. Control construction/admission/oracles/
reclamation remain outside the instrumented callback scope. Normal desktop
dependencies exclude the counter, enforced by the architecture policy. See
performance.md for exact scope and the physical/native acceptance still required.


Allocator gate source checkpoint: five callback workload cases pass in debug/release,
with root-strengthened MIDI PCM/no-rejection checks and strict all-target audio
Clippy. Measured entry/render scopes have zero total and signed-net allocation
counts/bytes; calibrated counters also detect final destruction. Canonical audio
PCM, full command bursts, real retirement-ring Stop and publication backpressure,
fresh installation and shutdown control reclamation are covered. This is scoped
headless allocator evidence. Next complete quantized/dense/rejection/fault and
backpressured-shutdown cases, then physical callback/native workflow gates and
the streaming/media architecture needed for long 100-track songs. No production
DSP change or deadline guarantee is inferred.


Bounded media reader checkpoint — 9 October: the control-side WavFrameReader
shares header and sample validation with resident import, reads bounded u64
source intervals and rejects ordinary descriptor mutation. All 25 media tests,
strict media Clippy and full Session regression tests pass. A sparse original
17-million-frame stereo PCM16 file proves far reads beyond existing resident
limits without complete payload allocation. See docs/media-streaming.md and
artifacts/wav-reader-acceptance-manifest.json. Next implement globally phased,
halo-correct incremental conversion, then immutable page identity/cache ownership
and Arrangement/Session/export integration. Existing native import limits remain.


Incremental conversion checkpoint — 9 October: ResamplePlan shares resident v1
weights and global-index phases, queries a clipped source halo and renders into
caller output. WAV conversion uses bounded caller scratch and checks reader
metadata/capacity before payload reads. All 33 media tests/strict Clippy pass;
frozen pre-refactor golden fixtures include both 44.1↔48k directions, downsample,
extreme ratios, identity and tiny sources. Root independent old-v1 comparison
passes 2,700 random/first/last windows per debug/release build across nine pairs.
Full Session regressions pass. See window-conversion-acceptance-manifest.json
under artifacts. Required next: shared immutable rate-pair kernel preparation,
source/page identity, bounded cache/scheduler/ownership, then production playback
and export integration. Long-project scalability remains incomplete.


Shared kernel checkpoint — 9 October: explicit ResampleKernel handles share one
immutable rate-pair table across 100 tested plans with independent source counts
and limits. All 38 media tests/strict Clippy pass; 2,700 release frozen-v1 windows
across nine pairs remain exact, and full Session regressions pass. There is no
global cache or render-time refcount churn. Next implement bounded cancellable
source-page fingerprint verification using exact verified bytes for parser,
decoder and conversion; then aggregate cache/generation/worker delivery.


Verified source checkpoint — 9 October: SHA-256 whole-file and 64KiB raw-page
manifest readers validate exact bytes before parsing/decoding and detect accessed
same-size/restored-mtime changes. Cancellation and errors clear output; descriptor
and lexical path are retained. All 47 media tests/strict Clippy, independent debug/
release 24-bit boundary/hash/edit checks, full Session suite, desktop/workspace checks
and guards pass. No sidecar/schema/production playback integration yet. Next finite
verticalslice must exercise actual bounded worker/page request-ready-return queues
and borrowed engine rendering, not another metadata-only helper; then integrate
native publication/lifecycle and persist source identity.


Next implementation contract is [page-delivery.md](page-delivery.md): add the real
hex-stream worker/cache boundary with borrowed engine page rendering, beginning
with fixed 4,096-frame pages and 1,024 buffers (32 MiB PCM payload). Account for old/
candidate/retired ownership and explicitly admit mandatory page demand. A smaller
320-page pool cannot cover the adverse 100-track old/new warm-window transition.
Prepare playback before publication; cold seek/launch prewarms before command
admission and exposes pending state. This is an implementation target, not current
native behavior. Actual worker/queue/engine PCM must pass before native wiring.

Engine checkpoint — 9 October: generation-scoped streamed source metadata and
borrowed page rendering are implemented alongside resident clips. Root debug/
release scalar acceptance matches 12,895 frames per warm/missing case through
page/loop seams and source phase; unrelated resident audio and clock progress
survive a miss. Provider-less playing render refuses streamed content before
mutation; paused and overflow contracts pass. Resident distinct-source release
profile retains its PCM checksum. See stream-engine-independent-manifest.json
under artifacts and performance.md for scoped timings. This does not exercise
worker IO or prove native streaming.

The real hex-stream worker/cache is in progress. Its acceptance includes fixed
ownership queues, collision-safe lookup, pinning, full-pool forward progress,
coalescing after deletion, render-side scheduling prohibition, complete float
validation, admitted conversion halos and actual shutdown accounting. Root has
prepared an independent real-worker harness using the original signed24 WAV and
the retained frozen v1 converter. Run it after the worker compiles; then complete
the 100-independent-long-source workload and targeted suites before native
integration. Keep canonical/Session lifecycle and callback readiness as the next
required dependencies, rather than treating the engine checkpoint as completion.

Headless worker acceptance — 9 October: the real hex-stream crate now owns
verified catalogs, explicit shared kernels, fixed 4,096-frame pages, bounded
request/ready/return/failure queues and collision-safe cache/coalescing indexes.
One staging buffer preserves full-cache progress. Backward-shift deletion avoids
dead-entry accumulation through long turnover. Eleven stream tests and root
debug/release frozen conversion/render checks pass; source/resource refusal,
restored-mtime mutation, distant float NaN, cancellation and pressured shutdown
are covered. The 100-track 17M-frame sparse-source profile passes with zero
warmed misses and all 1,024 buffers accounted after joined shutdown. Its exact
window timing and storage/cache limits are in performance.md, with source/log/
executable identity in artifacts/stream-delivery-final-manifest.json.

Root consolidates resident/MIDI helpers across both renderer paths and caches
stream presence on construction; full engine/stream/audio allocation suites and
strict Clippy pass after cleanup. Session/media regressions, final desktop build,
six close lifecycle regressions and policy/style/format checks pass. Native
resident limits stay enforced. Next finite unit is one canonical resident/
streamed compiler resolver; the following control/callback split, source leases,
Session source rendering and persisted identity remain required. The complete
sequence and mismatch/PCM acceptance are in page-delivery.md.

Canonical Arrangement bridge — source accepted, 9 October: one mixed source bank
resolves resident PCM or verified streamed metadata through the existing compile
loop. Preserve resident entry APIs and unused history entries. Require exact
referenced canonical asset/generation/rate/count/version matches; the catalog
owner verifies fingerprints. Acceptance includes mismatch refusal, resident
wrapper parity, solo/mix, MIDI preview/table sharing, canonical split phase and
fades, page/loop/EOF seams, irregular render partitions and cold-miss behavior.
Root additionally exercises the real worker against frozen v1 converted PCM.
This does not admit streaming into PreparedRevision or native Session playback.
Control/callback split, catalog leases and prewarmed publication remain next.

Five focused compiler tests and full engine regressions/strict Clippy pass.
Session/audio allocation-probe regressions, desktop build and format/policy/style
guards pass. Root real-worker debug/release checks match 60,452 PCM frames and
identical MIDI previews against resident wrappers/frozen v1 conversion, with
zero warmed misses and all eight buffers reclaimed. Three explicit loop-boundary
seeks cover continuity; linear Renderer does not automatically wrap transport.
Compiled loop metadata is preserved; streamed Playback loop scheduling remains
a required integration dependency. Provenance is in
`artifacts/stream-canonical-acceptance-manifest.json`.

Next implementation unit — stream control/page ownership split (9 October):
separate the joining worker/catalog owner from a page endpoint with only bounded
cache/queue operations. Keep the existing headless Runtime facade working.
Shutdown must receive the matching returned endpoint, join on control, and
account for all buffers despite full ready/return queues. Do not duplicate a
pool per revision or expose concurrent producers for the page request queue.
Test owner mismatch, premature control teardown, source generation reuse and
page turnover; instrument render endpoint operations for allocation/deallocation
with setup and destruction outside the measured scope. Native integration,
graph-attached leases and prewarm publication remain required dependent work;
the split alone must not claim that descriptors survive arbitrary control Drop.

Control/page split — source accepted: debug and release pass eight unit tests,
one real-worker render allocation probe and six worker integration tests; strict
stream Clippy with/without probes and workspace format/style/architecture guards
pass. Private endpoint probes include successful requests, failure retry,
installation/eviction under full return-ring pressure and pins, with allocator
sensitivity calibration. Root split actual-worker checks match 60,452 PCM frames
and reclaim eight buffers. A second root fixture uses identical catalog keys in
two runtimes: wrong-pair shutdown returns owners unchanged, correct subsequent
shutdown reclaims all 16 buffers. Provenance:
`artifacts/stream-control-split-acceptance-manifest.json`.

The next engine unit is shared resident/streamed resolution in PreparedRevision
and Session audio inventory/rendering, with provider-less streamed refusal and
exact phase/handoff/clock/mix semantics. Native callback publication remains
resident until automatic streamed loop scheduling, endpoint retirement/catalog
lifetime and separate bounded prewarm readiness are implemented. A metadata Arc
is not a descriptor lease; persisted fingerprints/reopen validation remain open.

Streamed Session engine unit — in progress: reuse the canonical source resolver
for both PreparedRevision outputs and Session one-shots. Replace the resident-only
Session source accessor with explicit source variants; no placeholder PCM. Share
page-span contribution logic with Arrangement, preserve frame-anchored launch/
Seek/Stop/handoff semantics, and add page-aware mixed/finite playback through the
existing loop scheduler. Provider-less playing graphs refuse before changing
output/clocks; paused/empty behavior remains unchanged. Cold misses affect only
the missing source, with transactional route/clock commit and bounded requests.
Native publication must return streamed candidates before admission until its
page endpoint, source lifetime and prewarm readiness exist. Verify resident suites,
metadata mismatch/handoff/page/loop/EOF cases and independent actual-worker PCM;
then record remaining source lifetime/persistence/native gates.

Streamed Session acceptance checkpoint: shared resolver/inventory/page rendering
now passes the engine suites and independent actual-worker debug/release PCM
comparison (110,758 frames, actual Arrangement wraps, launch/Seek/Stop/handoff,
zero warmed misses, eight buffers accounted). Native Control rejects streamed
candidates before publication, including Session-only sources and both runtime
policies; full audio allocation-probe regressions pass. The stream endpoint probe
also measures immediate Session launch plus page-boundary/EOF rendering with
zero total allocations and zero signed-net allocations/bytes. Strict audio/stream
Clippy passes after factoring span coordinates into a typed helper. Focused
follow-up reviews transactional late failures and malformed page coverage before
final acceptance. Native source leases/prewarm/persistence and device acceptance
remain open; resident native import limits remain in place.

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

Native streaming ownership/readiness — active contract work (9 October):
StreamControl currently cancels and joins on Drop; metadata Arc retention cannot
keep sources alive. The next implementation must retain the actual catalog
lifecycle owner across accepted graph/export/page-endpoint lifetimes and reclaim
it outside callbacks. Explicit shutdown must reject live references without
losing either owner or endpoint. Do not substitute detached worker cleanup for
joined pool accounting.

Acceptance requirements: premature controller/UI generation release keeps leased
sources usable; source generations cannot alias; graph retirement/full queues and
shutdown preserve ownership; last resource release/join is control-side; bounded
control prewarm uses a separate producer and readiness acknowledgment; callback
retains sole request/cache ownership; missing startup/seek/launch pages cannot
silently skip initial transients. Native admission refusal remains until this
whole publication contract is integrated. A focused read-only lifecycle review
is selecting the exact first implementation boundary. The full DAW roadmap,
including sustained 100-track/device workloads and Live feature parity, remains
unchanged.

Selected first ownership implementation: hex-stream real shared catalog owner,
held by StreamControl, callback pages and opaque control-created CatalogLease.
The owner retains the actual worker handle/catalog receiver; dropping the
controller must not cancel sources still held by graphs/pages. Final owner
release/join is off realtime. Matching shutdown refuses outstanding graph leases
with both owners intact before cache/queue mutation. Lease source coverage checks
full immutable facts, including generation/fingerprint; identical source keys in
different runtime pairs cannot satisfy pairing. Verify real-worker continued
rendering after controller release, endpoint release with retained graph lease,
busy/mismatch recovery and final control-side join, then existing allocation/pool
regressions. This is a required prerequisite, not native publication acceptance:
audio Publication/resource retirement, export retention and separate prewarm
installed-page acknowledgments remain the dependent units.

Native integration refinement: off-realtime joining must also stay off the UI
interaction thread. Audio Control::reclaim currently runs during UI ticks, so
final endpoint/catalog retirement must be transferred through a bounded media/
audio lifecycle worker before the last owner is released. Synchronous headless
shutdown remains valid for tests and jobs. Device-fault teardown cannot wait for
another callback; lifecycle disposal must account for callback closure resources
or retain them in the existing bounded quarantine until safe.

### Real catalog ownership accepted; implementation paused — 9 October

StreamControl and callback pages now retain the actual shared catalog owner.
Ready catalogs provide opaque CatalogLease handles with verified sorted full
metadata membership; fingerprints/generations must match and separate runtime
identities remain distinct. Control Drop no longer cancels a worker retained by
pages/leases. Matching shutdown refuses outstanding leases before mutation and
returns both halves intact; dropping leases permits retry and exact pool
accounting. Final owner release stops/wakes/joins off rendering.

Debug/release stream suites pass nine unit, one allocation integration and nine
worker integration tests. Strict Clippy, workspace formatting and architecture/
UI style guards pass. Independent debug/release checks match 110,758 Session PCM
frames, preserve cache/stats through Busy recovery, reclaim eight buffers, reject
wrong full metadata and distinguish identical-key catalogs; an uncached real page
read succeeds after controller Drop. Private test observation confirms final lease
release joins on the named control reclamation thread after endpoint/controller
release. Provenance: `artifacts/stream-catalog-lease-acceptance-manifest.json`.

This is headless ownership acceptance. Native graph/export lease attachment,
callback endpoint retirement, device-fault teardown and non-UI final reclamation
remain required; prewarm installed-page acknowledgment and sustained native
streaming are also open. Ordinary endpoint/lease final Drop is not realtime-safe.
The user requested a pause after this unit to redirect work toward quicker
practical usefulness. No further implementation or prewarm/native admission
work was started; preserve the full roadmap for that discussion.

### User-guided usability priority: left browser — 9 October

The user redirected work toward immediate usefulness and requested a Live-style
left browser on startup. Implement only this selected UI workflow; prior streaming
backlog remains retained, without automatic continuation into it.

Acceptance: browser opens by default in Arrangement and Session; compact nav/results
use existing palette, lowercase text and Ioskeley Mono; collapse/expand and bounded
width resizing preserve timeline space; ruler/Session paging uses actual workspace
width. Real folders/WAV samples load off the UI thread with bounded enumeration
and stale-result rejection. Search edits its own native text field without sending
transport/edit shortcuts. Double-click imports through existing session commands;
available instrument/project actions remain truthful. Native folder picker and
visible loading/empty/error/truncation states are required. No sample audition,
recursive indexing, factory packs/plugins, drag-to-timeline or persisted places
are promised by this unit. Verify library/UI tests, source/style guards, desktop
build, then native visual and folder/search/import/collapse acceptance.

Browser implementation checkpoint — 9 October: source and targeted checks pass;
native startup/layout and final placeholder contrast inspected. The user will
perform native interaction acceptance while present. Track results in todo.md
and docs/issues.md; do not resume the broader roadmap without user direction.

## User-selected hybrid tracks — source accepted, 9 October 2026

All canonical tracks accept both audio and MIDI clips. This replaces the
exclusive audio/instrument track restriction; it is an intentional deviation
from Live. Arrangement tracks may own both clip collections with the existing
frame/beat anchors. Session cells remain one clip per track/scene, but adjacent
cells on the same track may be audio or MIDI. Existing mixer controls and stable
track identity apply to either source. MIDI uses the existing original sine
renderer; device chains and instrument selection are separate future work.

Acceptance:
- Create, move, copy, remove and edit audio/MIDI clips on the same track; failed
  commands preserve project/history/IDs and all current count limits.
- Save/reopen complete hybrid Arrangement/Session state and explicitly migrate
  older audio/instrument projects without losing sources, IDs or mixer settings.
- Compile/render simultaneous Arrangement audio+MIDI on one track, and switch
  Session ownership between both kinds without stuck voices or stale source audio.
- Preserve prepared graph/release admission and off-callback ownership rules;
  cover mixed track callback scopes with allocation probes where affected.
- Expose both audio import and MIDI creation on every track in the compact UI,
  with source-based editing/selection and no type-gated dead-end controls.
- Run affected model/storage/engine/session/UI tests, strict lints, architecture
  and style guards, desktop build and relevant render/allocation checks.
- Give the user a short native checklist; they own UI testing while present.

Deferred: VST/device chains, recording, new routing, browser drag/preview and
changes to overlap or audio/MIDI timing policy. The broader roadmap remains
paused outside this user-selected deliverable.

Hybrid acceptance checkpoint: unified model, schema-13 writer/strict legacy
migration, same-track Arrangement/Session playback, selected-track WAV import
and source-based native controls are implemented. Workspace tests/lints/build,
190 GPUI support tests (one ignored), guards/fmt and independent debug/release
PCM/persistence acceptance pass. Mixed-source headless callback allocation
regressions pass in both profiles. Review bundle and user-native checklist are
in docs/hybrid-tracks.md; listening/device deadlines and user interaction remain
open. Broader work remains paused.

## Arrangement drag landing preview and MIDI moves — 9 October 2026

User-selected usability scope: replace the floating cursor ghost with an exact
in-track landing preview for Arrangement clips, and enable MIDI clip dragging.
One coherent UI executor owns implementation. Keep canonical audio frame/MIDI
beat anchors, hybrid same/cross-track destinations, original pointer grab offset,
existing snapping and validated commands/history. No project edit before release.

Acceptance: preview bounds/track must match the committed destination; viewport
cropping must preserve the true start; only the landing preview follows the drag;
invalid/off-lane/stale/busy/Escape paths must leave no edit or stale preview.
Audio/MIDI moves commit one undoable command. Preserve trim and piano-roll note
gestures. Run targeted UI regression tests/lints, desktop build and source guards.
User owns native interaction/listening acceptance; preserve running unsaved apps.
Deferred: new autoscroll, overlap policies, browser drag, multi-clip gestures and
other roadmap work.

Source acceptance: shared typed audio/MIDI drag and row-local landing preview
implemented. UI suite 192 passed/one ignored; final snap-off exact-frame repair
passes focused musical-time 8/8 and drag 11/11. Strict UI lints, desktop build,
fmt and both guards pass. Review bundle and user-native checks are recorded in
docs/clip-drag.md and todo.md. No native gesture acceptance claimed.

## Drag-start repair and persistent browser places — 9 October 2026

User reports that clips cannot be picked up in the landing-preview build. Fix
this regression before building on the gesture code. Acceptance requires actual
rendered GPUI pointer dispatch for audio and MIDI pickup/preview/drop, alongside
geometry tests; native user confirmation remains separate. Preserve running
review apps and their unsaved projects.

Next requested workflow: added browser folders survive app restart. Store bounded,
versioned application preferences separately from projects and library listings;
restore original paths/order without modifying casing, duplicating defaults or
forgetting unavailable drives. Disk work belongs on a background/control path.
Missing config means defaults; malformed/unwritable config needs a visible error
and preservation of the existing file. Save success must be observable before
claiming persistence. Test roundtrip, duplicates/capacity, restart loading and
failure preservation using isolated fixtures. Keep writes ordered/coalesced and
bounded. Deferred: preview audio, recursive indexing, folder removal/reordering,
other preferences, and broader roadmap.

Next-build acceptance checkpoint: drag preview clearing repaired and current-paint
lane eligibility verified; rendered pickup/drop and MIDI second-pickup after
simulated worker completion pass. Browser added places persist through the
hex-preferences boundary with background coalesced writes and explicit failures.
8 preferences/197 UI tests pass, one ignored; affected strict lints/build/guards/
fmt/diff checks pass. Combined review app and user checklist are in
docs/browser-places.md. Native acceptance remains open.

## Browser WAV drag and free/locked clip preview — 9 October 2026

User accepts both previous restart/drag workflows. Next: dragging a browser WAV
shows a cursor-attached clip outside track lanes, switches to the same exact
in-track snapped landing preview on valid hover, and imports onto that target
track/start on release. Ordinary clip drags should likewise become floating
outside lanes and lock inside. Keep one visual preview at a time.

Preserve audio frame/MIDI beat anchors, source paths, validated import/move
commands, undo/history and current limits. Bounded waveform/duration preparation
runs in background; no decoding on UI/callback. Pending data must be truthful.
Preview/drop target agree; cancel/off-lane/stale/busy drops produce no edits.
Rendered tests exercise cursor ghost, transition, cross-track drop and cancellation,
then user native acceptance. Outbound file dragging into another app is a
capability investigation in pinned GPUI; do not add unsafe native code or promise
OS drag behavior from an internal GPUI payload. No external app operation/testing
is authorized beyond preparing the drag affordance; user owns manual testing.
Broader roadmap remains paused.

Browser drag acceptance checkpoint: in-window browser WAV drag, conditional
ordinary clip ghosts, background bounded duration/waveform preparation and
canonical existing-track import dispatch are implemented. 200 UI tests pass
(one ignored), preferences/session suites and strict affected checks/build pass.
Review and native checklist in docs/browser-drag.md. Outbound OS file drag and
held-source pinning are explicitly deferred; broader roadmap paused.

## Browser sample drop creates a track — 9 October 2026

User accepted browser drag and authorized its commit/push; baseline main is
1d892dc. Next browser WAV workflow: hovering between visible track rows shows
a full-width insertion line, hiding the floating and clip-shaped landing
previews. Dropping atomically creates a hybrid track at that position and places
the sample at the snapped horizontal time. Blank space below the last row or
an empty Arrangement creates a bottom track. Existing body drop remains import
onto that track. Browser/ruler/mixer/outside-window are not creation targets.

Canonical stable before-track ID (or end) defines insertion; no competing UI
track model. New track, media and clip are one prepared history operation;
failed commands preserve IDs/project/history/media. Existing import semantics
remain append-at-zero. Track order is already persisted; review save/load and
undo/redo in the same change. Bound all preview/preparation and stale guards.
Rendered tests cover gap/full-line suppression, empty and bottom destinations,
existing-body drop, cancellation and current painted geometry after scroll.
Model/session tests cover order, timing, atomic refusal and history/persistence.
User owns native acceptance. Deferred: creating tracks by moving existing clips,
new autoscroll, OS outbound drag and broader roadmap work.

Track-insertion checkpoint: canonical prepared insert/import and worker request,
painted divider/end/empty targets, full-line suppression and rendered interactions
implemented. Full project/session suites and 205 UI tests pass/one ignored;
affected strict checks/build pass. User checklist and review bundle are in
docs/browser-drag.md; native acceptance remains open. Earlier accepted milestone
was pushed as 1d892dc; this feature is uncommitted. Broader roadmap stays paused.

## Track-creation drag acceptance repairs — 9 October 2026

User reports: empty/bottom append should retain cursor ghost alongside the end
line; hovering bottom UI shifts the workspace; inserted clips land left of the
pointer; existing clips cannot create inserted tracks. Treat prior insertion
native acceptance as incomplete. Fix concrete geometry/overlay bugs first.
Timeline starts at measured body.left; headers are on the right. Distinguish
between-row boundary (line only) from bottom/empty body (line + floating ghost).
Keep hover geometry/layout stable near bottom edge. Verify actual rendered
positions using independent painted lane bounds and root/track body bounds.

Extend Arrangement audio/MIDI dragging to create a hybrid track before a stable
track ID or at end and move the original clip as one compiled/undoable command.
Preserve source track, clip/note/asset IDs, source regions, fades, MIDI beat length
and original grab offset. No copy/import of existing media. Cancellation/failure
retain IDs/history/project. Test order/history/save-load/refusal and rendered
new-track destinations, exact time, dual append visual and bottom layout.
Native UI checks remain with user; no commit/push of these corrections yet.

Acceptance repair source checkpoint: timeline origin and absolute overlay layout
fixed; append retains line+ghost. Canonical audio/MIDI insertion moves preserve
IDs/source track/content and pass compiled history/save checks. Actual lane drop
routes insertion before ordinary move. 209 UI tests pass/one ignored, focused and
independent core suites, strict affected lint/build/guards/fmt/diff checks pass.
Native checklist/review in docs/browser-drag.md. Insertion work remains uncommitted.

## Empty-area highlight and Arrangement overwrite drops — 9 October 2026

User requests subtle full empty-section highlight: zero tracks means full drop
body highlight without cursor-y insertion line; with tracks means highlight below
last row alongside fixed end line and floating ghost. Dividers stay line-only.

User also requests drop overwrite: incoming clip occupies its destination interval;
underlying Arrangement clips on that hybrid track are cut into uncovered pieces
(trim/split/delete across both source kinds). Scope drop-specific commands and
browser existing-track import, not automatic rewriting of legacy overlaps or
a renderer sum-policy change. Exclude the mover from same-track victims. Exact
touch is not overlap; one typed candidate/compile/history operation, preserving
incoming identity and data and failure rollback. Audio fragments must preserve
source phase and fixed fade domain. The accepted MIDI source/held-note/loop fragmentation and frame-to-beat
rounding contract is in `docs/clip-overwrite.md`: finite right one-shot projection,
boundary-note retrigger, fresh note IDs and bounded pre-counted expansion.
No callback conversion or unbounded event expansion. Test all cross-kind/cover
shapes, count/ID limits, history/persistence, source continuity and rendered drop
dispatch/highlight geometry. User owns native acceptance; no commit/push yet.

Source milestone complete: drop-specific overwrite and full blank-area feedback
are implemented. 209 UI tests pass/one ignored; independent full project/session
suites and engine clip-fade PCM 5/5 pass, with strict affected lint/build and
guards/fmt/diff checks. Separate review app `artifacts/hex-overwrite-drop-20261009.app`;
native acceptance remains pending. Follow the checklist in `clip-overwrite.md`.


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

Arrangement navigation source milestone complete: continuous 256-line bounded
adaptive grid, cursor-anchored wheel zoom (line/pixel input), captured middle pan
with virtualized vertical scroll and cancellation. 216 UI tests pass/one ignored;
strict UI lint/build and guards/fmt/diff pass. Native review remains pending in
`artifacts/hex-arrangement-navigation-20261009.app`; see navigation checklist.


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
