# local checks

Run from the repository root:

```sh
python3 scripts/local-ci.py
python3 scripts/local-ci.py --profile merge --base BASE_SHA --source HEAD_SHA
python3 scripts/local-ci.py --quick
python3 scripts/local-ci.py --help
```

No flags runs the complete audit. Merge resolves the required plan from actual
Git inputs and Cargo dependencies. Quick runs guards/formatting only and records
partial feedback. See [development-workflow.md](development-workflow.md).

The runner executes sequentially and stops on failure. Read the numbered log for
the failing step; later `not_run` steps did not pass. Results are in
`artifacts/local-ci/<run-id>/summary.json` and adjacent raw logs. A qualifying run
must pass on the exact final commit with an unchanged source digest and no stale
flag. Preserve failed/interrupted logs; do not weaken assertions to obtain green.

For ordinary app-only Rust merge inputs, reviewed Python shared invariants run
alongside the unchanged Rust checks and repository guards. Tooling, policy,
build/configuration, mixed UI/core or unknown inputs, and new/missing/nested test
modules retain comprehensive discovery. Full/release profiles always discover all
modules. The summary records selected and excluded groups and exact selectors.

Explicit native acceptance inputs (`HEX_VST3_MACOS_RECEIPT`,
`HEX_VST3_DISCOVERY_RECEIPT`, `HEX_VST3_PACKAGE_SCANNER`,
`HEX_VST3_PACKAGE_FIXTURES`, or `HEX_TEST_SIGNING_IDENTITY`) keep comprehensive
Python discovery for app-only merges too. Native suites retain their own opt-in
requirements; a skipped fixture is not native acceptance.

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
package Clippy and formatting and remains partial. Merge/full/release plans are
unchanged.

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

Focused tests support iteration; they do not replace the resolved merge plan.
Allocation probes cover their named headless fixtures, not all callback faults,
native allocations, physical device deadlines or audible quality. Report those
limits. User native interaction and listening checks remain separate.

Use local checks only. The runner neither installs tools, starts the app, packages
an app nor contacts a hosted CI service. For the configured cloud prerequisites
see [cloud-environment.md](cloud-environment.md); Linux results do not establish
macOS native behavior.
