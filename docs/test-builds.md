# optimized native test delivery

Every Hex change requiring user testing uses `sh scripts/bundle-macos.sh` with
full exact-source local CI evidence, included ticket/PR revisions and a finite
checklist. This is the native macOS workflow; Hex is not a VST3 and does not use
plugin-release. Unoptimized debug executables are only appropriate for a
specific stated debugging need. Do not create uniquely named review apps for
future testing or delete historical review bundles.

## shared pipeline

1. Review the focused final PR and run `python3 scripts/local-ci.py` on its clean
   final commit. All required steps must pass with unchanged source digest.
2. Passing required exact-source checks authorizes merge under the shared policy.
   No hosted/paid CI or separate pre-merge user-testing gate is introduced here.
3. Immediately run the guarded post-merge delivery below. A merge commit can use
   its candidate's CI only when the complete Git tree/digest is identical and the
   verified candidate is an ancestor. Revalidate any changed source.
4. Record the installed receipt, CI report, source, included ticket/PR revisions,
   build version and checklist in Pulse. Successful accessible delivery permits
   `ready_for_review`; failed delivery keeps work open. Use the supported Pulse
   reconcile/completion workflow, preserving its gates.
5. Only explicit owner approval of that exact tested build and included revisions
   permits Done. Any later behavior/revision change requires an updated optimized
   build/checklist and renewed approval. Merge is not user acceptance.

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
[{"ticket":2408,"ticket_revision":9,"pr":"https://github.com/PORTALSURFER/hex/pull/167","head":"<full-40-character-pr-head>"}]
```

Include every ticket/PR revision whose changed behavior is supplied for testing;
these records are an explicit coordinator assertion, not inferred acceptance.
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
executable, hashes every packaged file and records full provenance in
`Contents/Resources/test-build.json`. This is local unsigned bundle packaging,
not Developer ID signing, notarization or distribution. A copy of the full local
CI summary and `USER-TEST.md` travel with the app. The complete CI logs remain at
the receipt's recorded report location.

An advisory activation lock serializes cooperating deliveries at the configured
location. A staged complete bundle is verified before directory replacement.
The prior app is renamed to a retained `.hex-rollback-<id>.app` sibling; its path
is recorded in the new receipt. Nothing prunes prior backups or historical apps.
A durable `.hex-test-build-transaction.json` journal supports recovery on retry:
an interruption between renames restores the old bundle; a corrupt new bundle
is retained for inspection and the old bundle is restored. A verified new bundle
can finish an interrupted activation. Invalid journals or uncertain first-install
states stop for inspection. Never delete the journal, stage or only rollback copy
merely to make a retry pass.

Process checks run before building, recovery and replacement, including case
aliases. A running target blocks replacement; save your work, quit normally and
retry. No force-quit, app launch or OS input automation occurs. Process inspection
and advisory locks cannot prevent an unrelated external launch in the small
replacement interval: do not launch the target during delivery. After success,
launch/relaunch the stable app yourself; replacing files does not prove a running
process loaded them. The sidecar `artifacts/hex-test-build-receipt.json` and bundled
receipt describe disk activation only.

For manual rollback, save and quit Hex, inspect the recorded backup identity,
retain the current app under a temporary recovery name, then rename the intact
backup to the configured location. Do not restore user settings/projects/media
from a bundle backup: this workflow never touches them. Retain receipts/evidence
until exact-build owner approval is recorded.

## automated and native evidence

The shared full local CI runner includes delivery regression tests for checksum
failure, unrelated identity/symlinks, running case aliases, serialized activation,
interrupted rename recovery, corrupt-new-bundle rollback and stale/incomplete CI
rejection. These deterministic filesystem tests do not establish native UI,
physical audio deadlines, listening quality or arbitrary crash/power-loss safety.
The coordinator supplies the optimized app and finite checklist; the user owns
native interaction tests while present.
