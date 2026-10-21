# development workflow

1. Read the current Pulse ticket, repository binding and canonical PR. Reuse its
   branch; preserve unrelated or held source. Keep one writer per area.
2. Define a finite working increment and observable acceptance checks. Store
   planning/progress in Pulse, not repository notes.
3. Implement against the owning source doc comments. Put changed API, ownership,
   units and failure contracts beside the code. Do not add feature Markdown.
4. Run an owning regression and needed focused checks; inspect the final diff.
5. Commit the final candidate, resolve its required local merge plan and run it:

```sh
python3 scripts/local-ci.py --profile merge --base BASE_SHA --source HEAD_SHA
```

The runner selects the complete required configuration list from actual inputs;
unknown, policy, dependency and mixed changes fall back to a full audit. Do not
select omissions manually. A failed, interrupted, partial or stale report cannot
qualify a merge. Repair the failure before expanding scope. Passing the required
exact-source plan gives standing merge authorization; preserve branch protection.

For iteration only:

```sh
python3 scripts/local-ci.py --profile edit --base BASE_SHA --source HEAD_SHA \
  --package hex-ui --features ui-test-support --test-filter owning_test_name
python3 scripts/local-ci.py --quick
```

Edit and quick reports are partial. Re-run checks after material repairs, conflict
resolution or changed executable inputs. A prose-only continuation can explicitly
reuse its original runtime report with `--reuse-report SUMMARY.json`; follow the
runner's unchanged-input checks. Never chain metadata reports as runtime evidence.

Keep one persistent checkout/target per executor and serialize target use with the
repository tools. Retain exact source/base/tree, the raw logs and summary with the
canonical PR; do not recursively copy historical evidence into new archives.
Review and verify the actual merge, then update Pulse with its real result.

Native UI/listening acceptance is separate from source checks and merge. For a
user-test app follow [test-builds.md](test-builds.md); use an optimized candidate,
finite checklist and exact included revisions. Never force-quit a user's project.

For optional end-user release-note preparation, see `python3 scripts/changelog.py
--help`. Generation is an explicit operation; ordinary CI validates retained notes
offline and does not call a model. Commit generated data before qualifying source.
