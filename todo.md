# hex todo

Updated 9 October 2026. Target: installed Ableton Live 12 Suite 12.4.6,
macOS arm64 first. This is the current request queue; [plan](docs/plan.md)
owns milestone gates and [features](docs/features.md) owns capability assessment.
Completed chronology and superseded work orders are preserved in the
[historical snapshot](docs/history/todo-2026-10-09.md).

## delivery rules

- Each new request gets its own focused PR; do not combine unrelated UI, DSP,
  hosting, or persistence changes. Large requests use several focused PRs.
- Record the PR and actual checks here when a work unit lands. Source-complete,
  native-accepted and merged are separate states. No automatic merging.
- Keep pending acceptance checks. Remove duplicate or superseded tasks from this
  queue, while retaining useful history in the archive and owning documents.
- Preserve unrelated work. Native UI and listening checks belong to the user;
  automated source checks belong to the implementing agent.

## merged PR history and remaining acceptance

PRs #1–#11 were verified closed as merged on 9 October 2026. Merging records
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
| [#11](https://github.com/PORTALSURFER/hex/pull/11) | selected-track ripple insertion | user accepted ripple behavior; merged; keyboard source-track anchoring queued separately |

The selection handle is normally invisible, highlights on hover, and uses a
hand cursor. Dragging below it paints a fresh selection. Point cursors pulse;
nonzero time rectangles remain steady. See [time selection](docs/time-selection.md).

## new requests — separate PRs

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
  behavior on 9 October. Merged. Keyboard source-track anchoring is queued below.
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
  Full local CI passed all 14 steps on clean source `494f641`; native plus-cursor
  and drag acceptance remain open. Fresh app: `artifacts/hex-option-copy-20261009.app`.
- [x] **multi-track keyboard paste stays on copied tracks** — separate focused PR after
  Option-drag. Normal and ripple keyboard paste of multiple tracks use the copied
  source track IDs; only the current horizontal time position determines placement,
  regardless of cursor/selection vertical position. A single-track clipboard uses
  the current destination track, allowing cross-track paste. Preserve gaps and order, refuse
  missing source tracks atomically, and keep visual time-chunk drags free to move
  or copy across tracks. Cover worker clipboard lifecycle, Undo and both shortcut
  variants; provide native acceptance checks. Source implemented; project/session
  tests, targeted strict Clippy, formatting and architecture checks pass. Native
  acceptance and review/merge remain open. [PR #13](https://github.com/PORTALSURFER/hex/pull/13)
  passed all 14 local CI steps on clean source `d1bcf26`.
- [x] **Space restarts from the edit position** — separate focused transport
  fix. Space starts at the current Arrangement edit cursor/selection, or project
  start if neither exists. While playing it may pause; the next Space restarts
  from that editing position, never resumes the paused playhead. Preserve text/note
  editor shortcut focus and bounded ordered seek/start delivery, with Stop still
  effective under queue pressure. Verify start position and stop/restart natively.
  Source implemented with one atomic PlayFrom action; engine/audio/UI and callback
  allocation regressions plus targeted strict Clippy/guards pass. Final local CI
  and native acceptance remain separate gates.
- [x] **preserve zoom when the project grows** — separate focused fix. Moving,
  copying or pasting to the right may expand timeline/project extent, but must
  retain the user's zoom and viewport. Fit/overview navigation remain explicit
  user actions. Check normal/ripple paste, clip/time drags, new imports and Undo;
  preserve valid minimum/maximum coordinate bounds without implicit zoom-out.
  Source implemented; controller worker-event regressions, strict UI Clippy,
  formatting and source guards pass. Final local CI and native acceptance remain
  distinct; [PR #14](https://github.com/PORTALSURFER/hex/pull/14) passed all 14 local
  CI steps on clean source `e04e2ec`.
- [ ] **project actions in macOS File menu** — separate focused UI PR after
  keyboard paste anchoring. Put Open Project, Save and Save As in the screen-top
  native File menu and remove corresponding toolbar actions. Preserve existing
  shortcuts, focus routing, pending-work handling and unsaved-project protection.
  Reuse existing controller commands and OS file dialogs; verify native menu
  operation with the user. This request is for the menu bar, not window buttons.
- [ ] **compact header composition** — separate slim title row with centered
  `hex` or loaded project name; smaller toolbar controls below. Preserve shortcuts,
  title dragging and useful space at minimum window sizes.
- [ ] **custom traffic lights** — replace native macOS controls with smaller
  app controls. Preserve unsaved-close protection, minimize, zoom/fullscreen and
  window lifecycle. Document any required native boundary before implementation.
- [ ] **pinned main track** — always-visible main row glued to the bottom of the
  Arrangement track region. Regular tracks and their trailing empty drop space
  scroll above it; the detail/device panel remains below the track region.
  Preserve master controls, scrolling, clipping, selection and drop coordinates.
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
- [ ] **persistent four-color theme settings** — expose four primary color
  roles in a compact settings menu with simple color pickers and live previews.
  Proposed roles: background, text, primary accent and secondary accent; derive
  surfaces, borders, hover/selection shades from these instead of independent
  editable colors. Persist across reload using the preferences boundary, validate
  malformed settings, retain defaults and offer reset. Keep waveform/clip contrast,
  focus visibility, lowercase labels and bundled typography. Confirm role mapping
  when implementing; current palette has more than four independent constants.
- [ ] **native device model/runtime** — canonical IDs, parameter/state/history
  persistence, one MIDI instrument slot and serial audio effects, with shared
  native/offline behavior. Define mutable DSP lifecycle, prepared graph ownership,
  bounded events, latency and callback retirement before stateful devices.
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
