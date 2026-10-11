## xattr Linux source compatibility — pulse2349

The exact xattr0.2.3/libc0.2.190 Linux ENOATTR failure was reproduced with
Rust1.94.0's Linux target. A verified, licensed external patch uses ENODATA on
Linux/Android while preserving macOS/BSD. The locked Linux compile regression
and macOS path/descriptor missing-attribute/error tests pass. No dependency
versions change. See [dependency boundary and evidence](docs/xattr-compatibility.md).
Full exact-commit local CI and final PR review are delivery gates; cross checks
do not prove a complete Linux desktop or native Linux syscall execution.

## 100-track Lattice effects profile — pulse2317

Original release profiler now checks whole-run standalone composition and timed
PCM hashes for100-track resident audio with Saturator, Compressor, Gate and
Gate+Compressor. Three captured runs and immutable source/binary identities are
in [performance evidence](docs/performance.md). The300-processor case is refused
by the existing admission budget; no limit is raised and general dense/native
performance acceptance remains Pulse2108. Full local CI belongs to the final PR.

## Operator live gesture route — pulse2320

One focused `hex-session` integration test now drives the registered Operator
through Service Begin/Change/Commit while one MIDI note plays with oscillator-one
feedback set to 0.3. It checks the noncanonical preview, compatible active
runtime handoffs, changed finite PCM against an unchanged twin, one-step commit
and Undo. After the 240-frame ramp, each output is checked against the twin at
the same timeline positions, scaled by the expected carrier value. The test
targets mutable-feedback carrier retargeting; analytic
zero-feedback notes intentionally keep their immutable active note law. The
focused test passes. Full exact-source local CI and PR review are delivery gates;
native audio/UI acceptance remains separate. This coverage does not reproduce or resolve Operator stop issue
2305. See [device gestures](docs/device-gestures.md).

## auto pan compact panel — pulse2322

Added a dedicated 144 × 184 px shared-frame composition for amount, free rate
and right phase, reused unchanged on Track and Return effects. All controls use
the shared compact dial entities and canonical gesture/typed-edit history path.
Headless rendered coverage checks the three bounds at 880 × 540, matching
Track/Return geometry, drag plus one Undo, exact typed amount/rate/phase values, a Return phase edit
and device/Return-owner retirement.
The panel has no preview cache. The focused test passes with one matching rendered case. Full final-commit
local CI is recorded in PR145/Pulse2322; native appearance, pointer behavior
and listening remain user acceptance. DSP/caps and Operator stop2305 are unchanged. See
[Auto Pan](docs/auto-pan.md).

# hex todo

## cloud environment maintenance — pulse2348

Focused [PR160](https://github.com/PORTALSURFER/hex/pull/160) records reusable
startup and failure-report guidance in [cloud environment](docs/cloud-environment.md).
The existing private PORTALSURFER/hex environment was corrected through
its supported setup workflow. Missing ALSA development metadata, command-level
network permission and protected-state CLI messaging failures are distinct from
product source bugs and native-only acceptance. The tested draft was published
as cecfgver_6acb5120834c819ab3681a5e3f5b46aa; an independent fresh instance
verified matching hashes, Rust1.94, native libraries, fetch, portable checks and
CLI round trips. Full PR CI is retained separately as repository merge evidence.
The reported locked Linux
xattr/libc compatibility repair was delivered separately by
[PR161](https://github.com/PORTALSURFER/hex/pull/161) / Pulse2349.

Updated 11 October 2026. Target: installed Ableton Live 12 Suite 12.4.6,
macOS arm64 first. Pulse project **Hex** (project 2, version 0.1) owns active issue planning.
This file retains request order and repository evidence; [plan](docs/plan.md)
owns milestone gates and [features](docs/features.md) owns capability assessment.
Completed chronology and superseded work orders are preserved in the
[historical snapshot](docs/history/todo-2026-10-09.md).

## operator playback diagnostics — 11 october

Pulse 2305: preserve render causes and frame/audio revision; write one background
session log per launch, retain the newest ten, and report logging failures.
Source tests cover fault lifecycle, callback allocation, file retention and
editor-to-log forwarding. Full local CI and a fresh signed build are the delivery
gates. The original parameter-triggered Operator stop remains unreproduced and
open for user reproduction; diagnostics do not claim a playback fix.
See [diagnostics](docs/diagnostics.md).

## lattice saturator — pulse 2309

An original stateless nonlinear `hex.saturator` is registered through the
existing native effect/device contracts. Its two curves, opaque state, generic
parameters, project save/load and undo/redo, track/Return rendering, live
parameter publication, and callback admission/render allocation probe are
implemented. Targeted device, engine, storage, and allocation-probe tests pass;
the effect callback measures zero total and signed-net allocations and checks
known PCM results through the track/send/Return order. The curves are original
choices and do not claim Ableton transfer-law parity. Root owns full local CI,
PR review and merge. Shared-panel appearance and native listening remain user
acceptance. See [Saturator](docs/saturator.md).

## lattice saturator panel — pulse 2312

Pulse 2312 adds a compact Saturator composition with shared selector/dials and
transfer plot, cached original processing-law preview and local draft updates.
The PR records source checks and local CI; native appearance/listening remains
user acceptance. See [Saturator](docs/saturator.md).

## lattice compressor — pulse 2311

The planned original `hex.compressor` is a zero-lookahead stereo-linked
feed-forward processor with a soft knee and fixed detector/gain history. This
slice adds the generic device implementation and focused coverage for knee law,
history transfer, track/Return routing, persistence, undo, and callback
allocation. Targeted checks pass: all 143 `hex-devices` tests, all 223
`hex-engine` tests (3 release-only cases ignored), all 39 `hex-storage` tests,
and all `hex-audio` allocation-probe tests, including the zero-allocation
initial/edited Track and Return callback oracle. Focused package Clippy checks,
format, and the architecture guard pass. It does not claim numeric parity with
Ableton Live. PR134 merged after all 16 local CI checks passed on the clean unchanged
final tree. The shared native panel and native listening acceptance remain open. See [Compressor](docs/compressor.md).

## lattice compressor panel — pulse2314

Dedicated compact shared-widget composition and cached static knee-law preview;
source and exact local CI evidence recorded in PR137. Native appearance and
listening remain user acceptance. See [Compressor](docs/compressor.md).

## native preview retirement — pulse2315

Shared typed live-effect cleanup now covers Return as well as Track preview
entries. Actual Return device removal regression failed before the fix; final
checks and full local CI are recorded with PR138. Native sanity check pending.

## lattice gate — pulse 2313

An original stereo-linked hysteresis `hex.gate` uses bounded opaque state and
the shared effect catalog. It defines threshold/return, floor, attack, hold and
release behavior, preserves compatible live runtime history, and declares zero
lookahead and zero output tail. Targeted checks pass: all 150 `hex-devices`
tests; 225 `hex-engine` tests (3 ignored); 17 storage unit tests plus 23
persistence tests; and the `hex-audio` allocation-probe suite. Track/Return PCM,
live history transfer, generic save/load/undo, catalog registration and both
initial and edited callback admission/render are covered; the Gate callback
probe reports zero total and signed-net allocations. Focused Clippy, formatting
and the architecture guard pass. This is an original processor, not a numeric or
visual Ableton parity claim. Root owns full local CI and PR review/merge; native
panel and listening acceptance remain open. See [Gate](docs/gate.md).

## delivery rules

- Use Pulse for project and issue planning. Discover IDs, check existing tickets
  and read the current revision before selective updates. Keep pending acceptance
  open; record unrelated discoveries as follow-up tickets. See
  [the planning workflow](AGENTS.md#7-where-things-are-tracked).
- Each new request gets its own focused PR; do not combine unrelated UI, DSP,
  hosting, or persistence changes. Large requests use several focused PRs.
- Record the PR and actual checks here when a work unit lands. Source-complete,
  native-accepted and merged are separate states. After final diff review and a
  green full local CI run on the exact unchanged commit, merge the focused PR
  using the standing user authorization; keep native acceptance separate.
- Keep pending acceptance checks. Remove duplicate or superseded tasks from this
  queue, while retaining useful history in the archive and owning documents.
- Preserve unrelated work. Native UI and listening checks belong to the user;
  automated source checks belong to the implementing agent.

## current priority — lattice devices

- [ ] **Editable Operator envelope curves** — Pulse2247: use the shared native
  response-graph widget to edit all six per-operator envelope values through
  bounded scalar/paired gestures, with keyboard and Shift-fine input, stable
  time mapping, numeric synchronization, one-step Undo and persistence. Current
  implementation is integrated on `codex/operator-envelope-ui`; focused
  widget checks (3) and rendered Operator checks (3) pass, including pointer
  capture, cancel/page retirement and saved Undoable edits. Full exact-source
  local CI is required before merge; its result/PR are recorded in Pulse2247.
  Native visual acceptance remains open. The original fixed-hold/time-axis contract is in
  [Operator envelope UI](docs/operator-envelope-ui.md).

### latest source checkpoint

The latest user test app is `hex-lattice-return-review-20261010-1b67f5f.app`,
combining preceding PR38–87 integration and verified source heads PR88–97.
Exact combined16-check CI passed at
`1b67f5f1c1fbbcc14b9239625b065f113f9eb242`
(`20261010T081954Z-2c027a93`, stale=false), following correction of merged
test-module formatting. Optimized build18.83s and strict ad-hoc signature passed;
independent binary SHA256 is
`298500e79ea281ba7d5ba12026c749dae480b33aac35d4b301a134af3afec0bb`.
Its manifest preserves60 PR links and unsupported scope. Earlier bundles remain
unchanged. The agent has not launched the app; native review/listening is open.

Native stateful seek is [PR91](https://github.com/PORTALSURFER/hex/pull/91);
Operator fixed tuning is [PR92](https://github.com/PORTALSURFER/hex/pull/92);
captured track-effect tails are [PR93](https://github.com/PORTALSURFER/hex/pull/93).
Session live-effect draining after source stop is
[PR94](https://github.com/PORTALSURFER/hex/pull/94), and conditional seek
cancellation before callback installation is
[PR95](https://github.com/PORTALSURFER/hex/pull/95). Pulse2208 and2211 are ready
for review with native acceptance open. Pulse2204 whole Return residual
ownership is[PR97](https://github.com/PORTALSURFER/hex/pull/97), included and ready
for review after all16 focused checks. Stack evidence is
[PR96](https://github.com/PORTALSURFER/hex/pull/96); the production8MiB reservation
is unchanged. Mutable source loops2214 are active separately; transport loop
seam/chase2215 depends on that work. Neither is in this bundle.
Constructor stack
measurements (2195), mutable loop boundaries (2165), sustained performance (2108),
complete Operator parity and plugin hosting remain open. One freely positioned
instrument per track is confirmed. The interactive100-track preparation profile
has been captured:48,000 frames at48kHz took200–211ms in nine fresh samples on an
AppleM5Pro. This is background readiness evidence, not a native deadline or
first-audible claim; see [performance](docs/performance.md).

The implementation request covers every currently open Hex ticket in Pulse,
including work without an existing PR. The priority below orders dependencies;
it does not narrow that scope. A combined review build follows implementation,
with native acceptance reported separately.

The user prioritized **lattice**, the device panel and first devices on
9 October 2026, before the main/return-track layout below. Pulse ticket 42 is
the first bounded prerequisite: real stereo DSP primitives in `hex-lattice`
and original linear Utility DSP in `hex-devices`. Canonical integration (16),
the real panel (17), complete Utility (18) and Operator (19) follow as focused
PRs. The user also requested a reusable device-control toolkit (44) supplying
knobs, sliders, editable fields, toggles, selectors and compact device frames.
It precedes the panel and keeps canonical/DSP state outside the UI toolkit.
Generic persisted state and device registration (70) accompany canonical chains
(43); continuous parameter gestures and grouped undo (65) are a separate change.
See [the lattice contract](docs/lattice.md); stateful filters and instrument
lifecycle remain explicit dependencies, not completed support.

Foundation source evidence: [PR #22](https://github.com/PORTALSURFER/hex/pull/22)
adds original linear DSP; [PR #23](https://github.com/PORTALSURFER/hex/pull/23)
adds generic registered state and canonical prepared chains. Full local CI passed
all 14 checks on clean `9efed7e` for PR #23 (105.880 s, unchanged digest,
`stale=false`). Source-frozen benchmarks and their limits are in
[performance](docs/performance.md). Both PRs remain unmerged; controls/panel,
stateful Utility and Operator remain open. Toolkit 44 is source-complete in
[PR #24](https://github.com/PORTALSURFER/hex/pull/24), with all 14 clean-commit
CI steps passing on `7f0bdea` (163.297 s, unchanged digest, `stale=false`).
Gesture 65 is source-complete in [PR #25](https://github.com/PORTALSURFER/hex/pull/25),
all 14 clean-commit CI steps passing on `d48ad24` (172.917 s, unchanged digest,
`stale=false`). Panel 17 is source-complete in [PR26](https://github.com/PORTALSURFER/hex/pull/26),
all 14 clean CI steps passed on `d955b63` (188.416s, unchanged digest). Original
Utility filters passed all 14 checks on `ee87e6e` (147.627 s, [PR27](https://github.com/PORTALSURFER/hex/pull/27));
stateful runtime 16 admission remains active. Native panel/Utility acceptance stays open.
Native interaction/listening remains open. See
[device controls](docs/device-controls.md).
Device-panel removal UX source follow-up on `codex/device-removal-feedback`:
settled Utility and sine removal now avoid the stale-target warning and empty
instrument card; Undo restores both. Eleven targeted device-panel UI tests pass,
including existing active/draft target-loss coverage. Full local CI and native
review remain open.
- [x] **Utility gain dB presentation** — Pulse2151: descriptor metadata presents
  linear-amplitude Utility GAIN in dB while retaining canonical linear bounds,
  edits and saved state. Zero displays as `-inf db`, unity as `0.00 db`, and
  maximum as `+35.00 db`; malformed, non-finite, underflowing and out-of-range
  typed input is refused. The normalized control curve and Shift increment are
  provisional original choices, not measured Live behavior. Source checks pass:
  23 widget tests (including rendered arrow/fine/reset/cancel/type), two
  contract tests, the Utility metadata test and 16 device-panel UI tests
  (including canonical dB-to-linear typing/arrow/cancel behavior), strict
  affected Clippy, architecture/style guards, formatting and diff check. Full
  local CI and native numeric-feel/audible acceptance remain open. See
  [device controls](docs/device-controls.md).
Pinned main 12 is source-complete in [PR30](https://github.com/PORTALSURFER/hex/pull/30),
all 14 clean CI steps passed on `3ee1edd` (270.703 s). Main-project selection 14 is source-complete in
[PR31](https://github.com/PORTALSURFER/hex/pull/31); all 14 clean CI checks passed
on `373e8c2` (276.455 s), including mixed-content 100-track splitting in one Undo. Empty-project and move-scope regression tests pass after storage recovery; all
four project split, seven main-row and eighteen edit-cursor tests pass. Native acceptance remains open; see [main track](docs/main-track.md).

## merged PR history and remaining acceptance

PRs #1–#17 were verified closed as merged on 9 October 2026. Merging records
repository integration, not completion of the native checks below.

| PR | focused change | remaining gate |
| --- | --- | --- |
| [#1](https://github.com/PORTALSURFER/hex/pull/1) | targeted PR workflow | workflow established; retain for future PRs |
| [#2](https://github.com/PORTALSURFER/hex/pull/2) | arrangement edit cursor and split | native boundary checks |
| [#3](https://github.com/PORTALSURFER/hex/pull/3) | unified clip detail view | native acceptance; complete DSP controls remain queued |
| [#4](https://github.com/PORTALSURFER/hex/pull/4) | clip editing responsiveness | native smoothness/flicker acceptance |
| [#5](https://github.com/PORTALSURFER/hex/pull/5) | track-scoped cursor and time rectangle edits | native selection/edit/undo/listening acceptance |
| [#6](https://github.com/PORTALSURFER/hex/pull/6) | smooth pulse and upper-third selection handle | native hover/hand/move/reselection acceptance |
| [#7](https://github.com/PORTALSURFER/hex/pull/7) | todo cleanup and focused request queue | no additional gate; open feature requests remain below |
| [#8](https://github.com/PORTALSURFER/hex/pull/8) | local validation pipeline | full 14-step clean-commit run passed; merged |
| [#9](https://github.com/PORTALSURFER/hex/pull/9) | time-chunk landing preview | user reports review build works as expected; merged; physical timing/listening not established |
| [#10](https://github.com/PORTALSURFER/hex/pull/10) | project-wide time-selection request queue | planning only; implementation remains queued |
| [#11](https://github.com/PORTALSURFER/hex/pull/11) | selected-track ripple insertion | user accepted ripple behavior; merged |
| [#12](https://github.com/PORTALSURFER/hex/pull/12) | Option-drag copying | accepted in combined review build; merged |
| [#13](https://github.com/PORTALSURFER/hex/pull/13) | multi-track keyboard paste anchoring | accepted in combined review build; merged |
| [#14](https://github.com/PORTALSURFER/hex/pull/14) | stable zoom during project growth | accepted in combined review build; merged |
| [#15](https://github.com/PORTALSURFER/hex/pull/15) | Space restarts at the edit position | accepted in combined review build; merged |
| [#16](https://github.com/PORTALSURFER/hex/pull/16) | native File menu project actions | accepted in combined review build; merged |

| [#17](https://github.com/PORTALSURFER/hex/pull/17) | captured time gestures and edge scrolling | user accepted review build; merged |

The selection handle is normally invisible, highlights on hover, and uses a
hand cursor. Dragging below it paints a fresh selection. Point cursors pulse;
nonzero time rectangles remain steady. See [time selection](docs/time-selection.md).

## new requests — separate PRs

- [ ] **native settings menu** — Pulse2115: remove the toolbar settings icon;
  open the same theme panel from macOS hex > settings… and Command-comma.
  Repeated activation focuses the current draft. Five existing theme tests and
  strict UI Clippy/guards pass; clean local CI and native menu acceptance follow.
  The user accepted theming itself on 9 October 2026 (ticket15).
- [ ] **larger swatches and visual color picker** — future Pulse2116, separate PR:
  larger clickable role swatches, XY saturation/brightness field, hue spectrum,
  HSB/RGB controls and synchronized precise values. Preserve the four-role model,
  contrast policy, preview/cancel/save and compact native layout. Not included in
  the menu-access change.

- [x] **captured time selection and edge scrolling** — focused source fix.
  Keep held selection/time-chunk gestures active over empty space, browser and
  outside the window until release. Clamp rows at existing first/last tracks;
  continue horizontal selection at the visible edge and autoscroll left/right
  while held beyond it, including stationary pointers. Preserve snapping,
  Option-copy, cancellation and one-release behavior; no project work per move
  or timer tick. Test fast boundary crossings, reentry, outside release, viewport
  limits and cleanup; native acceptance belongs to the user. Source implemented:
  274 UI tests (2 ignored), strict UI Clippy, formatting and source guards pass.
  Full 14-step clean-commit local CI passed `25b797e`. The user accepted the
  fresh native review build and authorized merging [PR #17](https://github.com/PORTALSURFER/hex/pull/17);
  merged as `a250f83`. Pulse ticket 38 is done; measured timing remains separate.

- [ ] **persistent recent projects** — Pulse ticket 39, [PR #18](https://github.com/PORTALSURFER/hex/pull/18), source complete.
  Native File submenu lists up to ten successfully opened paths, most recent first,
  preserving paths and existing open protections. Background bounded preferences
  survive restart; failed opens do not promote entries, malformed preferences are
  preserved. Targeted preference/UI checks and strict Clippy pass. Full 14-step local CI
  passed clean `c9e7ae4`; release `hex-recent-projects-20261009.app` is ready. Fresh native menu/restart acceptance remains open.
  See [recent projects](docs/recent-projects.md).

- [x] **queue cleanup** — [PR #7](https://github.com/PORTALSURFER/hex/pull/7):
  archive completed chronology and consolidate genuine open gates. Local links
  checked; historical snapshot verified verbatim. Merged.
- [x] **local validation pipeline** — [PR #8](https://github.com/PORTALSURFER/hex/pull/8):
  run the project guards, locked tests,
  allocation probes, strict Clippy and desktop build locally. Keep per-step logs
  and a structured result bound to the exact commit and tracked/untracked source
  digest. Quick mode is only a guards/format preflight, never merge evidence.
  See [local CI design](docs/local-ci.md); full-run evidence is recorded in the PR.
  Merged.
- [x] **time-chunk move preview** — [PR #9](https://github.com/PORTALSURFER/hex/pull/9):
  show selected audio/MIDI portions at their
  exact prospective destination and hide those portions at source. Preserve
  outside tails, gaps and destination replacement semantics. Restore the original
  display on cancellation or rejected edit. No project compilation on pointer moves.
  Full local CI passed all 14 steps on clean commit `4181d68`. The user accepted
  the review build as behaving as expected on 9 October; merged. Native timing
  and listening remain separate. See [owning doc](docs/time-selection.md).
- [x] **selected-track ripple insertion** — [PR #11](https://github.com/PORTALSURFER/hex/pull/11):
  Cmd-Shift-C copies time and
  Cmd-Shift-V inserts the complete time rectangle, pushing later material right
  only on touched destination tracks. Ordinary Cmd-C/V replaces; cutting leaves
  a gap. Preserve crossings, source phase, history and atomic failure behavior.
  Full 14-step local CI passed clean commit `c7ae2d5`; the user accepted ripple
  behavior on 9 October. Merged. Keyboard source-track anchoring landed in PR #13.
  MIDI loop materialization/refusal policy is documented
  in [time selection](docs/time-selection.md).
- [x] **Option-drag duplication** — [PR #12](https://github.com/PORTALSURFER/hex/pull/12) for supported draggable
  project items, including audio/MIDI clips and time chunks. Option selects copy
  instead of move; show a small plus/copy cursor indicator while active. Preserve
  source material and show the exact snapped destination copy, with Command's
  free-placement override retained. Support modifier changes during a drag; release
  uses the current mode. Drop creates fresh IDs in one atomic Undo operation;
  cancellation/failure leaves the original intact. Browser imports retain their
  existing copy semantics. Define Session/other draggable-item coverage explicitly.
  Source implemented for Arrangement audio/MIDI clips, new-track drops and time
  rectangles. Project tests, 267 UI tests (2 ignored), targeted strict Clippy,
  formatting and source guards passed, including a release-time modifier regression.
  Full local CI passed all 14 steps on clean source `494f641`. User accepted the
  combined review build; merged. Outside-lane selection capture is the active fix.
- [x] **multi-track keyboard paste stays on copied tracks** — separate focused PR after
  Option-drag. Normal and ripple keyboard paste of multiple tracks use the copied
  source track IDs; only the current horizontal time position determines placement,
  regardless of cursor/selection vertical position. A single-track clipboard uses
  the current destination track, allowing cross-track paste. Preserve gaps and order, refuse
  missing source tracks atomically, and keep visual time-chunk drags free to move
  or copy across tracks. Cover worker clipboard lifecycle, Undo and both shortcut
  variants; provide native acceptance checks. Source implemented; project/session
  tests, targeted strict Clippy, formatting and architecture checks pass. User
  accepted the combined review build; merged. [PR #13](https://github.com/PORTALSURFER/hex/pull/13)
  passed all 14 local CI steps on clean source `d1bcf26`.
- [x] **Space restarts from the edit position** — separate focused transport
  fix. Space starts at the current Arrangement edit cursor/selection, or project
  start if neither exists. While playing it may pause; the next Space restarts
  from that editing position, never resumes the paused playhead. Preserve text/note
  editor shortcut focus and bounded ordered seek/start delivery, with Stop still
  effective under queue pressure. Verify start position and stop/restart natively.
  Source implemented with one atomic PlayFrom action; engine/audio/UI and callback
  allocation regressions plus targeted strict Clippy/guards pass. [PR #15](https://github.com/PORTALSURFER/hex/pull/15)
  passed all 14 local CI steps on clean source `e540ee5`; user accepted the
  combined review build and authorized merge. Merged.
- [x] **preserve zoom when the project grows** — separate focused fix. Moving,
  copying or pasting to the right may expand timeline/project extent, but must
  retain the user's zoom and viewport. Fit/overview navigation remain explicit
  user actions. Check normal/ripple paste, clip/time drags, new imports and Undo;
  preserve valid minimum/maximum coordinate bounds without implicit zoom-out.
  Source implemented; controller worker-event regressions, strict UI Clippy,
  formatting and source guards pass. User accepted the combined review build;
  merged. [PR #14](https://github.com/PORTALSURFER/hex/pull/14) passed all 14 local
  CI steps on clean source `e04e2ec`.
- [x] **project actions in macOS File menu** — separate focused UI PR after
  keyboard paste anchoring. Put Open Project, Save and Save As in the screen-top
  native File menu and remove corresponding toolbar actions. Preserve existing
  shortcuts, focus routing, pending-work handling and unsaved-project protection.
  Reuse existing controller commands and OS file dialogs; verify native menu
  operation with the user. This request is for the menu bar, not window buttons.
  Native File menu source is implemented; toolbar Save removed, existing actions
  and shortcuts retained. [PR #16](https://github.com/PORTALSURFER/hex/pull/16)
  passed all 14 local CI steps on clean source `045201e`; user accepted the
  combined review build and authorized merge. Merged.
- [ ] **compact header composition** — Pulse ticket 10, [PR #19](https://github.com/PORTALSURFER/hex/pull/19), source complete.
  Separate slim title row with centered
  `hex` or loaded project name; smaller toolbar controls below. Preserve shortcuts,
  title dragging and useful space at minimum window sizes. Source implemented:
  shared 22 px title/26 px toolbar, 20 px icon controls. Long-title/minimum-size
  layout and title-data tests, strict UI Clippy and source guards pass; final
  local CI passed all 14 steps on clean `751b2d2`. Native review remains open
  in `hex-compact-header-20261009.app`. Custom controls are next.
- [ ] **custom traffic lights** — Pulse ticket 11, in progress, separate PR.
  Replace native macOS controls with smaller
  app controls. Preserve unsaved-close protection, minimize, zoom/fullscreen and
  window lifecycle. The focused [native boundary](docs/native-input.md) decision is documented.
  Source implemented; 283 UI tests (2 ignored), strict UI Clippy, desktop check
  and guards pass; cumulative vendor patch independently verified against its
  published archive. Full CI and native control/close/drag/fullscreen acceptance
  remain separate gates.
- [ ] **pinned main track** — always-visible main row glued to the bottom of the
  Arrangement track region. Regular tracks and their trailing empty drop space
  scroll above it; the detail/device panel remains below the track region.
  Preserve master controls, scrolling, clipping, selection and drop coordinates.
  Source now has a fixed 44 px Arrangement row, canonical master gain and regular-region
  hit testing; 304 UI tests/two existing ignores and strict UI Clippy/guards pass.
  Clean full CI, native scroll/drop/minimum-layout review and merge remain open.
- [ ] **return tracks above main** — Pulse2176 supplies schema17, canonical
  return IDs, gain/mute, generic effect chains, sparse sends and atomic history.
  Pulse2177 routes Arrangement, SessionPlayback and prepared export through
  bounded post-fader Return chains with registered zero-latency processing,
  compatible state inheritance and control-side retirement. Pulse2178 supplies
  pinned return rows, effect-only device workflow and named send menus. The
  section caps at 67 logical pixels and scrolls internally, preserving regular
  tracks and Main at 880×540 with details open. Audio and UI source are integrated;
  positive service/save/reopen tests, full local checks and native user acceptance
  remain open. Continuous drag/type sends are Pulse2179; general compensation is
  Pulse28. See [return tracks](docs/return-tracks.md).
- [ ] **project-wide time selection on main/master** — separate feature PR
  after the pinned main row. Selecting time there addresses every project track
  as one rectangle, including returns when implemented, gaps and every supported
  temporal lane. Copy/cut/replace-paste and selected-track ripple commands operate
  across the whole project; the main row is an editing proxy, not a second model.
  Define row mapping when topology changes, preserve outside tails and use one
  atomic Undo; failures must not partially edit tracks. Reuse immutable media and
  prepared metadata, keep capture/expansion/compilation on bounded control workers,
  and provide prompt feedback without PCM copying or whole-project work on pointer
  updates. Measure input-to-feedback and completion separately on 1- and 100-track
  dense mixed audio/MIDI fixtures. Both must feel smooth; do not claim total work
  is independent of project size. Future automation joins the same time contract.
  Source complete in [PR31](https://github.com/PORTALSURFER/hex/pull/31); all 14
  clean local CI steps passed `373e8c2`. Native interaction and dense timing remain open.
- [ ] **persistent four-color theme settings** — expose four primary color
  roles in a compact settings menu with simple color pickers and live previews.
  Proposed roles: background, text, primary accent and secondary accent; derive
  surfaces, borders, hover/selection shades from these instead of independent
  editable colors. Persist across reload using the preferences boundary, validate
  malformed settings, retain defaults and offer reset. Keep waveform/clip contrast,
  focus visibility, lowercase labels and bundled typography. Confirm role mapping
  when implementing. Four roles, cached derived tokens, RGB pickers, preview/
  cancel/reset/save and versioned independent preferences are now implemented.
  Focused source checks pass; clean full CI and native acceptance remain open.
  Pulse2116 adds four clickable role swatches, one shared native saturation/
  brightness and hue picker, direct HSB values and synchronized RGB controls.
  Conversion, field-drag/release, role preview, typed hue and preference reload
  tests pass; native layout/interaction and combined full local CI remain open.
  Contract: [theme settings](docs/theme-settings.md).
- [ ] **native device model/runtime** — canonical IDs, parameter/state/history
  persistence, one MIDI instrument slot and serial audio effects, with shared
  native/offline behavior. Define mutable DSP lifecycle, prepared graph ownership,
  bounded events, latency and callback retirement before stateful devices.
  Paired stateful effects and Utility v2 now pass affected source/probe checks;
  prepared processor latency metadata now distinguishes known frames from
  unknown, with Utility filters declaring zero fixed buffering/lookahead delay.
  Integrated full CI and native acceptance remain open. Contract:
  [stateful lattice](docs/stateful-lattice.md). Large-chain timing/admission is
  tracked separately as Pulse 2108; instruments remain a separate prerequisite.
- [ ] **bottom device panel** — compact horizontal device chain for the focused
  track, real parameters/bypass/reordering and clip/device view toggles. Build on
  the canonical device model; preserve editor selection and useful track space.
  Pulse2141 keeps the panel at 188 px, derives frame/body heights from shared
  geometry, overlays the add menu without resizing the chain, and scrolls excess
  Utility controls inside their frame. Rendered checks pass for fixed panel/frame
  bounds, menu stability, and reaching the last control; native scroll feel and
  integrated full local CI remain open.
- [ ] **native utility** — independently implement Utility's channel selection,
  polarity, gain, width/M/S, mono, balance, mute, DC and bass-mono controls.
  Separate stateful filtering from the linear core when needed. Test DSP/history/
  save-load and compare measured reference fixtures; identify unverified laws.
- [ ] **native operator** — independently reproduce the requested four-oscillator
  instrument, algorithms, envelopes, filters, modulation, voices and controls.
  Stage through focused PRs with original fixtures and bounded mutable runtime.
  Full one-to-one behavior remains an acceptance goal, not an established result.
  Use documented reference studies and targeted private analysis where needed.
- [ ] **CLAP hosting** — separate adapter boundary: discovery, instantiation,
  buses/events/parameters, state recovery, latency, editor lifecycle and failure
  behavior. Safe wrapper review and realtime ownership precede callback use.
- [ ] **VST3 hosting** — independent format adapter with the same host acceptance
  gates; cover scanning, bus negotiation, automation, state, native editors and
  failures. Do not treat a successful plugin load as full hosting support.

Device model/runtime precedes the device panel and native devices. Routing and
latency contracts precede complete returns and plugin chains. These dependencies
must not make UI requests disappear from the queue. Header and theme changes
remain separate from time editing and DSP.

## retained acceptance and fundamentals backlog

- [ ] **native interaction** — consolidate user results for selection/clipboard/
  ripple/preview, source controls, clip detail values, resizing, overview/panning,
  save/reopen and minimum-layout focus. Use owning documents' concrete checklists;
  do not rerun interactions already explicitly accepted unless affected by a change.
- [ ] **native audio reliability** — listening, device selection/disconnect/restart,
  stop under pressure, sustained stress, physical timing and first-transient checks.
- [ ] **long-project streaming** — graph/catalog leases, callback endpoint retirement,
  prewarm/readiness and sustained advancing 100-track mixed-rate native playback,
  edit/seek/launch and fault acceptance before lifting resident import limits.
  Headless worker/render/allocation evidence is recorded in
  [page delivery](docs/page-delivery.md) and [performance](docs/performance.md).
- [ ] **project recovery** — missing-asset relinking, automatic crash journal and
  interrupted-write recovery; broader native Save/Save As/Session round trips.
- [ ] **recording and MIDI I/O** — audio input/monitoring/arming, recoverable recording,
  MIDI controllers/expression/MPE, input/output, capture and Session-to-Arrangement.
- [ ] **musical editing** — real clip gain/pitch/warp, beat-anchored audio, source
  envelopes, metronome, scrub/loop handles, snap nudges and drag autoscroll.
- [ ] **routing, automation and compensation** — validated audio/MIDI graphs,
  sends/returns, envelope editing/recording, bounded sample-offset playback and
  cumulative latency alignment. Compensation does not remove physical latency.
- [ ] **dense UI and accessibility** — measured input/paint timing, keyboard focus,
  accessible names/roles, readable meters and operable compact controls.
- [ ] **export** — progress/cancel, selected ranges, stems and explicit overwrite;
  listening quality and sustained throughput acceptance.
- [ ] **reference evidence** — original CLIP/MIX/TRANSPORT/ARRANGE/Session fixtures,
  measured pan/fade/resampler/device behavior, exact-build provenance and tolerances.
- [ ] **Live-set adapter** — minimal user-authored `.als` fixtures, preserve unknown
  metadata and report unsupported behavior; no full-fidelity claim from parsing.

Publication license, release distribution and additional platforms remain separate
future decisions. No release or broad compatibility claim is implied by this list.

## storage recovery and retained CI evidence

The user restored disk space after compilation was blocked below 1 GiB.
[PR32](https://github.com/PORTALSURFER/hex/pull/32), Pulse 72, adds a read-only
headroom floor and safe manual retention guidance; it deletes nothing. Ten runner
tests, the real low-space refusal and all 14 normal checks on clean `5033c02`
(294.905 s, unchanged digest, `stale=false`) pass. Reports are retained under
`artifacts/local-ci/20261009T180255Z-a9a04b29/` and
`artifacts/local-ci/20261009T183220Z-a37843f7/`. Native app acceptance and PR
integration remain distinct from these checks. The full open Pulse backlog
remains the implementation scope; current checkpoints are not the final build.


## instrument foundation checkpoint

Operator prerequisite Pulse2119: standalone original FM4 kernel with four sine
operators, validated acyclic phase routing and authored-age/gate ADSR is source
implemented and targeted-tested. Pulse2154 admits its bounded analytic sampler
through the built-in catalog and shared Arrangement/Session/export paths. See
[contract](docs/fm4.md). Mutable feedback and complete Operator feature/reference
acceptance remain open. Keep this focused PR separate from panel-focus2117 and
loop-rail2120.

Pulse2114: schema15 optional instrument slot, generic registry and copied analytic
note samplers are implemented in source; targeted tests/strict checks pass.
Silent-slot MIDI extent/preview repair passes targeted tests; fullCI and focused PR
follow integration.
See [contract](docs/lattice-instruments.md). This is a prerequisite for Operator,
not Operator implementation. Source/native/merge gates remain separate.

Pulse2138 canonical chain-position prerequisite: project snapshots now retain one
optional instrument's gap among up to 64 effects, with stable-ID ordered-node
moves, Undo and strict schema16 persistence. Schemas1–15 migrate existing/default
instruments to the front while preserving explicit silent slots. No UI ordering
is enabled by this prerequisite.

Pulse2139 ordered instrument rendering: Arrangement and Session now process
audio through the effect prefix, inject MIDI at its persisted gap, then process
the suffix and apply track mix. Stateful prefixes/suffixes share one block
transaction; copied old-graph MIDI releases process only their captured suffix.
Focused engine/session/export and callback allocation-probe suites pass, including
non-front resident, streamed, rollback and release-tail fixtures. Clean full local
CI and review remain with the integration owner; no native device/listening
acceptance is claimed.

Pulse2135 Session instrument continuity: route reuse now includes MIDI source,
instrument ID, full versioned state and enabled status, while effect position
remains history-compatible. Changed enabled configurations carry matching old
held/releasing voices under their original sampler and suppress only that
route's matching candidate occurrences; bypass/removal releases old voices
without candidate suppression or disabled-period chase. Exact-cut onsets belong
to the candidate. Route-local re-enable floors follow TrackIds across graph
handoffs and reset on seek/stop, while source/timing edits retain the global
floor and carry other active Session routes. Engine, Session, callback allocation,
strict Clippy, formatting and architecture/style checks pass on the frozen
source; full local CI remains with the integration owner. See
[instrument continuity](docs/lattice-instruments.md).

Pulse2149 Arrangement instrument continuity: ordinary prepared playback now
uses the same bounded Session owner as native playback, so Arrangement-held
voices follow instrument edits with copied old samplers, exact-occurrence
suppression through the later old-voice/candidate-occurrence end, and the old
effect suffix. Repeated edits and track removal retain already-copied voices
without recapturing active source spans. Legacy `new/from_shared` remains a
separate API with explicit promotion/demotion fences; unsafe active Arrangement
state cannot be silently dropped or synthesized by a new graph. Prepared
instrument/stateful revisions whose Session runtime is unavailable refuse
rendering rather than silently using the finite renderer. Targeted engine and
Session transition tests pass, including active-voice refusal/retry, disabled
instrument promotion, copied Arrangement suffix tails, capacity rollback, and
exact-cut onsets. Clean full local CI and review remain with the integration
owner; timing and fixed per-instance storage are recorded in
[performance](docs/performance.md).

Menu follow-up2115: user accepted and manually merged PR37 into the stacked
integration branch fb4ec3a; all14 cleanCI passed e388613. Future picker2116 and
open-panel/focus-switching2117 is implemented in the focused detail-pane branch;
targeted UI, Session keyboard navigation and worker-reconciliation checks pass.
The pane's open state stays independent from user-driven clip/track content focus;
an empty clip context retains its last valid body height. Focused local CI remains
for integration, and native acceptance remains with the user.

Native metadata class facades2131: engine metadata/catalog/default/read/patch
helpers now take an explicit effect or instrument class; existing unqualified
helpers remain effect-only wrappers. Targeted engine tests cover Utility and sine,
wrong-class refusal and unchanged malformed/future instrument state. Four targeted
engine tests, all 33 Lattice/device tests, strict engine Clippy, formatting and
architecture/UI-style guards pass. Full local CI remains for integration; keep this
prerequisite separate from the instrument-slot panel2127.

Arrangement loop rail2120: source implementation adds a compact timeline-width
rail below the ruler, snapped drag preview, Command free placement, visible-edge
resize, click-to-enable, and the existing L/toolbar toggle. It uses the canonical
loop command; captured previews cancel if project generation, revision or the
canonical loop range changes. Five targeted GPUI pointer tests, three existing
canonical loop tests, strict UI Clippy, formatting, architecture/style guards and
diff checks pass. Coverage includes snap/free placement, zoom/pan mapping,
outside-rail release, lane-selection ownership, resize, activation, toggle and
stale-state protection. Clean full local CI and native acceptance remain open;
this stays a separate focused PR.

Indexed analytic sampler banks2125: prepared Arrangement occurrences and Session
intro/repeat spans retain compact per-track bank indices; query/release paths
resolve copied samplers without extending bank lifetime. Keys include only emitted
notes, preserving source-window admission. Focused engine/session/export and
callback allocation checks pass; clean full local CI remains the integration gate.
Accounted repeated-note sampler/index storage falls in the Session fixture, while
unique-heavy Arrangement storage rises; noisy timings establish no speedup.
FM4 registry/state admission is implemented by Pulse2154; native instrument
panel2127 is implemented in source below. Full Operator remains deferred. Pulse
remains authoritative for review and acceptance.

Instrument panel2127: canonical instrument slot now renders before effects, with
registered add/replace/remove/bypass and no reorder controls. Session routes
parameters and gestures by canonical slot ownership; UI caches class/type identity.
Sine exposes no fabricated parameters. Full Session tests, rendered panel actions,
nine gesture tests, strict affected Clippy and source guards pass. FM4 now uses
this generic panel; full Operator remains open.

Pulse2133: the analytic sampler payload is widened to41 owned `u64` words for
the standalone validated FM4 kernel, with borrowed per-frame sampling and
bit-exact legacy sine output. Pulse2146 adds an exact version-1 authored-state
codec and generic definition. Pulse2154 registers that definition as a built-in,
selectable instrument through the existing engine, Session, panel and export
seams; no device-specific storage or schema branch was added. Mutable FM feedback,
full Operator and physical listening remain open.
Pulse2146 source checks: 37 `hex-devices` tests, strict device all-target
Clippy, workspace formatting, architecture/style guards and GPUI patch guard pass.
Full local CI, review and any native device/playback acceptance remain open.
Bank and fixed Session MIDI
scratch growth are derived and documented in [lattice instruments](docs/lattice-instruments.md);
release timing and Session preparation before/after are recorded in
[performance](docs/performance.md). Focused source checks remain separate from
root full local CI and review.

Native add-versus-replace contract2148: adding an instrument now refuses an
occupied slot without changing history or publishing playback. Explicit replace
requests identify the existing device, class and registered type; replacement
preserves stable identity, chain position and bypass and remains Undoable. The
device menu's replace action routes through this request, while add stays add.
Targeted Session/UI checks pass; clean full local CI remains before review.
Native interaction acceptance remains with the user.

Pulse2136 native device browser and ordered rail: the existing left browser now
shows actual registered Utility and Sine definitions in its devices category and
starts drags without requiring prior track focus. The device rail renders the
canonical ordered chain with card-half stable-ID insertion targets, a wide empty
end target and compact insertion previews. Browser effects add before/after or
at the end; an instrument adds at the front only when its slot is empty. Existing
cards reorder by header grip and keep identity. Stale generation/revision, missing
source/anchor, occupied instrument, active gesture and pending edits refuse
without publishing a drop; release outside and Escape clear transient preview.
Rendered GPUI coverage exercises browser discovery, a hovered-card insertion,
outside-release cleanup and Escape; Session coverage exercises effect/instrument
add, occupied refusal, stable move, stale-state refusal and active-gesture refusal.
Targeted checks pass: 334 UI tests (2 existing ignored), Session drop/gesture
tests, affected strict Clippy, formatting and architecture/style guards. Full
local CI remains before review; native pointer/track-focus review remains with
the user. FM4 appears through the same registered catalog as sine and Utility.

Pulse2152: starting a native-device drag from the left browser opens the device
pane and switches its content from clip details to devices, while preserving the
current focused track and clip selection. A missing track remains an empty pane
and invalid drop target; cancel or release without a target clears the drag but
keeps the pane open. Ordinary track focus continues to honor the user's closed
pane choice. Four targeted GPUI device-browser/device-focus tests, strict
`hex-ui` Clippy, formatting, architecture, style and GPUI patch guards pass.
Full local CI and native review remain open.

Pulse2153: native metadata reports generic state-dependent parameter
availability. Utility left/right channel mode disables width and mid-side
controls without clearing authored values; stereo/swap restores editing. The
contract defaults valid parameters to enabled, and canonical patching remains
available for inactive stored values. The rendered actor test rejects disabled
width/balance drags and mid-side toggles without revision or history changes,
then confirms values and controls return in Swap mode. The targeted device-panel
tests (17), contract/device/engine tests, affected strict Clippy, formatting,
architecture, UI-style, GPUI-patch and diff guards pass. Full local CI remains
before review; native UI acceptance remains open.

Pulse2154: the validated FM4 definition is admitted to the built-in instrument
registry and uses the existing generic slot/state/session/export pipeline. No
schema or FM4-specific project-storage branch was added. Source tests cover
independent carrier PCM through Arrangement, Session and streamed hybrid playback,
PCM16 export, save/reopen plus Undo/Redo and cycle-refusal atomicity, browser drag
and parameter rendering, and zero total/net allocation/deallocation in callback
publication/rendering scopes. The probe separately confirms reclamation occurs on
control with the expected negative net allocation count/bytes.
Focused device, engine, Session, CLI, UI and audio allocation-probe suites, strict
affected Clippy and source guards pass. A short release profile found 100×16 FM4
block maxima above nominal periods; Pulse2108 owns the open performance follow-up.
Full local CI and user native listening remain separate acceptance gates. See
[FM4 contract](docs/fm4.md) and [performance evidence](docs/performance.md).

Pulse2156: FM4 preparation now skips operators that cannot reach a nonzero-level
carrier, using existing prepared-level fields so the bounded sampler ABI and
owned payload size remain unchanged. It still validates all authored oscillator
frequencies and routing, including unreachable operators. An independent full-
evaluation oracle compares sample bits for the optimized and packed samplers
across single/additive/serial/branched/silent graphs, signed-zero routing,
zero-level middle modulators, quiet/normal/full velocities and release ages.
Matched release profiles preserve whole-run PCM hashes; default single-carrier
workloads improved in the observed samples, while the fully audible four-stage
case showed a small noisy cost. These timings are not callback deadline proof;
see [FM4 performance evidence](docs/performance.md). Targeted checks and full
local CI remain before review; native listening remains separate.

Pulse2158: an injected mutable-only feedback instrument now exercises the real
shared Arrangement/Session event path with transactional runtime history, checked
occurrence IDs, canonically ordered note-off/on batches and additive instrument-gap mixing.
The test seam refuses unsupported mutable loops, active-state seeks, legacy
Arrangement rendering, inherited future route changes and candidate pre-cut
notes (including on newly added tracks); Back to Arrangement checks the
candidate's Arrangement notes rather than the old Session owner. Stop is the explicit reset.
Pulse2160 now carries nonquiescent mutable owners across revision handoffs with
their original gate deadlines and old effect suffix, or clamps held gates at
the cut when the candidate instrument is absent/bypassed. Mutable tail/seek/loop
chase limits remain separate prerequisites.
Source tests cover short-note partitioning, the 128-onset/256-edge query bound,
hybrid audio plus an ordered suffix, atomic refusal/retry and injected registry
preparation. Onsets sort by source frame and full occurrence key before ID
assignment, and emitted events use frame, off-before-on phase, then ID. The
varied-velocity/pitch oracle confirms stable same-frame voice order across
query partitions. With the 2160 tail owner/ledger bank included, the checked
mutable-event host component is 3,761,744 bytes per maximal 256-track
SessionPlayback on this 64-bit arm64 build, separate from the 2 MiB active and
2 MiB retained-tail declared payload limits. An active/candidate pair is
7,523,488 bytes for this component; older and retired publications can coexist,
so this is not a process-wide bound. See
[lattice instrument lifecycle](docs/lattice-instruments.md). Full local CI,
including the protected cache-ownership runner, remains with root; this does not
admit a built-in mutable instrument.

Pulse2159: export jobs now retain the exact captured `Arc<PreparedRevision>` and
render the linear Arrangement range with the shared SessionPlayback owner; no
project clone or worker-side recompilation is used. Prepared mutable export is
proof-only: start-zero is supported, nonzero starts are refused if the compiled
Arrangement has an enabled mutable instrument, and mutable Arrangement source
loops are refused. The global transport loop and unused Session loop inventory
do not wrap or block this export. Feature-gated integration coverage checks an
independent hybrid prefix/instrument/suffix PCM16 oracle, release across a
256-frame boundary, exact range lengths and atomic refusal without temporary
files. Focused engine/export/session tests, strict Clippy and local-CI runner tests pass;
full local CI remains with root; built-in
mutable admission, chase/range support and user-facing mutable instruments are
not established. The same enabled track slot currently prepares a mutable
factory for both Arrangement and Session; a Session-only mutable factory cannot
be represented by current preparation, so this case is not claimed as tested.


Pulse2167: standalone original six-value operator envelopes and a four-envelope
bank are implemented in hex-devices, with linear attack, exponential decay and
instantaneous-value release, explicit finite cutoff and frame units. Independent
curve/boundary/zero-stage/malformed/rate/partition oracles pass with all45 device
tests. Existing registered FM4 state/output is unchanged. Strict Clippy and source
guards and all16 clean local checks pass (PR65). Parent2161 stays open for FM routing/event binding,
rollback and native device integration. See [envelope contract](docs/operator-envelopes.md).


Pulse2168: original standalone PreparedOperatorFm4 applies four independent
envelopes before modulation/carrier routing through one private oscillator core.
All49 device tests and strict Clippy pass, including independent full-output
serial/additive FM oracles and exact legacy PCM. Initial helper overhead was
repaired with inlining/static shaping; final short paired median0.349→0.352s,
not native deadline evidence. Full local CI remains pending. No device registry,
state, UI or mutable voice admission changed; parent2161 native integration is
open. See [original enveloped FM contract](docs/operator-fm.md).

Pulse2163 exact compatible mutable continuity is implemented on the focused
engine branch. Prepared revisions retain exact bounded descriptors for mutable
tracks; unchanged Arrangement and launched Session owners carry both mutable
runtime twins, occurrence ledgers and next IDs across handoff. A real Editor
rename preserves held and recently released feedback history, and deleting an
earlier track verifies stable TrackId mapping while its old-law release tail
finishes separately. Changed Arrangement source is still refused while a
candidate occurrence would require chase. All 134 hex-engine unit tests and
its integration suites pass with the injected instrument seam, as do all 24
hex-lattice unit tests; all hex-audio
allocation-probe tests pass, including zero-total and zero-signed-net callback
checks for unchanged carry/refusal and control-side retirement. Strict affected
Clippy, formatting and architecture/style guards pass. Full local CI and review
remain with root. This does not add changed-source/config suppression, mutable
seek/chase or loop continuity, effect residual-tail quiescence, or a built-in
mutable instrument. Exact-carry handoff now preflights the complete effect chain
before copying history; a lattice-level adversarial suffix-compatibility test
covers rejection. A Session source-edit regression confirms the existing global
historical floor still carries an unrelated unchanged mutable route. The
PreparedRevision test seam does not inject effect registries, so the specific
incompatible-suffix case is not represented as a full compiled handoff fixture.


Pulse2166 implements changed mutable-instrument handoff suppression on the
focused engine branch. Dispatch proofs retain stable authored occurrence
identity and exact gate/release deadlines through NoteOff, including short
same-block notes. A zero-tail runtime regression verifies an exclusive-block
boundary NoteOff is delivered exactly once across further render partitions
and the runtime reaches quiescence. Exact candidate occurrences across
configuration/slot handoffs are suppressed through the later old-law or
candidate declared tail;
repeated revisions carry the existing owner once and extend the same proof.
Changed historical payload/timing with reused IDs still refuses chase. Proof
storage is fixed at 64 entries per runtime twin, justified by the compiled
overlapping-span ceiling that includes release tails; it does not narrow the
DSP voice limit. The maximal host component is 6,910,544 bytes versus 3,761,744
before this change. A release probe covers 100 repeated handoffs with 64 active
occurrences; the existing 100-track unchanged handoff profile preserves its
PCM checksum. See [mutable handoff performance](docs/performance.md) and the
[owned occurrence contract](docs/lattice-instruments.md). Engine-level
runtime/PCM/clock refusal remains retryable after capacity failure. Canonical
editor history while an audio candidate is pending is separately tracked by
Pulse2180 and is not established here. Engine/audio targeted tests, allocation
probes, affected clippy, format and repository guards pass; root performs full
local CI after this branch is frozen. Native UI,
device deadlines and multi-track stress remain unverified.

Pulse2180 adds durable UI-side audio admission reporting keyed by project
generation, canonical project revision and audio revision. The footer distinguishes
the current pending/applied/rejected canonical revision from the last admitted
canonical graph; numeric device/send previews remain separate transient overlays.
Exact rejected canonical candidate identity is captured before acknowledgment
pruning, so stale/coalesced rejection cannot overwrite newer accepted or pending
work. A preview rejection is visible only while its project generation, canonical
revision, gesture key and sequence are still current; it does not label its reused
canonical revision rejected. Preview application and cancel/restore do not change
canonical admission. Queue-full/deferred publication remains pending.
This does not roll back canonical project history, dirty state, undo/redo, or
worker save/export snapshots, and the callback protocol is unchanged. Focused
controller/tracker tests cover same-number project replacement, exact rejection,
coalesced older rejection with newer applied work, same-revision preview
apply/reject/cancel/restore, later canonical commit, stale applied work, recovery
reset and status transitions. Existing session-service tests cover compile
refusal history preservation and captured export revision; native footer behavior
and close/recovery interaction still need UI acceptance. Targeted checks and the
root acceptance/full local-CI pass remain outstanding; keep this ticket open
until those gates are recorded.

Pulse2113: the mixed Session profiler's independent oracle now handles canonical
hybrid snapshots when expressing Arrangement transport wraps as source periods.
The regression failed at frame72,001/120,001 before the repair and now matches
full exact PCM through direct/partitioned/prepared-wrapper playback under both
tempo fixtures. All six release profiler modes complete;100-track timings and
immutable source/binary evidence are in [performance](docs/performance.md).
Runtime DSP, scheduling and tolerance are unchanged. Full local checks and PR
review follow; no new native app behavior or user interaction test is required.

Pulse2170: distinct version1 Operator control codec and52 reusable parameter
descriptors are implemented, with strict224-byte round trips, finite/domain and
acyclic graph validation, immutable patches and original per-operator defaults.
All54 device tests pass, including independent PCM field-binding/routing oracles.
Existing FM4 state/output stays unchanged. Full local CI evidence is tracked in
its PR and Pulse ticket.
See [control contract](docs/operator-state.md).

Pulse2171: standalone original Operator now supports a584-byte immutable
configuration and40-byte notes, sharing one private oscillator loop with legacy
FM4. All57 device tests/strict Clippy pass; independent oracles and exact compact
PCM match across routing, release, rate and query boundaries. Short paired release
measurements show similar speed and bounded note-memory reduction, not native
deadline proof. Full CI evidence is recorded in the focused PR/Pulse ticket.
See [compact kernel contract](docs/operator-context.md).

Pulse2172 registers the original bounded acyclic Operator sine core through the
shared analytic-context instrument API. The generic catalog, 52-parameter view,
Arrangement/Session compiler, prepared export and callback all use the same
definition; 40-byte notes borrow one immutable per-instrument context. Independent
PCM oracles cover Arrangement, Session, seek, loop, changed-state handoff and
PCM16 export; generic opaque persistence and project undo/redo preserve state.
The callback allocation probe covers real Operator render, changed-state
publication, and control retirement. Strict package checks and full local CI are
pending review. This is not mutable feedback, full Operator, Live parity, or
native listening acceptance. See [registered instrument contract](docs/operator-native.md).

Pulse2183 adds independent sine/saw/square/triangle selectors for all four
Operator oscillators through generic Choice metadata and version2 native state.
The first224 version1 bytes migrate bit-for-bit, including signed zero, with
all-sine defaults; legacy sine PCM remains exact. Non-sine shapes use a bounded
control-prepared process-wide harmonic bank, strict per-note Nyquist caps and
four inline cap indices. Targeted hex-devices tests pass (66), including an
independent Fourier/DFT oracle, malformed payload refusal, generic registry mixed
rendering and100 distinct contexts each under the4 KiB admission ceiling. Context
size is592 bytes and notes are48 bytes. Table data is245,880 bytes; the measured
24-byte OnceLock cell includes its Box descriptor, for245,904 bytes before allocator bookkeeping. A reproducible
release headless profile is added but has not yet been run; project-level mixed
render/persistence, callback allocation-probe and native listening remain open.
Full local CI and review remain with the integration owner. See
[waveform contract](docs/operator-waveforms.md).

Pulse2187 adds a derived Operator routing preset choice, parameter ID61, without
changing version2 state or its240-byte payload. `serial`, `parallel` and
`two pairs` atomically replace only the existing routing matrix/carrier fields;
`custom` is inferred from exact topology equality and selecting it leaves routing
unchanged. Generic state/view, matrix/carrier classification, invalid input,
bit-near custom detection, exact manual classification, idempotence, invalid
choice/type, state size/version preservation and field-preservation tests are
included. Device tests (70), Operator Arrangement/Session admission tests (2),
native device view tests (5), strict device/engine Clippy, formatting, architecture
and style guards pass. One canonical Session preset edit passes exact-state Undo/Redo coverage.
Arrangement/render/persistence history remains generic because presets write only existing fields. Native UI acceptance and review
remain open. See
[routing preset contract](docs/operator-routing-presets.md).

Pulse2173: current architecture orientation now documents one freely positioned
instrument, actual prefix/instrument/suffix rendering and the canonical prepared
editor path separately from the fixed generated demo. Source was checked against
compiler/bootstrap/session/audio ownership; links and diff reviewed. Historical
milestones and native/mutable/performance limitations are preserved. Docs-only
local guards/format evidence is tracked in its separate PR/Pulse ticket.

Pulse2169 adds the generic shared immutable analytic-context seam. One bounded
context table is prepared per revision and shared by Arrangement and Session;
compact note payloads borrow the context for sampling, and copied releases
retain/remap old context owners across graph handoff. Compatible exact
device/state/type/rate contexts reuse their control-created owner. The measured
64-bit shared-context lease/remap component is 16,598,024 bytes per maximal
SessionPlayback, below the 18 MiB scoped cap; active/candidate pair is
33,196,048 bytes below 36 MiB. Context payload caps are 4 KiB each and 2 MiB
per revision or retained playback. Audio allocation-probe coverage checks old
PCM through a changed-state handoff, zero total/net callback allocations and
control-thread final context destruction. Sine/FM4 inline sampler layout and
PCM are unchanged. Shared contexts do not admit Operator; root owns final CI,
review and publication, while measured standalone Operator context/note work
remains a separate prerequisite.

- 10 October, Pulse2169 acceptance: exact TrackId joins shared-context reuse
  identity; bounded planning refuses short/capacity-exhausted storage. The last
  fallible phase stages one owner clone per unique context while previous roots
  remain intact, followed by no-fail runtime ownership moves. Borrowed sampling
  repairs a measured draft regression. Three 100-track baseline/candidate runs
  preserve PCM and differ ~0.5–0.6%; injected1,600-note shared lookup measurements
  are documented in docs/performance.md, without a native-deadline claim.

Pulse2174 candidate admission: a SessionPlayback may be handed off as a candidate
only while its fresh admission latch is open. Public apply/render calls and an
accepted legacy seed close admission; handoff closes it before the first runtime
mutation. Pre-mutation ReleaseBankFull refusals retain retry eligibility. Added
source regressions cover used and accepted candidate rejection plus preservation
of candidate/predecessor state and PCM. Engine tests (146), audio callback
allocation probes (16), affected strict Clippy, formatting and diff checks pass.
Full local CI remains with the integration owner; no native acceptance is
claimed.


Pulse2176 return-track model/storage: canonical stable ReturnId buses, gain/mute,
generic effect chains, bounded sparse regular-track sends and transactional
commands are in schema17. Send and device state survive snapshots, undo/redo and
strict round-trip; return removal cascades sends, track removal drops its sends,
and schemas1–16 migrate to empty returns/sends. Playback compilation explicitly
refused routes before audio2177. Model PR77 passed all16 clean exact-head local
checks; see [return track contract](docs/return-tracks.md).

Pulse2177 audio routing: implemented regular-track post-fader sends into ordered
Return effect chains, Return gain/mute, Main sum and one master gain in Arrangement,
SessionPlayback and prepared linear export. Return runtime owner identity is typed
Track/Return; processor admission requires registry preparation and explicit zero
algorithmic latency. Fixed 256-frame scratch and chains are prepared on control;
compatible bus state is inherited after preflight, seek/stop reset matches regular
effects, loop wraps preserve history, and removed-return runtimes retire on control.
Removed regular-track release tails keep existing Main-only processing and their
sends stop at the revision boundary. Focused routing, tail, handoff, unsupported
latency/class, prepared-edit atomicity, export, and callback allocation probes pass.
Three100-track/16-return release samples have matching PCM digests and documented
memory bounds in docs/performance.md. Full exact-head local CI and native listening
remain open; UI source integration is Pulse2178. See [return tracks](docs/return-tracks.md).

Pulse2178 pinned return UI: return rows are pinned immediately above Main and
scroll internally within a 67 px cap; return focus preserves regular-track
selection and project-wide time scope. Return effect controls reuse the regular
device descriptor/gesture lifecycle and reject instrument/cross-owner drops.
Per-track send menus show named destinations with a bounded 0..2 slider and
numeric value field. Pulse2179 extends this to a single-active typed send gesture:
pointer movement and valid text entry update transient previews and produce one
canonical history edit at the terminal action. A source track scrolling out of
view finalizes only its send draft; invalid text cancels back to canonical state
with a lowercase error, and refused commits keep the draft accessible by scrolling
the owner back into view. Closing for scroll/removal restores editor focus and
removed owner IDs retire cached controls. The top-level popup is clamped to live
window bounds, checked at 880×540, and each NumericControl is mounted once to keep
hit/focus ownership unique. Return-only rail and device rendering are extracted
into focused editor modules. Acceptance includes the service PCM/send and
Utility workflow, undo/redo, bypass/reorder/remove, active-gesture cancellation,
schema-17 save/reopen, regular-chain instrument positioning, removed-return
numeric-draft cancellation and clip/sample drop-target clearing. The 6 focused
send-gesture UI tests, 25 device-panel UI tests, 9 Arrangement/Main UI tests, 1
clip/sample drop UI test and 5 return service integration tests pass, as do affected
strict Clippy, format and source guards. Full clean-head
local CI and native user UI/listening acceptance remain open. The 100-track/16-return
audio profile has three additional clean-commit repetitions with matching PCM
digest; see [return-track performance evidence](docs/performance.md) and
[return-track contract](docs/return-tracks.md).


2026-10-10 review51 integration checkpoint: mutable occurrence suppression and
its zero-tail NoteOff repair passed all16 clean-head local CI at `90dae89`
(report `20261010T050643Z-055f49b7`) and are published as PR80. Continuous
return-send gestures and the popup input fix are integrated from `4f2d141`;
source-focused tests pass and that branch's full local CI is running. The
combined source retains the single freely positioned instrument contract.
Combined exact-head validation, a fresh signed build and native user acceptance
remain open at this documentation checkpoint. Pulse is the live review authority;
this paragraph supplements earlier source checkpoints rather than marking the
parent Lattice/Operator/return workflows complete.

Pulse2181 source checkpoint: `hex-engine::session_playback` now provides a
control-owned bounded store of per-track in-memory mutable-instrument snapshots.
Each keeps the committed runtime twin pair with owned occurrence proofs and the
next ID; exact `PreparedRevision` Arc identity, track, sample rate and performance
frame gate restore. Capture rejects an open speculative block; restore preflights
the whole candidate and changes the target only after compatibility/copy
succeeds. The store allows 16 entries and charges pair declarations, full
two-sided proof buffers and all inline slots against 256 KiB; the fixed slots
remain charged after clear. Restore's retained-store plus temporary-candidate
operation peak is capped at 512 KiB. One feedback fixture charges 17,976 bytes
on macOS arm64, with `size_of` terms varying by architecture. Test-only feedback
continuations match exactly for held notes and ended-gate feedback tails across
different block partitions. This oracle covers instrument state and the engine
occurrence ledger only; it does not validate transport, route, return or effect
history. Refusal tests cover uncommitted blocks, revision/frame mismatch,
unchanged target IDs and factory-free over-limit admission. These are composable
track checkpoints at zero replay distance, not project/seek checkpoints:
transport/source loops, Returns/effects, persistence, offline replay, callback
publication, and 2164/2165 integration remain open. Targeted engine checkpoint
tests (3), engine suite (153 passed/1 ignored), lattice suite (24) and strict
Clippy pass. Architecture, UI-style, GPUI-patch and format guards pass. Full
local CI and parent integration review remain outstanding. Do not mark Pulse2181
done or describe this as seek support.

Pulse2182 Arrangement preroll foundation (source checkpoint):
`SessionPlayback::prepare_arrangement_preroll` constructs an independent,
Arrangement-only candidate from the same prepared revision and renders forward
from frame zero in bounded 256-frame blocks. The caller chooses the replay-frame
budget; the hard limit is 8,388,608 frames, cancellation is checked before
construction, at each block boundary and before return, and accounted candidate
runtime storage is capped at 32 MiB. Preflight includes runtime chains, mutable
instrument twins and ledgers, route/tail/release/scratch/context tables; shared
immutable graph data and allocator bookkeeping are excluded. Actual vector
capacities are checked after construction. Enabled Arrangement transport loops
are explicitly refused. Tests compare future PCM for held and ended-gate mutable
instrument feedback, stateful Utility track/return effects and sends; they also
cover pause/play, zero frame, cancellation, budget/cap boundaries, loop refusal,
render failure, source immutability and measured preflight-vs-actual accounting
(20,638,328 bytes on this fixture, including the 2,048-byte render scratch). A
100-stateful-track release profile fixture is present and passes a debug smoke
run, but clean exact-head release timing evidence remains pending. This is not
live seek or native publication: live route fencing, Session ownership, loop
reconstruction and supersede/Stop admission remain deferred to 2164/2165. Targeted
preroll tests (8 passed/1 ignored), all SessionPlayback tests (108 passed/2
ignored), lattice tests (24 passed), affected strict Clippy and architecture,
GPUI-patch, UI-style and format guards pass. Additional regressions prove a
declared storage-budget refusal occurs before candidate runtime factories,
cancellation is observed after the final rendered block, zero-frame play stays
playing, and an empty project can advance beyond its content with exact clocks
and silence while leaving the source untouched. Full clean-head local CI and
native seek integration/acceptance remain pending. Keep Pulse2182 open until
integrated acceptance is complete.

2026-10-10 combined review52 source milestone: PR82 canonical admission status
and PR83 bounded instrument checkpoints supplement review51 (PR38–81).
Each focused exact head passed all16 local checks on a clean tree; the combined
exact-head suite and fresh signed app are the next gates. PR82 preserves saved
edits/history and separates transient preview failures from canonical audio
status. PR83 provides instrument/ledger state copies at an exact capture frame;
whole-Arrangement replay and native seek remain Pulse2182/2164. Native UI/listening
acceptance is still required, and no new PR has been merged into main.

2026-10-10 Pulse2183 integration checks: mixed-waveform Operator state now
round-trips through generic storage alongside the retained version1 fixture.
Prepared export matches partitioned prepared playback sample-for-sample after
PCM16 conversion. The actual headless audio callback renders, admits a waveform
change, retires old contexts and shuts down with zero total allocations and
zero signed-net callback allocation counts/bytes. Control-side retirement is
measured separately. Targeted export tests (2), storage persistence (19) and the
mixed-wave callback probe pass. Full local CI, clean release performance and
native UI/listening remain the next gates; no full Operator parity claim.

2026-10-10 combined review53 source milestone: Operator periodic waveform
choices (Pulse2183) and bounded headless Arrangement preroll (Pulse2182/PR84)
supplement review52. Both focused clean heads pass all16 local CI checks; source
reports are20261010T060551Z-824befe4 and20261010T055902Z-f0e1f590. Release
profiles and their limits are recorded in docs/performance.md. The previous
source-checkpoint notes above are historical; combined exact-head CI and fresh
signed build are next. Native seek integration/fresh-publication fences remain
2164/2186, and native waveform UI/listening remain for user acceptance. No
review PR has been merged into main automatically.

2026-10-10 Pulse2188 Arrangement seek worker source milestone: `Service` now
owns a dedicated nonblocking seek-preparation endpoint independent of the
canonical edit worker. One running request is cancellable at the engine's
<=256-frame boundaries; one newer pending request replaces older pending work;
one result slot never blocks new admission. New requests/invalidation remove
stale pending state. Replaced pending metadata and its prepared-revision `Arc`
are dropped by the submitter after unlocking; unread runtime candidates move
under the mailbox lock through fixed worker-owned retirement/ready slots and are
dropped by the worker after unlocking. Normal operation retains one candidate;
synthetic defensive dual-stale coverage confirms the two fixed slots remain alive
until the worker drains them. Latency-sensitive callers must retain their
audible/current prepared revision across supersede or invalidation until worker
ownership drains; final immutable graph release is not guaranteed to avoid the
caller thread. Shutdown signals and wakes the worker without joining the caller;
a teardown test verifies the detached worker exits and releases its captured
owner on the worker thread. Candidate construction starts directly from
the exact PreparedRevision Arc, and `Playback::from_arrangement_preroll` installs
that state without a post-preroll seek/reset. Results preserve request ID,
caller generation/revision, graph Arc, destination/work budget and play intent.
Engine preroll ceilings remain 8,388,608 frames and 32 MiB accounted candidate
runtime storage (at most two candidate budgets in defensive dual-stale state).
Eight worker lifecycle tests, one Service endpoint test and the
engine's 108-test SessionPlayback suite (2 ignored) pass, including held/released
mutable-feedback continuation through the `Playback` wrapper. UI use, current
project generation admission, live route/Stop fencing, interactive Session-route
promotion and native publication remain Pulse2164 and are not implemented by
this source milestone. The linear candidate refuses Session route actions and
cannot be installed as an interactive native runtime without the separate
promotion contract.

2026-10-10 Pulse2186 fresh-runtime publication fence: source adds a conditional
fresh-install policy tied to a control-captured route snapshot and admitted
command ticket. The callback validates active revision, active route-state
generation, Acquire-loaded command ticket and pending Stop immediately before
swap. Stale candidates receive a typed `runtime-fence-changed` rejection and
retire through the existing control-owned queue; Stop remains pending and affects
the old graph. Commands/Stop admitted after successful validation act on the new
graph. Retirement backpressure leaves the old graph operational and rechecks the
fence once a slot is reclaimed. Tests cover empty revision zero, exact match,
independent stale revision/route/ticket/Stop, both command/Stop linearization sides
and delayed stale rejection. A distinguishable candidate regression compares
exact old Session PCM and verifies its route owner survives a stale rejection.
A full retirement-ring refusal verifies the rejected candidate stays owned by
`held_retirement` until control reclaims space. A callback allocation-probe covers
accepted, rejected and backpressured paths. Audio unit tests (31), callback
allocation probes (19), strict audio Clippy, formatting and architecture, GPUI-patch
and UI-style guards pass. This does not supersede session-worker project identity/
admission checks or change unconditional fresh/inherited publication. Full local
CI, exact-head review and native integration remain pending; do not mark Pulse2186
done before those gates.


## 2026-10-10 combined native review 54

Combines source PR38–87, adding conditional fresh-runtime publication (Pulse2186,
PR86) and original derived Operator routing presets (Pulse2187, PR87) to
review53. Both focused heads passed all16 local checks independently:
7227e144 / `20261010T062045Z-817fcd13` and
66f0abe / `20261010T062240Z-7d26bc1a`. Canonical preset changes now have exact
one-edit Undo/Redo coverage. Combined exact-head CI, fresh release/signing and
user UI/listening acceptance are separate gates. Native seek worker integration
is active on Pulse2188 and not included in this checkpoint; full Operator parity,
mutable loops, plugin hosting and general latency compensation remain open.

2026-10-10 Pulse2190 interactive Arrangement preroll source milestone:
`ArrangementSeekMode::Interactive` constructs the ordinary Session runtime
before bounded Arrangement replay; linear export-style preparation remains the
default and unchanged. The seek request/result carry the mode so the future
caller can require interactive eligibility explicitly. Interactive candidates
preserve the exact prepared revision, clocks, mutable instrument history,
release tails, and track/return effect history, remain Session-route capable,
and can serve as predecessors for ordinary compiled graph handoff. Both modes
refuse enabled transport loops. A prepared-revision policy query keeps analytic
Sine/Operator instruments on direct absolute-frame evaluation while requiring
preroll for mutable instruments and stateful track/return processors.

The original high-residency interactive test overflowed the platform-default
thread stack; a reduced held-note oracle passes on a 2 MiB test thread after
sequentializing candidate/oracle construction and using one constructor call
selected by policy. The dedicated control worker reserves 8 MiB explicitly;
this is one per Service and is separate from the 8,388,608-frame and 32 MiB
candidate-runtime/scratch ceilings. `size_of::<SessionPlayback>()` is 104,568
bytes for this build target; this is not peak stack use. Pulse2195 tracks
measuring constructor stack residency for representative 100-track/stateful
workloads. Debug and release Service-worker verification now constructs both stateful
track Utility and sent stateful return Utility candidates on the normal worker.
Interactive continuation tests cover a held and an ended-gate mutable-feedback
note against independently partitioned uninterrupted output, track and return
filter history, Session route commands, and successor inheritance. Native UI
admission/publication, route/Stop fencing, enabled transport loops, and measured
100-track readiness remain open. Debug/release Service-worker checks and focused
crate tests/Clippy pass. An ignored release profile now reuses the 100-track
mutable fixture for both linear and interactive preroll; the interactive profile
uses 48,000 frames at 48 kHz and an 8 MiB worker stack. Captured release timing
evidence and its limits are recorded in [performance](docs/performance.md).

2026-10-10 Pulse2192 stateful Arrangement seek UI integration is implemented in
source on a focused branch. Pointer seek and Spacebar PlayFrom retain the exact
prepared revision, require an already-applied canonical Arrangement graph with
coherent Arrangement-owned routes, and publish through the admission-time fresh
runtime fence. A bounded latest intent waits behind a candidate whose publication
is unresolved; a second Spacebar during pending PlayFrom resolves to Pause even
while audio still reports stopped. Worker results that cannot be returned
immediately stay in one UI-owned retry slot; teardown hands that result to the
seek worker's existing retirement or empty ready slot. The bounded defensive
peak is up to two worker candidates plus one UI-held candidate, each within the
32 MiB ceiling. Existing `Control::reclaim` still runs on its caller, currently
the editor tick, so general callback-returned graph retirement is not claimed to
be off the UI thread. Focused session and UI seek tests pass; strict Clippy,
architecture/style guards, full local CI, exact-head review and native
output/listening acceptance remain pending. Enabled transport loops and measured
100-track seek readiness remain unsupported/open.

Root acceptance adds fault/shutdown admission gates and verifies all8 prepared
seek UI tests. After publication, cancellation suppresses UI completion but
cannot withdraw the accepted audio owner; canonical edits alone are not an audio
fence. A stronger cancellation barrier remains follow-up work.

## 10 October2026 — Pulse2205 Operator fixed tuning

Original per-oscillator Fixed mode, frequency and multiplier now use the generic
Lattice metadata/state contract. Version3 preserves old prefixes, uses288 bytes
and69 descriptors; shared context/note sizes are688/48 bytes. Independent PCM,
canonical refusal/history, opaque persistence and callback allocation checks pass.
See [the contract](docs/operator-fixed-tuning.md). Full local CI, focused PR and
combined native review/listening are pending; no full Operator parity claim.

## 10 October2026 — source review57 integration checkpoint

PR91 and92 plus Pulse2203 source are combined locally. Their focused pipelines
pass; cancellation2208 is being integrated separately before the next native
review build. The current006866f app remains available and unchanged. New Return
residual ownership2204 has its own branch. No automatic merge or native
acceptance is claimed. GitHub GraphQL PR creation hit a request rate limit;
GitHub CLI REST creation succeeded. Codex attachment registration reached its
100-identity limit, so verified PR links remain in Pulse and the ledger.

## 10 October2026 — Pulse2208 prepared seek cancellation barrier

Conditional fresh publications now arbitrate UI cancellation against the
callback's final apply CAS using a revision-keyed control-allocated token.
Cancellation before apply preserves old PCM and returns the candidate for
control reclamation; a callback winner completes before the canonical successor.
Ordinary inherited publication is unchanged. Race/queue failure/Stop/successor
unit tests, two callback allocation probes and the actual UI invalidation helper
test pass. Full local CI and native edit/Open/seek acceptance remain pending.
See [the contract](docs/prepared-seek-cancellation.md).

## 10 October2026 — review58 source checkpoint

PR38–95 are combined in this checkout, including native stateful seek (91),
Operator fixed tuning (92), captured track effect residuals (93), live Session
effect draining after source stop (94), and prepared seek cancellation (95).
PR91–93 and95 each passed their exact-head16-check local pipelines. PR94 passed
focused PCM, callback allocation and strict affected source checks; full combined
validation is next. Pulse2204 Return-chain residual ownership remains a separate
active change and is not included. No native acceptance, merge or new test bundle
is claimed at this source checkpoint.

## 10 October2026 — Pulse2195 playback stack evidence

An opt-in isolated subprocess probe measures constructor/preroll completion
thresholds for stateful Utility and100-track mutable/Return fixtures. Debug
passes at2MiB and overflows at1MiB; release passes at the512KiB probe floor.
Passing held/released PCM digests match across stack sizes and profiles. The8MiB
production worker reservation is unchanged. Requested-stack completion is not
peak/high-water stack measurement. Commands, raw logs, toolchain and executable
hashes are in[the evidence](docs/playback-stack.md). Affected debug/release
probes, strict feature-enabled engineClippy, formatting and architecture guards
pass. No production storage/DSP/schema change.

## 10 October2026 — Pulse2204 Return revision residuals

Whole old Return chains now retain residual output across removal, device
configuration/bypass/order changes, feeding the current Main under old bus
controls and current master gain. A separate64-slot bank shares bounded
track-tail accounting. Transfer validation precedes owner movement; exhausted
budgets preserve old PCM for retry. Stop/Seek, rollback and unchanged-history
semantics have focused coverage. Engine181pass/3ignored,12added tests, strict
engineClippy, formatting, architecture and the actual Return callback allocation
probe pass. Full exact-head local CI, PR publication and native listening remain
pending. No UI/schema/plugin/PDC expansion. See[the contract](docs/return-effect-tails.md).

## 10 October2026 — review59 integration checkpoint

Return residual ownership is[PR97](https://github.com/PORTALSURFER/hex/pull/97),
sourceff7cbbc. An old immediate-cut Return integration assertion was replaced
with an uninterrupted zero-fed filter oracle; all10 Return routing tests pass.
Its full local pipeline is rerunning after that repair. Constructor stack
evidence is[PR96](https://github.com/PORTALSURFER/hex/pull/96), source5707ae7;
its affected debug/release probes and strict checks pass. Both are combined on
`codex/lattice-review-59` for exact combined validation. The delivered e2b92a6
app remains unchanged and does not include96/97. No new native acceptance or
merge is claimed.

Pulse2165 is split into mutable source loops2214 (active) and transport seam/
loop-start chase2215 (todo, depends on2214). The existing transport-loop refusal
remains until bounded gates, chase, identity/capacity and rollback are verified.

## 10 October 2026 — Pulse2214 mutable source loops

Interactive mutable instruments now accept looping Session sources and finite
Arrangement source loops. Intro-only notes, clipped loop-end gates, distinct
occurrence identities and callback rollback retain the existing source contract.
Six focused tests and the actual callback allocation probe pass; exact-head local
CI and combined native review are next. Arrangement transport wrapping (2215) and
mutable Arrangement source-loop linear export remain refused.
See [the contract](docs/mutable-source-loops.md).

## 10 October 2026 — review60 source-loop test build

[PR98](https://github.com/PORTALSURFER/hex/pull/98) is ready for review at
1daa652; all16 exact source checks passed20261010T084224Z-35cfd2e9.
Combined sourcecd71021 passed all16 checks20261010T084321Z-a06c7557,
stale=false. Optimized release build took19.36seconds. Strictly verified local
ad-hoc app: artifacts/hex-lattice-source-loop-review-20261010-cd71021.app.
Its adjacent manifest records61 integrated PR links (38–98), exact source and
binarySHA256 eb35615920835b80792cb786bde0948f53dad47dc84bc66fff4b9dd3485a04fb.
Earlier review apps remain intact. Source-loop PCM, partitions, capacity/error
rollback and callback allocation probes pass; native repeated MIDI playback,
Utility/Operator behavior and save/reopen remain user acceptance. Transport wraps
2215 are active on their own branch, not included in this build; mutable
Arrangement source-loop linear export remains refused. No PR is auto-merged.
Codex attachment registration reached100 identities; verified PR links remain
in Pulse and this ledger. Performance comparison measurement is separate2218.

## 10 October 2026 — Pulse2218 comparator evidence

Measurement-only[PR99](https://github.com/PORTALSURFER/hex/pull/99), source54cbaf0,
adds a real chain-comparator release fixture.27 lattice tests, strict all-target
Clippy, formatting, release build/functional checks, CLI bounds/help and source
guards pass. Busy-host paired272-chain medians:143334ns shared-Arc matches,
2871083ns independent equal states,2900000ns final-device/final-byte mismatch.
Full median/p95 table, exact source/binary hashes and background load are in
[the report](docs/configuration-comparison.md). These are exploratory measurements,
not idle baselines or callback deadline evidence. Pulse2219 tracks idle-host
confirmation. Production behavior and the deliveredcd71021 app are unchanged.

## 10 October 2026 — Pulse2215 transport-loop implementation

A separate focused change adds bounded mutable Arrangement transport wrapping.
Pass zero keeps authored gate continuity; actual wraps attack active pre-loop
gates with fresh identities and the remaining gate duration. Admission proves
first-traversal, periodic and first-seam density against host and declared device
capacities. Loop-bound changes with active gates remain conservative refusals.
The144 SessionPlayback tests (3 ignored), strict engine/audio Clippy and the
wrap callback allocation probe pass with zero allocation/deallocation counts.
Full local CI, publication and native listening remain pending. This is not yet in the
delivered cd71021 app. See [the contract](docs/mutable-transport-loops.md).

Pulse2220 tracks a pre-existing aggregate event-capacity gap for finite
Arrangement and Session/source-loop admission. That reliability fix remains a
separate PR from transport2215. No hosted CI or automatic merge is used.


## 10 October 2026 — review61 transport-loop test build

[PR100](https://github.com/PORTALSURFER/hex/pull/100) is ready for review at
6c3f830; all16 source checks passed20261010T092023Z-84048646, stale=false.
Full CI caught and repaired a transport clamp affecting linear export; the
independent export PCM oracles remain unchanged and pass. Combined source
762aacd passed all16 checks20261010T092025Z-030a9f1f, stale=false. Optimized
release build took29.38seconds. Strictly verified local ad-hoc app:
artifacts/hex-lattice-transport-loop-review-20261010-762aacd.app. Its adjacent
manifest records63 integrated PR links (38–100), exact source and binarySHA256
2126c6d51eccb34a691f627d6699040fc047510b403d50b27b998ac98021970d.
Earlier apps remain intact; the app was not launched by the agent.

The app includes the comparator evidence99 and bounded mutable transport
wrapping100. Sustained MIDI loop playback, save/reopen and ordinary WAV export
remain user acceptance. Active mutable Arrangement loop-gate tempo/bounds edits
retain typed refusals; enabled-loop seek/preroll and mutable source-loop linear
export remain unsupported. Physical deadlines and full Operator parity remain
open. Pulse2215 is ready_for_review. Pulse2220 aggregate finite/source device
capacity admission is active on its separate branch and is not included in this
build. Verified PR links remain in Pulse after attachment registration reached
100 identities. No hosted CI or automatic merge was used.

## 10 October 2026 — Pulse2220 device-capacity admission

Finite Arrangement and Session/source-loop mutable MIDI preparation now checks
aggregate NoteOn/NoteOff density and release-inclusive voices against the prepared
device declarations. Cached Session templates and runtime handoff eligibility
include those copied event, voice and release limits. Same-source lower-capacity
and changed-release regressions prove fresh admission rather than stale reuse.

Targeted engine tests passed (208, 3 ignored), audio allocation probes remained
zero, strict engine Clippy and repository guards passed. Candidate rejection
preserves project/history/redo and previous nonzero PCM in an Editor regression;
service dirty/revision updates remain success-only by inspection and existing
transactional tests. Full exact-head local CI, PR publication and a fresh combined
build remain pending. No schema changes or callback preparation were added.

## 10 October 2026 — review62 capacity test build

Focused [PR101](https://github.com/PORTALSURFER/hex/pull/101), source4ecdca9,
passed all16 exact-head local checks20261010T094432Z-8ab91655, stale=false.
Combined source6b6c9fd passed all16 checks20261010T094503Z-122314a1,
stale=false. Optimized release38.38seconds; strictly verified ad-hoc app:
artifacts/hex-lattice-capacity-review-20261010-6b6c9fd.app. Adjacent manifest
records64 integrated source PR links (38–101), source and binarySHA256
c6de3241127134d620a0153694252ce206682733ec855f91574da028ccdd8346.

The app includes actual mutable device-capacity admission and Session cache
revalidation. User MIDI playback/looping, instrument reorder and save/reopen
acceptance remain pending; the app was not launched by the agent. Earlier apps
remain intact. Pulse2220 is ready_for_review; attachment registration reached
100 identities, so verified PR links remain in Pulse and this ledger. No hosted
CI or automatic merge. Pulse2222 per-oscillator amplitude velocity sensitivity
is active separately and absent from this app. Full Operator/reference parity,
plugin hosting, PDC and physical deadline acceptance remain open.

## 10 October 2026 — Pulse2222 oscillator velocity controls

Operator adds four independently validated amplitude velocity controls before
FM routing. Original bounded scales are prepared control-side; neutral defaults
preserve previous response. Statev4 appends16bytes, retains v1–v3 migration and
the v3 tuning prefix. The shared note stays8words/64bytes, including valid active
zero scales; callback decoding retains bounded payload checks and preparing-context
pairing without recomputing the velocity law per sample.

79 device tests, strict device Clippy, generic engine-view test, active registered
callback allocation probe and quick repository guards pass. The probe asserts
zero total/net count/bytes. Busy-host3-run direct profiles: before median0.697s
standalone/0.678s compact, after0.691s/0.702s; note/context/compact sizes change
600/688/48 to624/704/64bytes. No reliable speed, adapter timing, physical deadline
or Live numerical parity claim. Full exact-head local CI and native acceptance
remain pending. See docs/operator-context.md for workload and raw-log provenance.

## 10 October 2026 — review63 velocity test build

Focused [PR102](https://github.com/PORTALSURFER/hex/pull/102), source4a10773,
passed all16 exact local checks20261010T101119Z-4923c9f6, stale=false. Full CI
first caught stale descriptor/payload fixture counts; repairs retain independent
PCM oracles and add nonneutral history/storage coverage. Combinedccc6232 passed
all16 checks20261010T101108Z-a069085b, stale=false. Fresh optimized release
25.09seconds; strict ad-hoc signature and signed-binary hash independently
verified: artifacts/hex-lattice-velocity-review-20261010-ccc6232.app, SHA256
f2f9cfbc14cc0c926dacd7239fecb18eaf3a20d30b3e6dd2aa80471b9c75283b.
Its adjacent manifest records65 integrated source PR links (38–102).

The user checklist covers four amplitude velocity controls, notes at different
velocities, neutral previous sound and save/reopen. Native control/listening
acceptance remains pending; no app was launched by the agent, earlier apps remain
intact. Pulse2222 is ready_for_review. Codex attachment registration reached100
identities; verified links remain in Pulse and this ledger. No hosted CI or
automatic merges. Pulse2223 standalone bounded self-feedback DSP is separately
in_progress and absent from this app; native feedback admission remains a later
gate. Exact Live laws, full Operator, plugins, PDC and physical deadlines remain
open.

## 10 October 2026 — standalone Operator feedback kernel

Pulse2223 adds an original compact sequential feedback voice in hex-devices.
Four bounded amounts use prior fully shaped oscillator outputs as phase
modulation; incoming cross-modulation prohibits feedback on that oscillator.
Zero feedback preserves the existing analytic sampler exactly. Checked frame
advance and NoteOff retain atomic failure semantics and finite envelope tails.

84 device tests, strict device Clippy, formatting and architecture/style guards
passed. Independent sine, periodic and outgoing-DAG recurrence oracles cover
velocity/envelopes, partitions, zero-feedback exact PCM and release boundaries.
The 152-byte voice borrows its 704-byte prepared configuration. Busy-host
memoryless profiles do not time nonzero feedback or establish deadlines; missing
measurement-time provenance is explicit in docs/operator-feedback.md.
Exact-head full local CI and publication follow. Native feedback controls, state
codec and runtime admission are separately tracked by Pulse2225, including safe
same-Operator analytic/mutable transitions and preserved unavailable authored
values. Current review63 app remains unchanged; no native feedback/parity claim.

## 10 October 2026 — review64 feedback-kernel checkpoint

Focused [PR103](https://github.com/PORTALSURFER/hex/pull/103), source00cd95f,
passed all16 checks20261010T102648Z-f059c465; combined709d846 passed all16
checks20261010T102658Z-53636e26. Both clean/stale=false. Pulse2223 is
ready_for_review; native feedback admission2225 remains separate.

Fresh optimized release28.32seconds, strict ad-hoc signature independently
verified: artifacts/hex-lattice-feedback-kernel-review-20261010-709d846.app.
Signed binarySHA2566572a324c47d85b20af4b7adc607f8dda0e5d39a5a96018f0fcffc1b08bf2689.
Adjacent manifest records66 integrated sourcePRlinks38–103. This source includes
the standalone DSP prerequisite; registered native feedback controls/runtime
remain absent. User can continue testing existing velocity controls, MIDI
playback, device reordering and save/reopen. No agent app launch/native acceptance
or automatic merge. Previous review apps remain intact.

Next2225 preparation found a native memory budget decision: a152-byte voice
with64slots exceeds the existing4096-byte mutable twin cap even before IDs.
Astra is reviewing a narrow capacity/budget choice before implementation; no
silent global budget expansion or64-voice/scalability claim.

## 10 October 2026 — registered Operator feedback admission

Pulse2225 adds version5 state with four persistent authored feedback amounts.
Unavailable incoming-FM controls retain their values and use effective zero;
all-zero effective feedback retains analytic rendering. Nonzero feedback selects
a fixed 16-voice, 256-event mutable runtime with a 3,640-byte twin on arm64.
Higher polyphony and continuous active-voice parameter changes remain separate
Pulse2226/2229 follow-ups.

Actual Operator tests cover both active class-switch directions on Arrangement
and Session, exact-cut attacks, exclusive release quiescence, removed-source
tails, bounded worker seek, transport loops and prepared export. A concrete
Session mutable-to-analytic lost-note bug is prevented by pre-mutation refusal
for a retained same-device source; removed sources remain eligible for tails.
Refusal preserves applied playback; canonical desired-state history may already
be committed under Pulse2180. Callback allocation probes cover onset, held notes,
NoteOff, release and shutdown with construction/reclamation outside measurement.

Targeted checks passed; exact-head full local CI, focused PR publication and
combined native review build follow. Native UI/listening, physical deadlines,
full Operator parity and general live parameter update behavior remain open.

## 10 October 2026 — review65 native-feedback checkpoint

Focused [PR104](https://github.com/PORTALSURFER/hex/pull/104), sourcef927aea,
passed all16 local checks20261010T110805Z-f279bcc4; combinedcf53fb2 passed all16
checks20261010T110805Z-481ef91b. Both clean/stale=false. Pulse2225 is
ready_for_review; native UI/listening remains open. Four old tail/capacity
fixtures now render genuinely audible outgoing Arrangement notes, preserving
their original two-owner assertions after the exact-cut phantom-tail fix.

Fresh optimized release31.09seconds, strict ad-hoc signature independently
verified: artifacts/hex-lattice-feedback-native-review-20261010-cf53fb2.app.
Signed binarySHA2567483db2bf08a4975f3ce0b23004e3d4cfb0fc3b75a69f977704849d4814d3cd8.
Adjacent manifest records67 integrated sourcePRlinks38–104. This app enables
native feedback controls and the bounded16-voice runtime. Check feedback while
stopped, incoming-FM availability/reactivation, MIDI playback and save/reopen.
Source removal retains already-playing authored gates/releases while removed
later notes do not retrigger. Active class switching remains a conservative
refusal with canonical-versus-applied state visible under Pulse2180.

No agent app launch or automatic merge. Previous apps remain intact. Codex
attachment limit100 prevented PR attachment; verified link retained in Pulse
and this ledger. Vendor future-compatibility warnings remain Pulse2143; no
physical deadline,64-voice support or full Live feedback-law parity claim.

## 10 October 2026 — native-device UI focus

User accepted review65 feedback/playback, modulation availability and save/reopen.
PR104 stays open until an explicit merge decision. Live Operator parameter
updates2229 returned to todo after architecture research only; no source edits.

Pulse2232 is the active focused Utility UI PR: replace the generic wrapped list
with compact Input/Output grouping, visible knobs/switches and a mode-dependent
width/M-S control. Preserve existing canonical gestures, theme/font, fixed rail,
reordering and regular/Return-device behavior. Operator's dedicated oscillator/
envelope/routing panel follows separately under2233. Pulse remains planning
authority; native visual/input acceptance belongs to the user.

Utility visual acceptance now requires Live-style widget shapes, rounded corners
and composition, rendered independently with Hex theme colors. A generic
two-column arrangement is insufficient. The fixed device rail is 228px and
Utility uses a narrow 184px frame; verify reference proportions and all control bounds.

Utility source is frozen for publication validation: shared compact widget shapes,
rounded 184px frame, UI-only percent/side/hz codecs and shared popup behavior.
The mode value uses common pending retirement; switch/ack/undo/redo regression
passes. Targeted UI tests: 368 passed, 2 existing ignored; strict package clippy,
desktop build, fmt and architecture/UI/GPUI guards passed. Full local CI and
fresh signed combined build follow; native visual/input acceptance remains open.

## 10 October 2026 — review66 Utility UI checkpoint

Focused [PR105](https://github.com/PORTALSURFER/hex/pull/105), source4789861,
passed all16 local checks20261010T120917Z-6a782e22. Combinedb0fdd82 passed
all16 checks20261010T120917Z-443d6d1d. Both clean/stale=false. Pulse2232 is
ready_for_review; native visual/input/listening acceptance remains pending.

Fresh optimized release26.65seconds, strict ad-hoc signature independently
verified: artifacts/hex-lattice-utility-ui-review-20261010-b0fdd82.app.
Signed binarySHA256 cd21a38164e8c0f655ca4cb2d086ce1b27b34adb60034b772d838d66f1d58d45.
Adjacent manifest records68 integrated PR links. Utility uses a rounded184px
frame at the fixed228px device rail, compact needle dials/switches/selectors,
UI-only display codecs, vector headphone audition and shared mode popup.
Mode acknowledgement/Undo/Redo retires pending values correctly.

User checklist: compare regular/Return Utility layout; drag and type gain,
width and balance; right-click width to switch mode and Undo/Redo; exercise
polarity/channel/mono/bass/cutoff/audition plus bypass/reorder/remove.
No agent app launch, automatic merge or DSP parity claim. Earlier review apps
remain intact. Attachment limit100 prevented attaching PR105; verified link is
preserved in Pulse and this ledger. Operator panel2233 follows separately;
live Operator parameter work2229 remains todo.

## 10 October 2026 — Utility native visual refinement

User reviewed PR105's review66 build: parameter behavior is retained, but the
panel remains too wide and its controls too tall/thick. Pulse2232 returns to
in_progress. Refine the same PR against the supplied Live Utility screenshot:
narrower Input/Output columns, shorter controls, thin outlines and rounded
frames/buttons/readouts. Preserve Hex colors, lowercase/font rules, canonical
gestures, pending-value retirement, mode popup and regular/Return consistency.
The screenshot's logical scale is inferred; exact visual acceptance remains
with the user. Fresh source checks and a signed combined build follow the
refinement; Operator UI2233 remains a separate subsequent PR.

## 10 October 2026 — review67 Utility proportions checkpoint

User rejected review66's visual sizing; PR105 now uses a 144×184px Utility
frame,14px header/12px actions,64px columns,26px rings and14px rounded
controls/readouts. Explicit header top-corner rounding accounts for GPUI's
rectangular overflow masks. Theme/font/lowercase and canonical editing remain.
The device rail is213px. Regular and Return Utility reuse the same body.

Focused source2bc1d7b and combineda814194 passed all16 localCI checks
(20261010T123840Z-a3aca843 and20261010T123840Z-946d808b), clean/stale=false.
Earlier drop_non_drop test lint failure was fixed without suppressions;
superseded runs remain recorded. Optimized release45.19s, independently verified
strict ad-hoc signature and binarySHA256
c2e9915d23a3533405e855c57bdfac472dd9781dd69ad1f8acec4ca8142aff7a.
App: artifacts/hex-lattice-utility-ui-review-20261010-a814194.app.
Adjacent manifest retains68 integrated PR links. Pulse2232 is ready for review;
compare sizing/corners with Live and exercise gain/width/mode editing. Native
acceptance stays open. No app launch or automatic merge; Operator2233 follows
separately. Previous review apps remain unchanged.

## 10 October 2026 — shared device presentation follow-ups

Pulse2242 centralizes native frame rounding in hex-widgets: every regular
instrument/effect and Return frame inherits the4px library default, compact
headers share the renderer, and numeric readouts share a3px token. Device
composition modules no longer choose per-device outline radii. Preserve the
accepted fixed rail and Utility proportions. Targeted Utility/Return geometry
and parameter tests pass; full local validation and native review remain open.

Pulse2243 fixes selector choices painting behind/clipped by device bodies. Move
them to an anchored foreground overlay, preserving constant field bounds and
choice/history/focus/keyboard contracts. Separate focused PRs and a combined
signed test app follow; Pulse owns authoritative acceptance state.

## 10 October 2026 — review68 shared Lattice UI checkpoint

Utility is the testbed for the shared native Lattice UI, recorded in AGENTS.md
and widget/device guides. Device panels compose shared controls and bind
parameters; reusable appearance and popup behavior live in hex-widgets.
[PR106](https://github.com/PORTALSURFER/hex/pull/106) makes all native frames
inherit4px rounding and numeric readouts3px; compact/standard headers share a
renderer. Code source ddaef18 passed all16 localCI checks
20261010T124950Z-2a446d0e; later b0ebec6 changes documentation only.

[PR107](https://github.com/PORTALSURFER/hex/pull/107) moves shared selector
options to deferred anchored foreground popups. Field width/bounds stay stable;
menus escape clipping, occlude lower controls, flip at edges and scroll long
lists. Choice clicks emit once; Escape/outside/blur/disable dismiss. Bounds
measurement refreshes only when changed. All29 widget tests and strictClippy
pass. Focused741aad4 and combinedf117a83 pass all16 localCI checks
20261010T130201Z-e57b1b12 and20261010T130201Z-fd8ec4b9, clean/stale=false.

Fresh optimized release21.07s and independently verified strict ad-hoc signature:
artifacts/hex-native-device-ui-review-20261010-f117a83.app. BinarySHA256
6af107f2a1ecbf3c2df532c092092bf97c70d48950169f024cda2915541e8ca0.
Manifest retains70 integrated PR links. Pulse2242/2243 are ready for review;
check shared frame corners on Utility, Operator/sine and Returns, then dropdown
choice selection, outside/Escape dismissal and placement. No agent app launch
or automatic merge. Native visual/input acceptance remains open; earlier apps
remain intact. Attachment cap100 prevents attaching PR106/107 in Codex; verified
links are preserved in Pulse and here. Operator composition2233 remains separate.


### 2026-10-10 — dedicated Operator UI on the shared Utility baseline

[PR108](https://github.com/PORTALSURFER/hex/pull/108) adds a580px compact
Operator composition on frozen review68, including Utility105/commonframes106/
foregroundselectors107. Four D/C/B/A oscillator rows stay visible; full-width
ideal cycle, authored envelope and labelled source/target/output FM views expose
all77 registered controls. Shared MiniDial density, bounded curve renderer and
common numeric codecs preserve the Lattice library boundary. Canonical edits,
invalid-draft hiding prevention, selector dismissal and project/device page-state
retirement are source-tested. No DSP/schema or active-retarget behavior changes.

Focused codedca30f8 and combined7a7c7d9 pass all16 localCI checks, clean/stale=false:
20261010T133700Z-9c78f548 and20261010T133701Z-11001199. Targeted35 device-panel
tests and31 widget tests pass; final source review resolves fixed-mode sizing
and routing headers. Fresh optimized release17.78s, strict ad-hoc signature and
independent executableSHA25678ef924db386b0d724a1aacee8e8ff283cecb17acaf74536f92c8df035a0a396
verify. App: artifacts/hex-operator-ui-review-20261010-7a7c7d9.app; siblingmanifest
records source7a7c7d9ef18222cbe092e9232c1ed83396bc8714 and71 integratedPRlinks.
Later documentation-only delivery commits do not alter that artifact's source
identity. Earlier apps remain intact. Native visual/input/listening acceptance
is pending user testing; no app launch or automatic merge.

Pulse2233 owns review. Real draggable envelope breakpoints2247 and automatic
shell-to-display context2248 remain separate todo PRs; current curves are
read-only previews, and full Live DSP/UI parity is not established. Test Utility/
Operator shared styling, oscillator/fixed tuning and dropdowns, envelope/routing
values with drag/type/Undo, device reorder/remove and save/reopen. Codexattachment
cap100 prevents attaching108; verified link is retained in Pulse and here.

### 2026-10-10 — replace an instrument from its card

Pulse2262: browser instruments now replace an occupied instrument only when
dropped directly on that instrument's card. The explicit target retains the
instrument ID and chain position; effects and Arrangement clips remain intact,
and the replacement is one Undo step. Drops on effect cards or elsewhere on an
occupied rail remain refused. Type changes retire old device controls, pending
values and Operator page state before rendering the replacement. Targeted
Session and GPUI replacement tests pass, including wrong/stale targets, the
right edge of the wide Operator card, UI cache retirement and Undo. Full local
CI and native pointer acceptance remain pending.

### 2026-10-10 — Operator shell controls follow oscillator focus

Pulse2248: pressing an Operator shell level, ratio/frequency or fixed-multiplier
control selects its oscillator in the center detail and returns to the oscillator
page. The stable device/parameter control remains active for the same drag; a
focused shell numeric adjustment also follows its oscillator before dispatch.
Only the initiating control is excluded while other drafts are validated, so an
invalid draft continues to block hiding its detail and other devices remain
unchanged. The rendered GPUI regression covers all four rows, popup dismissal,
invalid drafts, one-step drag/Undo, focused Up-key editing, stationary typing and
neighboring Utility state. Focused test passes; full local CI, PR review and
native visual acceptance remain pending. See [Operator shell context contract]
(docs/operator-context.md#shell-to-detail-selection-contract).

## 10 October 2026 — Operator retarget kernel (Pulse2284)

- Added bounded shared ramps for oscillator level, carrier weight and effective
  feedback depth, preserving phase, feedback memory, age and NoteOff.
- Unsupported laws and transitions into/out of the mutable feedback class
  refuse before mutation; active-ramp continuation and exact endpoint handoff
  have focused regression coverage. Exact history copy remains separate.
- Isolated crate validation:106 device tests, formatting and strict all-target
  crate Clippy passed. Primary reviewed the integrated diff; full final-tree
  local CI follows on this focused branch.
- Measured current runtime3,808B/twin; future shared ramp216B projects to
 8,048B/pair within the existing8,192B ceiling. No native retarget wiring yet.
- Original release timing sample and limitations:docs/operator-feedback.md.
  Host integration/admission, callback probes and native listening remain2229.

## 10 October 2026 — Operator shared-ramp helpers (Pulse2285)

- Added state-only fresh-note enrollment at the shared parameter boundary,
  including a subsequent same-offset NoteOff at envelope age zero.
- Exact history copying rebinds configuration identities while preserving the
  ramp, phase, gate, feedback memory and future PCM. Rapid edits and zero-clock
  copies have focused regression coverage.
- Shared-ramp continuation remains defined after every voice expires; the host
  must advance one clock for all voices at closed render boundaries.
- Child validation passed 115 device tests, formatting and strict crate Clippy.
  Final integrated local CI and PR merge evidence are recorded in Pulse.
- This kernel prerequisite does not enable live registered parameter edits;
  runtime admission, callback probes and native listening remain Pulse2229.

## 10 October 2026 — EQ8 spectrum kernel (Pulse2287)

- Added original fixed-window stereo FFT telemetry with reusable storage,
  calibrated Hann normalization and 96 logarithmic bands. Anti-phase channels
  retain their power; silence has a defined floor.
- Four focused kernel tests, strict audio crate Clippy, formatting and
  architecture checks passed in the isolated child. Final integrated local CI
  and focused PR evidence are recorded in Pulse.
- No live analyzer is exposed yet. Capture worker remains Pulse2288; actual
  accepted post-node audio, UI display, callback overhead and native acceptance
  remain Pulse2250. See [spectrum analysis](docs/spectrum-analysis.md).

## 10 October 2026 — bounded spectrum worker (Pulse2288)

- Added fixed capture batches, a 32-batch queue, sequence-gap resets, off-callback
  rolling FFT analysis and a bounded result cache. Ordinary parameter revisions
  retain the actual contiguous output window.
- Review found and fixed excess FFT cadence on context changes, repeated idle
  FFT work, invalid-rate admission and unbounded result draining. Nine focused
  regressions and the zero-total/net capture allocation probe pass.
- Exact final-tree local CI and focused PR evidence are recorded in Pulse.
  Actual accepted post-node capture, retirement/freshness validation, UI trace
  and release callback overhead remain Pulse2250.


## 10 October 2026 — live Operator parameter handoff (Pulse2229)

- A target-bound proof lets supported level/carrier/effective-feedback edits
  preserve held and releasing mutable Operator voices in Arrangement and Session.
  Adoption derives its ramp from the actual audible state, including a running
  ramp, rather than assuming the previous canonical revision reached audio.
- Source, device, owner, topology and both runtime twins are checked before
  mutation. Incompatible active laws refuse; quiescent runtimes can install the
  fresh candidate. Exact-copy and generic detached-tail behavior remain distinct.
- Actual fixed storage is 4,032 bytes per twin / 8,064 bytes per pair on this
  arm64 build, under the unchanged 8,192-byte ceiling. Target proofs are 112 bytes
  and have separate bounded aggregate accounting; polyphony remains 16.
- Targeted lattice/device/engine/export checks and both actual Operator callback
  allocation probes passed (zero total allocations and zero signed-net count and
  bytes). Coverage includes skipped revisions, in-flight rapid edits, new notes,
  partitioned PCM, wrong-target and incompatible-law refusal, and twin atomicity.
- Full final-tree local CI is the PR merge gate. Native listening, physical
  callback deadlines and manual control acceptance remain open in Pulse2229.

## 10 October 2026 — registered Operator feedback profiler (Pulse2289)

- Added a reusable actual `PreparedRevision` + `SessionPlayback` release
  profiler for steady nonzero-feedback playback and supported level/feedback
  history handoffs. Setup, handoff and render samples are reported separately;
  PCM partition equivalence and optional density refusal are checked.
- Debug smoke passed for 1×1 and the requested 100×16 held-note workload; the
  intentional 17-note-per-track negative fixture is refused during revision
  preparation. Strict example Clippy and formatting checks passed.
- All 18 release runs (three interleaved workload rounds) passed partition PCM
  and density checks; raw timings/source/binary/hardware metadata are retained
  in `artifacts/profiles/operator-feedback-2289/`. The 100×16 median render
  exceeds both nominal block budgets; capacity work remains open in Pulse2226
  and sample-free retarget preflight is separate Pulse2291. See
  [performance evidence](docs/performance.md). Whole playback host bytes are
  explicitly unavailable through the public contract.
- Review refinements add a finite/audible partition PCM guard, reject run lengths
  that exceed the fixture's held-note gate, and scope retarget checksums to
  measured blocks. The 512-frame retarget block includes 240 ramp and 272 target
  frames for its single edited track.

## 10 October 2026 — sample-free Operator retarget preflight (Pulse2291)

- Retarget compatibility now uses the renderer's readonly boundary validation
  without evaluating DSP. Existing identity, note/ramp boundary, checked age,
  atomic refusal and both-twin checks remain; runtime storage is unchanged.
- All 117 device tests, strict device Clippy, formatting and both actual Operator
  callback allocation probes pass. New regressions cover fresh/carried/joined/
  rebound/rapid voices and explicit stale-target/expired/overflow refusal.
- All 18 interleaved before/after release runs have identical partition and
  retarget PCM checksums. Single-track/16-note handoff medians improve; dense
  project timings overlap and remain over budget. See
  [performance evidence](docs/performance.md); Pulse2226 stays open.
- Full exact-final-tree local CI and primary diff review are the merge gates.


### 11 october 2026 — pulse2292: bounded eq8 audition dsp prerequisite

- Added transient checked band/lane audition with 128-byte parallel history,
  always-running normal EQ, bounded 256-frame fades and compatible handoff/reset.
- Runtime remains bounded: measured 3632 → 3792 bytes; registered standalone
  counter probes have zero total/net counts and bytes, release evidence is in
  `docs/eq-eight-audition.md` and `docs/performance.md`.
- Kernel tests cover shapes/lanes, output gain, exact normal history after release,
  partition invariance, superseded switches and disable/mode/reset cancellation.
- Parent Pulse2253 still owns the headphone UI, callback lease/cancellation and
  user listening acceptance; this prerequisite does not expose a control.


### 11 october 2026 — pulse2250: integrated eq8 output spectrum

- Added typed owner/device post-node observation, transactional fixed-size capture,
  bounded analysis worker and selected EQ8 spectrum overlay behind the response curve.
- Analyzer target cancellation handles hidden panel/clip tab, focus/type/removal,
  same-ID project reopen and backend reset; ordinary parameter revisions retain the
  rolling window. Queue races/exhaustion and original capture-time staleness have
  targeted regressions.
- Actual publication generation/rate/revision fences cover reused IDs, deferred
  admission and same-project updates; registered callback probes report zero total/net
  allocations. Release capture overhead and its unpaced/backpressure/environment
  limits are recorded in docs/performance.md.
- Native spectrum display/input/listening, paced throughput and physical device
  deadlines remain for user validation; final-source full local CI is the merge gate.


### 11 october 2026 — pulse2294: shared checked operator ramp frame

- Registered Operator shares current/next ramp interpolation once per sample;
  voice identities, feedback boundaries, note age and private DSP state stay checked.
-124device tests and strict crate Clippy pass. Preserved old/new registered release
  fixtures retain exact PCM in18runs;16voice ramp rendering median drops about44–45%.
  Dense handoff/steady costs and performance limitations remain explicit.
- No storage/capacity widening:4032B/twin,8064B/pair,16voices/256events. Full final-tree
  local CI is the merge gate; native listening and quiet/physical deadline gates
  remain separate in Pulse2229/2226/2108.


### 11 october 2026 — pulse2296: compatible eq8 parameter history

- Added default-false Lattice retarget proof; registered EQ8 opts in only for
  matching rate/mode/enabled-bank topology. Ordered identity, matrix/bypass and
  both-twin history preflight remain checked; unchanged graphs skip the new check.
- Arrangement and Return adopt compatible gain/Q/type history once rather than
  accumulating old current-chain tails. Independently owned instrument-release
  suffixes and older carried Return tails stay separate.
- Independent Arrangement PCM and copied-history Return oracles, explicit
  incompatibility/twin tests and real callback zero-total/net allocation probes
  pass; original Utility Return coverage is preserved.
- Original rapid-edit baseline hits ReleaseBankFull; repaired version completes
  six2,000-block stress runs without changing capacity. Matched shorter release
  comparison and environmental limits are in docs/performance.md.
- Full exact final-tree local CI/review are the merge gates; audition lease/UI
  wiring remains separate Pulse2253 and native acceptance remains open.
- Pulse2297's proposed contributor signature is unnecessary: filter residual
  history remains valid across input changes. Cancellation/correction is pending
  live tracker recovery; do not implement that speculative subsystem.

### 11 october 2026 — pulse2253: native eq8 band audition

- Added the shared headphone toggle and physical filter-dot hold/drag audition.
  Arming/keyboard selection never starts audio; inside release fades normally,
  outside release/blur/disarm/lifecycle changes hard-cancel. Transient state is
  not saved and does not create an authored edit when held stationary.
- Separate bounded eight-command lease checks actual publication context, rate
  and revision floor; priority Stop/fault fences remain effective under pressure.
- Compatible Track/Return edits preserve held selection. Mutable release suffixes
  keep ordinary history exclusively; a separate both-twin-certified bridge moves
  only the exact held audition before all retired auditions are cleared.
- Source regressions cover stale same-track tails with ordinary PCM oracle,
  Return gain/Q/type handoffs and actual outside-release geometry. Actual Track
  callback allocation probes have zero total/net counts/bytes; separate Return
  audition callback allocation coverage is not claimed.
- Release measurements at source3e13a01 retain60scenario rows and exact per-block
  PCM oracles. See docs/eq-eight-audition.md and docs/performance.md for scope,
  hashes and timing/environment limits. Full final-tree local CI/review are the
  merge gates; native headphone/pointer/listening acceptance remains open.
- Pulse live access is currently failing. Latest2296completion,2297correction
  and2253review-state updates are queued temporarily, not saved to another tracker.


-11 October2026: Pulse2299 measured and rejected a held-only Operator expiry-scan
  optimization:24 matched old/new runs preserve PCM, but do not show consistent
  speedup. Runtime change stays offmain. Retained source hashes, timing ranges and
  limits are in docs/performance.md; higher-polyphony/native deadline gates remain
  Pulse2226/2108. Next target requires CPU call-tree attribution.


-11 October2026: Pulse2300 adds measured inline hints to existing Operator
  envelope queries, preserving exact arithmetic/PCM,4032B/twin and16voices.
  Three matched rounds improve single-track steady5.7–7.7% and post-retarget
  render20.6–22.1%; dense results/ranges and limitations are in docs/performance.md.
  Dense native deadlines/higher polyphony remain open in Pulse2226/2108.


-11 October2026: Pulse2301 extends the shared EQ8 audition callback fixture to
  Return targets with zero total/net allocations through Begin, compatible edits,
  source removal, release, cancel and shutdown. A second nonneutral band makes
  activation audibly distinguishable; separate Playback PCM oracles validate
  both target kinds through lifecycle transitions. Native testing remains2253.


-11 October2026: Pulse2302 rejects a zero-level Operator envelope-query mask:
  sparse steady improves, but all three single-track live-edit rounds slow down
  (10.6–12.1% sparse,5.6–12.9% all-active). Runtime candidate16a98ae stays offmain.
  Reusable all-active profiler control and36 matched logs expose the tradeoff;
  exact PCM, partition,17-voice refusal and callback allocation guards pass.
  Measurements/hashes/limits are in docs/performance.md;2226/2108 stay open.


-11 October2026: Pulse2316 adds a compact Gate panel using shared Lattice dials:
  threshold/return/floor at left and attack/hold/release at right. The same fixed
  rounded frame is used for Track and Return, with all six values still routed
  through canonical device gestures. Headless geometry/edit/undo and invalid
  threshold validation are in gate_ui_tests.rs; native appearance/pointer
  acceptance and full integrated local CI remain pending review.

## explicit GPUI grid literals — pulse2318

Type the two existing grid-repeat literals as f32, preserving layout while
removing the Rust1.98.1 fallback warning. Vendor baseline identity/license stays
unchanged; regenerated cumulative patch/hash and the narrow guard retain
provenance. Full exact-source CI and release compilation are acceptance gates.
Other dependency warnings remain in Pulse2143; see [native boundary](docs/native-input.md).

## shared knob reset lifecycle — pulse2319

Double-click reset now ends a pending invalid numeric edit even when already at
the default, releases local press/selection state and accepts the next external
update or keyboard step. A lean rendered regression reproduces the stuck edit
before the fix. Valid text's first-click blur commit stays intact. Full exact
local CI and source review are merge gates; native pointer acceptance remains
open. See [shared device controls](docs/device-controls.md). Operator stop2305
remains open; service-level Operator gesture coverage is follow-up2320.

## 11 october 2026 — pulse2321: original free-running Auto Pan

- Registered `hex.auto-pan` version 1 with amount, rate, and right-channel phase
  metadata, opaque validated state, and the generic native effect catalog.
- Added a sine-only stereo amplitude modulator with fixed scalar clock/ramp
  history, exact unity at zero amount, dry bypass with advancing clock, and
  non-finite frame preservation. Compatible same-rate runtime handoffs retain
  phase and unchanged in-flight ramps; edited targets ramp independently over
  five milliseconds.
- Regression coverage includes quarter-cycle stereo behavior, complementary
  pan, zero-offset tremolo, reset/partition behavior, finite-state validation,
  live handoff, generic catalog/project state, save/load, undo/redo, and the
  prepared audio callback allocation probe.
- This is an original implementation using public control vocabulary, not
  measured Live numerical parity. Additional modes and a dedicated compact panel
  remain separate; native UI/listening acceptance is open.
- Pulse2322 owns the dedicated compact panel. Full exact local CI and final diff
  review are merge gates; native listening and appearance remain pending. The
  unreproduced Operator stop in Pulse2305 remains open.

## 11 october 2026 — pulse2324: Operator face-value layout

The fixed-height Operator card now follows the reference's left oscillator,
center detail and right LFO/filter/pitch/global arrangement. Amp envelope is the
default center page. The right-shell controls are real shared widgets with local
view state only; they do not yet alter DSP or saved project state. The colored
global routing icon opens the existing canonical routing page. Ten Operator-
focused UI tests passed, covering compact and wide geometry, local control edits,
owner-switch cancellation, and existing envelope/routing behavior. Full local CI
remains a review gate; native visual acceptance remains with the user. Check
three-column spacing at the fixed rail height, visit amp envelope and routing,
and confirm right-shell controls respond after switching tracks. DSP and
canonical binding for LFO (Pulse2325), filter (Pulse2326), pitch/fine tuning
(Pulse2327) and global controls (Pulse2328) remain separate work. Pulse2305's
reported parameter-triggered playback stop remains unconfirmed and undiagnosed.
The reported B-oscillator mixing behavior is a separate, unconfirmed observation.
See [device controls](docs/device-controls.md).


## 11 october 2026 — pulse2332: Operator reference refinement

Refined colored oscillator badge and fixed switch sizes, shared open-arc dials,
label/value placement, flat center tabs, dark curve background, dB/time readouts,
and right-shell selector versus three-dial row proportions. Smooth envelope
drawing follows existing DSP laws; cached unchanged geometry removes redundant
construction. User clarified slow pointer following; drag Change now updates
retained graph and numeric entities immediately, avoiding a whole-editor notify
per successful movement. Its focused regression checks feedback before another
layout/worker pump while canonical state is still unchanged. Focused Operator
source tests pass. Rectangular value fields now scrub vertically, with actual
horizontal slider tracks preserved; its rendered regression fails before the fix.
Full local CI remains the
merge gate. Native screenshots guided two layout iterations; user drag/listening
acceptance remains Pulse2330. The narrow geometry microbenchmark does not prove
native latency or Live parity. See [comparison and limitations](research/observations/operator-visual-002.md).


## 11 october 2026 — pulse2337: stable Operator time axis and stage caps

Direct user clarification and two supplied Live screenshots establish fixed
stage fractions: attack contributes the first third, decay adds up to another
third after attack, and release is anchored in the final third. There is no
automatic zoom/fit/recenter. An invertible original logarithmic display mapping
keeps short times usable; intermediate Live numerical parity is unverified.
Attack authors0–20s; decay/release1ms–60s in descriptor/graph/current state.
The standalone kernel retains zero-stage mathematics but enforces upper caps.
Version6 retains the320-byte layout and clamps valid legacy1–5 stage values
into the new ranges without notice; malformed old domains/current states refuse.
All163 device tests and six focused UI/geometry tests pass, including
gesture/history/save and stage/migration regressions. Original geometry and
authoring negative controls fail as expected; full exact-final local CI is the
merge gate. Native visual, pointer and
listening acceptance remains with the user, separately from source checks.
See [controls](docs/device-controls.md) and [state](docs/operator-state.md).

## 11 October 2026 — shared value-field pointer (Pulse2340 / PR155)

- Shared numeric readouts retain arrow hover/typing; active value scrubs request
  a cached invisible macOS cursor after the existing drag threshold.
- Frame/window restoration owns no platform hide count. Rendered regressions
  cover outside release, cancellation, button loss, blur, deactivation, disable,
  root replacement and window removal. All39 widget tests pass; the hover test
  fails with the original I-beam behavior. Existing gesture/input tests remain.
- All16 original exact-source local CI checks passed on clean c9219e2. On
  11 October the user reported native arrow/hidden/restored pointer behavior,
  typing and Undo passed and requested merge. Main integration is revalidated
  before merging. The supplied checklist is in docs/device-controls.md.

### canonical capture — pulse2346 / pr159

- Added bounded stable TrackId/ReturnId capture plans and borrowed transactional
  buffers before Main/master, including empty chains and original-ID retained
  tails while preserving old sonic routing and shared Returns.
- Independent PCM/history/partition/refusal fixtures and total/signed-net callback
  allocation probes cover the finite contract in `docs/output-capture.md`.
- Required full exact clean local CI is a merge gate. Native macOS/audio,
  listening and physical throughput remain manual gaps; no export UI/preroll or
  publication is included. Active Pulse30 prerequisite linking remains blocked.

### operator enable controls — pulse2335 / pr164

- Strict version-8 state appends four canonical enable bools after the frozen
  LFO prefix: 380 bytes, IDs513–516, all enabled when migrating versions1–7.
- Five generic continuation-contract tests and three codec tests passed on an
  unchanged source digest; `docs/operator-enable-continuation.md` records the
  exact commit/digest and retained partial-run evidence.
- Active analytic engine integration and provider PCM-oracle tests are prepared
  source, with shared constructor/context and mutable LFO seams still awaiting
  owner handoffs. Actual DSP toggles, canonical badge interaction, history/save,
  callback probes, full local CI and user listening/UI acceptance remain open.
