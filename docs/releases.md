# local macOS releases

Use the repository release command from a clean committed checkout on macOS
arm64. It creates optimized, Developer ID signed, notarized and stapled ZIPs.
It does not upload packages, change Cargo versions, create Git tags, schedule
nightlies or launch/terminate hex. Native interaction and listening acceptance
remain separate from automated verification.

```sh
./release beta                  # e.g. 0.1.0-beta.1, then beta.2
./release nightly               # e.g. 0.1.0-nightly.20261023.1 (UTC date)
./release beta --version 0.2.0   # explicit stabilization target
./release status
```

The default target is the workspace Cargo base version. After that target has a
stable release in the source ancestry, the default advances its patch version.
Select a different target explicitly for a minor/major release. Betas are cumulative
since the previous stable source; cutting a beta/nightly never drains Unreleased.
Stable closes only its source range: subsequent main changes remain available for
the next target. Release-branch ancestry is explicit; identical prose never proves
that two commits represent the same change. Reverts remain distinct inputs.

On the first run, existing validated notes provide the baseline. For a repository
without those notes, specify `--base FULL_SHA`; no historical release is guessed.
The recoverable ledger discovers actual first-parent changes, including missed
collection events. Notes are prepared through the configured Codex CLI; there is
no API-billed fallback. Ordinary merges and app startup never invoke generation.
Inference timeouts receive at most two automatic retries, with completed records
and failed-attempt diagnostics preserved. Invalid output and other failures stop
without retry. `--generation-timeout` sets the per-input deadline (1–600 seconds).

```sh
./release notes --max-changes 20 # bounded catchup; retry preserves completed work
./release beta --checklist artifacts/BETA-TESTING.md \
  --included artifacts/included-test-revisions.json
```

`--included` accepts the same PR/head JSON objects described in
[native builds](test-builds.md). Without it, merged PR/head identities are derived
from the selected first-parent range. A default finite native checklist is
included; supply a checklist with more specific actions when handing off a feature.
The versioned offline HTML/PDF manual is rendered from `docs/manual/` before
candidate checks, using the same pinned Pandoc 3.12 and Typst 0.15.1 versions as
SIFT. Missing native tools are downloaded into ignored `artifacts/documentation-tools`
and verified against pinned archive hashes. Documentation generation never depends
on the SIFT or Plugin Release source checkout. The manual's manifest records input,
font, tool and output hashes; both documents are sealed into the app and included
beside it in the ZIP. To preview/check documentation independently:

```sh
python3 scripts/manual.py build --version 0.1.0-beta.1 --source FULL_SHA \
  --output artifacts/manual-preview
python3 scripts/manual.py verify --output artifacts/manual-preview
```

Full exact-source local CI runs after all release inputs are frozen, with those
same embedded notes and release label. It is followed by a guarded optimized
build, secure timestamp/hardened-runtime signing, notarization, stapling, and
independent extraction/signature/Gatekeeper checks of the final archive. There is
no unsigned fallback. The current package contains the default desktop application;
optional native VST3 helper profiles are not included by this release command.
Headless dev/test builds omit debug symbols by default to reduce disk use;
debug assertions and all checks remain enabled. Explicit
`CARGO_PROFILE_DEV_DEBUG`/`CARGO_PROFILE_TEST_DEBUG` settings are preserved and
included in guarded build-input identities. The packaged app uses the optimized
release profile.

Artifacts are stored persistently under `artifacts/releases/` in the primary
checkout, shared by its worktrees. Each candidate has its own directory and an
increasing numeric build sequence, embedded as `CFBundleVersion`. The product
version, channel and full source SHA are separate metadata. Version reservations
are serialized; failed/reserved versions are never reassigned. A completed
`release.json` binds the archive hash, signed app inventory, source/tree, original
change coverage, notes, included revisions and complete CI evidence. Retain the
entire candidate directory as the release record; back it up with the ZIP.
The local `channels/` index advances only after the verified archive and complete
manifest exist. It orders each channel by numeric build sequence; an older build
finishing later cannot move that channel backwards.

Only packages with a successful `release.json` are ready. Pending ZIPs and partial
stages are diagnostic custody and must not be distributed. A failure prints the
candidate and resume command:

```sh
./release resume /absolute/candidate/directory
./release verify /absolute/candidate/directory/release.json
./release install /absolute/candidate/directory/release.json
```

Guarded optimized builds reuse a Cargo target only when its owner records an
empty start and the same checkout, source and build inputs. A different
candidate or a target without this history requires an empty optimized target.
The first build in it recompiles dependencies. This protects against build
scripts and proc macros that read changed tracked files without Cargo watches.
The guard covers the recorded inputs and cooperating target owners. Arbitrary
inputs outside the recorded source and build environment remain nonhermetic.

To reserve a fresh optimized Cargo target before release preparation, give the
full CI run and optimized build separate existing absolute directories. The
optimized directory must be empty and have no owner marker.

```sh
mkdir -p artifacts/release-ci-target artifacts/distribution-target
CARGO_TARGET_DIR="$PWD/artifacts/release-ci-target" \
  ./release beta --fresh-release-target "$PWD/artifacts/distribution-target"
```

The option acquires the cooperative lock before freezing the candidate or
starting Cargo and retains it through the optimized artifact copy. Full CI
uses `CARGO_TARGET_DIR`, while the optimized build uses the explicit option
path. Both use `CARGO_CACHE_RUSTC_INFO=0` so their verification inputs agree.
Use the same option and CI target when resuming a frozen candidate while the
optimized target remains empty and unowned. After ownership has been
published, set `CARGO_TARGET_DIR` to the optimized target and
`CARGO_CACHE_RUSTC_INFO=0` for ordinary resume. An inspected pending owner
from the same checkout, source and build inputs can resume with
`--adopt-release-target`. Preserve older targets and select a separate empty
directory when their ownership cannot establish that history.
The fresh option and `--adopt-release-target` are mutually exclusive.
This uses the existing empty-target custody contract for cooperating runners.

Resume requires the exact original clean source and frozen inputs. It reuses
successful full CI and the same notarization submission; repairs to source need a
new cut. Diagnostics remain beside the plan. Verification checks the actual final
archive without requiring the historical source checkout. Installation reuses
the existing same-path installer and identity (`org.portalsurfer.hex.dev`), refuses
a running app, preserves user data, and does not launch the app. Save and quit hex
normally before installation. The configured target remains `artifacts/hex.app`
in the primary checkout, or the persistent `HEX_DEV_APP_PATH` override.

Signing uses `scripts/hex-test-build.json`; notarization uses the existing
`capture-release` Keychain profile in `scripts/hex-release.json`. A different
existing profile may be selected with `HEX_NOTARY_PROFILE`. Never put credentials
in source or metadata. Missing identity/profile and failed checks stop delivery.
Distribution signing includes only the hardened-runtime
[audio-input entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.device.audio-input)
needed for Core Audio recording, and a lowercase microphone usage description.

Stable release is an explicit source-accepted cut, not a relabeling of a beta ZIP:

```sh
./release stable --version 0.1.0 --accept-source FULL_NATIVE_ACCEPTED_SHA
```

The accepted SHA must equal clean HEAD. Stable gets its own frozen version,
build number, CI/build evidence and signed bytes. No updater or cross-channel
ordering is implemented. macOS arm64 qualification does not establish Windows,
Intel, physical device deadline, UI or listening acceptance.
