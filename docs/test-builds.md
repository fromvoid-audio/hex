# native macOS builds

For immutable beta/nightly/stable distribution archives with notarization and
versioned offline manuals, use the [release command](releases.md). Its explicit
`install` command reuses this same installer, app identity and configured path.
The local delivery below remains available for development builds.

Supply optimized exact-source builds for user testing. Save and quit hex first;
never force-quit, discard unsaved projects or claim replacing a file reloads a
running process. Preserve projects, media, preferences and historical artifacts.

## Run from source

```sh
./launch                    # git pull, then cargo run -r
./launch --diagnostics=off  # forward arguments to hex
```

The launcher runs in the checkout containing it. It stops on a failed pull and
forwards all arguments to the release binary. It does not install an app bundle,
run local CI, or terminate an existing process. Git and Cargo use their normal
configuration, including `CARGO_TARGET_DIR` when set.

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

Use the open Pulse review-checklist items of the issues in the candidate as the
source of applicable pending interaction and listening checks. Prepare the
candidate's finite checklist snapshot from those items, with exact build
provenance and explicit scope, write it to the ignored `artifacts/USER-TEST.md`
and supply it to `--checklist`. The snapshot is a read-only copy for the build;
the user ticks each item in Pulse and marks the issue done after all its items
are complete. See `AGENTS.md` for the full contract.

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

For explicit preparation without replacing a running app, the same qualified
command accepts `--prepare-only`. It requires full exact-source CI and writes a
verified candidate into the target-bound `.hex-pending-update` store beside the
configured app. It does not install or launch that candidate; the ordinary
launcher does not consume it. An update-capable installed test build shows an
available update and offers `install update on quit` or `save and restart for
update` in the hex menu. The first waits for a confirmed normal close; the second
waits for the selected project save and requests one restart after installation.
Cancelling, a failed save, or a changed project leaves the app running. A changed
candidate detected before quit also leaves the app running; if the final check
after quit refuses, reopen the existing app and retry. If an older installed build does not offer these actions, save and quit
it and use the supported installer once before using the deferred handoff.
Failure before pending publication preserves the installed app and the previous
candidate. Interrupted publication uses the existing installer recovery. Avoid
starting a build or CI while recording: preparation is not evidence of audio
deadline safety. A restart request is not proof that the new process launched;
check its build identity and the displayed update status separately.

The development app includes VST3 discovery by default. Set `HEX_VST3_SDK` to
the [pinned clean SDK checkout](local-ci.md#controlled-macos-vst3-fixtures) or
pass its path with `--vst3-sdk` when running `scripts/bundle-macos.sh`.
Delivery builds and signs the discovery helper and
scanner inside `hex.app`. Open the packaged app and use **scan trusted standard
folders** in the plug-in browser when you want to discover installed VST3s.
You can supply a prebuilt exact-candidate helper with `--vst3-discovery-receipt PATH`
or explicitly omit discovery with `--without-vst3-discovery`.

For the separate instance helper, build the native `--instance-only` profile
with `crates/hex-plugin-scan/native/macos_bootstrap.py` and pass its exact-candidate
receipt with `--vst3-instance-receipt PATH`. Delivery signs nested helpers and
records their final inventories. Use the current Pulse acceptance checklist for
the supported plug-in hosting workflow.

Read the actual installer result and retained `hex-test-build-receipt.json`.
Report candidate/merge mapping, full source, included revisions, CI report,
artifact identity, disk installation and relaunch status. A candidate report may
cover a merge only when its complete tree is identical. Disk installation,
launch and native acceptance are separate facts. A cleanup failure is incomplete
delivery even if the new app is present; preserve diagnostics and retry only via
the supported installer. Do not fabricate a successful receipt or collect old apps.

After an interrupted installation, save and quit hex normally before retrying
the same `scripts/bundle-macos.sh` command. The installer checks its temporary
custody first: it restores the previous app if publication did not complete or
the new app fails verification, and finishes cleanup if the published app verifies.
It refuses ambiguous custody, a legacy journal, unexpected files, or a running
app; preserve those paths and the diagnostic for inspection rather than moving
or deleting them. Recovery alone does not publish a new adjacent receipt. Check
the final command result, installed app identity, and receipt before reporting
delivery; obtain fresh exact-source CI if the candidate or its inputs changed.
