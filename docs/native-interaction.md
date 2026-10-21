# native interaction acceptance — Pulse #22

Consolidated 16 October 2026 for [PR244](https://github.com/PORTALSURFER/hex/pull/244).
This is the retained native acceptance ledger, not an implementation of new
controls. Pulse #22 owns results; owning documents retain behavior contracts.
The user confirmed in this chat that there are no additional native results.
No native interaction or listening tests were performed by the agent.

## retained finite acceptance

Do not repeat these checks merely because this ledger was consolidated. Recheck
only behavior affected by a later change, recording that source and reason.

| accepted behavior | evidence and limits |
| --- | --- |
| time-chunk landing preview | 9 October, source `4181d68`, PR9; presented preview looked good and worked as expected; [time selection](time-selection.md) |
| selected-track ripple insertion | 9 October, PR11, source `c7ae2d5`; [request evidence](https://github.com/PORTALSURFER/hex/blob/0c7bb72d1ad8dd136b11adf05759c200797fc962/todo.md) |
| Option-copy, keyboard paste anchoring, stable zoom during growth, Space restart, File menu | 9 October combined editing/menu review, PR12–16; [time selection](time-selection.md), [navigation](arrangement-navigation.md), [request evidence](https://github.com/PORTALSURFER/hex/blob/0c7bb72d1ad8dd136b11adf05759c200797fc962/todo.md) |
| captured time gestures and edge scrolling | 9 October, source `25b797e`, PR17 merged `a250f83`; [time selection](time-selection.md) |
| invisible full-height clip edge resizing | 9 October user report; subsequent bracket cursor appearance/scaling and through-clip grid were still unreviewed; [clip resizing](clip-resizing.md) |
| value-drag cursor appearance/restoration, typing and Undo in PR155 | 11 October finite user report; [native input](native-input.md); the later distinct knob Undo failure #2374 remains open |
| project hex-file save/load, transport start/stop, A–D enable visuals | 11 October user report on latest supplied PR164 candidate `c7e4015`; running binary was not independently rechecked; retained in Pulse #22 |

These are finite reports, not general crash safety, sustained-note listening,
measured paint/input latency, physical callback deadlines or complete Live parity.
Project save/load acceptance does not imply device-preset acceptance.

## artifact identity and availability

Read-only verification on 16 October found the stable app at
`/Users/portalsurfer/dev/hex/artifacts/hex.app`: version `0.1.0`, optimized source
`56a9aa51a26a52180beacc69849ff87c1025cfdb`, candidate PR166 / Pulse #2371 revision27.
Its receipt records release/locked compilation and disk installation on
11 October at 21:41:38 UTC, not a process launch. All six recorded file hashes
match and executable `codesign --verify --strict --ignore-resources` passes.
The bundle is unsigned; the executable is ad-hoc signed. Its bundled full local
CI summary is passed with `stale=false`; no fresh CI or delivery is inferred.

The bundled checklist covers PR166 feedback continuity, not this entire ledger.
This app predates later fixes and must not be used to accept them. Its bundled
`Contents/Resources/test-build.json` and `local-ci-summary.json` retain exact
provenance; rollback is the receipt's retained `.hex-rollback-…app` sibling.
The running process identity is unknown. Nothing was launched, quit or replaced.

For a new candidate use [stable optimized delivery](test-builds.md), with exact
full local CI, included ticket/PR revisions, receipt and finite checklist. Save
and quit normally before replacement; never force-quit. The user must relaunch
to load replacement files. Preserve settings, projects, media and old bundles.

## remaining staged user checklist

Record pass/fail/not tested, exact source/build, and any failure's action/value.
Use an original disposable project; retain user projects and media. Each stage
is independent; known defects and unavailable features do not block unrelated
checks. Do not repeat retained acceptance without a changed-behavior reason.

1. **selection and focus:** on audio/MIDI tracks, check point pulse versus steady
   area, forward/reverse selection and endpoint row focus, then cut/delete/Undo.
   Check upper-third hover/hand and reselection below the handle.
   Check browser text and open MIDI editor shortcuts stay in their own context.
   Clipboard/preview/ripple/captured-drag cases above are already accepted;
   sustained-note boundary listening remains separate. Owner: [time selection](time-selection.md).
2. **source controls and detail values:** select audio then MIDI, close/reopen
   detail and confirm selection stays. Check waveform/ruler/source markers;
   edit supported position, duration, source in/end, play start and loop fields.
   Confirm one Undo/Redo and saved/reopened edited values. Closing MIDI detail
   returns shortcuts to Arrangement; check existing Session source panels.
   This tests changed clip values, not the already accepted basic file actions.
   Owner: [clip view](clip-view.md); do not infer unsupported gain/pitch/warp.
3. **resize appearance:** inspect left/right bracket shape, hotspot and scaling
   on wide/narrow audio and MIDI clips, plus through-clip grid visibility.
   Full-height edge behavior is accepted; only repeat its drag/cancel/Undo tests
   if the candidate changed that behavior. Owners: [clip resizing](clip-resizing.md),
   [native input](native-input.md).
4. **overview and panning:** inspect beginning/middle/end clips and viewport
   outline; zoom/pan and click the map, then add/remove a clip and inspect an
   empty project. With several tracks, middle-drag both axes and release outside
   Arrangement. Verify pointer-anchored wheel zoom and browser wheel isolation;
   test Natural Scrolling on/off only when the user elects to change that setting.
   Owners: [overview](arrangement-overview.md), [navigation](arrangement-navigation.md),
   [native input](native-input.md). Stable zoom on growth is already accepted.
5. **minimum-layout focus:** at 880×540, resize the browser and switch track/clip,
   Arrangement/Session and clip/device contexts. Open detail stays open with
   truthful content; closed detail stays closed; stale controls cannot edit an
   old owner. Check readable fields, titlebar clearance and useful track space.
   Owners: [clip view](clip-view.md), [device controls](device-controls.md).
6. **effects and owner retirement:** on an identified candidate containing the
   corresponding revisions, add Saturator, Compressor, Gate and Auto Pan to a
   Track and Return. Inspect compact controls/previews; drag/type while playing,
   Undo/Redo once, bypass/reorder/remove and save/reopen. Remove a Return with
   an active preview and create a new owner: no retired preview may appear.
   Invalid knob text → Enter → double-click reset must restore default and
   permit fresh typing. Owners: [device controls](device-controls.md),
   [Auto Pan](auto-pan.md), [Saturator](saturator.md),
   [Compressor](compressor.md), [Gate](gate.md). Retained issues:
   #2309/#2312, #2311/#2314, #2313/#2316, #2315, #2319, #2321/#2322.
7. **Operator and EQ8:** retain dedicated #2330 layout review, #2248 oscillator
   shell/fixed-frequency/draft focus checks, #2247 envelope handle/fine/key/
   cancellation/history checks, and #2250 live spectrum silence/bypass/context
   retirement checks. Owners: [device controls](device-controls.md), [EQ8](eq-eight.md).
   Do not infer these results from the accepted A–D toggle appearance.

## existing defects and deferred checks

Pulse readback on 16 October retains the following; source merge is not native
acceptance. Re-read their live records before continuing or recording results.

| issue | current evidence | consequence for this ledger |
| --- | --- | --- |
| #2374 knob Undo/Redo | in_progress, draft PR171, legacy human policy/review hold | reported failure remains open; no passing result inferred |
| #2373 Operator Tab traversal | blocked, PR178 merged; verification/delivery evidence outstanding | retest only on a delivered candidate containing the fix |
| #2375 discoverable device presets | ready_for_review, PR187 open, review hold | separate from accepted project save/load; not accepted here |
| #2372 held-note enable listening | blocked on usable live note input | retain deferred listening; no invented held-note result |
| #2470 shared device layout | in_progress, draft PR205, unpublished panel edits in primary checkout | later layout acceptance must identify that candidate; edits preserved |
| #2611 default-stack analytic handoff overflow | PR243 merged, ticket blocked with required delivery outstanding | existing delivery follow-up, not evidence of an accepted current app |

## recording and completion

For each new report append build/source, included ticket/PR revisions, exact
steps and expected/observed result to Pulse #22. Keep previous approvals intact;
new defects use their own focused linked issues after duplicate checks. Do not
turn unperformed tests into failures or project-wide prerequisites. Keep native
results separate from source tests and optional human testing separate from
implementation delivery policy. This ledger and its documentation PR cannot
claim the full native acceptance task complete while these results are absent.
