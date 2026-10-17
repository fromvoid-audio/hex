# native freqs tuning: paused cloud handoff, 17 october 2026

This is an incomplete source checkpoint, not merge or delivery evidence. The user
paused ALL local execution because concurrent agents slowed the Mac, then explicitly
authorized bounded checkpoint commits/pushes and cloud continuation. Do not restart
Mac builds, tests, profiling, installation, or native app actions without a new resume.
The coordinating chat will dispatch cloud continuation; this chat creates none.

## requested outcome and canonical ownership

Adapt the user's own Freqs VST (spoken “Fricks”), `/Users/portalsurfer/dev/freqs`,
into native Hex project-wide tuning: top-level tuning entry, the complete scale
editor, persistence, every native pitched playback/export path, and MTS-ESP master
compatibility. Do not embed the VST or replace GPUI. The finite feature inventory,
reference hashes, data contract, and native/MTS boundaries are in
[project-tuning.md](project-tuning.md). This prerequisite alone is not the outcome.

Pulse live `https://pulse.iocore.dev` goal 4, tag 15, owns these existing issues/PRs:

| issue | canonical PR | canonical branch |
| --- | --- | --- |
| #2739 model/persistence | https://github.com/PORTALSURFER/hex/pull/350 | pulse/task-2739-g1-63c73bbb3785 |
| #2740 runtime | https://github.com/PORTALSURFER/hex/pull/351 | pulse/task-2740-g1-5443132fafde |
| #2741 editor | https://github.com/PORTALSURFER/hex/pull/352 | pulse/task-2741-g1-7668cc7aa36b |
| #2742 MTS | https://github.com/PORTALSURFER/hex/pull/353 | pulse/task-2742-g1-40b3264e1752 |

All four are draft PRs assigned PORTALSURFER, attached to the owning Codex chat.
No merge, review approval, native acceptance, optimized delivery, or installation
has been established. Preserve the canonical PRs and their history; do not create
duplicate branches/PRs. Read fresh Pulse ticket revisions and repository workflow
before changing statuses. Dependencies are 2739 -> 2740 -> 2741/2742. Current Hex
worktrees have independent copies: merge runtime changes into editor carefully,
especially overlapping Session code and architecture policy. None of the WIP
branches is automatically a coherent complete integration.

Separate discovered Freqs ABI bug: Pulse #2743, existing draft
https://github.com/PORTALSURFER/freqs/pull/3. No Freqs changes were made here.

## foundation checkpoint and evidence

PR350 head `ac29a1add5a036c220e0ea0059cd37288ff6462b`, based on
`f81a33f993b588faf9aa3a747cf6b1006e3c249f`. Adds std-only validated `hex-tuning`
scale/analysis/FREQS1/2/3 codec, ProjectTuning + SetTuning/history, strict schema30
persistence/legacy defaults, and explicit enabled-tuning runtime refusal until
PR351 replaces both guards. Default is disabled exact A440 12-TET; opening the
editor must not enable, dirty, or change the project. Freqs seven-note major is an
explicit choice, not the migration default. Legacy writers refuse unsupported
nondefault tuning rather than losing it. The complete validated 48-value scale
and audition settings persist; local library names and MTS process state do not.

Focused project/storage checks passed 348 tests. Full local CI on exact foundation
head passed steps 1–12: runner, architecture, GPUI/dependency guards, UI style,
device layout, fmt, future lints, workspace tests, prepared export instruments,
UI-feature tests, and device-gallery tests. Step13 audio-allocation-probe FAILED.
Within desktop tests: 4 passed, 6 failed, 1 ignored. First visible causes include
`prior native lease never released` in
`native_audio::tests::exact_root_domain_retirement_intent_survives_cleaning_and_recovers_before_reuse`
and `canonical worker did not respond` in
`controller_input_tests::resident_controller_and_keyboard_publications_keep_source_identity_through_worker_bridge`;
four other tests then reported poisoned shared test locks. Heavy concurrent local
load existed, but it is NOT a proven explanation or permission to ignore failures.
Later full-CI steps did not run. Required verification is failed/incomplete; do not
merge based on this run or loosen timing/ownership tests to manufacture green.

Mac-only retained evidence:
`/Users/portalsurfer/.codex/worktrees/0bed/hex/artifacts/local-ci/20261017T113518Z-f2c06be1/summary.json`
and sibling `13-audio-allocation-probe.log`;
`artifacts/tuning-reference/local-ci.log`, `manifest.json`, `unpublished.patch`.
The runner/process group was stopped and verified absent at pause. No local retries.
These ignored logs/patches are retained on the Mac, not promised available in cloud.

## editor checkpoint: implemented source, incomplete integration

This PR352 checkpoint starts from foundation ac29a1ad. Changes:

- `crates/hex-ui/src/editor/tuning/{mod,plots,library}.rs`: top toolbar entry and
  cmd-alt-t, native overlay, numeric nodes/reference/count/audition settings,
  pitch dragging/insertion/removal, cached variation palette with bounded worker
  analysis, lattice/pair metrics/root triads, eight-slot library actions and file
  prompts. Lowercase theme text and titlebar inset are retained. The full feature
  list still has open items below; no native user acceptance occurred.
- UI controller/transport candidate fields distinguish canonical/device/tuning
  preview source identities by generation, key, sequence and revision. Source
  acknowledgements must not be mistaken for proof of native application or MTS
  publication. Audit all candidate constructors and stale receipt handling.
- `hex-session/src/tuning_gesture.rs` and Service changes: bounded/coalesced preview
  protocol (one inflight, one latest, reserved terminal), grouped commit/cancel,
  generation/sequence guards, device/tuning overlay exclusion; worker-side library
  operations. Preview changes no canonical history; commit one SetTuning.
- `hex-storage/src/tuning_library.rs`: bounded eight-slot FREQS library and safe
  explicit overwrite/create-new publication; corrupt inputs preserved. No user
  library was modified. Library location is Hex application-support scales,
  independent of Freqs personal library.
- `hex-widgets/src/numeric.rs`: optional validated coarse/fine unit steps. Needed
  because normalized count step is fractional; existing default laws unchanged.
- `hex-devices/src/tuning_audition.rs`: isolated ORIGINAL reference-tone kernel,
  not yet declared in lib.rs or connected to engine/Session/audio. 16 bounded
  triangle/square/saw voices, PolyBLEP, envelopes, owner tokens and transactional
  phase-preserving retarget. Do not mistake selected-track keyboard for audition.
  Controls alone do not implement audible audition.

Late source review fixed selection after count shrink and discarded numeric
subscriptions/entities across project replacement, external canonical edits and
close using an epoch guard. These late edits and cached triad display were not
compiled or tested before pause. No implementation repairs after pause.

Actual validation:

- Earlier `cargo check -p hex-ui --tests` passed, with two warnings subsequently
  addressed. Exact prior source was uncommitted; this is not final-tree evidence.
  Log `/tmp/hex-tuning-ui-check2.log`.
- Latest `cargo check -p hex-ui --tests --features ui-test-support` FAILED with
  missing `tuning_preview_identity` in `PendingPlayback` initializer at
  `crates/hex-ui/src/editor/arrangement_seek.rs:550`. It remains UNFIXED in this
  checkpoint. Log `/tmp/hex-tuning-ui-check3.log`. Latest source edits were made
  while the check compiled; do not infer any other final-tree passing result.
- Session worker's five focused gesture tests and strict Session clippy passed
  before later UI edits. Full Session run was 111/113, two existing 3-second device
  gesture tests timed out, also reproduced in isolation under concurrent load;
  do not call the suite green or assume load alone explains this.
- Standalone audition kernel's four rustc tests passed before late UI work,
  using unsafe forbidden and warnings denied. It still needs production wiring,
  ownership/end-to-end callback probes, more boundary coverage and review.
- New storage library tests, widget stepping test, editor rendered interaction
  regressions, final clippy/format/full CI remain outstanding.

## runtime decisions to retain when integrating PR351

Immutable validated 128*f64 effective table is exact-bit compared, audition
excluded. Legacy neutral table keeps legacy preparation/PCM. Native sine/FM4 and
Operator ratio modes use it; fixed Hz keeps absolute Hz and MIDI key identity
stays u8. Unsupported providers refuse nonneutral preparation explicitly.
One shared analytic context stores the table (+1024 bytes), scalar identity in
cache keys; do not copy tables per clip or mutate interned prepared banks.
Held and released voices must retain phase, envelope, feedback/filter/expression
and actual age across rapid/skipped revisions. Per-occurrence sampler override,
not mutation of shared bank, handles analytic continuation. FM4 ephemeral payload
repacking has no authored serialization change. Operator uses its existing mutable
continuation with twin preflight. Detached tails preserve their originating law.
Keyboard adds bounded per-voice sampler overrides (~44KiB across primary/scratch)
in the single selected-track runtime, not per track; measure copy cost and zero
allocations rather than hand-wave it. Private family IDs inherit only proven
same-except-tuning source changes; exact existing Arc proof remains otherwise.
Fresh Open/ordinary edits must cancel inheritance; no retained old-graph Arc chain.

Session owns authentic old canonical/prepared tuple. PR351 handles discrete
SetTuning/Undo/Redo and held keyboard integration. This PR352's preview_tuning
still uses older preparation and MUST be adapted after runtime merge so grouped
live previews also preserve correct families and actual live keyboard state.
Preparation/control refusal preserves canonical/history. A later callback
voice refusal preserves old actual PCM/runtime but does not rewind already
committed desired canonical state. Desired/native-applied/MTS-published revisions
remain distinct. No universal two-phase project commit or rejection of every
valid table because an unused key is above Nyquist.

## MTS boundary and deferred integration

PR353 contains a separate audited safe adapter in `third_party/mts-esp-master`,
with private unsafe ABI dependency boundary, fixed production system dylib path,
correct raw RegisterMaster(void*) with null owner, typed failures, local gate,
owner-thread !Send/!Sync handle and exact-once scoped deregistration. See its
`docs/mts-esp-boundary.md` and final checkpoint handoff. It is not yet wired into
root workspace/dependencies or desktop/session/UI. No real library registration,
foreign-master takeover/reset or privileged installation was performed.

Use one desktop-owned serialized broker, active project ownership independent of
popup/focus. Audit exact upstream version/signatures/name metadata and macOS
signing/library validation. Acquisition republishes full current table. Missing
or incompatible library/foreign master must be visible; native tuning remains
usable without MTS. Disable/replacement/shutdown cannot be dropped behind preview
coalescing. Don't claim system-wide sample atomicity or all third-party plugin
support. Offline export never registers. Preserve distinct desired/native/MTS
identities and reject stale generations/receipts. A helper process was rejected
because IPC-disabled MTS connectivity would differ; don't silently switch design.

## finite continuation and acceptance

1. Read repository AGENTS.md, README, architecture/runtime/local-ci/test-builds
   docs and these canonical Pulse tickets; recover existing branches/PRs. Inspect
   cloud platform/capabilities before choosing checks, preserving native GPUI.
2. Review/repair runtime WIP and its focused tests; complete routed MIDI/export,
   continuation/tail/fixed-Hz/legacy behavior, zero-allocation and copy-cost
   evidence. Integrate Session runtime first, then adapt editor previews.
3. Repair known editor compile error and audit all new struct initializers.
   Verify epoch/generation cancellation, shrink selection, sequence filtering,
   one history step per drag, transactional refusal and library corruption/input
   bounds with meaningful regressions. No decorative audition controls.
4. Integrate reference-tone kernel through bounded input/mixer/control ownership,
   live retarget, note-off/stop/replacement, and held/recent pitch highlights.
   Complete Shift-repeat snapping, selected/readout/density behavior, unsaved
   library-name UX, true audition input and remaining finite feature inventory.
5. Integrate and audit MTS broker/status/client count/retry/lifecycle, safe
   dependency policy/pinning and signing. Native playback must never depend on
   library presence or silently fall back to equal temperament.
6. Perform final diff review and repository-required exact-source full local CI
   where platform supports it. Investigate existing failed tests honestly; never
   enable paid/hosted GitHub CI. User has standing merge authorization only for
   focused completed PRs with exact final full CI green. Keep drafts incomplete.
7. macOS-specific required checks, optimized `cargo build --release -p
   hex-desktop --locked`, stable-path `scripts/bundle-macos.sh` delivery and native
   interaction/listening/MTS-client acceptance remain pending and locally paused.
   Follow current docs/test-builds.md. Never force quit Hex or risk unsaved work.
   User performs native interaction tests; provide finite checklist, exact build/
   source/included PR revisions and accurate reload/install state.

Cloud suitable: source review/implementation, pure Rust/model/DSP/storage/Session
unit tests supported by environment, documentation and canonical PR maintenance.
Linux cloud cannot establish macOS GPUI/CPAL/device deadlines, installed MTS dylib,
Apple signing or actual native-user acceptance. Do not replace native framework,
weaken gates, buy infrastructure, enable GitHub Actions or use billed fallback.
Mac-dependent work stays paused until explicit user resume. At checkpoint,
list_projects exposed only local Hex project
`912ba002-878e-461b-b1a5-71ca929dc859` and ChatGPT project `hexdaw`
`g-p-6880b7b4e9b481918baa3206afeb7186`; no configured cloud environment ID was
established. Coordinator must select an existing suitable environment.

The reference Freqs committed baseline is available at f90669b0b193a55d049fc1cd7d30d7127233edca.
Its dirty user edits and ignored reference patch were NOT pushed by this task.
Preserve them; do not claim cloud access to unpublished reference contents or Mac
credentials/user libraries. Ported Hex source and hashed feature inventory are
checkpointed. No unrelated worktrees, libraries, user app or media were altered.
