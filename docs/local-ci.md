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
