# native macOS builds

Supply optimized exact-source builds for user testing. Save and quit hex first;
never force-quit, discard unsaved projects or claim replacing a file reloads a
running process. Preserve projects, media, preferences and historical artifacts.

## Update and open

```sh
./launch          # fetch/fast-forward clean main, optimized build/install/open
./launch --test   # add full exact-source local CI
./launch --open   # verify/open installed app only
./launch --help
```

Normal update refuses dirty/non-main/ahead/divergent source and running installed
or raw desktop processes. Without `--test`, it records source-only verification
and tests as not run. `--open` does not update or build.

`--force-restart` is a separate explicit destructive opt-in: it can terminate the
verified development app after normal quit fails. Never infer that authorization
from a build request. `--recover-release-target` handles supported recreated or
pending targets without terminating hex; read its exact diagnostic and help first.
Do not edit release ownership markers or delete uncertain installation custody.

## Qualified user-test delivery

Review the final diff and obtain a complete passing full local-CI report on the
unchanged clean candidate. Prepare ignored metadata files under `artifacts/`:

```json
[{"pr":"https://github.com/PORTALSURFER/hex/pull/NUMBER","head":"FULL_40_CHARACTER_PR_HEAD"}]
```

Include every supplied PR revision. Write a finite observable test checklist,
explicitly identifying native/listening/platform gaps, then run:

```sh
sh scripts/bundle-macos.sh \
  --ci-report artifacts/local-ci/RUN/summary.json \
  --candidate FULL_CANDIDATE_SHA \
  --included artifacts/included-test-revisions.json \
  --checklist artifacts/USER-TEST.md
```

Use `sh scripts/bundle-macos.sh --help` for supported options. Only an explicitly
requested development delivery may use `--dev-profile` with its resolved scoped
merge report; identify it as scoped development evidence, not full qualification.
Build-only delivery is source-only and does not claim passing tests.

The configured stable target is `artifacts/hex.app` in the primary checkout,
identity `org.portalsurfer.hex.dev`. `HEX_DEV_APP_PATH` may configure one persistent
machine-specific path; do not create revision-named copies. Packaging builds
`cargo build --release -p hex-desktop --locked`, uses the configured Developer ID
certificate/team and preserves original notices. Missing signing identity or
failed verification refuses publication without an ad-hoc fallback.

For explicit VST3 helper packaging, build the pinned clean SDK inputs with
`crates/hex-plugin-scan/native/macos_bootstrap.py build --help`. The
`--discovery-only` and `--instance-only` profiles are separate minimal bundles;
the latter requires its owned duplex protocol and has no command-line self-test.
Pass their clean exact-candidate receipts to `scripts/bundle-macos.sh` with
`--vst3-discovery-receipt PATH` and `--vst3-instance-receipt PATH`, respectively.
Delivery signs the nested helpers and records their final inventories. Including
an instance executable does not establish track integration or plug-in support;
use the current Pulse acceptance checklist for the supported workflow.

Read the actual installer result and retained `hex-test-build-receipt.json`.
Report candidate/merge mapping, full source, included revisions, CI report,
artifact identity, disk installation and relaunch status. A candidate report may
cover a merge only when its complete tree is identical. Disk installation,
launch and native acceptance are separate facts. A cleanup failure is incomplete
delivery even if the new app is present; preserve diagnostics and retry only via
the supported installer. Do not fabricate a successful receipt or collect old apps.
