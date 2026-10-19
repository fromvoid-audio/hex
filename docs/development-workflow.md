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
| Prose only: README, todo, docs Markdown | changed local links, conflict/whitespace checks, editorial claim review |
| Known launcher/packaging Python and their tests | complete script tests, including private real Cargo/signing fixtures, source guards and format |
| UI/widgets Rust with supported reverse dependencies | script guards, default UI/widgets tests, UI support-feature tests, actual gallery, default and feature strict Clippy, desktop build |
| Core/audio/model/storage/routing, manifests, lockfile, features, build scripts, third-party code, toolchain/policy, unknown or mixed executable scopes | complete audit |

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
