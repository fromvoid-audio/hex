# optimized native test delivery

Every Hex change requiring user testing uses `sh scripts/bundle-macos.sh` with
full exact-source local CI evidence, included PR revisions and a finite
checklist. This is the native macOS workflow; Hex is not a VST3 and does not use
plugin-release. Unoptimized debug executables are only appropriate for a
specific stated debugging need. Do not create uniquely named review apps for
future testing or delete historical review bundles.

## startup and explicit test mode

`./launch` starts the supported installed optimized app, with bundle integrity and
release-profile verification. It does not invoke Git, run CI, build, replace the
app or claim a Cargo target lock. Existing running apps are activated normally;
startup neither quits them nor changes projects/settings. Missing or unsupported
artifacts produce an actionable `./launch --test` instruction.

`./launch --test` explicitly opts into updating clean main, full local CI and the
optimized build/install workflow below, then opens the verified app. Save and quit
before that mode. The flag survives source-update re-exec so requested tests
cannot fall through to ordinary startup.

## repository-owned build and install

Review the focused PR and run `python3 scripts/local-ci.py` on its clean final
commit. All required steps must pass with unchanged source digest before merge.
Build and install with this repository's command below; Pulse is a ticket manager
and does not authorize, schedule or attest Hex builds or installation.

A merge commit can use its candidate's CI only when the complete Git tree/digest
is identical and the verified candidate is an ancestor. For a squash merge, use
the clean verified PR-head checkout after verifying the merged PR's complete tree
is identical and record the head/merge mapping with the build evidence.
Revalidate changed source. Keep the installed receipt, CI report, source, included
PR revisions, version and finite checklist together. Record failures accurately;
disk installation and merge do not establish native UI or listening acceptance.

## delivery command

Keep metadata inputs in ignored `artifacts/`, outside the source digest:

```sh
# full 40-character candidate PR head, not an abbreviated or dirty revision
sh scripts/bundle-macos.sh \
  --ci-report artifacts/local-ci/<run-id>/summary.json \
  --candidate <verified-pr-head> \
  --included artifacts/included-test-revisions.json \
  --checklist artifacts/USER-TEST.md
```

`included-test-revisions.json` is a nonempty list such as:

```json
[{"pr":"https://github.com/PORTALSURFER/hex/pull/167","head":"<full-40-character-pr-head>"}]
```

Include every PR revision whose changed behavior is supplied for testing. Ticket
references may be included for context but are not required by the installer.
The checklist must describe finite observable behaviors and identify any native,
listening or platform gaps. The installed UI already displays the package version
and short source revision captured by `hex-ui/build.rs`; the receipt supplies full
identity. Finder bundle metadata also records version/build and full source.

## stable target and safety

`scripts/hex-test-build.json` configures `artifacts/hex.app` relative to the primary
checkout (Git common directory's parent), even when invoked from a worktree.
On this Mac the default is `/Users/portalsurfer/dev/hex/artifacts/hex.app`. The
existing case-insensitive `Hex.app` alias remains the same development location.
An explicit `HEX_DEV_APP_PATH` overrides it for a separately configured machine;
use one persistent setting, never a new name per revision. The final basename
must remain `hex.app` (case insensitive).

The identity stays `org.portalsurfer.hex.dev`, executable `Contents/MacOS/hex`.
Only an existing bundle with that identity/executable can be replaced. Symlinked
app paths are refused. Projects, media, preferences and unrelated installations
are never copied, migrated or removed. The workflow builds with
`cargo build --release -p hex-desktop --locked` and consumes Cargo's optimized
compiler-artifact event, supporting Cargo configuration, `CARGO_TARGET_DIR` and
configured target triples without guessing `target/release` paths. It shares the
local-CI Cargo target lock/ownership and disk-headroom safeguards. Raw Cargo
commands do not participate in this advisory lock; avoid concurrent raw builds.

Packaging retains font/GPUI licenses, verifies the native linker's ad-hoc signed
executable with `codesign --verify --strict --ignore-resources` so an unsigned
local bundle is not mistaken for a sealed app. Independent receipt checks still
verify every resource, and executable signature validation also runs inside
publication verification so invalid code preserves the prior usable app. It hashes
every packaged file and records full provenance in
`Contents/Resources/test-build.json`. This is local unsigned bundle packaging,
not Developer ID signing, notarization or distribution. A copy of the full local
CI summary and `USER-TEST.md` travel with the app. The complete CI logs remain at
the receipt's recorded report location.

An advisory installation lock serializes cooperating installers at the configured
location. The installer builds and verifies a complete bundle in the fixed
`.hex-install/stage.app` workspace beside the stable app. It flushes the bundle
before publication, moves the existing app to temporary `.hex-install/previous.app`
custody, then renames the complete candidate into place and verifies it again.
Successful installation deletes the temporary old app and workspace. No retained
`.hex-rollback-*.app` copy or version-backup journal is created. To return to older
code, rebuild that source revision using its matching verification evidence.

Retrying the same command under the installation lock handles interruption: a
missing stable app is restored from temporary custody; a completed verified app
finishes cleanup; a failed published candidate is replaced by the intact prior
app. Partial staging is discarded before rebuilding. Unknown workspace contents,
symlinks or ambiguous states stop for inspection. An older installer's
`.hex-test-build-transaction.json` also stops installation without changing its
history. Do not delete uncertain custody just to make a retry pass. Historical
review apps, rollback copies and old receipts are never collected by this tool.

Process checks run before building, recovery and replacement, including case
aliases. A running target blocks replacement; save your work, quit normally and
retry. No force-quit, app launch or OS input automation occurs. Process inspection
and advisory locks cannot prevent an unrelated external launch in the small
replacement interval: do not launch the target during delivery. After success,
launch/relaunch the stable app yourself; replacing files does not prove a running
process loaded them. The sidecar `artifacts/hex-test-build-receipt.json` and bundled
receipt describe disk activation only.

## automated and native evidence

The repository full local CI runner includes installer regression tests for
production bundle assembly from Cargo-reported custom target paths, checksum
failure, unrelated identity/symlinks, running case aliases, serialized activation,
interrupted publication/cleanup retry, temporary custody restoration and stale CI
rejection. A macOS regression compiles a linker-signed fixture and verifies valid
unsigned-bundle packaging, resource tampering rejection and invalid executable
signature rejection even after rehashing. These deterministic filesystem tests
do not establish native UI,
physical audio deadlines, listening quality or arbitrary crash/power-loss safety.
The coordinator supplies the optimized app and finite checklist; the user owns
native interaction tests while present.
