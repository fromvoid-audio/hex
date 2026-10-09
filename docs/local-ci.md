# local ci

Run the project validation pipeline from any working directory with:

```sh
python3 /path/to/hex/scripts/local-ci.py
```

From the repository root, the usual command is `python3 scripts/local-ci.py`.
The runner resolves all paths from its own location, uses Cargo's existing
target/cache configuration, and executes checks sequentially. It does not install
tools or hooks, change the machine's Cargo configuration, launch the app, bundle
an application, or contact a hosted CI service.

The full run executes its own runner tests, the architecture, GPUI patch and UI
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

`--quick` runs only the three source guards and formatting. Its summary status is
`partial`, and it is never sufficient merge evidence. A full successful summary
is tied to the recorded unchanged commit and working-tree digest; any source edit
afterward makes that evidence stale for the new tree.

The runner is not a substitute for the user's native UI and listening acceptance.
Those results remain separate from automated checks.

## read-only disk headroom

Before running checks and each full-run Cargo step, the runner records free space at the workspace and at
`CARGO_TARGET_DIR` (or the workspace's default `target`), probing the nearest
existing parent without creating a cache directory. If Cargo configuration
redirects the target, pass that location explicitly through `CARGO_TARGET_DIR`
so this probe covers the actual build volume. Full runs default to a 2 GiB floor;
quick source guards default to zero. `--min-free-gib N` sets a bounded nonnegative
integer floor for that invocation. This is a minimum headroom check, not an
estimate of peak build storage: a fresh native dependency build can need much more.

Insufficient headroom or an unavailable probe stops checks, returns nonzero and
retains a failure summary with later steps marked `not_run`. If the filesystem
cannot even write the report, the CLI reports that limitation and its intended
path; it does not claim a saved summary. There is no automatic deletion,
retention daemon or global Cargo configuration change.

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
a successful full CI run. Full normal-path CI and combined bundling await
restored storage headroom.
