# hex todo

Updated 9 October 2026. Target: installed Ableton Live 12 Suite 12.4.6,
macOS arm64 first. Pulse project **Hex** (project 2, version 0.1) owns active issue planning.
This file retains request order and repository evidence; [plan](docs/plan.md)
owns milestone gates and [features](docs/features.md) owns capability assessment.
Completed chronology and superseded work orders are preserved in the
[historical snapshot](docs/history/todo-2026-10-09.md).

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
- [ ] **return tracks above main** — add/remove return tracks pinned immediately
  above main, below the scrollable regular-track region. Implement canonical
  return routing, sends, history and persistence, not decorative rows. Pulse2176
  now supplies the model/storage prerequisite; see [return tracks](docs/return-tracks.md).
  Pulse2177 now routes Arrangement, SessionPlayback and prepared export through
  bounded regular-to-Return-to-Main buses. Effectful returns require registered
  zero-latency processing, preserve compatible history, and retire removed buses
  on control; general compensation remains open. Pulse2178 must place return
  rows above Main and prove compact native interaction. Keep both sections
  usable at small heights and prevent implicit feedback cycles.
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
refused routes before audio2177. See [return track contract](docs/return-tracks.md).

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
Full workspace checks, release workload/memory evidence, and native listening
remain open; UI pinned rows remain Pulse2178. See [return tracks](docs/return-tracks.md).
