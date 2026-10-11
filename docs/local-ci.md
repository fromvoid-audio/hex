# local ci

For the private reusable Codex Cloud setup, startup activation, command network
permissions and Linux/native validation boundaries, see
[cloud environment](cloud-environment.md). Portable cloud checks are useful
evidence but do not replace the complete runner below.

Run the project validation pipeline from any working directory with:

```sh
python3 /path/to/hex/scripts/local-ci.py
```

From the repository root, the usual command is `python3 scripts/local-ci.py`.
The runner resolves all paths from its own location, uses Cargo's existing
target/cache configuration, and executes checks sequentially. It does not install
tools or hooks, change the machine's Cargo configuration, launch the app, bundle
an application, or contact a hosted CI service.

The full run executes its runner and native delivery regression tests, the architecture, GPUI patch and UI
style guards, formatting, locked workspace and UI-feature tests, audio and stream
allocation probes, strict workspace and feature-specific Clippy, and a locked
desktop build. A failed or unavailable command stops the run. Later steps are
recorded as `not_run`; their status is never reported as passed.

Each run writes deterministic numbered step logs and `summary.json` under a unique
`artifacts/local-ci/<run-id>/` directory. The summary records actual argument
vectors, UTC start/end times, exit codes, the starting and ending Git commit and
dirty paths, and a SHA-256 digest of tracked and non-ignored untracked file
contents. Ignored build outputs and the run's own ignored logs are excluded by
Git's file listing. If the repository identity changes during execution, the run
is marked `stale` and returns nonzero even when commands passed. Interrupted and
missing-tool runs retain a summary, logs and explicit incomplete/skipped steps.

`--quick` runs only the three source guards and formatting. Formatting does not
use Cargo's target cache, so quick mode neither resolves nor claims/cleans a Cargo
target. Its summary status is `partial`, and it is never sufficient merge evidence. A full successful summary
is tied to the recorded unchanged commit and working-tree digest; any source edit
afterward makes that evidence stale for the new tree.

## continuous integration and merge policy

As requested on 10 October 2026, agents merge a completed, focused PR immediately
once its final diff has been reviewed and full local CI is green on its exact
final commit and unchanged source digest. The user gives standing merge
authorization; no additional approval is needed. Merge dependencies first and
revalidate source changes or conflict resolutions. Quick, stale, partial,
interrupted or failed runs never authorize a merge. Preserve branch protections
and use local checks only; no hosted CI or background merge service is configured
by this policy.

A known unresolved correctness bug must be fixed even if the existing suite is
green. Every reported bug should strengthen a focused deterministic regression
at the owning boundary, failing before the repair and passing afterward. Prefer
extending existing fixtures and behavior tests over duplicate tests, broad
snapshots or expensive matrices. Keep lifecycle, boundary and malformed-input
coverage that protects real risks. When reliable automated coverage is unavailable,
record the limitation, manual reproducer and follow-up in Pulse.

The runner is not a substitute for the user's native UI and listening acceptance.
Those results may follow merge and remain separate from automated checks; retain
pending validation in Pulse and do not claim acceptance from a green suite.

## cargo target ownership

Before a run containing target-using Cargo steps, the runner resolves `Cargo.toml` with
`cargo metadata --no-deps --format-version 1 --locked`. The retained summary
records the canonical Git worktree root and common directory, the Cargo workspace
root, the exact resolved target directory, workspace package identities, and the
metadata command/result. It then pins that canonical path in `CARGO_TARGET_DIR`
for headroom probes, cleanup and every child Cargo command. Metadata failure is
recorded and stops the run before any build or cleanup.

Build phases on macOS/Linux use a nonblocking cooperative file lock keyed by that canonical
target path. It covers cache ownership checks, handoff cleanup and every
target-using Cargo step in the runner. Formatting and other commands that do not
use build artifacts stay outside this lock. A second local-CI run targeting the same directory fails
immediately with the current holder when available; it does not wait or remove
the persistent lock file. The lock is advisory: direct Cargo invocations and
other tools do not participate. Operators must avoid running raw Cargo commands,
release builds or another build tool against a target while local CI owns it.
This runner does not claim universal process locking.

The runner stores a small ownership marker and lock under the operating system's
temporary `hex-local-ci-targets` directory, separate from Cargo's cache. An empty
unclaimed target can be claimed. A nonempty target with no valid marker, a
malformed marker, a target-directory identity change, or a different repository
fails closed unless the caller supplies `--adopt-target`; that option explicitly
asserts that the caller has checked sole ownership. Failed or interrupted cleanup
does not transfer ownership. The marker is updated only after successful
preparation, and the lock is released on interruption without unlinking its
inode.

When the same Git repository is used from a different worktree, the runner
performs a handoff while holding the lock. It invokes Cargo clean with the
resolved target and `--profile dev`, naming every workspace package by its full
package ID from the locked metadata rather than an ambiguous name/version
selector. It does not clean the whole target. Cargo's external dependency
artifacts and release profile are preserved. The actual alternate-worktree
regression fixture in `scripts/tests/test_local_ci.py` changes a workspace API,
asserts the old API artifact is gone before the second worktree rebuilds, and
verifies that an external path dependency artifact and release binary remain
unchanged. Unknown caches require explicit adoption rather than a guessed
workspace-package cleanup.

The ownership protocol coordinates concurrent invocations of this runner only.
It cannot detect a direct `cargo build`, a separate local-CI implementation, or
another process that ignores the advisory lock. Do not share a Cargo target with
such a process during a run or while a worktree handoff is cleaning it.

## read-only disk headroom

Before running checks and each full-run target-using Cargo step, the runner
records free space at the workspace and at the Cargo-resolved target directory,
probing the nearest existing parent without creating a cache directory. Cargo
metadata resolves environment and Cargo configuration; the runner pins that
exact target for subsequent probes and commands. An explicit `CARGO_TARGET_DIR`
still overrides Cargo configuration and is recorded in the summary. Full runs default to a 2 GiB floor;
quick source guards default to zero. `--min-free-gib N` sets a bounded nonnegative
integer floor for that invocation. This is a minimum headroom check, not an
estimate of peak build storage: a fresh native dependency build can need much more.

Insufficient headroom or an unavailable probe stops checks, returns nonzero and
retains a failure summary with later steps marked `not_run`. If the filesystem
cannot even write the report, the CLI reports that limitation and its intended
path; it does not claim a saved summary. Headroom checks do not delete files;
workspace dev-artifact cleanup is limited to the explicitly described locked
target ownership handoff. There is no retention daemon or global Cargo
configuration change.

## generated-cache and evidence retention

A full disk was encountered during panel verification on 9 October 2026. After
confirming no Hex compiler/build used it, manually removing only Hex's inactive
`target/debug/incremental` recovered 12 GiB. Accepted bundles, logs, source,
private research and other projects were preserved. Subsequent checks use
`CARGO_INCREMENTAL=0` where appropriate to limit regrowth; this does not change
machine-wide settings.

Inspect cache size and active build ownership before manual cleanup. Reproducible
inactive incremental/package caches may be rebuilt. Never delete an active or
shared target, source/media, another project's cache, an accepted/test bundle,
private experiment input/capture or the only copy of validation evidence. Keep
successful exact-commit summaries plus their logs while their PR/build is under
review, and retain failure logs needed to explain repairs. After acceptance,
archive useful reports with their source/build identity before manually pruning
superseded generated runs or caches. There is no unattended cleanup policy.

Worktrees editing incompatible dependency APIs need separate target caches:
shared compiled artifacts produced a mismatched device-contract enum in a clean
pinned-main CI run. The stateful DSP executor therefore uses its own ignored
core cache, while the UI baseline keeps the existing target. Use separation only
when concurrent checks or divergent API evidence justify its disk cost; preserve
other active targets and never treat a clean source digest as proof that a shared
compiled cache came from the same dependency tree.

## live low-headroom check — 9 October 2026

On source checkpoint `1ff6462`, the full runner refused before every pipeline
step at 965,148,672 free bytes (0.90 GiB), below the 2 GiB floor. It exited 2,
retained `disk_headroom`/`disk_error` and all 14 `not_run` records, and removed
no files. Report: `artifacts/local-ci/20261009T180255Z-a9a04b29/summary.json`
(copied to the primary checkout with its identity intact). Ten mocked runner
tests passed earlier; this refusal is additional failure-path evidence, not
a successful full CI run. After the user restored storage, all 14 normal-path
checks passed on clean `5033c02` in 294.905 s, with unchanged source digest and
`stale=false`: `artifacts/local-ci/20261009T183220Z-a37843f7/summary.json`.
[PR32](https://github.com/PORTALSURFER/hex/pull/32) contains the focused guard.
The combined application build follows the remaining implementation work.

## native test post-action

After the shared exact-source local checks and authorized merge, deliver the
optimized app at the configured stable location using [test-builds.md](test-builds.md).
Packaging consumes the full runner evidence; it does not introduce another CI
pipeline or infer user approval from merge/disk activation.
