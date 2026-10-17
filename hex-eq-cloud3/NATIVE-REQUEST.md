# finite EQ native follow-up — later coordinated cohort

Source assignment complete. All six canonical EQ PRs are merged, all original head receipts are retained, and no source/review failure remains. Gesture #2729 is already user accepted; preserve that signoff and do not create another approval gate. Remaining native acceptance and the one combined optimized delivery belong to the coordinator/launcher. No immediate Mac job, app launch, rebuild or replacement is requested.

## Exact source evidence

PR349 tested head: `b63424fde3e88915f747f8223bb2c9ea183403c3`, all20 PASS, clean unchanged source `ef0d9e227138b9b9942f430633ee964dd044fd369ea84ba013a10c38ebcb2a79`; original report `receipts/project349/20261017T173807Z-bd574ca8/summary.json`, SHA256 `b5863b7cb75efe50493197a4f90554afd74c9edd93a59069fd7f261bee53b54e`.

Actual merge: `92ecd8d2137c5d270d5d0c77a11dad58b50daf15`; parents `642df957bf228b40c0467adf70980fc9ddc762cf` and the tested b634 head. Actual/recomputed merge tree both `d0ed350292177aa1cdfd63e71de21431fc8ed93f`; tested-head tree is DIFFERENT, `ee4b3e196e9f1c066099a239e86853b28161d9fa`. Verified all six included heads and original requested342/343 heads are ancestors. Head CI is never relabeled as merge-tree CI.

The already-running post-merge run independently completed20/20 on exact92ec, source `26eb0e6bd73d299c11f9bac3081f9e0d92882aa0f8c5fcca0cfd1ff76e8e56fd`, report `receipts/project349/20261017T175136Z-f4235a03/summary.json`, SHA256 `35c5a1121499813b96f0e2568c148df91400943a012f4849ee617e30bc319ce5`. It qualifies only that pinned merge. The coordinator owns qualification of any different later combined delivery source.

| Ticket / canonical PR | Original qualified head | Original full20 report SHA256 | Native state |
| --- | --- | --- | --- |
| #2729 / PR342 | `023592e02b297c8c50839a5fe123674ca57831b0` | `e025730959f31170b1949a737bbeb16a693770a4c0a6a072dc5569c4e91f9858` | user accepted gestures; preserve signoff |
| #2730 / PR343 | `1146cd61d186420660823d5b7a7e27ad6f83fc2d` | `7e3d48994f214cab1205c3c8816eb9bb54fc3ecc8fa8c15866abcd631df31b6f` | pending later coordinator cohort |
| #2736 / PR347 | `942d77399ffc53fc1b6f4d7e4713771c594faecb` | `3bfdd07c6eb70cab64b50b2521883b727f3afd86ec9f8cfc4595f7046688b017` | pending later coordinator cohort |
| #2731 / PR344 | `7cf0128cadc129b47b3490098691c62f062b7499` | `65c4e4f68bbe451ef1246ea4d46751d77a16ea9443e6169c262ef929be87dd07` | pending later coordinator cohort |
| #2737 / PR348 | `1248960969b843413c350bbd27b310326c9e9443` | `9e363ba9d2a7acf9639632c3aa4234c4241417ec46f6662dbf4d0783ec9e1110` | pending later coordinator cohort |
| #2738 / PR349 | `b63424fde3e88915f747f8223bb2c9ea183403c3` | `b5863b7cb75efe50493197a4f90554afd74c9edd93a59069fd7f261bee53b54e` | pending later coordinator cohort |

## Scheduling and prerequisites

Coordinator reported a separate frozen earlier UI cohort dfa21f5a installed at `/Users/portalsurfer/dev/hex/artifacts/hex.app`, source build3751944026, signed executable SHA256 `219bb79505abbcff085a5643cb04e3cd317e2be01255c6450c1368e79ce30ae3`. It includes344/348 but excludes349/373; it is CLOSED, not launched/native accepted. Leave it untouched. This request joins a later optimized cohort.

Use one coordinator-controlled local thread at a time across Hex/Yield. Launcher cloud1 owns the one combined signed delivery. Preserve all existing worktrees, user projects and unpublished changes. No hosted CI, duplicate source PR, purchases, force-quit or second review app. Native mouse/key focus, actual GPU rendering, listening and CoreAudio deadlines require Mac evidence. Source checks alone establish none of those.

For the later chosen clean40-character source, verify all six heads above are ancestors. Read current AGENTS.md/docs/test-builds.md and use managed toolchain/target ownership. Reuse a complete Mac CI receipt only for the identical full tree/digest with valid ancestry; otherwise run the required matrix ONCE for that chosen cohort. Keep its source qualification separate from manual acceptance.

```sh
python3 scripts/local-ci.py
cargo test -p hex-audio --release --lib --features allocation-probe \
  profile_project_spectrum_maximum --locked -- --ignored --nocapture
```

The required matrix includes callback allocation/custody checks. The explicit profile covers actual capture/worker at0/32/128/256 selected tracks; it excludes driver/DSP/GPU deadlines. Retain raw output, hardware, rate/buffer/workload and outliers. Historical Linux actual-worker evidence is included unchanged; its26319848-byte payload predates the8-byte pending-return slot, current known payload26319856 bytes.

Only when the coordinator schedules later app replacement/manual testing: arrange normal user save/quit and use a separate saved test project. Launcher unions this six-entry included-revision contribution with other advertised features, then uses the repository-owned optimized Developer ID delivery. Missing signing identity stops safely.

```sh
sh scripts/bundle-macos.sh \
  --ci-report artifacts/local-ci/<same-source-mac-run>/summary.json \
  --candidate <later-verified-40-character-cohort-sha> \
  --included artifacts/included-test-revisions.json \
  --checklist artifacts/USER-TEST.md
```

## Finite remaining acceptance

Use two independently audible tones/tracks, a Return and sparse EQ. Record PASS/FAIL/UNRUN with exact cohort SHA, device/rate/buffer, screenshots/listening notes and available callback/underrun telemetry. Do not reopen the accepted2729 gesture signoff.

1. Dual spectra: conspicuous EQ plus host trims produces real distinct input/output curves; input is before entry input trim, output after effect/output trim. Hide input independently. Check ordinary/Return EQ, bypass, silence/anti-phase stereo, stopping/keyboard monitoring and reopen.
2. Quality/layout: inspect bass readability, capture-driven decay and fixed−96..0dbfs display without claiming extra FFT resolution or exact Prism parity. At880x540 and1500x650, full labels/actions/readouts, response/automation rows and graph remain usable and unclipped.
3. Backspace: focused selected EQ node removes only that node/automation; Undo/Redo restores identities. Empty/held/pending/stale/automation-point cases preserve track/clip isolation; numeric/text editing retains its keys.
4. Project signals: no selection shows real Master; one/two/ALL256 selected canonical tracks produce independent correctly assigned-color layers plus Master, never a selected-track sum or silent prefix cap. Color edits, reorder/removal/reselection and project/output replacement clear stale/reused identities.
5. Tap semantics: each track is actual direct post-device/post-fader/balance output including owned tails; Master includes Returns/master gain before monitor-only metronome/browser preview. Vary effects/faders/mute/bypass/Return/master gain; nonlinear bus output is not claimed to equal the sum of track curves.
6. Lifecycle/input/GPU: collapse stops new project capture/analysis after bounded queued work drains; expand starts fresh. Resize outside release commits view height; Escape/focus/window/context change cancels without project Undo. Compact live header retains ALL layers at constrained populated details, restores preferred height in larger windows, and leaves Auto Pan/Operator/Return details and timeline usable.
7. Callback/profile bounds:33 fixed packet owners,32 forward/33 return slots, at most263168 stereo contribution frames/callback, whole-window refusal on exhaustion, no callback allocation/final-drop/lock/log/FFT. Worker uses the existing thread, max2 packets/8 stereo layers per turn. Caps30Hz/100ms up to32 tracks,10Hz/250ms for33–128,5Hz/400ms for129–256. Exercise selection churn/collapse/output restart and all256 layers at the supported rate plus one other available rate. Record real-device sustained audio/deadline and GPU responsiveness separately from worker profile. If a256-track fixture or telemetry is unavailable, mark that item UNRUN and report the bounded fixture/instrumentation need.

Return one finite result table, exact source/build/signature identity, minimum-window screenshots, raw profile/callback evidence and listening observations. Installation/merge is not human/native acceptance. Failures return to the owning canonical scope; unrun checks stay pending with the coordinator, without new source-feature scope or forced immediate Mac work.
