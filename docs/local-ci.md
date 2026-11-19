# local checks

Run from the repository root:

```sh
python3 scripts/local-ci.py
python3 scripts/local-ci.py --profile merge --base BASE_SHA --source HEAD_SHA
python3 scripts/local-ci.py --quick
python3 scripts/local-ci.py --help
```

No flags runs the complete audit. Merge resolves the runner's plan from actual
Git inputs and Cargo dependencies. Quick runs guards/formatting only and records
partial feedback. Runner certification and reviewed change-based verification
are distinct evidence routes under AGENTS.md; neither implies native acceptance.
See [development-workflow.md](development-workflow.md).

## Default sequence for an issue

Use the cheapest check that can fail first; do not use the merge runner as
first feedback.

1. **Focused tests** for the changed boundary and its named consumers, for
   example `cargo test -p OWNER --lib FILTER` (target under ten seconds; see
   [focused commands](#focused-commands)).
2. **Preflight**: `python3 scripts/local-ci.py --quick` plus
   `cargo clippy -p OWNER --all-targets [--features …] --locked -- -D warnings`
   for each touched package and its relevant feature configurations. This
   catches style, guard and lint failures in about a minute.
3. **Merge qualification**: run the merge profile (below). Where the selector
   has no narrower supported boundary and would widen far beyond the change's
   actual impact, a reviewed change-based check set may qualify instead under
   AGENTS.md, recorded in the PR as shown in the template below. Allocation-probe,
   persistence and Undo/Redo requirements of the touched boundaries still apply.

Complete verification before merging. When the base moves during step 3,
[carry](#continue-after-the-base-advances) eligible evidence instead of
rerunning everything. Do not run a routine post-merge check.

When several sessions share the machine, run one Cargo-heavy command (test,
clippy, build or runner) at a time. A session's own worktree target may need
`--adopt-target` on its first runner use; never adopt another session's target.
Do not edit a checkout while a runner is using it: a changed tree marks the
run stale, and Python self-tests that start scripts would test the edit.

The `--quick` profile runs guards and `cargo fmt --all --check`. It can run in
an unchanged checkout during another executor's build.

For Cargo-heavy verification, wrap the entire command (including scripts that
may spawn Cargo) in the shared slot:

```sh
python3 scripts/cargo-slot.py cargo test -p OWNER --locked
python3 scripts/cargo-slot.py --wait-seconds 60 cargo test -p OWNER --locked FILTER
python3 scripts/cargo-slot.py python3 scripts/local-ci.py --profile merge --base BASE_SHA --source HEAD_SHA
```

All participating Hex and Yield worktrees on the same user account must use the
same `~/.cache/hex/cargo-slot.lock`. From a Yield worktree, invoke this wrapper
by its absolute path in a Hex checkout. An upgraded wrapper first reserves the
next turn using the sibling `cargo-slot.lock.admission` lock, then waits for
the execution lock. Once it reports `next cargo slot reserved`, later upgraded
wrappers wait behind it. Order among waiters for the reservation is unspecified.
Legacy wrappers still share the execution lock and can contend for the next turn.

Set `--wait-seconds` to the remaining time available for admission. It covers
both locks, accepts zero for an immediate attempt, and exits 75 with a timeout
message before starting the command when the budget expires. Without it, the
wrapper waits until it acquires both locks. `cargo slot acquired` confirms
admission; the child exit status reports the command result. Record admission
wait separately from build and test execution. The active command keeps its
slot until it finishes or releases its inherited descriptor.

The wrapper passes only the execution lock to the child, so killing the
wrapper does not free the slot while the child retains that descriptor. Leave
both lock files in place. These advisory locks are separate from the runner's
target-custody lock and cover cooperating commands. Check for nonparticipants and
coordinate with their owners before starting; do not kill their work.
`HEX_CARGO_SLOT_FILE` is for isolated tests: different values do not coordinate
with each other.

Reviewed check set template for a PR description:

```text
## Verification (reviewed change-based check set)
Base BASE_SHA, source SOURCE_SHA.
Owners: crate::module (responsibility); contracts: …; consumers: …
- cargo test -p OWNER --locked [--features …]: passed (N tests)
- cargo test -p CONSUMER --test NAME --locked: passed (N tests)
- cargo clippy -p OWNER -p CONSUMER --all-targets --locked -- -D warnings: passed
- python3 scripts/local-ci.py --quick: passed
Omitted, with reasons: …
Not a runner-certified plan; native acceptance remains in Pulse.
```

The runner executes sequentially and stops on failure. Read the numbered log for
the failing step; later `not_run` steps did not pass. Results are in
`artifacts/local-ci/<run-id>/summary.json` and adjacent raw logs. A runner-certified run
must pass on the exact final commit with an unchanged source digest and no stale
flag. Preserve failed/interrupted logs; do not weaken assertions to obtain green.

Failed, interrupted, stale and incomplete runs print a final diagnostic after
cleanup and source checks: profile, owning step, command, workspace, summary and
direct numbered-log path. Recognized compiler/test failures get at most 20 lines
and 4000 characters from the last 64 KiB of the log. Earlier or unrecognized
diagnostics remain in the complete log; unreadable logs have a controlled fallback.
Console text removes terminal controls and redacts known sensitive environment
values and credential-like assignments/options; retained logs and argv stay intact.

If report publication fails, stderr identifies the failed write/publication phase
and retains the actual command result and raw log path when work already ran.
The last summary may remain incomplete; even successful commands cannot qualify
without the report. Keep those original bytes and logs, restore storage, and run
fresh checks. A publication failure does not authorize checkpoint continuation.
Commands over 2000 characters are explicitly truncated and refer to the summary.

The printed rerun is diagnostic only and partial, even when its original step was
part of a merge plan. It does not acquire the runner's target lock or reproduce
the complete verification context. Use the same toolchain/configuration/target
and normal repository runner for guarded checks. A stale result requires freezing
the intended candidate and rerunning its required plan. Diagnostic presentation
does not change exit codes, qualification, skipped steps or ownership cleanup.

The copied plugin transport probes live in `hex-plugin-host`; run
`cargo test -p hex-plugin-host --features allocation-probe --locked` alongside
the audio allocation checks when changing that boundary. The local runner
includes both owners; enabling a dependency feature alone does not execute its
unit tests. The counter stays dev-only and native worker I/O stays off callback.

## Merge selection

The pure audio-admission, navigation-profile selection, pending-edit receipt,
play-intent, recovery-state, viewport, MIDI-thumbnail geometry, CPU-meter,
track-meter scale and disk-overload tests run in the small `hex-ui` target
`unit_editor_state`; the selector includes it for each owner and its enclosing
modules. Recovery-state changes also select its real prepared-audio integration
test. Run the unit target with
`cargo test -p hex-ui --test unit_editor_state --locked`.
The source-local piano-roll geometry tests use the separate `ui-unit-tier`
feature and `unit_piano_roll` target:

```sh
cargo test -p hex-ui --features ui-unit-tier --test unit_piano_roll --locked
```

They are intentionally absent from the default and `ui-test-support` library
runs; the full and merge
plans explicitly select the unit target when required. The existing ignored
dense-scene benchmark can be selected on that target with `--release -- --ignored`.
Timeline geometry and signed viewport tests use the same feature in the
`unit_timeline` target. Run it with:

```sh
cargo test -p hex-ui --features ui-unit-tier --test unit_timeline --locked
```

The merge selector also runs this target for changes to its viewport dependency.
Automation plot tests use:

```sh
cargo test -p hex-ui --features ui-unit-tier --test unit_automation --locked
```

Device display tests use:

```sh
cargo test -p hex-ui --features ui-unit-tier --test unit_device --locked
```

Section hit tests use:

```sh
cargo test -p hex-ui --features ui-unit-tier --test unit_section --locked
```

Editor logic tests, including Session route view state, EQ response sampling,
overview-waveform, loop and stretch projections, waveform-grid, peak and fade paint geometry,
clip-drag geometry,
arrangement-map and group-overview projections, browser tree and sample state, recent-project labels, session-receipt reconciliation,
output-meter logic, project tempo value conversion, and passive time-move interval splitting, use:

```sh
cargo test -p hex-ui --features ui-unit-tier --test unit_editor_logic --locked
```

The seven default standalone targets run with the default package tests.
Whole-package plans that elide the default package run select those standalone
targets directly. A change confined to reviewed standalone unit test targets and
harnesses runs the owning unit targets and keeps the reviewed feature Clippy
checks. Changes to product source still select its unit and rendered support
coverage.

The windowed library and standalone MIDI pressure test use
`ui-test-support` in separate invocations:

```sh
cargo test -p hex-ui --features ui-test-support --lib --locked
cargo test -p hex-ui --features ui-test-support --test midi_pressure_boundary --locked
```

Run `cargo test -p hex-ui --tests --locked` for the default library and
default standalone unit targets. The six `ui-unit-tier` targets above complete
the unit tier. The two `ui-test-support` invocations form the integration tier,
including rendered focus and shortcuts, pending-edit receipts, delayed
acknowledgement repaint, and open/save flows. For each tier, time the combined
setup and test execution with compilation measured separately. The unit limit
is 30 seconds and the integration limit is three minutes.

Plans containing self-tests run the existing changelog, architecture, dependency,
UI/layout and formatting checks first, once each, before Python self-tests and
slower Cargo checks. A failure stops the plan and records later steps as `not_run`.
Ordering changes no required commands, configurations or qualification rules;
quick/edit/custom probes remain partial. Preflight source/target checks still apply.

`scripts/ci_plan.py` selects checks from both sides of changed Git paths, Cargo
ownership and transitive normal/dev/build/optional/target dependencies.

**Owner boundaries (policy since 1 November 2026).** A merge runs the test suites
of the packages that own the changed source, in every configuration, plus reviewed
runtime consumers with no Cargo edge (opaque device state in hex-storage). Every
transitive Cargo consumer still compiles and lints, default and feature
configurations, so an API break cannot reach main, but consumer suites are not
rerun. The owner's boundary tests are what guarantee behaviour downstream. If a
consumer later breaks, the defect is a missing owner boundary test: add it at the
owner. Package manifests and build scripts can change how consumers build, so they
keep the whole affected closure's suites.

**Profile-only root manifest changes.** A root `Cargo.toml` change whose
every changed table is `[profile.NAME.package.MEMBER]` (a workspace member, never
`"*"`) using only `opt-level`, `debug`, `codegen-units`, `incremental`,
`split-debuginfo` or `strip` selects those members as if their crate roots
changed: their whole suites test, consumers lint. These keys change codegen, not
program meaning. Any other root-manifest change, including `debug-assertions`,
`overflow-checks`, `panic`, `lto`, dependencies or workspace members, keeps the
comprehensive audit.

**Module scope.** Inside an owner, a changed `src/` file runs only the tests of
its own Rust module subtree. The planner resolves `mod`/`#[path]` from the crate
root and runs `cargo test … -- module::path::` in each of the owner's test
configurations, including sharded ones via `test_shards.py --filter`. A module
with no tests of its own selects its nearest ancestor module that has tests.
The crate root, `include!`, non-`src/` inputs and manifests keep whole-crate
tests. One configuration may hold none of a module's tests, so module-scoped
steps are checked together: a merge whose changed modules pass no test fails
with "changed modules ran no tests". Example targets (the device gallery) are
skipped for module-scoped owners. Smaller, self-tested modules therefore mean
faster merges; an untested module inherits its parent's often-large suite.

Standalone `scripts/test_shards.py` refuses an empty combined selection, including
filter typos and skips removing every case. The runner passes `--allow-empty` only
for module-scoped configurations whose coverage it checks together. That option
reports an empty selection without claiming tests passed; callers must verify
aggregate coverage themselves. An empty target alongside selected tests is allowed.

A changed module root (for example `editor/mod.rs`) runs its complete test
subtree, including tests in child modules, regardless of which child names
appear in the diff. Module-scoped doctest steps run only when a changed file
carries a doc example (a fenced block in `///` or `//!` comments). A reviewed runtime
consumer that is itself the changed owner (for example hex-storage) is
module-scoped like any owner.

For a module-scoped `hex-session` automation-take adoption change, the
planner also runs the `automation_take_density` integration target when it is
present. It exercises the real durable overflow boundary and must run at least
one test. Whole-package checks already include it; unrelated modules stay scoped.

Python tooling follows the same rule. A changed `scripts/NAME.py` (or
`scripts/NAME`) runs its own `test_NAME` module, and a changed
`scripts/tests/test_X.py` runs itself. Both add the shared planner invariants.
Scripts without their own test module, config files and unknown inputs keep
comprehensive discovery. Ordinary unscoped full audits and comprehensive merge
checks without explicit native acceptance inputs
run that inventory through `scripts/python_shards.py --discover` with up to
eight workers and batches of up to eight ordinary cases from the same
module or package. When Git on the
original PATH is Apple's `/usr/bin/git` launcher, sharded runs resolve the
selected Git once with `xcrun` and put only that executable ahead of the
original PATH for its child processes. Full audits with explicit native
acceptance inputs, scoped full plans, and the release and health profiles
retain serial comprehensive discovery. Unscoped full reports record sharded
discovery explicitly. Reports predating that marker retain their serial
configuration for certificate validation. Owner selections run through `scripts/python_shards.py`:
ordinary cases from the same module are batched up to eight per process in a
shared work queue with a default cap of 12 workers.
Subprocess-heavy test methods explicitly marked `__hex_shard_bounded__ = True`
use at most two workers. Owner selections and discovery queue them after the
first ordinary wave and share the same worker limit. Real Cargo fixture
classes marked `__hex_shard_early__ = True` are queued as isolated cases
ahead of other ordinary batches. Both modes report every case and include
its execution time.

Module-scoped test steps build only the library (or, for a binary-only crate,
the binaries): a `module::path::` filter can never match an integration test,
example or bench, so building and linking those targets was pure overhead
(hex-session has 28 test and 7 example targets, hex-storage 34). Explicit
source-by-path targets are exceptions. Arrangement focus, track indentation and
track-reorder subtree changes also select the standalone `hex-ui`
`track_group_focus` unit target. It owns the in-memory visibility and indentation
assertions. Changes to those sources also select the rendered consumers in the
`ui-test-support` library.

**One-minute budget.** Merge runs keep test setup and execution, excluding Cargo's
build phase, under 60 s. local-ci sums executed test steps (wall time minus the
reported build phase; carried steps excluded) into `test_budget` in the summary.
Exceeding the limit prints a warning naming the slowest steps; it does not fail
the run, and a run that is otherwise green qualifies (user decision,
1 November 2026; overruns are investigated in Pulse #4100). Record a recurring
slow owner suite in Pulse, and optionally pass `--over-budget-reason "Pulse #N …"`
to keep the reason in the summary.
Mixed tooling/Rust changes combine comprehensive Python checks with affected Rust
configurations. Contributor prose, including AGENTS.md, gets link/path checks;
executable selector/runner policy gets comprehensive tooling tests and guards.

The first narrower device boundary is compressor implementation and owning tests.
With host interfaces, registration and build inputs unchanged, it selects real
compressor DSP, session revision, generic device-chain, callback/allocation,
storage round-trip and UI source tests. All affected Cargo/runtime consumers still
compile through default and relevant feature Clippy checks. Whole unrelated
instrument/export and UI/audio behavior suites are omitted with reasons in the
plan. A new consumer or macro/include input widens to complete affected-package
tests; missing required consumers/targets refuse. Other device modules currently
retain affected-package coverage until their runtime relationships are reviewed.

Changes that only touch code other packages cannot observe test the owning
packages only: integration tests, benches and examples, and `src/` files whose
every `mod` declaration (resolving `#[path]`) is `#[cfg(test)]` or
`#[cfg(all(test, …))]`, directly or through a parent test module. `any(`/`not(`
conditions, inline-module nesting, `include!(` in the package, deleted files and
any production path in the same change keep the consumer closure.

hex-desktop's allocation-probe tests run through `scripts/test_shards.py` as four
disjoint processes: its native fixtures serialize on a process-wide output lease,
so one process ran them back to back (109 s). Each shard owns its own lease and
prints its own `test result`; every listed test runs exactly once (70 s).

Packages whose only extra feature is reviewed as additive for tests (see
`ADDITIVE_TEST_FEATURES` in `scripts/ci_plan.py`) run their tests once with the
feature and keep a separate doctest run; default-feature Clippy still applies.
The hex-ui gesture barrier and waveform geometry unit tests use the default
library. Merge plans select the relevant modules,
`editor::device_gesture::unit_tests::` and `editor::waveform_geometry::tests::`.
Run either directly with `cargo test -p hex-ui --lib --locked MODULE`.

Package plans run the owners' default tests and applicable allocation/UI/export
feature tests. Consumers lint in two invocations: default features
(`affected-clippy`, which keeps `cfg(not(feature))` branches compiling) and every
reviewed feature configuration together (`affected-feature-clippy`). Separate
per-feature lints recompiled the shared dependency chain each time (55–112 s
against 38–42 s, measured 1 November 2026). Formatting checks only the changed
owners' packages. The
desktop build runs when hex-desktop owns the change. Feature combinations need reviewed contract rules; individual
feature runs do not prove all combinations. Global manifests/lockfiles, toolchain,
vendor, unowned resources and unknown inputs conservatively select the comprehensive
audit with explicit input reasons. Review unknown effects rather than assuming
an omitted domain is covered. A merge or core path alone does not require full.
Full/release profiles and native delivery keep their comprehensive configurations.

The existing summary retains changed inputs, exact selected argv, boundary and
Python exclusions/rationale, source digest, counts, logs and cache identity. Empty
named targets, zero passing tests, stale source or mismatched reuse evidence cannot
qualify. Quick/edit/custom probes remain partial. For an unsupported boundary a
reviewed change-based command set remains available under AGENTS.md; record its
source/base, contracts/consumers, configurations, results and limitations in the PR.

Reviewed evidence reuse must identify the original tested inputs and assumptions.
The runner's `--reuse-report SUMMARY.json` is narrower: it accepts an original
complete runtime report for a prose-only continuation with matching executable
inputs and guarded dependencies. It does not compose arbitrary per-owner reports.
Each report records `native_acceptance_opt_in` from the actual runner environment,
without publishing fixture paths or signing credentials. Reuse requires an
explicit `false` in the original report and its reuse receipt. Missing historical
flags are unknown: retain the old report and run fresh checks. Any current
`HEX_VST3_*` or `HEX_TEST_SIGNING_IDENTITY` opt-in refuses reuse before target
preparation or stage execution, including when the environment changed after
plan resolution. External fixture/receipt/SDK bytes and Keychain state can change
at the same path; those checks require fresh observations. Ordinary fresh runs
and direct qualification of complete full audits retain their existing workflow.
Neither reviewed merge evidence nor a merge certificate replaces the full audit
required by [qualified native delivery](test-builds.md).

For ordinary app-only Rust merge inputs, reviewed Python shared invariants run
alongside affected Rust checks and repository guards. Tooling, executable policy,
build/configuration, mixed UI/core or unknown inputs, and new/missing/nested test
modules retain comprehensive discovery. Full/release profiles always discover all
modules. The summary records selected and excluded groups and exact selectors.

Explicit native acceptance inputs (any nonempty `HEX_VST3_*`, including partial
SDK/fixture/receipt setup, or `HEX_TEST_SIGNING_IDENTITY`) keep comprehensive
Python discovery for app-only merges too. Native suites retain their own opt-in
requirements; a skipped fixture is not native acceptance.

## Continue after the base advances

When `main` moves while a merge run is in progress, or right after it passes,
merge the new base into the candidate and rerun with the earlier passed report:

```sh
git fetch origin main && git merge --no-edit origin/main
python3 scripts/local-ci.py --profile merge --base "$(git rev-parse origin/main)" \
  --source "$(git rev-parse HEAD)" --carry-report artifacts/local-ci/PASSED/summary.json
```

A passed merge run prints this hint with its own summary path. The carry is
accepted only when every executable path changed since the earlier candidate
came verbatim from the base advance, touches none of the candidate's own paths,
and is ordinary package source. A changed manifest, lockfile, build script,
toolchain, tooling or unknown input refuses the carry, and so do a different
Cargo inventory, guarded toolchain/dependency identity or native opt-in. A
refused carry is recorded and every step runs fresh; it never fails the run.

Per step, guards and formatting always rerun. A Cargo step is carried only when
the dependency closure of every package it names excludes all packages the base
advance changed; a workspace-wide step carries only when no package changed.
The Python step reruns just the selection the changed paths require. Carried
steps keep `status: carried`, `execution: carried` and `carried_from` (the report,
commit and log that actually executed them, preserved through chained carries);
the summary's `carry` lists each decision and its reason. A carried report is
not a fresh full run and cannot feed `--reuse-report`.

## Investigate main on request

The health profile remains available when a specific investigation calls for
checking a merge commit on an up-to-date main checkout:

```sh
git switch main && git pull --ff-only
python3 scripts/local-ci.py --profile health --base HEAD^1 --source HEAD
```

`health` compares the merge commit with its first parent and runs formatting
plus the tests of the merged change's own packages, not every consumer (tooling
paths add the Python self-tests; unknown or high-risk inputs test the
workspace). It is a fast red-main signal, never merge qualification, so a pass
reports `partial`. A failure prints the merge that turned main red: fix forward
in a focused PR before merging anything else, and tell other active sessions.

When health runs Python checks for exclusively reviewed tooling paths, it selects
each changed owner's test module and the shared planner invariants. Mixed,
unowned, unknown, or native-acceptance inputs retain comprehensive discovery.

## macOS target storage

Use a persistent Cargo target on APFS. Set `CARGO_TARGET_DIR` to a fresh directory
on that volume before the normal local-CI command. For an existing populated
target, retain the ordinary ownership checks; do not adopt or delete another
executor's cache merely to bypass a refusal.

The dev and release guards identify the nearest existing target ancestor's local
device with `df -P`, then read `diskutil info -plist`. Each subprocess has a
five-second timeout. Missing, failed or malformed macOS volume metadata refuses
preparation before cleanup or owner publication. macOS ExFAT targets refuse
explicitly: observed AppleDouble cleanup failures and test-executable launch
stalls make that storage unsuitable for these checks. The launch-stall cause
is unproven; the guard does not change OS security or relocate executables.
Other platforms retain their existing storage behavior without a new filesystem
qualification claim.

If a test executable stalls before the harness starts on an APFS target, try a
separate fresh APFS target outside the existing target directory, for example:

```sh
export CARGO_TARGET_DIR="$PWD/artifacts/cargo-ci-target"
```

Keep that target for the same executor and run the normal checks again. Preserve
the previous target and failed report; changing the location is not evidence that
every test meets its time limit. Measure test setup and execution, including any
copying or other setup, and investigate any remaining slow test separately.

For a one-time dev target admission, first create an empty target directory,
set its explicit absolute path as `CARGO_TARGET_DIR`, and pass `--fresh-target`
to a full or scoped run.

```sh
mkdir -p artifacts/cargo-ci-target
CARGO_TARGET_DIR="$PWD/artifacts/cargo-ci-target" \
  python3 scripts/cargo-slot.py python3 scripts/local-ci.py --profile full --fresh-target
```

The target must already exist, be empty and have no existing owner. An absent
target is refused before Cargo metadata: the runner cannot yet prove the identity
of a directory it creates before opening it. Create an empty directory yourself
and retry; existing target contents are never removed. The reservation
pins directory ancestry and holds the same lock across planner metadata, runner
metadata, create-only owner publication and guarded checks. Metadata temporarily
disables the rustc information cache, then must report the selected workspace
and target and leave the directory fresh. The report records `claim-fresh` and
the admitted target identity. Use the ordinary command to reuse that owned cache
later. Resume and adoption have their own ownership contracts. Config-inferred
targets use the ordinary path.

The programmatic dev and release guards can also accept a live
`FreshTargetReservation` acquired before the caller starts Cargo. The reservation
invalidates authority when closed. Raw Cargo and other processes which ignore the
cooperative target lock remain outside this observation.
A target-wide `.admission-pending` sentinel beside its lock makes cooperating
lock users refuse before cleanup or build, including explicit adoption and
release recovery. Its contents are not trusted: any entry, including a dangling
symlink, requires preserving it and choosing a fresh target. The standalone dev
publisher syncs this sentinel before creating the existing dev owner record,
verifies and syncs that record, then removes the sentinel before transferring the
same lock to the runner. A failed publication before removal preserves the
sentinel. A failed final directory sync denies handoff after the verified owner
has been made durable. After a crash the sentinel can still be present, in which
case the target remains unavailable to cooperating consumers.
The refusal relies on the same intact lock-state directory; a replaced state
directory can hide the sentinel. Use a target dedicated to current cooperating
runners. The ordinary guarded path may run initial Cargo metadata before locking.
For the opt-in identity-resolution API, the caller must acquire a reservation
before its first Cargo operation and retain it through the later custody
decision. Metadata must report exactly the explicit target and leave it fresh;
resolution alone neither admits the target nor publishes ownership. A target
inferred only from Cargo configuration does not meet this API's proof contract.
The dev publisher uses the existing owner schema for pre-existing empty targets.
Identity-bound creation for absent targets remains unsupported.

The summary records the identified filesystem, probe duration and dev cleanup
duration (including failed cleanup). A target
preparation failure names that phase and the next unstarted stage; a failed
cleanup links `target-handoff-clean.log`. It does not blame the preceding passing
format or test stage. Preserve the failed report and original target contents,
choose an APFS target, and rerun with normal provenance and deadlines.

## Continue after a disk refusal

A run that stops **between stages solely because free space fell below the
operational floor** can retain a `checkpoint` in its failed summary. After
recovering space through supported cleanup of your own inactive generated
development outputs, repeat the exact profile/base/source with:

```sh
python3 scripts/local-ci.py --resume-report artifacts/local-ci/ORIGINAL/summary.json
# For a scoped plan, also repeat its original --profile, --base and --source.
```

The original report stays failed and unchanged. A new report identifies each
inherited stage, its original report/log hashes and the remaining executed
stages. Inherited timing is null in the new run; original timings remain in the
original report. `disk_history` retains each headroom probe. Compare original
versus new command counts, cache cleanup, elapsed time and minimum free space;
inheriting observations does not imply that Cargo artifacts are reusable.

Continuation requires a nonempty passing prefix, an unexecuted suffix, the same
clean exact source, complete plan/argv/configurations, check/build environment, command
binaries, toolchain, dependencies, target directory identity and unchanged owner
record. The normal exclusive target lock and conservative cache invalidation
still apply. A conservative macOS/Linux process snapshot refuses any visible
Cargo/compiler job, including jobs whose target is unknown; wait for them to
finish. An unavailable/unrecognized process inventory refuses too. Preserve the
original summary and raw logs at their recorded paths.
Shell bookkeeping and the per-invocation `CODEX_TOOL_CALL_ID` are excluded from
the environment identity; actual build/check configuration remains bound.
Missing/changed evidence, test failures, zero passing tests, interruptions, stale
source, changed/foreign/recreated ownership or a lower/changed disk floor refuse.
No automatic deletion or adoption occurs; the minimum remains at least 2 GiB.
Explicit `HEX_VST3_*` or `HEX_TEST_SIGNING_IDENTITY` inputs disable checkpointing:
external fixture/receipt trees and Keychain state can change at unchanged paths.
Those runs still execute their complete ordinary validation; use a fresh run.

Only original runs can continue; continued runs never emit another checkpoint.
A refusal before a guarded Cargo target was established has no checkpoint. A
refused attempt that has not changed the target owner can retry the original
checkpoint after its blocker is removed. Once the owner advances, a failed
continuation requires a fresh run. Do not run raw Cargo writers against the
target concurrently; the cooperative lock cannot control unrelated commands.
Local hashes detect accidental evidence changes, not coordinated forgery. A
complete resumed full audit can qualify native packaging only when all required
stages pass and the packaging consumer revalidates the inherited lineage; a
partial/scoped plan cannot replace that audit. `--reuse-report` keeps its separate
prose-only complete-certificate contract.

## Measure feedback time

Each executed step records monotonic `start_offset_seconds` and `duration_seconds`;
the run records `time_to_first_result_seconds` and total `duration_seconds`.
Offsets start at runner construction, after CLI plan resolution. Measure invocation
wall time separately when that preparation matters. Durations survive command
failure and interruption; `not_run` steps have null timing. UTC timestamps remain
for correlation, not precise elapsed measurements.

Per-step `timing_estimates` contains log-derived Cargo build-phase, Rust harness
and Python harness seconds plus the unclassified residual. Cargo's build phase
includes more than compiler CPU. Harness output is rounded and may be nested;
missing values and negative/overlapping residuals are null, not zero. Residual
time can include setup, I/O, process overhead and other work; do not label all of
it setup. These diagnostic fields never change qualification or test counts.

For a comparison, retain each summary and raw log with exact source/base,
toolchain, hardware, features, command and target/cache conditions. Start with a
warm quick run and one named owning edit regression. Use three paired runs for
cheap workloads, alternate order, and compare medians; retain warm-up/compilation
samples separately. Reuse justified full and representative UI merge reports
instead of adding repeated full audits. Label historical/different-source reports
and unavailable maintenance/diagnostic measures explicitly. Do not delete caches
to manufacture a saving or infer reduced detection from fewer executables.

Inspect a retained report without rerunning checks:

```sh
python3 - SUMMARY.json <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))
print(s['commit'], s['status'], s.get('time_to_first_result_seconds'), s['duration_seconds'])
for step in s['steps']:
    print(step['step_id'], step['status'], step.get('duration_seconds'), step.get('timing_estimates'))
PY
```

## Target ownership

Use one persistent Cargo target per executor, with no concurrent raw Cargo writer.
Repository tools acquire an advisory target lock. Unknown/foreign/pending cache
ownership refuses before normal checks. Read the diagnostic before recovery.
The lock file is opened without following a final symlink and its existing bytes
are left unchanged. Contention reports that the target is in use without reading
or displaying the untrusted lock-file contents as a holder identity. Older
runner versions may still display stale holder information from that file.
`--adopt-target` explicitly invalidates supported workspace development outputs
and establishes the checkout's development owner; it is not permission to remove
another executor's active cache, release artifacts or user data.

Development and release ownership are separate markers: a target populated by
local CI has no release owner, so packaging refuses it unless it is empty or
explicitly adopted. Set separate `CARGO_TARGET_DIR` values for local CI and
release packaging when both are needed; see [native macOS builds](test-builds.md).

Package-scoped handoffs use Cargo-supported plain package names only after
complete metadata proves each selected workspace/path name is unique across
sources and versions. Provenance retains full package IDs. Ambiguous names or
removed/version-changed path dependencies require a fresh target; qualifiers
cannot make Cargo cleanup narrower. Ordinary package handoffs preserve unrelated
dependencies and the other profile. Existing compiler-profile changes retain their
separate profile invalidation rule.

The runner may refuse insufficient disk headroom. Inspect and remove only known
reproducible, inactive generated outputs. Do not delete projects, source, accepted
artifacts or another worktree's target merely to get a run started. Do not edit
ownership markers to claim a successful check.

## Controlled macOS VST3 fixtures

The opt-in native instance tests use a clean, pinned VST3 SDK checkout and
original fixture bundles, not installed plug-ins. `build.py` owns the exact SDK
and submodule pins; `macos_bootstrap.py` checks those revisions, tracked changes
and MIT license files before building. Keep the checkout outside the repository
and choose fresh output directories. Set `HEX_VST3_SDK` locally to its path.

```sh
test ! -e artifacts/vst3-local-controlled
test ! -e artifacts/vst3-local-instance
python3 crates/hex-plugin-scan/native/macos_bootstrap.py build \
  --sdk "$HEX_VST3_SDK" --output artifacts/vst3-local-controlled
python3 crates/hex-plugin-scan/native/macos_bootstrap.py build --instance-only \
  --sdk "$HEX_VST3_SDK" --output artifacts/vst3-local-instance
python3 crates/hex-plugin-scan/native/macos_bootstrap.py verify \
  --receipt artifacts/vst3-local-controlled/controlled-macos.json
python3 crates/hex-plugin-scan/native/macos_bootstrap.py verify \
  --receipt artifacts/vst3-local-instance/controlled-macos.json
HEX_VST3_MACOS_RECEIPT="$PWD/artifacts/vst3-local-controlled/controlled-macos.json" \
HEX_VST3_INSTANCE_RECEIPT="$PWD/artifacts/vst3-local-instance/controlled-macos.json" \
  cargo test -p hex-plugin-host --lib --locked \
  native::tests::epoch::actual_current_capture_uses_same_quiescent_instance_and_keeps_startup_historical \
  -- --ignored --exact
```

Receipts are bound to the native sources and SDK pins. Rebuild them after those
inputs change. This controlled helper test does not establish installed plug-in,
native UI, listening, device deadline or release acceptance. Keep SDK paths and
local receipts out of Git; record the exact receipt hashes and result with the
issue or PR being verified.

### Signed VST3 editor regression timing

Use the optimized desktop test target to time the signed regression that
checks whether a late editor value reaches the original worker across graph
publication. Use the verified fixture receipts from the commands above.

```sh
HEX_VST3_MACOS_RECEIPT="$PWD/artifacts/vst3-local-controlled/controlled-macos.json" \
HEX_VST3_INSTANCE_RECEIPT="$PWD/artifacts/vst3-local-instance/controlled-macos.json" \
  cargo test --locked --release -p hex-desktop --features allocation-probe --bin hex-desktop \
  -- --ignored --exact --test-threads=1 \
  native_audio::tests::plugins::capture_source_guard::late_editor_value_remains_on_original_worker_after_unlaned_publication
```

Measure first and repeated invocations separately and record both process wall
time and the test harness duration. Report compilation separately. A fresh
copy of the desktop executable can spend measurable time before libtest
produces its first output, including on a listing-only run.

### Long Re-Pitch regression timing

The verified-worker Re-Pitch regression imports a file past the resident limit,
applies history, seeks to audio beyond that limit, and saves and reopens the
project. Use the compact macOS arm64 test build for focused first-launch timing.

```sh
cargo --config 'profile.release.package.hex-desktop.opt-level="z"' \
  --config 'profile.release.package.hex-desktop.strip="symbols"' \
  --config 'profile.release.package.hex-ui.opt-level="z"' \
  --config 'profile.release.package.gpui.opt-level="z"' \
  --config 'profile.release.package.hex-engine.opt-level="z"' \
  --config 'profile.release.package.hex-audio.opt-level="z"' \
  test --release --locked -p hex-desktop --features allocation-probe \
  --bin hex-desktop --no-run --message-format=json
```

Use the executable from Cargo's `hex-desktop` test artifact. Prepare a local
APFS copy, sign that copy with `codesign --force --sign -` and verify it with
`codesign --verify --strict`. Run the following exact test with
`--exact --test-threads=1` on a fresh APFS copy of the signed executable, then
repeat on the same executable for warm timing.

`native_audio::tests::resident_lifecycle::ordinary_long_repitch_plays_and_reopens_through_verified_workers`

Record executable copy setup, process wall time and harness time. Report the
compilation and one-time signature preparation separately. The fresh copy tests
a new executable identity on the current filesystem cache.

### Dense playback baseline

The opt-in macOS arm64 profile uses the resident Session and prepared callback
with generated audio. The native and mixed runs use the same 48 tracks and 96
native effects. The mixed run also admits four instances of the controlled VST3
processing fixture. Export both verified receipt paths above before the mixed
run. The fixture uses a fixed 256-frame VST3 transport quantum. Select 64, 128,
or 256 frames per measured callback and keep that size fixed when comparing
revisions. Smaller callbacks feed fragments into the fixed transport quantum.

```sh
HEX_PLAYBACK_PROFILE_BLOCK=256 cargo test --release --locked -p hex-desktop \
  --features allocation-probe --bin hex-desktop \
  -- native_audio::tests::plugins::performance_profile \
  --ignored --nocapture --test-threads=1
```

The native-only test can run without the VST3 receipts. Each profile prints the
warm-up and measured block counts, elapsed-time percentiles, blocks that exceeded
their nominal budget, first nonzero audio, observed presentation lag, and PCM
fingerprint. Run alternating candidates on the same machine and compare
fingerprints only within the same workload and block size. Measure aggregate
process CPU and physical output/monitoring latency separately. The paced
headless callback does not verify device deadlines or listening quality.

## Focused commands

For partial edit feedback, optionally select the owning library or named
integration test from the package's Cargo metadata:

```sh
python3 scripts/local-ci.py --profile edit --base BASE_SHA --source HEAD_SHA \
  --package hex-storage --test-filter injected_precommit_failure --lib
python3 scripts/local-ci.py --profile edit --base BASE_SHA --source HEAD_SHA \
  --package hex-storage --test-filter TEST_NAME --test INTEGRATION_TARGET
```

Omit `--lib`/`--test` to retain package-wide test selection. They are mutually
exclusive and require an explicit edit package/filter; unknown targets, disabled
required features and zero passing matches refuse qualification. Edit still runs
package Clippy and formatting and remains partial. These options do not change
runner merge/full/release plans.

```sh
python3 scripts/check-architecture.py
python3 scripts/check-gpui-patch.py
python3 scripts/check-dependency-patches.py
python3 scripts/check-ui-style.py
python3 scripts/check-device-layout.py
cargo fmt --all --check
cargo test -p PACKAGE --locked TEST_NAME
cargo test -p hex-audio --features allocation-probe --locked
cargo test -p hex-stream --features allocation-probe --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The headless callback probes share the `allocation_probes` integration target.
Use the former target name as a module filter to run one fixture family, for
example:

```sh
cargo test -p hex-audio --locked --features allocation-probe \
  --test allocation_probes -- track_gain_allocations::
```

The calibration, stage-clock and device-comparison targets still support
isolated runs. The stream-pressure target requires both `allocation-probe` and
`stream-test-support`.

For the focused Limiter stage-clock allocation check, a smaller test binary
can reduce cold-start time while retaining debug assertions and the same test
coverage. This opt-in command optimizes only `hex-engine`; it does not change
the workspace's default test profile or the application build:

```sh
CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_STRIP=symbols \
  cargo --config 'profile.test.package.hex-engine.opt-level=1' test \
  -p hex-audio --locked --features allocation-probe --test callback_stage_clocks \
  stage_clocks::limiter_output_gain_uses_delayed_content_clock_without_callback_allocations \
  -- --exact
```

Measure the selected check's wall time, including test-process startup, and
report compilation separately. This targeted profile is not a substitute for
the other affected owner and consumer checks, or for an unoptimized debugging
run when one is needed.

For a focused streamed-readiness or ordinary audio publication regression whose
library test startup exceeds the unit limit, an optimized test harness can retain
debug assertions and overflow checks:

```sh
mkdir -p artifacts
CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true \
  CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
  CARGO_PROFILE_RELEASE_STRIP=symbols cargo \
  --config 'profile.release.package.hex-engine.opt-level="z"' \
  --config 'profile.release.package.hex-stream.opt-level="z"' \
  --config 'profile.release.package.hex-audio.opt-level="z"' \
  --config 'profile.release.package.allocation-counter.opt-level=0' \
  test -p hex-audio --features allocation-probe --lib --release --locked \
  --no-run --message-format=json > artifacts/audio-test-build.jsonl
```

For the stream owner, use `-p hex-stream --features allocation-probe` with the
same profile settings. The Cargo JSON `compiler-artifact` with target `hex_audio`
or `hex_stream` and `profile.test` true identifies the executable. Invoke it
with the full test name and `--exact`;
confirm exactly one test ran and include process launch in the timing. Use the
corresponding `--test` target for separately linked consumer and calibration
checks. Do not set global `RUSTFLAGS` for symbol stripping: the PCM constructor
layout gate deliberately rejects that unsupported compiler configuration. The
profile does not guarantee every launch meets the limit or replace the other
affected owner, consumer, and configuration checks.

For allocator-probe sensitivity checks, the original two tests can also be
compiled as an optimized, separately linked executable while retaining the
test target's debug assertions and overflow checks. The pinned counter is
compiled with the same opt-level, debug-assertion and overflow settings as the
ordinary debug test profile:

```sh
mkdir -p artifacts
cargo --config 'profile.release.package.allocation-counter.opt-level=0' \
  --config 'profile.release.package.allocation-counter.debug-assertions=true' \
  --config 'profile.release.package.allocation-counter.overflow-checks=true' \
  rustc -p hex-audio --features allocation-probe \
  --test allocation_probe_calibration --release --locked --message-format=json \
  -- -C debug-assertions=yes -C overflow-checks=yes -C strip=symbols \
  > artifacts/allocation-calibration-build.jsonl
```

Use the JSON test artifact's `executable` path to run each calibration test
with its full name and `--exact`. Include process startup in each wall time;
report compilation separately. The normal test target and discovery remain
unchanged. This is an optional measurement profile, not proof that every cold
launch fits the limit or that the affected audio/stream tests are covered.

Focused tests can form part of a reviewed change-based merge check set when its
affected contracts, consumers and configurations are covered; an isolated passing
test alone does not establish that scope.
Allocation probes cover their named headless fixtures, not all callback faults,
native allocations, physical device deadlines or audible quality. Report those
limits. User native interaction and listening checks remain separate.

Use local checks only. The runner neither installs tools, starts the app, packages
an app nor contacts a hosted CI service. For the configured cloud prerequisites
see [cloud-environment.md](cloud-environment.md); Linux results do not establish
macOS native behavior.
