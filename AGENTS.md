# AGENTS.md: guide for AI agents and contributors

**Hex** is a native digital audio workstation written in **Rust**, using **GPUI**
for its desktop interface. The goal is to reconstruct Ableton Live's fundamental
Arrangement and Session workflows, audio/MIDI behavior, editing, recording,
automation, routing, and project fidelity, while improving architecture,
responsiveness, and measured performance.

## Active alpha compatibility policy

Hex is active alpha software. Until the user selects a satisfactory feature set
for release, breaking source, model and project-format changes are allowed.
Do not spend effort maintaining backward compatibility or adding migrations,
historical project conversions or compatibility readers solely for earlier alpha
versions. This includes incompatible unpublished branch formats. This policy
supersedes older backward-compatibility and migration requirements in this file,
repository documents and existing task descriptions for future Hex changes.
Select the release compatibility policy when the user chooses the release scope;
do not infer a compatibility freeze from an alpha version or schema number.

Active features must coexist in one coherent current format. Preserve strict
current-format validation, complete save/reopen round trips, explicit errors for
unsupported formats, runtime and real-time safety, and Undo/Redo integrity.
Never automatically delete, rewrite or convert existing user files to accommodate
a breaking change. This policy does not authorize removing all existing migration
code or unrelated cleanup. Historical implementation and verification evidence
remain factual records, not new compatibility obligations. PR, validation and
delivery requirements remain in force.

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
startup; no alternate font or silent system-font substitution. See docs/ui-design.md.
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
Read docs/reference-project.md for pinned source pointers and the patterns Hex
adopts. Use its command-driven engine/UI separation and evidence discipline,
adapted for real-time audio. Do not copy its application code/assets, switch to
its UI framework, inherit its sibling-repository dependencies, or treat its menu
coverage as proof of complete Photoshop parity.

Reference the installed `/Applications/Ableton Live 12 Suite.app`. Its inspected
build is **12.4.6 (2026-09-10_0de5c8fa9a)**; architecture and executable hash are in
`docs/research/reference.md`. Start with macOS arm64. Recheck the installed build
before new reference studies. “Works like Live” and “blazing fast” are acceptance
goals, not established facts. Read this file first, then the relevant docs.

## 1. Orientation (5 minutes)

| Read | Why |
| --- | --- |
| `README.md` | Current capabilities, dependencies, run commands, limitations |
| `docs/architecture.md` | Offline engine, current crate boundaries, future graph model |
| `docs/reference-project.md` | PhotoCraft patterns and audio-specific adaptations |
| `docs/ui-design.md` | Required lowercase text, bundled font, visual checks |
| `docs/runtime.md` | GPUI/audio seam, command delivery, transport and scene contracts |
| `docs/plan.md` | Milestones, acceptance gates, deferred dependencies, next work unit |
| `todo.md` | Implemented work, open checks, ordered backlog |
| `docs/features.md` | Honest assessment of all fundamental DAW areas |
| `docs/performance.md` | Measured baseline, methodology, performance gates |
| `docs/research/reference.md` | Exact Live reference identity and bounded binary inventory |
| `docs/research/workflow.md` | Behavior experiments and targeted decompilation protocol |
| `research/observations/template.md` | Required provenance and confidence for a reference experiment |

Read actual source before changing behavior. Documentation may lag the code;
resolve discrepancies and update the owning document. There is currently no
`xtask`, generated parity scorecard, corpus fetcher, control server, or release
pipeline. The local validation runner is documented in `docs/local-ci.md`; do not
claim hosted CI or automatic enforcement is configured.

## 2. Workspace map

```text
Cargo.toml                   workspace configuration and safety/style lints
Cargo.lock                   pinned resolved dependencies
crates/
  hex-dsp/                   host-independent stereo kernels and validated gain units
  hex-lattice/               native registry/runtime contracts and stereo compatibility reexports
  hex-lattice-contract/      neutral native device metadata and versioned state
  hex-devices/               independently testable original instrument DSP
  hex-project/               canonical control-side IDs, edits, history and snapshots
  hex-media/                 bounded WAV decode, source-rate conversion and cached waveform peaks
  hex-library/               bounded folder listings and persistent sample catalog
  hex-preferences/           versioned bounded application browser-place settings
  hex-stream/                verified source worker, bounded page cache and ownership queues
  hex-storage/               versioned metadata and recoverable create-new files
  hex-export/                fixed-block linear PCM16 WAV rendering and create-new publication
  hex-session/               canonical controller and bounded media/history coordination
  hex-engine/
    src/lib.rs               original deterministic offline arrangement renderer
    src/live.rs              fixed preloaded demo runtime, commands, telemetry
    examples/profile.rs      release render microbenchmark
  hex-cli/
    src/main.rs              generated demo and offline WAV export through hex-export
  hex-audio/
    src/lib.rs               CPAL stream, bounded queue, priority Stop, callback
  hex-widgets/               reusable native GPUI controls; injected display/style and edit events
  hex-ui/
    src/lib.rs               window and UI service entry point
    src/app.rs               GPUI view state, controls, drawing, shortcuts
    src/theme.rs             required lowercase text and embedded font
  hex-diagnostics/            per-session worker log I/O and ten-file retention
  hex-desktop/
    src/main.rs              startup/composition only; injects backend into UI
scripts/
  local-ci.py                sequential local validation and retained evidence
  inventory-live.sh          read-only reference bundle inventory
  bundle-macos.sh            local development app bundle; no publication
  check-architecture.py       crate goals and dependency policy
  check-ui-style.py           lowercase text/font/bundle source guards
research/
  observations/              behavior specifications, results, provenance
  private/                   ignored binaries/analysis databases; never distribute
artifacts/                   ignored demo exports, profiles, development app
```

**Clean, maintainable crate boundaries are mandatory.** Every cohesive subsystem
belongs in its own crate with one clear reason to exist. Each crate README defines
its purpose, dependencies, and what it does not own. Do not accumulate project
models, codecs, DSP, device I/O, UI, and startup in one giant crate. Conversely,
do not create a crate for every helper or empty speculative subsystem: split when
there is a real responsibility or independently testable boundary.

Current direction: `hex-lattice-contract` has no workspace dependency;
`hex-project` depends only on its neutral device metadata/state contract, never
DSP implementations. This deliberate shared-contract edge supports modular
devices without per-device storage branches. `hex-lattice` depends on the contract
and `hex-dsp`, and owns native processing/registration; `hex-dsp` owns
host-independent mathematical kernels with no workspace dependencies; `hex-devices`
owns registered algorithms and depends on lattice, its contract and hex-dsp.
The native DSP system is named **lattice**; read
`docs/lattice.md` for staged device integration and stateful lifecycle gates.
`hex-engine` depends
on hex-project/hex-devices and has no UI/device-I/O dependency;
`hex-audio` depends on engine and owns CPAL/rtrb; `hex-export` depends on engine
and owns shared offline WAV rendering/publication; `hex-ui` depends on
engine/audio/session contracts and owns GPUI; `hex-desktop` composes audio/UI and
application lifecycle; `hex-cli` uses project/engine/media/storage/export for
headless workflows. Media depends on engine/project; storage depends on project
and owns serialization. Session coordinates project/media/storage/export work
through bounded control workers; UI uses read-only project views and controller
commands, not its own editing model. Library owns bounded filesystem listings and recursive WAV discovery with a persistent derived sample catalog, preserving original paths;
UI owns browser navigation, filtering and focus. Library does not decode audio, edit projects or perform callback work. Catalog enumeration/cache I/O belongs on background control workers; see docs/sample-library.md. hex-preferences owns
versioned bounded application browser-place serialization and recoverable file
publication, independent of project storage/history. UI loads/saves it through
bounded background work. Read docs/browser-places.md; preserve original paths
and malformed settings, surface errors, and keep filesystem I/O off UI/callback.
Desktop must remain small. UI must not own
device construction or callback DSP. Audio must not own GPUI or project editing.

`hex-widgets` depends only on pinned GPUI and the standard library. It owns
reusable control rendering, focus and local gestures; `hex-ui` supplies lowercase
display text, inherited bundled typography, style and canonical bindings.
Widgets do not construct devices, edit projects, perform DSP, own audio queues
or access files.

`hex-stream` depends on engine/media/project and owns the verified page worker,
fixed pool and rtrb ownership queues. Engine borrows its narrow `MediaPages`
interface and has no stream/media dependency. The current streamed renderer is a
headless path; native audio/session integration is still required. rtrb is
allowed in audio and stream, while CPAL remains exclusive to audio. Do not lift
resident import limits or admit streamed native revisions until source/catalog
leases, readiness and callback retirement are integrated and checked.

Run `python3 scripts/check-architecture.py`. The check rejects undeclared crates,
forbidden workspace edges, misplaced GPUI/CPAL/rtrb dependencies, and missing
purpose/dependency/non-goal README sections. Register new crates in that policy
in the same change. Keep APIs narrow, module sizes readable, errors explicit,
public responsibilities documented, and dependencies acyclic. Put related behavior
and tests with their owning crate; don't duplicate state across boundaries.

Explicit test-only edges are allowed: `hex-audio` may use `hex-project`, `hex-media` and
`hex-lattice-contract` as dev-dependencies for canonical prepared-publication and
active-device allocation fixtures. Under the non-default `allocation-probe`
feature, `hex-audio` may also use `hex-lattice` as a dev-dependency and enable
the engine's `instrument-test-support` feature to inject a bounded mutable
instrument fixture through normal revision preparation. `hex-export` may use
`hex-project`, `hex-lattice` and `hex-lattice-contract` as dev-dependencies;
its non-default `instrument-test-support` feature forwards the engine fixture
seam for shared prepared-export PCM tests. Production audio must continue to
depend only on engine and its device/queue libraries; production export depends
only on engine and the standard library. The architecture guard distinguishes these dev edges from
runtime dependencies.

The next general architecture has one canonical editable project, stable IDs and
asset references, a control-side compiler, and an audio-owned compiled graph.
The fixed demo runtime is a temporary vertical slice. Do not grow it into a second
editable project model or add speculative crates just to fill a diagram.

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
  its license/provenance and narrow delta; see `docs/native-input.md` and run
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

1. **Behavior lives in the engine.** New musical or project behavior should be
   represented by a typed command with validation and tests. UI controls dispatch
   it; they do not implement a competing scheduler or DSP path. The current demo
   uses `hex_engine::live::Command`; a general editor command registry is future
   work. Pure view/window state may remain in GPUI.
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
6. **Keep GPUI thin and inspect it visually.** Separate UI display state from
   authoritative audio/project state. Draw cached waveform summaries and visible
   content; do not traverse/render a whole project every frame. Consolidate design
   tokens as the UI grows. Verify focus, shortcuts, resizing, interaction, and
   accessibility. Current custom controls do not yet expose rich accessibility.
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

1. Read `docs/features.md` for the honest fundamentals assessment. A visible menu
   item or button is not evidence that a feature works.
2. Read `docs/plan.md` and `todo.md` for the current focus: canonical editable audio
   project, WAV import, compiled playback, clip editing, save/load, and undo/redo.
3. Check `docs/performance.md` for measured regressions and unverified budgets.
4. Read related source and research observations before changing a contract.

Before substantial work, define finite requirements and acceptance criteria,
including deferred dependencies. Estimate size/risk from actual uncertainty. Use
the configured read-only budgeting helper before a large task, agent batch, or
material expansion; unknown data does not justify invented quota claims or model
switches. If budget concern arises, stabilize at a useful tested milestone and
review scope with the user. Never buy credits, change billing, consume resets, or
start API-billed fallback inference without explicit authorization.

Reference experiments and decompilation remain an ongoing research track.
Independent Hex functionality may progress while reference parity is unknown.
Do not mark an unchecked milestone complete because its starter code builds.

## 5. Before you finish a task

For Rust changes, run the checks that cover the affected behavior:

```sh
python3 scripts/local-ci.py              # full PR validation on the final tree
python3 scripts/local-ci.py --quick      # guards/format preflight only
python3 scripts/check-architecture.py
python3 scripts/check-gpui-patch.py
python3 scripts/check-ui-style.py
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p hex-desktop                 # desktop/backend changes
cargo run --release -p hex-engine --example profile  # demo-render hot path
sh scripts/bundle-macos.sh --help         # exact CI/revisions/checklist required; docs/test-builds.md
```

The full local runner is the default PR validation path. A quick run is useful
while editing but is never sufficient merge evidence. Tie a full run to its
recorded commit and unchanged tracked/untracked file digest; rerun after any tree
change. See `docs/local-ci.md` for the exact steps and retained logs.

Use targeted crate checks when appropriate; broaden only for a concrete dependency,
new failure, or unresolved risk. Re-run checks after a material repair. Docs-only
changes need link/path/claim review, not a full native rebuild.

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

Inspect the final diff when Git exists; otherwise review the changed files against
known starting content. Preserve unrelated work. Update the owning docs and
`todo.md` with actual results and remaining uncertainty. A concise final report
states what changed, why, what passed, and what remains open. Keep the workspace
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
  As requested on 10 October 2026, the user gives standing authorization to merge
  focused, completed PRs immediately when full local CI passes on the exact final
  commit and unchanged source digest. Do not wait for another approval or for
  native UI/listening tests. A quick, stale, partial, failed or interrupted run
  does not qualify. Merge dependencies first and revalidate after source changes
  or conflict resolution; verify that integration preserves the tested behavior.
  Known unresolved correctness bugs must be repaired before merging, even if an
  incomplete test suite is green. Never force-push `main`, skip required checks
  or weaken branch protection.
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
`in_progress`; use `ready_for_review` after implementation and source checks are
complete but review or native/user acceptance remains, and `in_review` while
review is underway. Explain blockers in the description. Before every update,
call `get_ticket` and pass its current revision as `expected_revision`; on
conflict, re-read and reconsider. Update fields selectively, preserving
descriptions, tags and source context. Mark `done` only when acceptance criteria
are met; source-complete work with pending native/user validation remains open.
Record unrelated discoveries during an active PR as follow-up tickets instead
of expanding that PR.

Preserve existing Pages/Linear records, imported context and source links. This
policy does not authorize bulk migration, deletion, archival, synchronization or
writes to other trackers. If Pulse tools, live connectivity or authentication
are unavailable, report the limitation and keep a clearly labeled temporary
plan in chat; do not silently switch trackers or claim that unsaved tickets were
created.

Repository documents retain technical contracts, request order and evidence:

- `docs/plan.md`: milestone contracts and acceptance gates, with Pulse pointers.
- `docs/issues.md`: technical constraints, decisions and supporting evidence.
- `todo.md`: dated implementation evidence and request-order reference; Pulse
  owns the active queue and ticket states.
- `docs/features.md`: fundamental workflows and honest capability assessment.
- `docs/ui-design.md`: mandatory lowercase text and Ioskeley Mono.
- `docs/reference-project.md`: PhotoCraft adaptation and pinned sources.
- `crates/*/README.md`: each crate's purpose, dependencies, and non-goals.
- `docs/architecture.md` and `docs/runtime.md`: boundaries, ownership, contracts.
- `docs/performance.md`: reproducible measured baseline and open performance gates.
- `docs/research/` and `research/observations/`: reference identity and behavior evidence.
- `artifacts/` and `research/private/`: ignored local outputs and analysis material.

No hosted/release pipeline, generated scorecard or devlog is currently configured.
The local validation runner is not hosted CI. Use Pulse for planning rather than
inventing a second authority.
Do not commit, push, publish, choose a distribution license, or change permissions
unless the user authorizes that action; the standing PR merge authorization above
applies. Inspect checkout topology and status before editing. The workspace now uses
Git on `main` with the private `PORTALSURFER/hex` remote, created at user request on
9 October 2026.

## 8. Keeping current projects complete

Schema-13 hybrid-track audio/MIDI metadata persistence, independent canonical
Session MIDI and unwarped audio one-shot sources/scenes/cells, finite Arrangement
source looping and strict schema-1–12 migration are implemented in source.
Every track owns both clip collections; source kind determines editing/rendering.
See docs/hybrid-tracks.md for integrated acceptance status. Schemas 1–10 receive an empty Session grid; schema 11 preserves its MIDI/Stop grid. Session
copies own fresh clip/note IDs and share MidiSource validation with Arrangement.
Session graph inventory, launching and compact UI are implemented and source-tested;
native input/listening and physical deadline acceptance remain open. Unwarped audio
is frame anchored; warped looping remains required. Read docs/session-audio.md
before changing its model, runtime or source controls. Schema 9 MIDI receives
one-shot source defaults. Schemas 1–4 receive neutral track balance and master gain defaults;
schemas 1–5 receive neutral clip fades; schemas 1–6 receive zero source phase; schemas 1–7 receive canonical 120 bpm/4/4
tempo and meter maps. Schemas 1–8 populate the hybrid track's audio collection. MIDI notes use
beat anchors and globally unique IDs; MIDI sources render an original bounded
sine instrument through shared native/offline playback. Candidate edits must compile
before history commits; native piano-roll creation, direct editing and saved-value
checks are implemented. Configurable-grid native acceptance passes; held-modifier gestures,
additive marquee/minimum-layout native selection acceptance and measured input/paint timing remain open. Read `docs/midi.md` before
changing the MIDI model, scheduler or runtime. Read `docs/musical-time.md` before changing timing: conversion
belongs on the control side and existing audio remains frame-anchored. Read `docs/clip-fades.md` for the fixed
fade domain, split continuity and deliberate trim reanchoring contract. As it grows:

- Identify the current schema explicitly; new model fields require current-format
  save/load and history review in the same change. Historical alpha migrations
  are not required under the active alpha policy above.
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
requirements. Read docs/performance.md and docs/latency.md before changing audio
scheduling, device contracts, transport or large-project interaction. Keep UI
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
source data, mixer state and history integrity. Earlier alpha format migration is
not required. Read docs/hybrid-tracks.md before extending this model.

## Shared Lattice device interface

Utility is the visual testbed for the shared native Lattice UI, not a special
appearance implementation. Frames, knobs, sliders, numeric readouts, toggles,
selectors, popup layering and common interaction contracts belong to
`hex-widgets`. Native device panels compose these controls and bind descriptors
and canonical commands. Shared styling changes must propagate to every device
using the control; do not fork appearance by device identity. Reusable density
variants may serve different layouts. Keep device-specific DSP and parameter
semantics outside the widget library.
