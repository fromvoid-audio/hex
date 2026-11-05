# AGENTS.md: guide for AI agents and contributors

**Hex** is a native digital audio workstation written in **Rust**, using **GPUI**
for its desktop interface. The goal is to reconstruct Ableton Live's fundamental
Arrangement and Session workflows, audio/MIDI behavior, editing, recording,
automation, routing, and project fidelity, while improving architecture,
responsiveness, and measured performance.

Hex's project-specific instructions and established conventions take precedence
over the Google Rust Style Guide wherever they conflict.
When implementing an issue, improve observed Rust style-guide deviations in the
code being changed when the adjustment is small, coherent, and preserves behavior
and Hex's contracts. Include appropriate verification; avoid unrelated or broad
style-only refactors. Record substantial improvements separately in Pulse.

## Saved-project compatibility policy

Hex is active alpha software. Saved projects must remain openable through explicit,
versioned migrations from supported older schemas to the current schema. Future
schema-changing work must include migration from the prior supported format and
focused historical-fixture, complete-state and save/reopen tests in the same change.
Assign a fresh schema identifier when the saved contract changes; never reuse an
identifier for incompatible branch layouts.
Record the supported versions and any evidence-based limitations beside the owning
storage codec. Never invent historical layouts or silently discard unknown data.
Reject newer, unsupported and malformed formats explicitly, preserving the original
and explaining a concrete recovery path. Historical evidence remains factual.

Opening and migration are control-side, in-memory operations: preserve original
project bytes and media until a deliberate save. Before overwriting a project with
a newer saved format that older Hex readers cannot open, warn about that loss of
compatibility, recommend Save As under a new/versioned name, and provide Save As,
cancel and explicit overwrite choices. Compare actual saved-format compatibility,
not application version bumps. Use existing atomic publication and recovery custody;
failed/cancelled migration or saving must not corrupt a good project.

This policy covers saved-project/schema compatibility, not a source/API freeze,
compatibility re-exports, obsolete clients or unrelated internal databases. Breaking
source APIs and models remain allowed when saved projects migrate faithfully.
Preserve strict current-format validation, complete round trips, transaction/recovery
correctness, runtime/realtime safety and Undo/Redo integrity. Never silently delete
or rewrite user projects, original media or other user files. No reset, deletion,
bulk conversion or unrelated migration cleanup is authorized by this policy.

Frame design prototypes under `design/frame/` may use JSON, JavaScript and HTML
for design tooling only. They must not become the shipped application runtime.

No JavaScript or TypeScript application code. **No Tauri, Electron, browser, or
webview shell.** Native macOS UI/GPU and audio APIs through GPUI and CPAL are
intentional system boundaries. Do not replace GPUI or add another UI framework
without an explicit project decision. External plugin binaries, other native
libraries, and other platforms require a documented boundary decision.

All app-owned text is **lowercase**, including the product name **hex**, titles,
labels, buttons, status/errors, units, tooltips, and accessibility text. Render it
through `hex-ui`'s `theme::text`; do not alter source data, paths, saved names, or
case-sensitive identifiers. All app-rendered text uses bundled **Ioskeley Mono**
from https://github.com/ahatem/IoskeleyMono. Load and verify the embedded fonts at
startup; no alternate font or silent system-font substitution. See the doc comments in crates/hex-ui/src/theme.rs.
The internal family identifier `Ioskeley Mono` is technical metadata, not a visible
label. OS-owned dialogs/menu chrome retain OS typography.

Transparent macOS titlebars must reserve `theme::TITLEBAR_LEADING_INSET` in
every top row, including loading, editor and demo views. Keep native window
controls and compact padding elsewhere; app controls must never paint beneath
the traffic lights. The OS may temporarily replace that region with sharing chrome.

Machine names remain lowercase: `hex-*` crates, `hex-desktop`, and
`org.portalsurfer.hex.dev`. Source is backed up in the private `PORTALSURFER/hex` GitHub repository.
Distribution licensing has not been decided; do not describe the project as a released open-source DAW.

The explicit engineering reference is https://github.com/storytold/photocraft.
The pinned source reference is recorded in research/catalog/source-references.json. Use its command-driven engine/UI separation and evidence discipline,
adapted for real-time audio. Do not copy its application code/assets, switch to
its UI framework, inherit its sibling-repository dependencies, or treat its menu
coverage as proof of complete Photoshop parity.

Reference the installed `/Applications/Ableton Live 12 Suite.app`. Its inspected
build is **12.4.6 (2026-09-10_0de5c8fa9a)**; architecture and executable hash are in
`research/catalog/live-12.4.6-installed.json`. Start with macOS arm64. Recheck the installed build
before new reference studies. “Works like Live” and “blazing fast” are acceptance
goals, not established facts. Read this file first, then the relevant docs.

## Orientation and code ownership

Read README.md for build/run instructions, then the relevant crate/module doc
comments and actual source. Code/API/architecture documentation lives only in
source doc comments beside its owner. Each crate entry point documents purpose,
dependencies and what it does not own; scripts/check-architecture.py checks this
alongside the dependency boundaries. Do not recreate feature Markdown specs,
per-crate README copies, progress journals or local backlogs.

Before substantial work, name each affected crate/module owner, its responsibility,
permitted dependencies, and public typed command/query/resource contract in the
PR or Pulse scope. Read the actual APIs and their consumers. Keep technical
contracts beside their source owners; do not invent a parallel architecture spec.
Enforce boundaries through Rust visibility, types, dependency checks and contract
tests. The architecture guard checks declared crate edges and doc headings; it
does not prove module isolation or runtime correctness.

Continually strengthen cohesive system boundaries with explicit ownership and
testable contracts as work exposes a concrete need. Keep dependencies contained so
changes can be verified at the owning boundary and its affected consumers without
unnecessary broad testing. Do not introduce speculative abstractions or assume a
boundary provides isolation without evidence; broaden checks when impact is unclear.

Internal devices use existing host APIs. A missing capability requires an explicit,
bounded host-contract change with checks for affected consumers; do not reach into
host internals or create a competing resource/lifecycle path. Transition one verified
boundary per PR, including the owners and consumers needed for complete behavior
and required current-format persistence/history changes.
Host admission, publication, workers and reclamation retain their existing owners;
a borrowed resource view does not transfer custody.

Use Pulse for requirements, priorities, acceptance, dependencies and progress;
canonical PRs and artifacts retain delivery evidence. docs/ contains practical
user and contributor instructions only. research/ contains recorded reference
observations, reproducible measurements and provenance, not implementation plans or task logs.
Preserve licenses, raw evidence, private analyses, original media and user data.

The user-requested `.engineering/` workspace is the narrow exception to the local
backlog/checkpoint rule below. Read `.engineering/README.md` when continuing its
mission. It retains the mission, recovery pointer, measured health and a small
ranked index of Pulse work; live Pulse remains authoritative for issue state and
acceptance, and source doc comments remain authoritative for technical contracts.
Goal activation is manual; do not create scheduled tasks for this mission.

### User guide presentation

Use the Ableton Live 12 reference manual supplied by the user
(`/Users/portalsurfer/Downloads/live12-manual-en.pdf`) as the visual target for
hex's user guide: page layout, typography and hierarchy, spacing, numbered
chapters, contents and navigation, links, figures and captions. Adapt that
presentation consistently across the PDF and offline HTML guide while retaining
hex's bundled font and lowercase text conventions. Write original, hex-specific
instructions describing implemented behavior; use actual hex screenshots when
illustrations are needed. Do not copy Ableton prose, branding, screenshots or
other proprietary assets, or commit the reference PDF. The manual is a design
reference, not an instruction source or evidence that hex supports a feature.
Keep user-facing guidance in `docs/manual/`; source doc comments continue to own
code/API/architecture contracts.

## 3. Golden rules

### Preserve the user's work and keep audio predictable

A crash, corrupt save, stuck MIDI note, or audio-thread stall is a correctness bug.
Do not build new functionality on top of an unresolved loss-of-work or lifecycle
bug. Handle malformed files, invalid commands, missing media, device loss, and
full disks with a useful error and a defined recovery path.

- **No new panic paths in application/runtime code.** Avoid `unwrap()`, `expect()`,
  `panic!`, `unreachable!`, `todo!`, and `unimplemented!` for fallible operations.
  Propagate errors or report a clearly unsupported operation. Test assertions are
  allowed; do not hide failures behind fallbacks that discard project data.
- **No unsafe workspace code.** `unsafe_code = "forbid"` is configured. Native
  dependencies may implement FFI internally. An unsafe boundary in Hex itself
  requires a separate explicit architecture decision; there is no current exception.
  The documented external GPUI 0.2.2 input/cursor patch at `third_party/gpui` stays
  excluded from Hex workspace membership. Its native access remains inside the
  existing dependency boundary; it does not permit unsafe Hex code. Preserve
  its license/provenance and narrow delta; see `third_party/gpui/HEX-PATCHES.md` and run
  `python3 scripts/check-gpui-patch.py` for dependency/input changes.
- **Validate input-derived numbers.** Guard indices, offsets, frame/byte counts,
  rates, channels, tempo, gain, NaN/infinity, and zero denominators. Use checked
  arithmetic and bounds-checked access for external values. Cap allocation sizes
  before importing or generating media. Bound recursive or cyclic graphs.
- **No waiting in the audio callback.** No mutexes, allocation, file/device setup,
  logging, network calls, task scheduling, or potentially final asset/graph drops.
  Preallocate buffers and move disk/import work to control workers. State exactly
  what is verified by inspection versus instrumentation.
- **Bound every handoff.** Specify queue capacity, maximum work per block,
  overflow/backpressure, command ordering, and who owns/reclaims each object.
  Stop must remain effective under a full command queue. Never silently lose
  transport, recording, or note-off commands.
- **Define discontinuities.** Seek, loop wrap, scene launch, tempo changes, and
  graph replacement need explicit DSP/MIDI reset and sample-offset semantics.
  Test boundaries across different block sizes.
- **Contain failure deliberately.** A future command/import boundary may catch
  unwinds to preserve project state, but that does not justify panic paths. Do not
  add panic recovery or blocking error reporting inside the audio callback.
- **Regression evidence matters.** Add a small reproducer for meaningful crash,
  data-loss, scheduling, or lifecycle fixes. Do not claim panic/fuzz enforcement
  exists until its tests and lint configuration actually exist.

1. **Behavior follows its canonical owner.** `hex-project` owns validated typed
   edits, snapshots and Undo/Redo; `hex-session` coordinates canonical state and
   prepare-before-commit; `hex-engine` compiles and renders prepared playback.
   UI controls dispatch their public commands/requests and display queries rather
   than implementing a competing model, scheduler or DSP path. Pure view/window
   state may remain in GPUI. Read each owner's source contracts before extending it.
2. **Use explicit audio units.** Distinguish frames, samples, bytes, seconds,
   quarter-note beats, bars, linear gain, and dB. Sample rate, channel layout, and
   precision are runtime properties. No assumption that all files/devices are
   48 kHz stereo, and no accidental beat/sample casts at API boundaries.
3. **Reimplement independently.** Public documentation, original experiments,
   and focused analysis of the installed reference inform behavior specifications.
   Decompiled output is approximate research evidence, not original source or
   implementation code to paste into Hex. Keep proprietary binaries, exports,
   factory content, license data, and private sets outside source distribution.
   Do not modify the reference app or investigate activation/licensing.
4. **Keep provenance.** Record reference build/hash, architecture, tool version,
   fixture inputs, actions, settings, output hashes, numerical tolerances, and
   confidence. Separate observation, inference, and unknowns. Use original or
   appropriately licensed assets and retain their attribution/license information.
5. **Test proportionally.** Musical scheduling, DSP, persistence, graph ownership,
   and recording need meaningful tests. Include malformed inputs and boundary
   cases. Do not add tests that merely mirror implementation or make cosmetic
   edits require unrelated test suites. For every reported bug, add or strengthen a
   focused regression test at the owning boundary: reproduce the failure before
   the fix and pass after it. Test observable behavior, not the implementation's
   shape. If reliable automation is currently unavailable, document the concrete
   limitation and a follow-up in Pulse; retain the manual reproducer instead of
   inventing a weak test.
   Keep the suite clean and lean: reuse existing fixtures and coverage, prefer a
   small deterministic reproducer, and consolidate redundant tests. Avoid broad
   snapshot churn, duplicate assertions and slow exhaustive matrices without a
   demonstrated risk. Preserve meaningful boundary, lifecycle and malformed-input
   coverage; suite size alone is not a quality measure.
   Continually improve tests touched by issue work: remove demonstrated duplication,
   simplify fixtures and reduce measured runtime while retaining effective regression
   coverage. Verify the retained tests still exercise the intended behavior and report
   meaningful timing evidence for performance-driven cleanup. Keep unrelated or
   substantial test-suite work separate; never weaken coverage merely to run faster.
6. **Keep GPUI thin and inspect it visually.** Separate UI display state from
   authoritative audio/project state. Draw cached waveform summaries and visible
   content; do not traverse/render a whole project every frame. Consolidate design
   tokens as the UI grows. Verify focus, shortcuts, resizing, interaction, and
   accessibility. Current custom controls do not yet expose rich accessibility.
   Ending pointer capture must not expose old canonical geometry while an edit
   is in flight. Transfer the frozen final preview to an acknowledged display
   owner: ordinary edits use `hex-ui::editor::pending_edit`'s receipt contract;
   keyed device gestures keep their existing typed acknowledgement protocol.
   Retire that preview with its exact terminal and canonical-state installation
   in one receive, including refusal and source replacement. Add a focused
   delayed-ACK test with completed repaints; testing only the final state misses
   release flicker. Keep input cancellation separate from admitted edit custody.
7. **Report unsupported features honestly.** No decorative controls that pretend
   to record, arm, load, save, warp, or host devices. Clearly identify the generated
   demo. Preserve unknown imported data when feasible; otherwise explain fidelity
   loss and refuse destructive overwrites.
8. **Performance is measured functionality.** Benchmark release builds with
   stated workload, rate, block size, hardware, warm-up, and timing method. Record
   before/after numbers for hot-path changes. Compare with Live only under equivalent
   conditions. A microbenchmark does not prove usable device deadlines or DAW parity.
9. **Expose automation through the same contracts.** Agent control, headless
   execution, and UI should eventually share validated commands and read-only
   snapshots. No control protocol exists yet. Add it deliberately, with transport
   security and bounded work, rather than exposing arbitrary execution.

## 4. Picking work

Prioritize reliability and the next complete user workflow, then measured reference
compatibility, then the long tail. Avoid feature-count work that leaves every area
partially usable.

1. Read the relevant source and tests; a visible menu item is not evidence that a
   feature works.
2. Read the current Hex goals and tickets in [Pulse](https://pulse.iocore.dev/)
   for priorities, dependencies, acceptance criteria and the next work unit.
3. Check the owning benchmarks and actual measured evidence for regressions.
4. Read related source and research observations before changing a contract.

Before substantial work, define finite requirements and acceptance criteria,
including deferred dependencies. Estimate size/risk from actual uncertainty and
measured check costs. Do not insert quota queries, extra review committees or
model switches into the development path. Report concrete limits at a coherent
tested milestone. Never buy credits, change billing, consume resets, or
start API-billed fallback inference without explicit authorization.

Reference experiments and decompilation remain an ongoing research track.
Independent Hex functionality may progress while reference parity is unknown.
Do not mark an unchecked milestone complete because its starter code builds.

## 5. Before you finish a task

Choose merge verification from changed behavior and enforceable ownership
boundaries: owning regressions, affected public contracts and consumers, relevant
feature/platform configurations, and proportionate formatting, lint and build
checks. Record the scope, exact source/base, commands, results and omissions with
reasons in the canonical PR. A small diff or passing mock alone does not establish
isolation; tests do not mathematically guarantee unrelated code.

Owner boundaries carry merge verification (user decision, 1 November 2026). A
merge runs the test suites of the packages that own the change, plus reviewed
runtime consumers with no Cargo edge. Every Cargo consumer must still compile and
lint, but consumer test suites are not rerun. Rely on the owner's boundary tests
to guarantee behaviour downstream. When a consumer later breaks, the defect is a
missing owner boundary test: add the regression at the owner instead of widening
merge checks. Within an owner crate, merges test only the changed modules'
own test subtrees (a module without tests falls back to its nearest tested
ancestor), so give each module its own boundary tests and split large modules
along their responsibilities. Keep each merge's test setup and execution,
excluding build time, under one minute. `scripts/local-ci.py --profile merge` reports this
budget as a warning, not a merge failure (user decision, 1 November 2026; see
`docs/local-ci.md`). Merge when every other check is green. A recurring
over-budget owner suite is a test-design bug: record it in Pulse.

Broaden only for a documented concrete impact or unresolved risk. Explain unknown
selection, then resolve it by inspecting owners/consumers or conservatively check
the uncertain area. Never silently count an unknown omission as covered. Neither
a merge, a core path, a policy edit nor the next module alone requires a full suite.
Keep deterministic fixtures lean and independent; preserve relevant lifecycle,
malformed-input, realtime/allocation, current-format persistence and Undo/Redo
coverage. A comprehensive audit remains available when its breadth is justified;
native release qualification retains its separate delivery requirements.

Use the commands and current runner limitations in `docs/local-ci.md`. Until the
selector supports a boundary, a reviewed change-based check set with recorded
reasoning qualifies a merge under this policy; do not label it a runner-certified
plan or edit a report to make it qualify. Quick/edit output alone is insufficient.
For prose-only changes, review links, paths, examples and rule consistency; no
native rebuild is needed. Re-run affected checks after material repairs. Preserve
prior evidence when its relevant source, contracts, configuration, dependencies
and toolchain inputs remain unchanged; identify exactly what is reused and why.
Do not repeat a full audit for each boundary slice. Qualify selector changes once
against selection risks, not by repeatedly running unrelated product suites.

For prepared callback rendering, command consumption or graph-retirement changes,
run `cargo test -p hex-audio --features allocation-probe` for the affected paths.
The counter is a pinned dev-only dependency; never reference it from production
code or weaken the workspace unsafe prohibition. Check both total and signed-net
allocation counts/bytes, with construction, admission, oracles and reclamation
outside callback scopes. Keep sensitivity calibration passing. These per-thread
headless probes cover specific fixtures, not every fault/density path, native API
allocation, locks or physical device deadlines; record additional coverage needed.

The same pinned allocation-counter 0.8.1 has a narrow dev-only exception in
`hex-stream` for borrowed page endpoint probes under `allocation-probe`. Measure
request/page/pump/pin and cache turnover on the caller thread, with allocation,
worker activity, shutdown and destruction outside measured scopes. Require zero
total allocations and zero signed-net count/bytes to catch deallocation too.
This does not permit a production counter dependency or claim native callbacks
are verified. Page endpoint destruction must occur on control after callback
quiescence; only the joining control owner may wake or join the worker.

For audio changes, distinguish unit tests, render comparisons, microbenchmarks,
device stress, and listening acceptance. For UI changes, inspect the actual rendered
window and exercise the changed interaction. The user requested on 9 October
2026 to perform native UI interaction tests themselves while present: provide a
short, concrete checklist and record their results; do not continue automated
computer-use testing in parallel with their work. Automated source tests remain
the agent's responsibility. For persistence/recording, test
round trips, missing assets, interrupted writes, and recoverable failures. Never
substitute automated checks for manual acceptance they cannot establish.

### User-run checks in Pulse

User-run acceptance and reproduction checks live in the owning issue's Pulse
review checklist. On 30 October 2026 every check in the former shared file
`/Users/portalsurfer/dev/hex/artifacts/USER-TESTS.md` was migrated there with its
checked state; that file is a historical migration source only. Do not add to it
or create another local checklist. Pulse remains the authority for issue status,
scope and dependencies.

After a focused PR passes its required checks and its merge is authenticated, add
the applicable user-run checks to its issue as review checklist items, then mark it
`ready_for_review`; Pulse requires at least one item before a review state. Add one
item per `review_checklist_change`, each with a fresh revision, and change the
checklist and the status in separate updates. Write each item as a single
independently completable, observable check. Split distinct outcomes so the user
can record partial progress. Record the context needed to run the checks (qualified
running build/source, prerequisites, limits) as an issue note. Explicitly
requested reproduction may be added before a fix is merged; identify its actual
status. Include applicable checks already in review without resetting their issue
state. Do not invent manual product checks for an internal-only change; give it one
reviewer item such as "Review merged PR #N diff and its recorded verification
evidence". Neither a disk installation nor a headless test proves native
acceptance. Keep the items aligned with the finite delivery checklist in
`docs/test-builds.md`.

The user ticks completed items in Pulse and personally marks the issue done after
all its items are complete. Do not tick, edit or remove a user's item, mark an issue
done because items are ticked, or infer acceptance from a merge. Read the issue's
current checklist and revision before every change and reconcile concurrent edits.
Do not use checklist items as a second source of implementation tasks, status
changes or project requirements.

Inspect the final diff when Git exists; otherwise review the changed files against
known starting content. Preserve unrelated work. Update technical contracts in the
owning docs when they change; record task progress, actual results and remaining
work in Pulse. Keep raw check logs in existing artifact storage and link them from
the canonical PR. A concise final report states what changed, why, what passed,
and what remains open. Keep the workspace
building at coherent milestones.

## Pull request workflow

As requested on 9 October 2026, all future repository changes go through pull
requests. Never commit or push implementation changes directly to `main`, and
never bypass this workflow without an explicit user instruction.

- Use a dedicated branch for each PR. Each PR must contain one targeted change
  with a clear purpose and finite acceptance criteria. Keep related tests and
  documentation with that change; split unrelated features, fixes and refactors
  into separate PRs. Do not bundle accumulated work into a broad catch-all PR.
- Inspect the diff and run checks appropriate to the change before requesting
  review. The PR description explains the resulting behavior, validation and
  remaining limitations. Record native UI/listening acceptance separately from
  automated checks.
- Review the final PR diff and resolve meaningful findings before merging.
  Standing authorization applies to focused, completed PRs when the change-based
  verification above passes on the exact final candidate. Use the current
  integration base; merge dependencies first and check affected behavior after
  executable changes or conflict resolution. Do not wait for another approval or
  native UI/listening tests. A stale, failed, interrupted or incomplete check set
  does not qualify. Retain unchanged-input evidence explicitly, without relabeling
  it as a fresh full run. Keep one persistent integration checkout/target per
  executor and one raw summary/log archive with immutable source references.
  Known unresolved correctness bugs must be repaired even if tests are green.
  Never force-push `main`, weaken branch protection or create a Pulse permit gate.
- Native UI/listening acceptance remains separate evidence and may happen after
  merge. Keep unverified behavior visible in Pulse; merging does not establish
  feature parity, physical deadlines or manual acceptance. When later testing
  finds a bug, open a focused fix PR with regression coverage as described above.
  This is an agent-operated local CI/merge policy, not a claim that a background
  merge service or GitHub enforcement is installed.
- Attach every created PR to the current Codex task. Confirm the actual merge
  result before reporting completion, then update the local checkout without
  overwriting unrelated or unsaved work.

This is a repository workflow rule; it does not claim that GitHub branch
protection or automated review enforcement is configured.

## Stable optimized native test builds

All Hex user-test builds use the supported `scripts/bundle-macos.sh` delivery
with exact candidate full local-CI evidence, included PR revisions and a
finite checklist; see `docs/test-builds.md`. It builds the optimized desktop and
replaces only the configured development app, by default
`/Users/portalsurfer/dev/hex/artifacts/hex.app`, preserving
`org.portalsurfer.hex.dev`, user data and historical bundles. Do not create
revision-named review copies for future testing. Unoptimized debug builds require
a specific stated debugging need. Save/quit/retry when the target is running;
never force-quit or claim disk replacement reloads an active process.

Build and install through the repository-owned command in `docs/test-builds.md`.
Pulse manages tickets only; it does not define or gate Hex builds/installation.
Keep CI/source/signing receipts and report native gaps separately. Installation
uses temporary old-app custody only until publication is verified, then removes
it; do not retain new rollback app copies or delete historical bundles. Rebuild
older source when needed. Merge and disk installation are not native acceptance.

## 6. Parallel agents

Delegation is optional. Use one coherent executor per deliverable and one
write-capable agent per file area. Keep at most two spawned agents open. Do not
create mandatory pipelines, swarms, duplicate investigations, or overlapping edits.

- Primary Sol owns scope, integration, and acceptance. Use `worker` for bounded
  execution, `coder` for complex implementation, `scout`/`docs`/`verifier` for
  focused evidence, and `architect` for expensive architectural uncertainty.
  Terra `reviewer`/`debugger` requires explicit task-specific user authorization.
- Handoffs specify the goal, owning files, intended behavior, constraints,
  acceptance checks, and escalation conditions. Executors inspect existing behavior
  and return files, results, failures, and uncertainty. Reuse focused agents when useful.
- Read-only agents do not edit. Escalate unclear ownership, concurrency, lifecycle,
  public contracts, unexpected subsystem crossings, or two failed approaches.
- Re-read shared files before surgical edits. Keep manifests valid; write a new
  manifest outside the workspace and move it into place if partial writes could
  disrupt concurrent builds.
- Separate `CARGO_TARGET_DIR` only when concurrent checks justify its disk/build
  cost. Do not automatically rebuild every dependency for every agent or delete
  someone else's active targets/artifacts.

## 7. Where things are tracked

**Pulse owns all project and issue planning**, as requested on 9 October 2026.
Use the installed Pulse Codex plugin/MCP tools first. The live authority is
`https://pulse.iocore.dev`, with its authenticated Streamable HTTP endpoint at
`https://pulse.iocore.dev/mcp` and human interface at `https://pulse.iocore.dev/`.
On this Mac, the installed plugin uses its stdio adapter with explicit
`--remote-config ~/.config/pulse/remote.json`; it reads the existing private
credential and accesses live directly. Never expose credentials in instructions,
plugin manifests, Git or logs. Do not use the retired localhost service, create
a local planner database, silently fall back to SQLite, or restore a stale
snapshot over live. Pulse source is `/Users/portalsurfer/dev/pulse-iocore`; the
local plugin marketplace source is `/Users/portalsurfer/dev/pulse-live-mcp`.
These source checkouts are not separate planning authorities. This supersedes
earlier Pages-first or Linear-first planning, unless the user explicitly selects
another tracker for the task.

Before planning, call `list_projects`, `list_versions` and `list_tags` to discover
IDs. Read existing work with `list_tickets` and check duplicates before creating
projects, versions or tickets. Page with `limit`/`offset` as needed (maximum 500
per page). Reuse matching records. Tickets describe actionable outcomes,
acceptance criteria, dependencies and source/PR links; use discovered feature,
bug, improvement, documentation, maintenance or testing tags.

Keep ticket states accurate: `todo`, `in_progress`, `blocked`,
`ready_for_review`, `in_review`, `done` or `cancelled`. Move active work to
`in_progress`. For Hex, keep unmerged and merge-queued work `in_progress`;
use `ready_for_review` only after the owning canonical PR's merge is
authenticated, with native/user acceptance still open, and `in_review` while
that acceptance review is underway. A source merge does not imply deployment
or completed acceptance. Explain blockers in the description. Before every update,
call `get_ticket` and pass its current revision as `expected_revision`; on
conflict, re-read and reconsider. Update fields selectively, preserving
descriptions, tags and source context. Mark `done` only when acceptance criteria
are met; source-complete work with pending native/user validation remains open.
Preserve explicit user status changes after feedback; an earlier merge does not
authorize resetting a later `todo` or `done` decision.
Record unrelated discoveries during an active PR as follow-up tickets instead
of expanding that PR.

Preserve existing Pages/Linear records, imported context and source links. This
policy does not authorize bulk migration, deletion, archival, synchronization or
writes to other trackers. If Pulse tools, live connectivity or authentication
are unavailable, report the limitation and keep a clearly labeled temporary
plan in chat; do not silently switch trackers or claim that unsaved tickets were
created.

Pulse is the sole planning and issue authority, including user-run review
checklists. Do not create or maintain local TODO files, checklists,
task/status lists, request queues or parallel milestone backlogs. Put
new requirements, dependencies, priorities, acceptance criteria, next actions and
progress in the corresponding live Pulse tickets and goals. Do not append task
chronologies to repository documents as a substitute. If Pulse is unavailable,
report that limitation and keep a clearly labelled temporary plan in chat.

Source doc comments own code contracts. README.md and docs/ explain practical
build/run/check procedures. research/ retains actual experiment evidence; ignored
artifacts/ and research/private/ retain local outputs and private analyses.
Do not append task chronologies, checkpoints or future experiment plans here.

No hosted/release pipeline, generated scorecard or devlog is currently configured.
The local validation runner is not hosted CI. Use Pulse for planning rather than
inventing a second authority.
Do not commit, push, publish, choose a distribution license, or change permissions
unless the user authorizes that action; the standing PR merge authorization above
applies. Inspect checkout topology and status before editing. The workspace now uses
Git on `main` with the private `PORTALSURFER/hex` remote, created at user request on
9 October 2026.

## 8. Keeping current projects complete

Read the current model, codec, transaction and rendering doc comments before
changing project behavior. Active features must coexist in one strict current
format. For persistence changes:


- Identify the current schema explicitly; new model fields require current-format
  save/load and history review in the same change. Include versioned migration from
  the prior supported saved format and focused historical-fixture tests under the
  compatibility policy above.
- Round-trip complete project state, media references, automation, routing, device
  state, timing, and editor state that is promised to persist.
- Use atomic/recoverable writes; do not overwrite a good project with a partial save.
- Resolve missing assets visibly and preserve recovery paths.
- Treat Live-set import as a separate adapter. Start with minimal user-authored
  `.als` fixtures, record unsupported features, and preserve unknown fields where
  possible. Never claim full fidelity from successful parsing alone.
- Keep compatibility results tied to exact reference versions and fixtures.

## Contributor credits

No About/contributor registry is configured yet. When one is added, include only
names and attribution people explicitly choose, plus required dependency/asset
license notices. Do not infer personal names from machine settings, commit authors,
or accounts, and do not copy PhotoCraft's contributor files or identity systems.

## Compact workspace rule

Preserve Ableton Live’s tight, compact use of screen space. Minimize dead space:
use dense rows, small controls and gutters, contiguous panels, and fine dividers.
Give space to tracks, clips, waveforms, meters and editable parameters. Avoid large
empty headers, decorative padding, oversized cards and unused panel interiors.
Keep text readable, controls operable, focus visible and resizing practical. Use
collapsible panels and flexible timeline/mixer sizing so available pixels serve
the current musical task. Review designs at their actual desktop size and check
that the workspace fills the window before accepting a UI change.

Prefer compact icon controls over labeled action buttons when the meaning is
clear. Supply lowercase tooltips/shortcut hints and visible active/focus states.
Keep meaningful track/device names as text; do not replace readable musical
context with ambiguous decoration. Use Live as the layout/composition reference.

## Modern performance and latency rule

Treat 100-track projects, dense clip editing and large device workloads as core
requirements. Read the owning source contracts and benchmarks before changing audio scheduling,
device contracts, transport or large-project interaction. Keep UI
feedback prompt; decoding, plugin loading and graph compilation must not block
interaction or occur on Play. Distinguish pending edits from applied audio state.

Use modern Apple Silicon/x86-64 capabilities when measured benefit justifies
SIMD, cache/layout or bounded multicore work. Preserve portable fallbacks and the
unsafe-code policy. Parallelize independent graph paths, never ignore serial
chain dependencies or spend callback deadlines on unnecessary synchronization.
Do not equate high core utilization or accelerator use with better performance.

Devices declare algorithmic latency; the host propagates cumulative timing and
aligns reconverging audio/events. Never describe compensation as removing physical
latency, silently bypass devices, or trade away initial transients for an instant
Play claim. Prepared playback should start without deliberate host waits; record
first-audible and monitoring constraints separately. Publish hardware/workload
and sustained timing evidence before scalability or speed claims.

## Hybrid track rule

All canonical tracks accept both audio and MIDI clips under one stable TrackId
and shared mixer controls. Clip/source kind determines editing and playback,
not track capability. Do not reintroduce exclusive audio/instrument track types,
paired track identities or cross-type move/import rejection. Arrangement owns
both clip collections; Session owns one audio/MIDI/Stop cell per track/scene.
Preserve frame-anchored audio and beat-anchored MIDI, current overlap policies,
combined ID/count/admission limits, source-specific editing and safe release tails.
Identify current persistence changes explicitly and preserve current-format IDs,
source data, mixer state and history integrity. Supported older saved formats require
explicit versioned migration. Read the canonical project model and its tests before
extending this model.

## Shared Lattice device interface

Utility is the visual testbed for the shared native Lattice UI, not a special
appearance implementation. Frames, knobs, sliders, numeric readouts, toggles,
selectors, popup layering and common interaction contracts belong to
`hex-widgets`. Native device panels compose these controls and bind descriptors
and canonical commands. Shared styling changes must propagate to every device
using the control; do not fork appearance by device identity. Reusable density
variants may serve different layouts. Keep device-specific DSP and parameter
semantics outside the widget library.
