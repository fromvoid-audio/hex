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
Commands over 2000 characters are explicitly truncated and refer to the summary.

The printed rerun is diagnostic only and partial, even when its original step was
part of a merge plan. It does not acquire the runner's target lock or reproduce
the complete verification context. Use the same toolchain/configuration/target
and normal repository runner for guarded checks. A stale result requires freezing
the intended candidate and rerunning its required plan. Diagnostic presentation
does not change exit codes, qualification, skipped steps or ownership cleanup.

## Merge selection

Plans containing self-tests run the existing changelog, architecture, dependency,
UI/layout and formatting checks first, once each, before Python self-tests and
slower Cargo checks. A failure stops the plan and records later steps as `not_run`.
Ordering changes no required commands, configurations or qualification rules;
quick/edit/custom probes remain partial. Preflight source/target checks still apply.

`scripts/ci_plan.py` selects checks from both sides of changed Git paths, Cargo
ownership and transitive normal/dev/build/optional/target dependencies. Rust,
package manifests and package build scripts select owning packages and consumers,
including reviewed runtime relationships such as opaque device state in storage.
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

Package plans retain default tests/lints, applicable allocation/UI/export feature
configurations, gallery and desktop build checks, and independently declared
additional features. Feature combinations need reviewed contract rules; individual
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
`--adopt-target` explicitly invalidates supported workspace development outputs
and establishes the checkout's development owner; it is not permission to remove
another executor's active cache, release artifacts or user data.

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
