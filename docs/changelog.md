# end-user changelog

The editor's **changelog** button beside the log control reads notes embedded at
build time. `cmd-shift-l` opens the same reader; Escape closes it and returns
focus to the editor. It performs no network or model calls. Notes show their declared
coverage revision; later unrelated changes do not silently extend that coverage.
Missing notes are visibly unavailable rather than a fabricated release summary.

Before freezing a delivery candidate, commit the implementation and run:

```
python3 scripts/local-ci.py --prepare-changelog --changelog-base BASE_COMMIT --changelog-through END_COMMIT
python3 scripts/changelog.py validate --through END_COMMIT
```

The first preparation requires an explicit baseline. Later preparations default
to the preceding coverage end. The existing configured `codex exec` performs the
AI generation with a bounded Git diff, schema, and prompt; no provider or model
configuration is changed. Generation failures retain previous valid notes and
print the retained diagnostic directory. Commit `changelog/generated.json` before
running candidate checks. Never generate or edit tracked notes after qualification.

Ordinary local CI validates the schema and immutable declared input range offline.
It does not call a model, require notes to cover unrelated subsequent commits, or
claim missing notes are a generation success. Explicit `--through` validation
fails for missing or stale requested coverage. Full source patches are private
preparation inputs, not shipped in the compact end-user dataset.

For a bounded capability test, `export --base BASE --source END --output NEW_DIR`
exports the exact input, schema, prompt, and hashes. An exported packet alone is
not generated notes or proof of unattended inference. Cloud inference currently
has recorded proxy failures; successful configured Mac execution is separate
route evidence. Native viewer acceptance and final candidate qualification remain
separate from generator validation.

Actual configured Mac CLI output from an exported packet can be consumed with
`import-result --packet DIR --result RESULT --execution EXECUTION --evidence-commit SHA`.
This validates exact exported bytes, range, schema, prompt, original result hash,
and successful execution before atomic publication. This supplementary evidence
transport does not pretend the unavailable cloud inference route succeeded.
