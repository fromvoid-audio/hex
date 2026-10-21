# development checks and cache reuse

Use one persistent integration checkout and Cargo target per executor. Switch
branches only after the owning process reaches a terminal outcome and its source
and raw result are retained. Preserve existing canonical PRs and old worktrees
with unique evidence; a local integration branch may push normally to the same
canonical PR. No target is shared concurrently without its repository-owned lock.

An implementer runs a meaningful owning regression and necessary strict checks.
The merger inspects the actual composed diff, runs one final required plan, and
records the completed review and authenticated merge/tree result. Repeat focused
checks for material integration/conflicts/failures, not merely because the owner
and merger are different people. Do not weaken a failed assertion or invent test
equivalence from matching names.

| Merge input | Required execution |
| --- | --- |
| Prose only: README and docs Markdown | changed local links, conflict/whitespace checks, editorial claim review |
| Known launcher/packaging Python and their tests | complete script tests, including private real Cargo/signing fixtures, source guards and format |
| UI/widgets Rust with supported reverse dependencies | shared Python invariants, script guards, default UI/widgets tests, UI support-feature tests, actual gallery, default and feature strict Clippy, desktop build |
| App-owned core Rust source only | shared Python invariants plus every Rust/guard configuration in the complete audit |
| Manifests, lockfile, features, build scripts, third-party code, toolchain/policy, unknown or mixed executable inputs | complete audit, including comprehensive Python tests |

Python self-test impact is independent of Rust scope. An ordinary merge that
changes only Cargo-owned `.rs` files in `src/`, `tests/`, `examples/` or `benches/`
uses eight explicit shared invariant cases plus the cheap selector tests. This
retains classification/fallback, rename/deletion/untracked handling, required
profile configurations, zero-match refusal and semantic build-input identity.
The whole `test_ci_plan` module is not selected: its remaining cases include real
Cargo builds. The exact methods live in `scripts/python_test_plan.py`.

Only a single UI or core category can use that fast path. All tooling changes
(including launcher changes), Python test changes, `build.rs`, Cargo/configuration
and toolchain source inputs, assets, vendor code, unknown paths, mixed categories
and new/missing/unregistered test modules select comprehensive Python discovery.
Explicit full/release and no-flag audits remain comprehensive. Prose-only merge
checks remain metadata-only. No prior result cache or framework substitution is
involved. Every selected Rust command, feature, guard and target-ownership flag
is unchanged. Selected/excluded groups, exact selectors and reasons are recorded
in the existing hashed plan and printed at the Python step; actual unittest
counts remain in its log and summary. Missing names, zero matches and all-skipped
requested configurations refuse qualification.

A core merge can have full Rust scope with reduced Python self-tests. That is
merge evidence, not full-audit native/release qualification. The delivery receiver
recomputes the plan and requires the exact complete audit command list, including
comprehensive Python discovery, unless its existing explicit development-profile
option is selected. Explicit full/release audits remain the qualification path.

Docs accompanying a known executable scope do not falsely force a mixed full
run. Both sides of renames, deletions, dirty and nonignored untracked inputs are
considered. Merge plans require clean committed source. Cargo's actual package
inventory includes normal/dev/build/optional/target-dependent declarations and
feature/target configurations. Default and nondefault checks remain separate;
this change removes no tests on name-overlap evidence.

```sh
python3 scripts/local-ci.py --profile merge --base BASE_SHA --source HEAD_SHA
python3 scripts/local-ci.py --profile edit --base BASE_SHA --source HEAD_SHA \
  --package hex-ui --features ui-test-support --test-filter owning_test_name
python3 scripts/local-ci.py                         # complete audit
python3 scripts/local-ci.py --profile release        # complete audit
```

Edit and quick are partial feedback, never merge evidence. A requested test scope
with zero passing cases or only skipped cases refuses qualification; genuine
Python/prose scopes do not need unrelated Rust tests. The summary records the
resolved immutable base/source/tree, policy, arguments, configurations, actual
Rust binary counts/Python executions, reasons, toolchain and input identities.
Counts across configurations are executions, not automatically distinct tests.

For a prose-only continuation after a completed runtime plan, use explicit
`--profile merge --base BASE_SHA --source NEW_SHA --reuse-report SUMMARY.json`.
The original complete passing schema-2 report must belong to an ancestor, with
identical executable inputs and guarded toolchain/dependency identity. Every
intervening changed path must be prose, and changed links/claims are checked.
Reference the original runtime report, not a chain of metadata reports. Failed,
partial, stale, zero-match or custom component results cannot be reused. Source
continuations with repository symlinks also refuse runtime-evidence reuse;
external targets cannot be certified by a Git-only source digest. Source
files, assets, generated data, manifests, build scripts, policy and toolchain are
executable inputs; only the documented prose paths are excluded. Runtime code
must not consume these prose paths as data; such a change needs a policy/input
review and full audit. This is scoped merge evidence, not a new full/native run.

Cache ownership is independent of test qualification. Version-2 dev markers bind
the checkout/repository/target and content, mode, symlink targets and membership
of local dependencies, including optional/dev paths. Unknown/legacy dependency
identity requires one selective invalidation; changed local dependencies and
workspace outputs are cleaned, unchanged external dependencies and release
outputs are retained. Removed/version-changed dependencies or ambiguous cleanup
names refuse. The lock is acquired before Python's private Cargo fixtures, and
end snapshots/signals cannot silently advance ownership or retain a live lock.
Selected compiler, rustdoc and wrapper environment overrides bind actual
executable bytes, mode and resolved targets. Compiler selection inside Cargo
config is conservatively refused; use explicit executable environment overrides
instead. Relative Cargo home and PATH entries resolve from Cargo's working
directory. Qualifying executable source symlinks are refused, including in the
no-flag full audit; their external targets are not immutable Git source.

Optimized release cache reuse requires the same known ready owner, target inode,
checkout, pinned toolchain, arguments, configuration and dependency inputs. An
ordinary workspace source edit invokes Cargo incrementally. It still writes
pending before building, identifies the actual new artifact and current source,
and writes ready only on successful completion. It never adopts an old binary as
a new build. Foreign/recreated targets, incompatible configuration and failed
pending handoffs retain their refusal/explicit recovery behavior.

Supported entrypoints also supply frozen build-display/configuration inputs to
the UI/stream build scripts. Identical bytes are not rewritten. Actual source,
HEAD, untracked-input, changelog and configuration changes refresh their inputs;
raw Cargo retains conservative refresh and unsupported-configuration refusal.
These are build observations, not native-runtime or arbitrary-Cargo attestation.

Keep canonical Git source/base/tree, finite scope/results and raw logs in one
durable archive/ref. Reference earlier failed receipts once rather than copying
entire source trees or recursively archiving their history. Notify on meaningful
progress, failure or required action; observation delay alone never restarts a
runner. AI changelog generation stays off the merge critical path. Release
channels remain the separate Pulse2873/2874–2877 plan; this change enables no
nightly, publication, installation or app-control automation.


Cache reuse binds workspace package content as well as configuration. A completed
build records compact package hashes and the observation time. Normal edits from
a clean recorded commit remain incremental when all changed input timestamps are
newer than that completed observation. Deleted, backdated, dirty-base or otherwise
unproven changed inputs invalidate their owning package. Both development checks
and optimized builds enforce this rule; neither trusts restored source mtimes.
Changed path dependencies invalidate the corresponding dependency artifacts too.

Compiler and wrapper executable identities are recorded separately. A changed or
legacy unknown compiler identity invalidates the owned **profile**, including its
path and registry dependencies. This is a toolchain transition, not a clean on
every commit; development and release profiles remain separate. Pending/failed
runs never advance a completed source witness. The release owner stays bounded:
package hashes and a clean Git commit identify changes without a per-file ledger.

`CARGO_TARGET_DIR` is an ownership/location input, not a semantic compilation flag.
An invocation-owned recovery CI target may therefore certify the same source and
configuration later built in the separately locked release target. Target inode,
checkout identity, expected owner hash and recovery refusal checks still apply.
A metadata-only report contains no executed-runtime or dependency witness and is
refused by the locked CI-qualified release receiver. Build-only launch remains
explicitly `tests: not_run`; this does not turn prose checks into runtime tests.


The completed freshness fence also includes already-future input mtimes. Changed
content with an unchanged timestamp witness is never treated as a normal fresh
edit. This keeps clock-skew/backdating cases conservative without cleaning
unchanged builds or ordinary later edits. Semantic native inputs include the
pinned cc target spellings (`VAR_target`, `VAR_normalized_target`, `HOST_VAR`,
`TARGET_VAR`), compiler/archiver flags, `CRATE_CC_NO_DEFAULTS` and `CROSS_COMPILE`.
The existing PCM gate's `SYSROOT`/`CLIPPY_ARGS` inputs are recorded too; their
unsupported-layout refusals remain intact. Malformed report/step/count records
raise a controlled delivery refusal before they can qualify a build.

The actual UI build script observes the complete guarded source token. Real
Cargo consumer controls verify retained-mtime out-of-crate font/changelog bytes
are refreshed across same-HEAD dirty development edits and clean optimized
commits, without an extra cleanup. Optimized preparation refuses dirty source
before building: the same-HEAD dirty font/notes controls verify that refusal
preserves the READY owner and existing artifacts. They do not claim a dirty
optimized rebuild. These specific existing inputs therefore do
not require duplicate invalidation. This is compiled-consumer evidence, not
native font rendering or general attestation of arbitrary build-script inputs.
