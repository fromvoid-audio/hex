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
  native-accepted and merged are separate states. No automatic merging.
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
  return routing, sends, history and persistence, not decorative rows. Keep both
  sections usable at small heights and prevent implicit feedback cycles.
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
  Contract: [theme settings](docs/theme-settings.md).
- [ ] **native device model/runtime** — canonical IDs, parameter/state/history
  persistence, one MIDI instrument slot and serial audio effects, with shared
  native/offline behavior. Define mutable DSP lifecycle, prepared graph ownership,
  bounded events, latency and callback retirement before stateful devices.
  Paired stateful effects and Utility v2 now pass affected source/probe checks;
  integrated full CI and native acceptance remain open. Contract:
  [stateful lattice](docs/stateful-lattice.md). Large-chain timing/admission is
  tracked separately as Pulse 2108; instruments remain a separate prerequisite.
- [ ] **bottom device panel** — compact horizontal device chain for the focused
  track, real parameters/bypass/reordering and clip/device view toggles. Build on
  the canonical device model; preserve editor selection and useful track space.
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
implemented and targeted-tested. See [contract](docs/fm4.md). Catalog/runtime
admission, feedback and complete Operator feature/reference acceptance remain
open. Keep this focused PR separate from panel-focus2117 and loop-rail2120.

Pulse2114: schema15 optional instrument slot, generic registry and copied analytic
note samplers are implemented in source; targeted tests/strict checks pass.
Silent-slot MIDI extent/preview repair passes targeted tests; fullCI and focused PR
follow integration.
See [contract](docs/lattice-instruments.md). This is a prerequisite for Operator,
not Operator implementation. Source/native/merge gates remain separate.

Menu follow-up2115: user accepted and manually merged PR37 into the stacked
integration branch fb4ec3a; all14 cleanCI passed e388613. Future picker2116 and
open-panel/focus-switching2117 is implemented in the focused detail-pane branch;
targeted UI, Session keyboard navigation and worker-reconciliation checks pass.
The pane's open state stays independent from user-driven clip/track content focus;
an empty clip context retains its last valid body height. Focused local CI remains
for integration, and native acceptance remains with the user.

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
