# optimized native test delivery

Reviewed Hex change candidates supplied for user testing use `sh scripts/bundle-macos.sh` with
full exact-source local CI evidence, included PR revisions and a finite
checklist. This is the native macOS workflow; Hex is not a VST3 and does not use
plugin-release. Unoptimized debug executables are only appropriate for a
specific stated debugging need. Do not create uniquely named review apps for
future testing or delete historical review bundles.

## launch modes

`./launch` is the standard update-and-start path. It requires a clean local `main`,
fetches `origin/main`, fast-forwards only when local main is an ancestor, builds
that exact source, and installs and opens the verified optimized build at the
configured stable path. Dirty, non-main, ahead or divergent checkouts are refused.
When source update changes `launch`, it re-executes the updated entrypoint while
preserving the selected mode.

Default launch always fetches and invokes the optimized build, even when an app
is installed. It neither runs tests nor requires a previous green CI report.
Build-only delivery freezes the clean exact commit and source digest, rechecks
them before publication, and retains source/artifact/signature verification and
release ownership safeguards. Its receipt records `source-only` verification,
`tests: not_run`, and no CI report; bundled `source-verification.json` is source
evidence, never a passing test report. Cargo can reuse valid dependencies normally.

`./launch --test` adds full exact-source local CI before the same build and start
workflow. Without the explicit restart option below, save and quit hex before
either mode. Ordinary launch refuses to replace a running installed app or a raw
`hex-desktop` process before CI or Cargo build work, and never terminates one.
The `--test` spelling survives source-update re-exec.

`./launch --force-restart` opts in to quitting the verified stable development
app before the normal update/build/start workflow. **Unsaved work may be lost.**
It requests a native normal quit, waits up to ten seconds, then requests native
forced termination of that same process if needed and waits up to five seconds.
Before the first native request, the launcher compiles the exact Swift helper
once into an exclusively owned private0700 temporary workspace. Cold compilation
has a separate120-second budget; every native operation keeps its unchanged
fifteen-second timeout and reuses that same binary. Original source, private copy
and executable identities/hashes are rechecked; source/binary replacement, missing
toolchain, compiler failure or timeout refuses without a fallback or app launch.
Compiler/helper children created by this invocation are stopped on timeout or
interrupt. The private helper/cache files are removed after control finishes;
source/binary provenance and phase diagnostics remain with the restart report.
If owned-child exit cannot be confirmed after bounded stopping, the private
workspace is retained and further control/cleanup/retry refuses instead of
claiming successful child retirement or opening an app.
There is no persistent shared executable cache or silent warm-up. It never targets
an app by name or PID alone: bundle URL, executable, development identifier and
launch time are rechecked before every request, with the installed signature and
sealed inventory verified as well. Multiple processes, a replacement identity,
unverifiable app or busy activation/build lock refuse without retargeting.
The supported persistent `HEX_DEV_APP_PATH` setting is used by this opt-in path
with the same validation as the default target. `Hex.app` casing is accepted only
when the actual filesystem proves the configured bundle and native executable
have the captured device/inode identities. Separate case-sensitive bundles never
match just because their names differ only in case. Observed native path strings,
PID, launch time and identifier remain part of the exact process identity; all
nine private protocol fields travel together. Symlinks and changed bundle or
executable inodes refuse, including replacement while the helper compiles.
Linux tests cover owned filesystem objects and a labeled case-insensitive lookup
model; actual Swift compilation, AppKit casing and running-process behavior still
require source-specific macOS qualification.
Ctrl-C stops further escalation, recovery and launch. A cancelled normal quit
can still be followed by force under this explicit option; omit the option to
retain the ordinary save-and-quit guard.

Matching well-formed pending release ownership can be recovered only for this
checkout/common repository, actual target inode, compatible package inventory
and known ancestor source. The exact inspected owner bytes are rechecked under
the existing release mutation lock. Existing installer recovery handles only its
fixed owned paths and regular Finder metadata; unknown files, symlinks, legacy
journals and foreign ownership refuse and remain preserved. No arbitrary operator
adoption, project/settings/media cleanup, old-bundle collection or artifact reuse
is added. The fresh candidate is rebuilt and verified; build or install failure
preserves recoverable state and never opens an app. Inspect the actual diagnostic
before retrying, including when no app is running.

The option remains test-free unless combined with `--test`; both flags survive
source update. It cannot be combined with installed-only `--open`. This is future
operator invocation, not permission for unattended force-quit during verification.

`./launch --recover-release-target` is a separate explicit option for a recreated
Cargo target whose retained release owner belongs to this exact checkout and
common repository. Save and quit **both** the configured app and any raw
`hex-desktop` first. This option never requests quit or force termination and is
incompatible with `--force-restart` and `--open`. It still updates clean main,
rebuilds, installs and opens normally; tests remain opt-in with `--test`.

Read-only inspection under the existing activation/target locks binds the old
owner bytes by SHA256 and the observed replacement directory device/inode. The
old source must be a clean known ancestor of the fetched source, and retained
package name/version/spec ownership must match the current locked workspace
(new members are permitted). Missing, foreign, malformed, symlinked or unchanged
targets refuse. Historical ready-artifact metadata is not reuse or qualification:
the old executable need not remain in a recreated target and is never adopted as
a new build. The owner marker stays untouched during inspection.

At the guarded mutation boundary, under the same target-wide lock, packaging
rechecks configured-app/raw-desktop inactivity, source, exact owner hash,
replacement directory and package ownership. Races or a busy cooperative lock
refuse before marker changes or cleanup. Success uses the ordinary pending
handoff, selective workspace **release-only** clean, fresh optimized build,
artifact verification and ready publication. It does not rewrite a marker to
pretend a stale artifact is ready. Original automatic `--force-restart` recovery
still requires the original target identity; it cannot silently adopt a replaced
target. Uncooperative raw Cargo/external file writers do not honor these advisory
locks: exclusive ownership remains an operator requirement, not a global
filesystem/process-atomicity guarantee.

`./launch --open` verifies and opens the installed optimized app without Git, CI,
Cargo or replacement. It can activate an already running installed app. Missing or
unsupported installed artifacts direct the user to `./launch` to build the latest
source. `--open` and `--test` cannot be combined.

## repository-owned build and install

Review the focused PR and run `python3 scripts/local-ci.py` on its clean final
commit. All required steps must pass with unchanged source digest before merge.
This developer validation requirement is separate from the runtime launch modes.
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

The default launcher uses the same repository-owned installer with `--build-only`
instead of `--ci-report`; those options are mutually exclusive and one is required.
This explicit mode verifies clean exact `--candidate` source before/after the
optimized build and before publication without running or requiring CI. It does
not change the full-CI contract of the command above. Build-only bundles carry
source verification instead of `local-ci-summary.json`; their receipt's CI path
and hash are null. Do not describe them as tested.

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

Packaging retains font/GPUI licenses and requires the explicit existing Developer
ID certificate SHA1 and team in `scripts/hex-test-build.json`. These are public
identity metadata; signing uses the existing private key in the macOS keychain.
There is no automatic identity selection or ad-hoc fallback. Missing identities,
keychain denial and signing failures stop before publication, preserving the
installed app. Another machine must explicitly configure its signing identity;
changing the identity can require a new macOS permission grant.

The staged app is signed with `org.portalsurfer.hex.dev` and no network timestamp.
Strict full-bundle verification requires that identifier, the configured exact
certificate and team, an Apple Developer ID Application certificate, and a stable
designated requirement containing the identifier/team rather than a changing
code hash. This keeps signing identity stable across builds; actual persistence
of macOS folder permission across replacements still needs native acceptance.
This is local Developer ID signing, without notarization or distribution.

The sealed schema-2 `Contents/Resources/test-build.json` records provenance,
signing policy, the input executable hash and immutable payload hashes. It is
written before signing to avoid a signature/hash cycle. Its inventory excludes
the executable, generated `_CodeSignature` files and receipt itself; strict
native signature verification authenticates all of those, including the receipt.
The adjacent `hex-test-build-receipt.json` additionally records the final complete
file inventory and actual native signature/requirement. Both delivery modes use
this same signing and seal policy. CI-backed bundles include the full local CI
summary; build-only bundles include `source-verification.json`, with `source-only`
verification, `tests: not_run` and null CI receipt fields. The selected evidence
and `USER-TEST.md` are sealed along with the receipt. Complete CI logs, when
present, remain at the receipt's report location. Legacy schema-1 unsigned bundles retain their original
executable signature/resource-hash verification for normal startup. An actually
Developer ID signed app cannot select those weaker legacy checks by editing its
receipt. Neither format grants permissions or changes privacy settings.

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

The fixed, locked installation workspace permits one incidental Finder file,
`.DS_Store`, only when it is a regular file. Cleanup unlinks that exact file;
it never recursively deletes the workspace or accepts other hidden files,
directories or symlinks as metadata. This also covers Finder adding the file
after the new app is verified and temporary old-app custody is deleted.
If Finder recreates it immediately before directory removal, ENOTEMPTY triggers
one bounded reinspection/retry with the same rules. Unknown members still stop;
continued interference or any other removal error reports incomplete delivery.

Publication is committed when the new stable app verifies. Cleanup is outside
the rollback block: after custody deletion there is no prior app to restore.
Only after cleanup does the installer publish the matching adjacent receipt,
and only successful exit from the release guard records ready ownership. A
later error therefore means **incomplete delivery**, even if the app itself is
valid: the sidecar can still describe the previous app and release ownership
can remain pending. Preserve the failure log, both receipts, signed app inventory
and release marker; do not synthesize a sidecar or edit the owner to say ready.

There is no finalize-only command. After inspection, retry using qualified
installer source and its matching full CI evidence. If the preserved release
marker is pending, explicitly add `--adopt-release-target` to the ordinary
`scripts/bundle-macos.sh` command. Recovery verifies the inactive app and cleans
only the fixed temporary apps and regular Finder metadata, then the normal
pipeline rebuilds, signs and publishes the requested qualified source. Explicit
release adoption selectively cleans current workspace package release products
through Cargo under the existing target lock; it does not delete projects,
settings, media, historical apps or the installed app. An old installer that
refuses Finder metadata needs a newly qualified fixed source; copying a repaired
script into an old qualified checkout does not preserve its CI qualification.
The rebuilt app's source can consequently differ from the incomplete app.

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
rejection, signing configuration, signing-failure preservation and signed-receipt
downgrade refusal. A macOS regression compiles a linker-signed fixture and verifies
valid legacy packaging, resource tampering rejection and invalid executable
signature rejection even after rehashing. On a machine with the configured key,
`HEX_TEST_SIGNING_IDENTITY=4EDF56A7ADD0B4786C2EEE23A50D61620BCE7E4E python3 -m unittest discover -s scripts/tests -p test_hex_test_build.py`
also signs two different native fixtures, checks equal designated requirements,
rejects mismatched certificate/team, and rejects sealed CI/source-evidence and
receipt tampering. Portable production-assembly tests cover signing failure in
both modes and source drift during build-only signing before publication.
Without that explicit opt-in the real keychain test is skipped. These filesystem
tests do not establish macOS permission persistence, native UI,
physical audio deadlines, listening quality or arbitrary crash/power-loss safety.
The coordinator supplies the optimized app and finite checklist; the user owns
native interaction tests while present.
