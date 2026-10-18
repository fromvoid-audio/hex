# hex cloud environment

Live Pulse [ticket 2348](https://pulse.iocore.dev/) owns reusable environment
maintenance. [PR 160](https://github.com/PORTALSURFER/hex/pull/160) is its sole
repository coordinator. Product changes use separate tickets and PRs.

## authority and publication

Use the existing private **hex** environment bound to `PORTALSURFER/hex` in
Settings > Codex Cloud > Environments. Open its actions menu and choose Edit.
The settings list may also contain an unpublished environment with the same
name; do not publish or replace that separate entry by inference.

Read the actual configuration draft and supported runtime status. Preserve
Only me sharing, repository bindings, existing network policy, proxy and
credential bindings. Never copy secrets or make protected runtime state writable.
Installation/startup corrections belong in that environment's tested draft.
Saving a draft and fixing a running container are separate from publication.

The [official cloud environment workflow](https://learn.chatgpt.com/docs/environments/cloud-environments)
states that publication captures prepared files for new tasks; existing tasks
keep their own saved state. After testing and publishing, start one new task
from that environment and verify its reported configuration version and files.
Do not describe an existing-container recheck as fresh-instance validation.

## task startup and evidence

The previously published setup supplies `/workspace/.hex-tools/activate.sh`.
Source it in the command shell before ordinary Cargo checks. Verify `rustc
--version` and `cargo --version`; the supported ordinary toolchain is 1.94.0.
Explicit future-compiler regression tests may select another installed toolchain
without changing the ordinary default. The retained portable test setup uses
`RUST_MIN_STACK=16777216` for large engine fixtures.

Before network commands, inspect supported runtime readiness and the non-secret
`/etc/codex/network-policy.json`. Network policy readiness and command permission
are separate. A Git failure connecting to the inherited proxy on port 8080 is
not sufficient evidence that the proxy is broken. Original cloud tasks reproduced
socket `EPERM` in the default command sandbox; the same exact branch fetch
succeeded with the executor's supported command-scoped network permission.
Use that narrow permission when available and permitted by the current runtime.
Do not unset the proxy, disable TLS checks, broaden allowlists, copy credentials,
or guess unsupported tool parameters. If the executor lacks the supported grant,
report the actual permission blocker.

Cross-thread coordination must use supported chat tools or the native coordinator
relay with human authorization. `codex queue` failed in one cloud task because
its protected runtime state database could not be opened. Do not chmod, copy or
replace that state to make a local CLI act as a chat messaging service.

Report failures with configuration version, actual activated toolchain, source
commit/branch, exact command, sanitized error, observed cause and any session-only
remedy. Discover live Pulse projects/versions/tags and duplicate tickets before
recording a fault. Link environment faults to 2348 and retain product PR ownership.
Read the current ticket revision before every selective update. Keep credentials,
environment dumps and private source/media contents out of reports.

## validation boundaries

Run portable checks on the selected task source, using locked dependencies.
Retain command results and distinguish failure from later `not_run` steps.
The full local runner remains required merge evidence; partial portable tests
do not replace it. No hosted CI or paid infrastructure is configured by this
workflow. Linux checks do not establish macOS GPUI interaction, CoreAudio,
physical deadlines, reference behavior or listening acceptance.

Native Linux compilation additionally needs ALSA development metadata and any
other native dependency development packages. Multiple original tasks failed
`alsa-sys` because `alsa.pc` was missing; one task installed workspace-local
libraries only in its own container. Reusable setup must install and verify
those prerequisites before publication. Installing libraries cannot repair the
reported locked `xattr 0.2.3`/`libc 0.2.190` `ENOATTR` source incompatibility;
that separate repair is Pulse 2349 / [PR 161](https://github.com/PORTALSURFER/hex/pull/161),
merged during maintenance. The failure below describes the older tested setup
source, not a claim that current main still has that compile error.

The corrected installer extracts signed Debian trixie amd64 development/runtime
packages for ALSA, Fontconfig, FreeType, XCB and XKB under
`/workspace/.hex-tools/native`, without sudo or system package changes.
It relocates only extracted pkg-config metadata. Activation selects those
pkg-config, linker and runtime-library paths. After activation,
`pkg-config --modversion alsa fontconfig freetype2 xcb xcb-xkb xkbcommon
xkbcommon-x11` must succeed. Required package dependencies are included in the
saved installer; its helper contents are embedded in the install field, so
future setup does not depend on files in another task's checkout.

## evidence — 11 october 2026

Original setup chat `01a12a02-8571-74ef-a44f-67217d9ec047` reported 1,011
portable tests passing, three ignored, streaming allocation probes, strict
Clippy, source guards and offline CLI/project round trips. This is prior setup
evidence, not a validation run on today's task branches.

Original cloud owners report configuration version
`cecfgver_6acb4612b1a4819abb859cce45b2685c`, activated Rust/Cargo 1.94.0,
and the same command-scoped fetch remedy. PR 150 and PR 153 independently hit
missing ALSA metadata in full local CI. PR 154 and PR 151 report the subsequent
Linux xattr/libc compile incompatibility. PR 158 reports protected-state failure
from the local `codex queue` CLI. These reports are retained in live Pulse 2348.

Edit setup chat `01a12a2a-48fe-75b7-991b-1032552a2065` tested draft
`cecfgdraft_49184df0e6c0819aba9cc462e3c82ca9`, revision 3, against clean source
`291aabc7fc53d4e9644b1ad5c38307ba815aaaf6`. The saved fields read back exactly.
Final installer repeat, clean-shell Rust/Cargo 1.94 activation, task-branch
fetch, native headers/link/runtime smoke and CLI smoke passed. Portable tests
passed 1,011 cases with three ignored; 19 stream allocation-probe cases,
source guards, formatting and strict portable Clippy passed. FreeType/XKB
initialization and loading all seven libraries used the workspace installation.
The UI check reached the separate xattr source error and exited 101; no complete
Linux workspace/native CI pass is claimed.

Tested configuration SHA-256 values:

- install script: `7410101cce2f0ee3f113699ac36527b03f8108b6a323f2be14eaf223bc2dc3b6`
- start skill: `e5db147711e511f9ccb3c4623b4a38fbfb93760cbfe5c4f7c9c6e3a2819d75a0`

The browser saved the tested draft and confirmed **Environment published** on
11 October 2026, preserving Only me sharing and existing policy fields. New
version: `cecfgver_6acb5120834c819ab3681a5e3f5b46aa`, under the same config
`cecfg_6acb4435ff40819aac5661508f09b5fb`.

Independent fresh task `01a12a38-5e50-7217-9e8d-8aa6be9e7054` reports that
new version and a distinct instance. Initial pending/offline state progressed
to current running/connected observations, spec 3/3 with no failure. It inherited
the matching install/start hashes, activation, native packages, smoke helper
and retained evidence. Clean-shell Rust/Cargo 1.94, 16 MiB stack, seven
pkg-config packages and native headers/link/load/initialization smoke passed.
The exact PR160 branch fetch succeeded through supported command permission
with the inherited proxy unchanged.

On clean current main `5a797959af9228f1c9ae4db25e1c9d39e9339cc9`, containing
the separate PR161 repair, fresh-task guards, formatting, locked xattr check,
180 project tests, CLI build and PCM roundtrip/overwrite-refusal smoke passed.
No packages or product files were changed in this fresh task. Evidence was
appended to live Pulse 2348, revision 21. Runtime HTTP policy enforcement was
reported unknown; the browser's preserved policy settings are configuration
evidence, not proof of enforcement. Native macOS/audio acceptance and complete
Linux workspace CI are not established by this probe.

### editor test filesystem sandbox — pulse2808 / PR414

Only `cfg(test)` editor construction resolves default filesystem paths into one
fresh, exclusively created temporary root per editor. Its Home/Music, browser
start directory and Places are resolved before startup jobs; default browser,
sample catalog, recent-project and theme files live there too. Missing Music
falls back to that private Home. Explicit populated, missing or malformed fixture
paths and explicit browser directories/Homes are preserved exactly; the resolver
never adopts them or derives scan roots from an arbitrary preference-file parent.
Creation errors fail clearly without ambient Home fallback or environment overrides.
The `ui-test-support` feature by itself does not change application startup.

An Arc guard belongs to the editor and each actual filesystem job, including
inner directory/tree scans, preview preparation, preference load/save and catalog
load/scan/save futures. A view's outer awaiting future is insufficient custody:
real I/O can finish after the view is gone. Only after the last guard is released
is the exclusively owned root removed on an independent cleanup thread, without
blocking UI Drop. Explicit caller-owned directories remain the caller's responsibility
and are never deleted by the sandbox. This does not join Session workers or promise
all application workers have stopped.

A test-only asynchronous gate inside the real catalog save job verifies that the
editor can be released while the job is held, the real save retains its root,
and cleanup happens afterward without recreation. GPUI deferred Entity releases
are flushed explicitly in that test. This is filesystem lifetime evidence, not a
native-app or audio-listening acceptance result; older runs are not retroqualified.
