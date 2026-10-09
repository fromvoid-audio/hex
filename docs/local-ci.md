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
