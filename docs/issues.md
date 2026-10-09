# Implementation issues and decisions

This document records discovered constraints, unresolved behavior and their next
acceptance step. `todo.md` owns task status; `docs/plan.md` owns milestone order.
These entries do not imply that a Live behavior has been confirmed.

| ID | Issue / current decision | Next evidence or implementation step |
| --- | --- | --- |
| project-001 | Default native playback now uses the canonical arrangement; --demo preserves the fixed prototype. | Bounded compilation/publication/reclamation is integrated; native open/load checks passed; seek/split/discard checks pass; continue device stress acceptance. |
| project-002 | Timeline and source offsets use project-rate sample frames. Mixed-rate WAVs convert on the control worker; saved source metadata is distinct from playback frames. No beat-based edits, tempo map or warp. | Device/project rate conversion and musical timing need separate contracts; native mixed-rate import passed. |
| project-003 | Undo uses up to 128 metadata snapshots, capped at 256 tracks, 4096 assets and 16384 clips per project. Large histories may use significant control-side memory. | Measure large edit workloads before replacing snapshots with command deltas; PCM must remain outside history. |
| project-004 | Overlapping clips sum in the offline renderer. Gain is linear 0..2; source regions repeat explicitly. These are Hex contracts. | Run ARRANGE-001, MIX-001 and CLIP-002 against the pinned Live build before claiming exact equivalence. |
| project-005 | Schema-4 native create-new saving (strict schema-1/2/3 migration) and worker loading are integrated. Save-as resolves relative references against the original document base. Post-publication cleanup/directory-sync failures identify the complete published target and keep session state conservatively unsaved. Existing-file updates, asset relinking and crash/forced-termination recovery remain unimplemented. Ordinary window-close/Cmd-Q protection is integrated. | Existing-file updates and relinking need separate safe contracts; do not treat post-publication durability errors as unpublished files. |
| ui-001 | GPUI custom controls still lack complete assistive-technology semantics. Small icon controls require discoverable meaning. | Lowercase tooltips and shortcut hints now; verify keyboard/focus and implement accessible roles/names with framework support. |
| ui-002 | Frame's eight-track/device study exceeds the current four-track/two-scene demo capability. | Native UI must show actual runtime tracks and controls; introduce real features before exposing import, record, devices or save controls. |
| audio-001 | A manual default-output restart path is source-integrated and restores the current graph/cursor stopped; physical failure/reopen remains untested. There is no device-selection workflow. Gain changes have no smoothing and tempo changes pitch. | Native failure/restart and disconnect tests, gain/fade discontinuity contract and DSP implementation; ten-minute stress and listening checks remain open. |
| audio-002 | One nominal callback deadline exceedance was visible during an early short native interaction run; later paused relaunch showed zero. This counter is not a CoreAudio xrun measurement. | Instrument and run reproducible device/UI stress before attributing a cause or claiming deadline safety. |
| ui-003 | Native smoke testing found clipped mixer mute buttons and an awkward wrapped collapsed-browser label; these were repaired. Final interaction/resizing automation was interrupted repeatedly after rebuild. | Recheck gain/mute, expanded/collapsed browser and 960×600 behavior on the final bundle; do not claim those final checks passed. |
| performance-001 | The fixed four-track demo does not establish 100-track or large-device performance. Span rendering now scans clips once per block; metadata history still copies snapshots. | Profile 100 tracks / 10,000 clips with live edits; prepare active-clip scheduling and measure history/UI memory and latency before adoption. |
| latency-001 | Compensation aligns paths but cannot erase intrinsic lookahead, interface latency or unknown live input. Immediate exact output through every arbitrary plugin is not a physically valid guarantee. | Implement prepared transport and explicit monitoring/startup policies with impulse, event-timing and physical loopback tests; see latency.md. |
| performance-002 | No multicore render graph, SIMD comparison or representative x86-64 stress results exist. Long serial chains limit useful parallelism. | Establish graph ownership/scheduling first, then benchmark bounded workers and safe runtime-dispatched DSP against portable baselines. |
| media-001 | Import accepts resident mono/stereo PCM16/24/32 or float32 WAV with versioned windowed-sinc source-rate conversion. Input is capped at 64 MiB / 8 million source frames per asset; resident playback is separately capped at 8 million frames. | Native mixed-rate acceptance passed; streaming and missing-media/relink UI; source-content hashing remains absent. |
| storage-001 | Schema 4 strictly migrates schema 1/2/3 and refuses unknown fields/schema/converter versions and non-UTF-8 paths. save_new never overwrites; interrupted preparation can leave a hidden .hex-pending file. Post-publication cleanup/sync failures now report the committed path distinctly. | Next, add bounded exact-byte FileToken/load_with_token/check_unchanged for external-edit conflict detection. Replacement saving/UI, backup ownership/retention, recovery/relink and hidden-temporary recovery remain deferred. |
| export-001 | Native export captures a current compiled revision and publishes linear PCM16 WAV without overwrite; a fresh bundle matched the independent saved render byte-for-byte. | No progress/cancellation, user ranges, stems, overwrite, alternate formats, listening acceptance or sustained throughput measurement. |
| audio-003 | Prepared handoff is integrated with CPAL. A device fault may stop callbacks before retirement acknowledgment; explicit CoreAudio pause failure does not prove callback quiescence through the pinned safe backend API. | One-stream lease and off-RT stop/disposal worker now retain/retry on stop failure; tests cover lease and disposal behavior. Physical disconnect/pause failure and native canonical editing acceptance remain open. |
| research-001 | No Live function or golden reference render is verified. No dedicated decompiler was found in PATH or the checked application names on 8 October. | Select an arm64-capable tool and pin it before targeted analysis. Start with user-owned timing/loop/overlap fixtures; decompile only a question tied to an ambiguous requirement. |

## Decisions on 8 October 2026

- Stable IDs never rewind through undo/redo or branch edits. Invalid commands do
  not mutate project state, consume visible IDs, or destroy a redo branch.
- Referenced assets cannot be removed. Track deletion removes its clips while
  retaining media metadata, and undo restores the complete track content.
- Compilation refuses missing or mismatched decoded media; it never substitutes
  silence for unresolved assets. No PCM is copied into edit history.
- Application controls should favor small icons with tooltips. Live's composition,
  compact density and utilitarian hierarchy guide the native UI; Yield supplies
  the palette and typography. Decorative feature placeholders are excluded.

- Native canonical review found virtualized rows measured only their header width,
  collapsing the timeline and hiding waveforms. Explicit full-width list/row
  constraints were added and rebuilt waveform inspection passed. Native
  import/edit/play/stop/save-new otherwise passed short interaction checks.

- Source integration caught a let-chain newer than declared Rust 1.85. Keep
  compatibility-aware syntax and inherited rust-version across every crate.
  Current checks run on the installed toolchain; a complete minimum-version
  dependency build has not been verified.

- Source-rate conversion persists algorithm version 1 and original rate/count.
  Reload checks source shape before conversion; unchanged rate/count cannot detect
  changed sample content. Future hashing/relink policy must address that separately.
  Saved project-rate/device-rate mismatch remains an explicit open failure.

- Mute/gain/solo publication currently changes compiled mixing at block boundaries
  without de-click smoothing. Solo is precomputed for straight tracks; muted soloed
  tracks stay silent under an explicit Hex policy. Routing-aware solo, cueing and
  reference mute precedence are deferred; add signal-transition fixtures before
  implementing smoothing rather than promising inaudible changes.

## Native viewport acceptance findings — 8 October 2026

The rebuilt bundle can coexist with an older running process. A bundle rebuild
does not update that process; verify the process start time and visible new
controls after a native quit/relaunch. That mismatch is resolved. Current native
inspection confirms keyboard selection/solo and close discard, but catches
collapsed track names and viewport actions with no visible zoom. Track names
now receive an explicit bounded width; acceptance and action routing review are
in progress. Keep native seek/split acceptance open.

Follow-up native acceptance passed after explicit header width, direct viewport
button handlers and unique tooltip-based widget IDs (pan arrows previously reused
clip-nudge glyph IDs). Button zoom changes ruler/clip geometry; Alt-Right pans
cropped waveforms and Cmd-0 fits. Cmd-equal via native automation still has no
visible effect; retain this specific shortcut uncertainty. Ruler click positioned
the cursor at frame 88127 (1.998 s); Cmd-E split the selected one-shot clip there.
Saved artifacts/native-viewport-solo-check.hex is schema 3 with solo and exact
source partitions. Its CLI render is byte-exact to the independently constructed
PCM16 source concatenation across all four first-track clips. This is numerical
continuity evidence, not listening or Live parity.

Canonical loop region and schema 4 are now integrated. Native set/toggle/undo,
wrap and save/reopen pass; Command-Plus/Minus zoom aliases pass on the current
layout. Tiny-loop repeated clip scans were measured and repaired using stateless
period copies in the caller buffer. Do not extend this optimization to stateful
devices without an explicit lifecycle/event contract. Loop boundary fades remain
absent. Linear CLI export does not repeat the global loop.

Native Open replacement confirmation now says “replace project · open again to
confirm”; previously it said the project had edits even after a successful save.
This is a wording correction; the existing replacement admission gate remains.

## Existing-project save prerequisites — 8 October 2026

Opaque exact-byte tokens now detect external edits without modifying files. A
byte-identical inode replacement is deliberately accepted; a formatting-only
change conflicts. Tokens preserve legacy source bytes rather than current-schema
re-encoding and are capped at 8 MiB. This is not an atomic compare-and-swap: a
noncooperating writer can race between comparison and future replacement. Before
enabling Save Current, define persistent copied-backup ownership and finite
retention, refuse unrelated sidecar collisions, and distinguish postcommit
warnings while adopting the newly published content token. No replacement API
or native Save Current workflow is enabled yet.

## Dense native Arrangement regression — 8 October 2026

Original muted fixture artifacts/native-100-track-check.hex contains 100 tracks,
10,000 clips and one shared 176,400-frame WAV, with no global loop. During native
Open/inspection, process 79581 reached about 98.6% CPU and computer-use calls took
64/137 seconds or timed out. A one-second sample at 13:22 localized the main
thread to GPUI drawing/painting and bounds-tree work; it did not implicate callback
DSP. At fit zoom, existing width thresholds already omit waveform bars, so a
waveform-bar overload is not established. This is a real acceptance failure under
investigation, not a passed 100-track responsiveness gate. The test instance was
closed through its native close button; no shell process termination was used.

Recovery storage API now retains one target-bound validated prior-generation
sidecar and provides load_recovery. Review repaired two permission issues: copied
recovery data inherits the document's restrictive basic mode, and Unix staging
starts at 0600 with inherited permissions applied before any bytes are written.
Targets whose basic permissions change during staging are refused. Exact token,
prior-byte recovery, bounded one-generation roll, collisions, symlinks and
injected commit failures pass; nonportable ACL/xattr preservation is unsupported.
The API is not wired to native Save Current yet. A failed replacement after backup
publication can leave a duplicate of the current document in the rolling sidecar;
no older generation is retained by this single-backup policy.

Read-only triage counted 474/565 main-thread samples through UniformList::paint.
Pinned GPUI virtualizes rows; source still unconditionally invalidates the editor
every 16 ms even while stopped. Removing idle invalidation is the first evidenced
repair. The sampled stack does not independently establish the loaded project;
the fresh-build repeat must visibly confirm the 100-track fixture before claiming
its acceptance. Keep worker polling and transport/acknowledgment updates active.

## Native traffic-light overlap — 8 October 2026

Native acceptance isolation: the main development app now contains five tracks
and unsaved user edits (revision 23 at the last observation). Do not replace its
project or restart it for checks. A separate `hex-acceptance.app` was created with
a distinct bundle identifier. Its import attempt was interrupted by native user
control; no imported project or drag acceptance was established. Only that empty
test instance was closed. Source work can continue independently of this gate.

User reported the transport beginning beneath the native macOS window controls.
The transparent titlebar overlays those controls on the first content row. Source
now reserves 80 logical pixels through one shared theme constant in both editor
and demo, while other panel padding remains compact. UI check/style guard and the
initial rebuilt bundle pass. A fresh native view subsequently confirmed that
top-bar content begins after the reserved area. Standard OS close, minimize and
fullscreen controls are retained; custom traffic lights are unnecessary for this
layout. Their interaction and fullscreen sizing remain unverified because the
OS screen-sharing pill obscured the controls during inspection. Preserve the
user's running instance and unsaved edits rather than forcing a restart.

## Mixer integration and neutral render path — 8 October 2026

First integrated schema-5 run exposed a session test hard-coding current schema 4.
The expected current version was updated to 5; strict legacy migration tests remain
separate. Review also removed repeated balance multiplication and a unity-master
pass from the callback's clip-span hot path by preparing combined channel gains
and skipping the neutral final pass. Paired loop microbenchmarks have mixed median
results; no general speedup is claimed. The prior native schema-4 render remains
byte-identical. Stereo balance is an explicit original attenuating Hex law; Live's
numeric coefficients have not been measured. Smoothing, split stereo pan, device
routing and native mixer/listening acceptance remain open.

## Output recovery bootstrap constraint — 8 October 2026

Recovery source implementation opens the default output on the lifecycle worker,
refuses a sample-rate mismatch before stream creation, retains one open reservation
through result consumption/late retirement, and restores the current compiled
graph plus stopped cursor without editing project metadata. A generation guards
stale UI events; Play/seek/split wait until the restored graph and new seek are
acknowledged. The 187-test integrated run, strict workspace Clippy, formatting,
guards and rebuilt bundle pass. Physical device failure/reopen, mismatch interaction
and listening acceptance are not established; the older release app remained
active during the attempted native check. The later canonical startup bootstrap now opens through the bounded lifecycle
worker and binds the service to the actual rate; demo startup remains synchronous.
189 integrated tests and source checks pass. A fresh native app reached the empty
editor at 44,100 Hz with the titlebar reservation visible. Startup timing, closing
during pending open, physical recovery and normal native-control interaction
remain unverified.

## Toolbar action routing — 8 October 2026

Native inspection found hover feedback but no add-track edit when an icon used
App::dispatch_action, which routes through the globally active window. Canonical
action icons now use their listener Window::dispatch_action. Strict UI Clippy,
formatting and the rebuilt bundle pass. Fresh native add-track advanced revision
0 to 1; balance right advanced to 2, main gain to 3, and toolbar undo restored
main unity at revision 4 while preserving balance r10. A default-output restart
at revision 1 retained the track/unsaved state and reported audio ready · stopped.
These checks used empty audio content; physical device failure, waveform playback
and listening after recovery remain open.

## Dense Arrangement investigation follow-up — 8 October 2026

Read-only source tracing confirms finite-height uniform-list virtualization. At
full fit, the 100 clips per fixture track are 1% width and skip waveform bars.
Visible clips still allocate interactive elements on redraw and GPUI measurement;
canonical clip vectors preserve insertion order, so binary-search time culling
would be incorrect without a separate maintained index. No root cause or speedup
is established. Native control interrupted the fresh replacement attempt; keep
the existing project intact and the confirmed-loaded profile gate open.

## Fade-controller static analysis uncertainty — 8 October 2026

Ghidra headless import/decompile now works on the unchanged arm64 reference. A
named fade-controller callback branches to a target whose inferred body includes
shared discontiguous flows. PIC rewriting, unreachable-block and overlapping-global
warnings make controller units and function boundaries uncertain. No audio fade
behavior is confirmed and no Hex implementation changed from this output. Use
FADE-CONTROLLER-001 provenance and an original rendered fixture before drawing
algorithmic conclusions. Proprietary input/database/pseudocode stay private.

## Clip-drag integration findings — 8 October 2026

Review repaired an accumulating geometry cache, stale render-time drag admission,
selection losing its canonical track after cross-track movement, and the Stop
button sharing Escape cancellation behavior. The final source prunes cache IDs,
checks state at gesture construction and drop, follows canonical ownership after
move/undo/redo, and keeps explicit Stop unconditional. Tests cover ID collisions
across replaced projects, stale revisions/viewports, malformed geometry, no-ops,
metadata continuity and history. 196 integrated tests and source/build checks pass.
Native gestures and resize acceptance remain unverified; snap, trim handles and
autoscroll remain future work.

## Clip fade integration — 8 October 2026

Fade-out evaluation now divides the remaining frame count directly; subtracting
a large ratio from unity would round near-end gains prematurely to zero for
large valid domains. A penultimate-sample regression covers this case. Invalid
domain/segment bounds fail transactionally. Split envelopes retain phase while
trim explicitly reanchors; this policy and one-frame endpoints are original Hex
contracts, not deductions from the approximate reference controller decompile.

The initial inline compiled metadata enlarged every clip and neutral profiles
showed increased medians. Dispatch now occurs once per active clip; only
non-neutral fades use a control-allocated box. The observed neutral medians
improved, with PCM digests unchanged. Residual differences and noisy maxima are
recorded in `performance.md`; sustained device and dense native acceptance stay
open. GUI envelope sampling includes exact ramp breakpoints so a short fade
is not misleadingly stretched across a long clip.

Acceptance repaired breakpoint translation on inherited splits: original-domain
positions must subtract the retained offset once, rather than adding it first.
Full-view and cropped-view tests now cover the exact local transition frames.
Integration also updated the session save test's current-schema assertion from
5 to 6 while retaining explicit legacy fixtures. The final 212-test workspace
run, strict Clippy, guards/fmt and rebuilt development bundle pass.

An isolated muted native fixture verified inherited readouts, the 100→105 ms
edit, saved fade metadata and undo/redo. Redo conservatively marks the project
dirty even when its content matches a previous save; close protection correctly
prompted. The prompt was canceled, the test was saved again, and the isolated
instance then quit cleanly. Content-aware dirty equivalence is a future usability
refinement, not evidence of lost data. Listening, minimum-window/dense interaction
and measured Live fade curves remain unverified.
## Source phase and pointer trim integration — 8 October 2026

Canonical loop source phase, bounded edge trims, loop splitting and schema-7
migrations pass source tests. The independent saved CLI fixture verifies original
three-sample PCM through left/right extension, recursive splits and undo/redo.
The legacy neutral export is byte-identical. Short paired neutral profiles have
higher medians in two cases and substantial tail noise; compiled metadata stride
and nonzero-phase costs need controlled follow-up before performance acceptance.

Native testing used a separate `org.portalsurfer.hex.trimcheck` bundle and an
original muted fixture. The existing acceptance app now contains user edits;
it was not closed, saved or replaced. A right-edge drag committed exactly one
edit and saved valid schema-7 metadata, but lost the first part of pointer motion.
GPUI 0.2.2 passes its threshold-crossing MouseMove position to the on_drag
constructor, rather than the initial MouseDown position. Trim and move must
capture the original pointer with a lifetime that survives redraws. The repair stores one keyed pointer-down record on the view so redraws cannot
lose it. Fresh native checks confirm full displacement, both trims, source-phase
persistence, undo/redo, cross-track movement and off-track cancellation. Escape
during a held gesture, resized/cropped handles and dense interaction remain open.

## Compiled metadata performance follow-up — 8 October 2026

Optional phase/fade metadata restores a 72-byte common compiled stride instead
of the reconstructed prior 80-byte field layout. Four alternating paired short
profiles favor neutral/phase-only clips, with identical digests; fade + phase
results are slightly higher. Keep this tradeoff visible rather than claiming a
universal speedup. Source regressions, 228 workspace tests and strict checks pass.
Sustained device and dense native behavior remain unverified. The running
user-modified acceptance app was preserved; only the non-running main bundle
was rebuilt.

## Instrument compilation admission — 8 October 2026

The existing native MIDI guard prevented an unsupported instrument edit from
committing history. Once synthesis exists, note density, release-tail polyphony,
Nyquist and tempo quantization can still invalidate an otherwise canonical edit.
Replacing that guard with unconditional admission would leave the editor ahead
of its playable graph. Validate candidate compilation before committing history,
retain revision/dirty/redo on refusal, and test failed note and tempo edits.
Original analytic synthesis chases absolute-frame notes at seek and repeats the
same global-loop region; future stateful devices need their own lifecycle contract.

## Native instrument presentation — 8 October 2026

Original instrument audio now compiles, plays and exports through the shared
engine, with candidate edits validated before history commits. An isolated native
MIDI-only fixture loads, publishes revision1 and starts/stops its transport;
native export equals CLI bytes. The Arrangement lane still draws only audio
clips, so the instrument track appears blank. Next UI work must draw musical
clips and notes and expose compact canonical authoring/selection/undo controls.
Do not count native Open/Play as a completed MIDI editing workflow. Listening,
local MIDI loops, expression/recording and general device hosting remain open.

## Instrument interval pruning repair — 8 October 2026

Acceptance review found that a query ending before a median node's onset pruned
the entire subtree, including earlier active notes on the left. Native/CLI byte
equality alone failed to reveal their common omission: first two of the four
fixture notes were silent. Fix traversal so maximum-end bounds prune expired
subtrees, while a future median only excludes itself/right. Add independent
isolated-note comparisons across early, middle and late blocks before accepting
new exports. Existing initial synth fixture exports are diagnostic evidence,
not accepted four-note content until regenerated after the repair.

Pruning repair result: four-note combined/isolated render comparisons pass at
128/512/43/257-frame partitions, with gap and later-seek checks. CLI independently
checks nonzero content for all four onsets. Rebuilt native export matches repaired
CLI bytes and all four note windows peak at206 PCM16 units; previous two-window
silence is gone. Repaired CPU results replace the invalid initial benchmark in
performance.md. Listening and sustained device stress remain open.

## Native MIDI editor acceptance repairs — 8 October 2026

Native inspection revealed keys stretched to the body height while the note grid
used 148px. Keys, grid and note heights now share the same body dimensions; clicking
the labeled60 row persisted pitch 60. Fixed octave labels to signed Live convention
and abbreviated the selected-note readout so velocity stays visible. Native
parameter/history/clip save/reopen workflow passed; dense view currently caps 512
notes and explicitly reports truncation, rather than claiming complete dense
inspection. Note drag/resize, rapid pending gestures, minimum-window inspection,
expression and local musical loops remain required follow-up work.

## MIDI gesture acceptance findings — active repair

Source review found gesture admission rejected positive sub-quarter gates even
though canonical metadata permits them. Motion resolution could also snap an
off-grid onset on a click with no meaningful displacement. Preserve original
metadata for no motion and allow short-note body movement; resize needs a defined
minimum for short clips. Cropped note geometry must expose only true visible
edges, with bounded handle width. Off-roll/nonfinite release cancels rather than
committing a clamped accident. These findings are being repaired before native
acceptance; passing initial geometry tests did not cover them.


Gesture repair result: short/off-grid no-op, clipped real-edge eligibility, bounded
6px handles and finite inside release checks pass nine regression tests. Native
inspection additionally caught .relative() overriding .absolute() on notes; remove
the override so drawing and hit geometry share pitch rows. Rebuilt native body/
both-edge/off-roll/undo/save checks pass, including exact short-note gate/offset
retention. Held-drag Escape, dense rendering and minimum-window checks remain open.

## Dense MIDI reachability — active

The interim 512-note draw/hit cap omits valid visible notes. Canonical overlaps
also hide lower notes under a topmost rectangle. Remove silent reachability loss
through complete cached Canvas geometry, deterministic overlap selection and
zoom/navigation. Avoid repeated full-clip scans and per-note layout/event geometry
clones on every pointer frame. Work stays in hex-ui; rendering performance still
needs measurement rather than a claim based on bounded 65k work.

Dense-navigation review also found new MIDI clips used their absolute arrangement
start for the clip-local piano view. At late cursor positions this could place
the whole empty clip outside the view. New-clip creation must initialize local
start at zero and selection/fit must clamp the view to its owning clip. Repair
is part of this active unit; test a clip with nonzero arrangement placement.


## Dense MIDI Canvas native acceptance regression — 8 October 2026

The isolated `hex-midi-dense-check.app` loads the schema-9 600-note fixture,
selects its clip and renders the new zoom/fit controls and pitch keys, but the
Canvas body is blank: neither grid nor notes appear. Geometry tests and desktop
build success therefore do not establish native painting. The UI executor is
repairing the Canvas layout/paint integration before this unit can be accepted.
No user project was replaced; the isolated fixture remains clean and stopped.
The first inspection accidentally launched the copied older `hex` executable
while the new binary was named `hex-desktop`; after closing that clean process,
the bundle executable was corrected and the blank Canvas was reproduced.

Repair confirmation: the Canvas lacked intrinsic flex sizing. Positioning it
absolute with inset zero inside the sized relative roll restores native grid and
all 600 visible notes. Fresh selection of note 553 and velocity edit/save passes;
independent complete project comparison confirms exactly that edit. Native zoom
in redraw also passes. Overlap and minimum-window acceptance remain open.


## Minimum-width transport output status clipping — resolved 8 October 2026

Native resize to 880 × 540 preserves the compact piano-roll controls, readout,
24 rows, fit and pan. The top transport output string is cut off at the right
window edge rather than fitting its allotted width. Constrain the flexible status
region and retain readable core controls; verify at the minimum width and the
default 1240 × 760 size. No device or audio behavior change is required.

Acceptance isolation update: the dense-check app received user changes and has
unsaved revision 9. Preserve that instance and its bundle; overlap acceptance used
a new `hex-midi-overlap-check.app` with its own identifier and saved fixture.

A first attempt to add flex sizing to the output label passed compilation but
did not establish correct native ellipsis at 880 pixels; the source change was
reverted. Inspect GPUI text/layout bounds before another repair. The overlap
acceptance bundle contains that diagnostic build; canonical source remains the
previous transport layout. Initial background launch briefly showed a black
window until activation; startup paint behavior needs separate reproduction.

Accepted repair: GPUI 0.2.2 no-wrap truncation reuses its measured text width
within layout passes. A flexible status region with a one-line clamp retains
definite width for reshaping and correctly ellipsizes at default/minimum sizes.
A full-label tooltip preserves readable device metadata. Fresh isolated native
checks confirm ellipsis at 1240 and 880 logical pixels and the complete tooltip.
MiniTooltip now accepts SharedString; existing static descriptions convert at
their call sites. This stays within hex-ui and changes no audio contracts.

Native acceptance observation: on the latest isolated app, Open completed but
the inactive window did not visibly repaint until a body click activated it.
The same activation also resolves the initial background-launch black view.
This is an observation, not an established GPUI regression: distinguish OS
background-window capture behavior from missing invalidation with a focused
foreground startup/Open reproduction before changing lifecycle behavior.

## MIDI grid native automation interruption — 8 October 2026

The isolated grid-check app successfully opened
`artifacts/native-midi-grid-start-20261008.hex`; its screenshot shows the two
expected clips and `project opened`, revision 1. Subsequent coordinate clicks
fail with native automation `noWindowsAvailable` or `windowNotFoundAtPosition`,
although refreshed AX exposes the editor window and read-only process inspection
confirms the same process remains alive. Rebinding and the exposed Raise/window
actions did not restore coordinate input. This is an acceptance-tool limitation,
not evidence of a Hex crash or a passing grid interaction. Preserve the running
fixture and other dirty native projects. Grid author/move/resize/save acceptance
remains open; source implementation can progress independently.

Follow-up: invoking the exposed native fullscreen control works and reveals the
selected empty piano roll. Before another controlled pointer action, automation
reported user changes; a fresh screenshot now shows unsaved revision 11 with
authored notes. Preserve this grid-check instance and bundle as user work. The
coordinate failure is not yet explained, and this modified project is unsuitable
for controlled saved-value comparison. Fullscreen activation is verified; a
normal enter/exit/resize cycle and modifier editing still need isolated acceptance.

Fresh snap-check bundle restores coordinate input and completes controlled grid
authoring and compact snapped/free gesture/save checks; saved fixtures are listed
in todo.md. The prior coordinate failure's cause remains unknown. Held-modifier
dragging is still unavailable in the documented automation API. A later cleanup
observation unexpectedly showed an empty revision-0 workspace instead of the
saved revision-12 fixture; only the snap-check process was present in read-only
process inspection. Do not infer a Hex reset/crash or close a potentially user-
changed window from that observation. Saved evidence files remain independently
verified; app/process identity and user activity need fresh observation before
further native mutation.

## MIDI selection/draw and Windows modifier gaps — 8 October 2026

The Live 12 public manual distinguishes normal selection from draw mode. Hex's
current single empty click inserts a note, so adding marquee selection requires
an explicit normal/draw mode contract rather than silently keeping that behavior
in normal mode. This belongs to the next multi-selection UI unit. The manual also
uses Alt on Windows for snap inversion, whereas current GPUI platform-modifier
wiring uses Control there. macOS Cmd insertion is native-accepted; Windows input
parity is not claimed and needs its own platform mapping/acceptance work.

## Large-selection removal complexity — resolved 8 October 2026

Acceptance review of the new batch removal command found repeated vector removal,
which shifts remaining notes for each selected ID and can become quadratic for
large alternating selections. Replaced it with validation/index sorting followed
by one stable retain pass. Tests preserve exact survivor ordering and complete
undo/redo snapshots. No timing claim is inferred from this complexity correction;
dense group-edit interaction/compile measurements remain required.

## Selection acceptance capture and deferred checks — 8 October 2026

The isolated selection-check native bundle accepts Shift selection, marquee,
select-all/Escape, normal/draw authoring and group movement/deletion. Exact saved
project comparisons establish preserved expression/spacing/untouched metadata.
The capture returned a 2480×1522 canvas with visible app content in its upper-left
1240-pixel region and white outside it; pointer coordinates still mapped at twice
the visible image coordinates. The attempted corner resize did not change the
observed layout, so minimum sizing is unaccepted for this milestone. Do not infer
a rendering defect or change display settings from this alone. Native additive
marquee, group redo/no-op/off-roll and piano-focused arrow routing remain next
acceptance work. Continuous drawing and group edges/parameters remain separate
implementation units.

Selection follow-up narrows the remaining gates: exact native focused arrow,
group no-op/off-roll and deletion undo/redo checks now pass. Shift-marquee cannot
be exercised with the documented atomic drag API (no modifier parameter), and
minimum-window capture remains unresolved. These do not prevent source progress
or imply whole-application parity.

Fresh group-keyboard-check capture returned consistent full-size pixels; resizing
to 880×540 succeeded, with seven selected notes and compact controls readable.
This closes the minimum-layout gate for current selection/group keys, without
establishing the root cause of the earlier half-canvas capture. Group-keyboard
no-op at clip end also preserves native redo and exact saved state.

## Continuous drawing requires mixed edit semantics — 8 October 2026

The public Live 12 manual (§10.4.1) describes melodic note erasure and pitch-locked
backtracking, with temporary Option inversion. Atomic insertion alone cannot
represent a complete draw stroke over existing notes. AddMidiNotes remains the
current validated insertion prerequisite; the following draw unit needs atomic
insertion/removal, deterministic sparse-event interpolation and retracing rules,
pitch-lock preference/modifier state, stale/off-roll/Escape cancellation and
bounded local preview. Do not implement a stroke as multiple controller edits or
claim insertion-only strokes establish reference parity. Installed build remains
12.4.6 (2026-09-10_0de5c8fa9a); no new executable analysis was needed for this
publicly documented behavior.

Mixed drawing transaction prerequisite is now source-accepted: EditMidiNotes
validates removal/insertion together, preserves stable order and one-step history,
and rolls back removals on candidate compile rejection. This resolves the model
dependency recorded above. Pointer interpolation, erasure/overlap semantics,
pitch-lock/backtracking/Option inversion and native draw acceptance remain open.

## Session route visual acceptance — 9 October 2026

Root independently passes the first
revision-matched UI implementation's 157 tests (one ignored), but review found
that a cell's active-owner branch hides a pending retrigger of that same clip.
Repair is active: derive current and pending markers independently from accepted
route telemetry, preserving both same-clip retrigger and different-cell queued
launch information. Add semantic marker tests for pending/apply/Cancel and
nonplaying ownership before source acceptance. The track header already reflects
pending state; this is a cell-level visibility gap, not an engine scheduling bug.

Acceptance follow-up: current and pending markers are now derived independently
in the focused read-only `editor/session_route_view.rs` module. Regression covers
same-cell retrigger, current A/pending B, apply, Cancel and nonplaying ownership.
Root passes 158 UI tests (one ignored), strict UI/desktop Clippy, desktop build,
formatting and both guards (`artifacts/session-route-view-final-root-*.log`).
The renderer uses checked route slices; channel replacement clears cached data
even if its first read fails. Native appearance and physical interaction remain
unverified because the Mac is locked; the active bundle was not overwritten.

Quantized controls follow-up: the toolbar now exposes existing Immediate and
fixed four-quarter scheduling plus typed pending cancellation. Root passes 159
UI tests (one ignored), strict UI/desktop Clippy, desktop build and guards;
logs are artifacts/session-quantization-root-{tests,clippy,build}.log. Selection
is UI-local and sampled at admission, not saved or meter-aware. A separate
`artifacts/hex-session-preview-20261009.app` was assembled with current accepted
source, including close-handler hardening; executable identity is in
artifacts/session-preview-20261009-manifest.json. Native visual/listening/Discard
acceptance remains pending, and the running development app was preserved.

## Session deletion/focus targeting — 9 October 2026

Source inspection found DeleteClip requires a selected clip ID even in Session
mode, while a stop cell intentionally selects no clip. Stop cells therefore
cannot be cleared by that key path. Grid/header pointer selection also lacks
explicit editor focus after the shared piano roll; scene-header selection can
retain the previous cell context. Source repair is accepted: explicit focus/context and
fresh canonical deletion targets, occupied-cell ClearSessionCell, empty no-op,
scene-only RemoveScene, and preserved selection on admission refusal. Existing
piano note deletion and Arrangement behavior must remain intact. Native outcomes
are not observed; do not characterize this as a reproduced data-loss event.

Acceptance now passes in the complete feature-enabled UI suite (162 tests,
one ignored), strict UI/desktop Clippy, desktop build and source guards. Pointer
focus is present in every grid-cell path and scene header; canonical resolver
tests include MIDI/Stop/empty/scene-only/stale IDs and piano-focus precedence.
Native pointer-to-keyboard acceptance remains pending.

## Session keyboard workflow — 9 October 2026

After the deletion/focus repair, inspection found left/right still resolve
Arrangement nudges and up/down select tracks in Session mode. There is no Enter
action for selected cell/scene launch. Implement mode-aware canonical grid
navigation with clamped track/scene order and viewport reveal; preserve piano
note editing. Empty/stale selections must not fall through to Arrangement edits
or launch another source. Enter reuses existing quantization and accepted-graph
admission rather than adding scheduler behavior. Source work is underway; native
behavior remains unobserved while the Mac is locked.

Composition finding: the old Session row prepended its scene header to the
tracks, contradicting both Live's rightmost scene column and the new resolver's
scene-to-last-track Left movement. Correct header/rows together and include the
scene column in horizontal keyboard neighbors. This is a source-confirmed
composition mismatch, not an observed native input failure. Public reference:
[Session View, sections 7.1–7.2](https://www.ableton.com/en/live-manual/12/session-view/).

Source acceptance passes: pure stable-ID navigation, actual GPUI key dispatch,
offline launch refusal and piano-focus separation. A first header-after-tracks
repair still left the scene column floating with few tracks; full-width rows and
a flexible spacer now pin it to the window edge. Test-platform geometry confirms
header/body alignment and right edges at 880/1440 px. Root passes 170 UI tests
(one ignored) and strict Clippy; native appearance/input remains unverified.

## Atomic scene organization — 9 October 2026

Scene duplication/reordering were absent from the canonical command layer.
DuplicateScene and MoveScene now use the existing isolated candidate/compile/
history path, with count/ID preflight and stable target IDs. Model tests cover
independence, source/Stop/empty preservation, ordering, history and invalid-target
refusal; worker tests cover prepared inventory and saved/reopened PCM parity.
Root passes 99 project/43 Session tests and strict Clippy. Missing-scene duplicate
refusal also preserves redo (scene-organization-refusal-tests.log). The current
note model has no per-note expression field; do not claim expression preservation
or Suite parity. Scene-only compact toolbar controls and Cmd-D/Cmd-Alt-Up/Down
now use canonical worker commands. Selection intent survives only while its
source remains selected in Session mode; newer navigation/mode changes do not
jump back after completion. Both worker failure and queued Busy/Closed rejection
clear that intent. Actual GPUI key/worker tests cover success, history, newer
selection, mode changes, failure and piano focus. The terminal rejection helper
is tested directly because Service exposes no deterministic closed-channel seam.
Full feature-enabled UI suite passes 176 tests (one ignored); strict Clippy,
build, format and both guards pass. Root navigation acceptance is recorded in
artifacts/scene-ui-root-acceptance.log, with terminal cleanup acceptance in
artifacts/scene-ui-terminal-root-tests.log. Native appearance/input remains open.

## Session audio source gap — 9 October 2026

100-track acceptance follow-up: root's new shared/distinct PCM integration tests
exposed private AssetId construction in the benchmark fixture and a subsequent
inferred-index strict-Clippy error. Both fixture repairs are implemented; two
100-track tests and targeted strict Clippy pass, including nonzero-frame relaunch,
tempo handoff, exact silence after exhaustion and Stop/Arrangement return.
The first shared release run is retained separately, including its outliers and
initial unique-versus-logical working-set reporting limitation. Remaining command
and edit workloads, distinct timings and stable run provenance are still in
progress; see docs/performance.md. No native deadline claim follows from this run.

Benchmark review follow-up: both expanded shared/distinct release runs pass PCM
checks and have zero nominal render-period exceedances. Root review found that
`single_clip_retrigger_action` alternates clip IDs, primarily measuring source
switching. Split switch and same-clip retrigger measurements, and advance audio
outside the action timer so launch anchors are nonzero before final acceptance.
Expanded logs are retained as `artifacts/session-audio-profile-{shared,distinct}-expanded-checkpoint.log`,
with original source snapshots and `artifacts/session-audio-expanded-checkpoint-manifest.json`.
Distinct active/exhausted maximums of 116.667/122.750 µs remain recorded; later
runs must not erase them or infer their cause. No production runtime defect was
established by this benchmark-label finding.

The prior accepted Session baseline supported MIDI and Stop only. Independent
unwarped audio sources are now implemented across the model, schema, worker,
engine and UI; final worker acceptance and native checks are tracked separately. Resident
project-rate PCM already exists for Arrangement assets,
but a Session audio source must own its bounds/ID independently of Arrangement
placement. This crosses model, strict schema migration, worker preparation,
compiled inventory, dual-clock render/handoff and UI selection boundaries; adding
cell paint alone would leave inconsistent launch/edit/save behavior. The next
contract and acceptance checklist cover these boundaries in docs/session-audio.md.
Public Clip View
documentation requires Warp for audio looping: the initial unwarped mode is a
one-shot, and warped loops remain an explicit requirement. Reference inventory
and planned experiments: research/observations/session-audio-001.md. No native
audio-source behavior or reference PCM match has been observed.

Model/storage checkpoint passes root's 102 project and 23 storage tests plus
strict Clippy. Source setter compilation was repaired during this checkpoint;
copy/source/history and malformed asset/range/global-ID tests now pass. The
schema-11 migration test now covers existing Session MIDI and notes; Stop-cell
preservation is added in the worker/storage acceptance follow-up. Wrong-track
snapshots, stale source IDs, global ID exhaustion and combined clip limits pass.
Root integrated tests pass, including sounding mixed-route and exact quantized
onset oracles. The compact source row remains visible during pending edits.
Native input/listening and physical-deadline checks remain open, so these source
results do not establish app-level Live parity.

## Cross-project Session route inheritance — 9 October 2026

Owning-source review confirms a project-open lifecycle defect. Successful Open
creates a fresh prepared Playback and sets project_replaced, but the UI submits
it through ordinary publication. The callback inherits active runtime state
before consuming priority Stop. Session Stop resets clocks while retaining the
selected owner; handoff matches IDs within the two projects without project
identity. An unrelated opened file with reused track/clip IDs can therefore
inherit the old Session route. Failed Open leaves the existing project intact.
This is source-confirmed; no native reproduction is claimed.

Repair implemented: carry an explicit InstallFreshRuntime publication policy
only from successful project replacement. Install the fresh paused runtime at
frame zero with Arrangement owners, skip inheritance, and retire the old graph
through the existing bounded control-reclamation path. Normal edits/history,
imports and recovery keep Inherit. Do not rely on a fallible ordinary queued
return-to-Arrangement command or change priority Stop ordering. Accept reused-ID
projects, deferred retirement/backpressure, Stop responsiveness, exact owner/
clock/pending state and old-PCM lifetime until reclaim; retain failed-Open and
normal-inheritance regressions before closing this issue. Root's same-ID reset
and real full-retirement-ring/priority-Stop tests pass; old and new graphs remain
owned until control-side reclamation. Native replacement and end-to-end UI
coalescing acceptance remain open.

Review also found a coalescing edge: if UI publication is backpressured, an edit
event can replace the pending successful-Open candidate. The replacement must
retain the reset policy for that same project generation until admission; using
only the latest event's project_replaced flag loses it. Add this regression
alongside the callback reset test. An audio-import path chooser must likewise
reject an older project generation even if the new file reuses its target IDs.
Both async lifecycle guards are implemented in source. A GPUI regression now
drives production receive through successful Open, two actual canonical gain
edits while a candidate waits, a failed Open, control-queue admission, a later
edit and another successful Open. Candidate policy, exact revision/generation
and latest canonical gain pass. Logs:
`artifacts/project-open-coalescing-ui-root-{tests,clippy}.log`.
This covers event handling and successful queue admission; actual native
dialog/queue timing and stale file-chooser completion remain separate gates.
The publication policy installs the caller-prepared fresh graph; it does not
reset an arbitrarily pre-mutated candidate. Successful Open's service constructor
must provide the fresh paused/Arrangement graph.

## Worker Session persistence coverage — 9 October 2026

The existing worker MIDI workflow covered Arrangement edits, but lacked an
end-to-end Session save/history/reopen/render case. New original fixtures now
cover two independent Session sources and a Stop scene alongside an Arrangement
instrument. Root review corrected a silent second source and extended cancellation
past its actual due frame, with a bounded loop that cannot hang on a stalled
clock. Exact project/ID/source preservation and irregular-partition render parity
pass through loop and one-shot ends. All 42 Session tests and strict Clippy pass
in artifacts/session-workflow-root-{tests,clippy}.log. This is headless acceptance;
native Save dialogs, physical output and reference parity remain open.

## Active development bundle preservation — 9 October 2026

The old bundle script unconditionally copied over artifacts/hex.app, even when
the running process could own unsaved work. It now checks native executable paths
before building and again before publication, including case-insensitive Hex.app
aliases. The real active development process is refused with exit status 1;
shell syntax passes and the preserved executable remains SHA-256
`51b22f7b1f0ad89e034e769e8f91006e2e20146eea308b2e0bd5c2d0ace29405`.
Evidence: artifacts/bundle-active-guard.log. This checks observed processes and
does not provide an OS-level launch interlock; separate review bundles remain the
normal route while the user's app is active.

## Ordered priority Stop — 9 October 2026

Initial source inspection of prepared audio's
`consume_commands` found its independent Stop boolean discards the entire
queue sampled at the next callback. This intentionally protects Stop against a
full older queue, but also suppresses a successfully admitted Play/launch that
arrives after Stop before that callback. Existing tests encode that broad policy;
the UI intent repair deliberately preserved it. A separate bounded ordered-Stop
contract retains queue-full and ticket-exhaustion Stop availability. Rapid
Stop→Play is handled by that audio change, not by the UI token tracker alone.

Source acceptance follow-up: ordered Stop is now implemented in prepared audio;
root independently passes 18 unit and 16 integration tests plus strict all-target
audio Clippy (`artifacts/ordered-stop-root-{tests,clippy}.log`). Review replaced
the initial math-only race evidence with the actual captured-cut processor: a
post-cut Play remains queued/unconsumed, repeated Stop remains pending, and the
next actual block applies Stop then Play. Older Session launch is discarded;
invalid later atomic launch remains stopped with a rejection. Queue capacity and
pop budget share the same 32-entry constant; changing those independently would
require a persistent discard watermark. Native Stop→Play remains unverified.

## Unsaved-project close repair — 8 October 2026

Latest repeated-report acceptance — 9 October: the running development executable
still has SHA-256 `51b22f7b1f0ad89e034e769e8f91006e2e20146eea308b2e0bd5c2d0ace29405`,
and has not received the strong-editor/untyped-window repair. Six current
close-related tests pass, including actual asynchronous Cancel/Discard after
bootstrap replacement and Cmd-Q. Desktop build passes. The inactive separate
`artifacts/hex-discard-review-20261009.app` has been refreshed from this build;
SHA-256 `5a4b619feb839d6d58d49dc068fe033c5204ba63bd1792bc93a0c195cd1f8afc`.
The latest repeated report was checked again: all six close-related tests and
the desktop build pass, as do architecture and style guards. Only the inactive
review bundle was refreshed; the two running user apps remain unchanged. Native
app selection again reported a locked Mac, so current native acceptance is pending.
Logs: `artifacts/close-discard-latest-{tests,build}.log`; identity and current
acceptance status: `artifacts/discard-review-20261009-manifest.json`.
CUA selection could not find a development window, and the subsequent inventory
reported a locked Mac. Neither observation proves the reported issue's current
native cause. Fresh native verification remains pending; no existing user
project was discarded or running bundle replaced.

Lifecycle regression acceptance — 9 October: two GPUI test-platform tests now
exercise the actual asynchronous close handler after a bootstrap root is replaced
with an editor carrying unsaved project state. Cancel retains the dirty window;
Discard removes it, including the Cmd-Q path. Repeated close requests while the
sheet is open do not create another prompt. The complete UI suite with optional
`ui-test-support` passes 162 tests (one ignored), strict UI/desktop Clippy and the
ordinary desktop build pass. Logs: artifacts/close-window-lifecycle-{regression,
clippy,build}.log. This verifies framework lifecycle behavior, not a fresh native
NSAlert result. Native access is still blocked by the locked Mac. The running
development and Session-preview executables remain untouched.
Separate review build: `artifacts/hex-discard-review-20261009.app`, identifier
`org.portalsurfer.hex.discardreview20261009`; matching executable/build hash and
verification status are in artifacts/discard-review-20261009-manifest.json.

Latest isolated observation attempt: CUA inventory returned running app entries,
but selecting the separate discard-review app still reported a locked Mac and
failed automatic unlock. Inventory success alone does not establish native
input access. No project edits, prompt responses or user-project discard were
performed. Preserve any review process launched by getApp as well as the two
existing apps; do not overwrite its bundle while active.

Repeated live report — 9 October, lifecycle follow-up: process inspection confirms
the running development executable started after the earlier bundle refresh and
its on-disk hash is still `51b22f7b1f0ad89e034e769e8f91006e2e20146eea308b2e0bd5c2d0ace29405`.
Do not assume another stale-bundle rebuild fixes this report. The prompt now retains
its editor until the answer is processed, stops playback, and explicitly removes
the confirmed window outside the editor-update closure (including Cmd-Q). Failed
window updates are logged rather than silently ignored. Response codes remain
GPUI's observed zero-based 0/1; no speculative native-code translation was added.
This is lifecycle hardening, not a reproduced root-cause claim. Four close-related
tests, strict UI Clippy, desktop build, formatting and both source guards pass;
logs: `artifacts/close-discard-lifecycle-{tests,clippy,build}.log`. The tests cover
admission protection, not native sheet completion. The Mac is locked, so native
Cancel/Discard verification remains pending. `artifacts/hex-close-fix.app` is a
separate review bundle; the running development app and unsaved state were preserved.

The native startup window retains its bootstrap handle type after the editor
replaces the root view. The old close prompt downcast that handle to EditorView;
on failure it returned after displaying the sheet, never consuming Cancel or
Discard. The prompt now captures an untyped window handle and retains a strong
editor entity until its answer is handled. Already closed windows remain harmless failed updates;
Cancel preserves the project, and Discard stops playback before close/quit.

Native isolated acceptance: add a disposable instrument track, close, Cancel,
close again, Discard; the test process exits. Cmd+Q, Cancel, Cmd+Q, Discard also
exits in the final build. Observed GPUI native responses are zero-based 0/1;
no response-code workaround remains. Targeted close test, UI strict Clippy,
desktop build, formatting and architecture/style guards pass. Temporary
response diagnostics were removed. The existing development bundle is rebuilt
only after confirming it is inactive; no user's unsaved project was discarded.

Close-fix follow-up: the targeted dirty/pending-work test passes again, and the
current desktop build has been copied to `artifacts/hex.app` after confirming
`org.portalsurfer.hex.dev` is inactive. Existing running acceptance apps and their
project state were preserved. Earlier isolated Cancel/Discard and Cmd+Q native
acceptance above remains the interaction evidence for this repair.

Latest reported-close follow-up — 9 October: the development app was confirmed inactive,
then rebuilt with the existing weak-entity prompt fix. The dirty/pending close
test, desktop build, formatting and architecture/style guards pass again.
Bundle executable SHA-256:
`82ab21016d45271acb1ba5f035176567da10feb3506e4a8483498de9dc3d3e0c`.
The Mac was locked and native automation could not unlock it, so a fresh native
click-through remains unverified; earlier isolated acceptance is retained as
historical evidence, not a new run. No project was opened or discarded.

Repeated Discard report — 9 October: rechecked the weak-entity close handler
and rebuilt the inactive `artifacts/hex.app` from current source. All four
close-related UI tests pass (`artifacts/close-discard-followup-tests.log`), and
the bundle build passes (`artifacts/close-discard-followup-bundle.log`). Bundle
and desktop executable SHA-256 both match
`51b22f7b1f0ad89e034e769e8f91006e2e20146eea308b2e0bd5c2d0ace29405`.
Native automation is still blocked by the locked Mac; no fresh click-through
or user-project discard was performed. The source repair remains unchanged.

## Transport and mixed-source clock parity — 8 October 2026

Native publication acceptance — 9 October: Session-capable native graphs now use
the mixed kernel and bounded fallible action/publication path. Root acceptance
passes 139 engine, 22 audio and 41 Session-layer tests plus strict all-target
Clippy. A newly found legacy promotion hole is repaired: finite Arrangement
permits huge seeks outside the Session beat domain, so promotion validates the
base timing before touching the candidate. Counter exhaustion remains explicit
and refuses promotion separately. Both refusal cases preserve the current graph.
The transient-deferred graph test now also proves priority Stop and stopped
handoff; queued scene/track-stop/return tests prove PCM/cursor parity through
device-like partitions. See native-session-root acceptance/Clippy artifact logs.
The final callback fixture includes an Arrangement-only bass track with no
Session cell, so scene launches also verify simultaneous mixed ownership;
all 22 audio tests pass again (artifacts/native-session-mixed-callback-tests.log).

Open integration constraints: canonical Session UI and owner-aware MIDI source
editing are in progress; applied-route indicators need native route telemetry.
Recovery currently restores a paused Arrangement cursor, not a snapshot of
Session routes/pending actions/releases. Native interaction remains blocked by
the locked Mac, independently of source progress; no sustained device/plugin
deadline or full DAW parity claim follows from these tests.

Callback tempo/source transfer follow-up — 9 October: active mixed playback now
has byte-for-byte queue/publication regression against a reconciled headless
kernel after tempo, Session source phase/loop and gain edits, including small
device-like partitions and preserved dual clocks. All 23 audio tests pass; see
native-session-transfer-callback-tests.log and native-session-transfer-audio-tests.log.
This closes the source transfer checklist, not physical device acceptance.

Session launch/start atomicity — 9 October, repair active: UI admission of two
separate launch and Play actions avoids a queue-capacity half batch, but cannot
avoid starting old routes if the callback rejects the launch. The selected
contract is one fixed-size `PlaybackAction::LaunchAndPlay`, valid only for clip
or scene launch. It commits the route command first and starts transport only
on success; typed rejection leaves transport unchanged. Engine/audio executor
owns this additive contract and callback regression, UI executor consumes it
after the API is available. Do not claim two-action batching solves this case.

Native follow-up during Session UI work — 9 October: opening the existing bundle
for inspection returned `cgWindowNotFound`; a subsequent CUA surface inventory
confirmed native app enumeration is unavailable because the Mac is locked and
automatic unlock failed. An empty app inventory is not proof that hex is
inactive. Do not replace the bundle from that observation or discard any user
project. Source development remains possible; fresh native acceptance is open.

Session UI root review — 9 October, repairs active: the first source pass exposes
the canonical paged grid and shared Session MIDI editor, but queued transport
must not be discarded just because an unrelated project edit is pending.
Generation invalidation applies to all actions; edit/revision gating applies
to Session-bound actions. Coalesced rejection processing must acquire the
rejection snapshot before reading the applied high-water mark, account accepted
revisions first, then drop terminal rejected waits. Otherwise a stale applied
read can lose a successful revision. Launch request feedback also needs bounded
ticket tracking so “waiting for audio callback” resolves on consumption/rejection.
Rapid launch/space toggle intent needs an explicit test; click intent remains
distinct from applied route indicators. These are acceptance repairs, not a
claim of final native UI acceptance.

The grid currently fills the full editor width using paged track columns and
virtualized scene rows; page width derives from the window bounds. Independently
sized side panels will need measured panel bounds, and natural horizontal
scrolling remains a workflow improvement. Never leave offscreen spacers in a
paged layout that hides the later pages. Minimum-size/resizing and appearance
still require native inspection.

Session UI source review repairs accepted — 9 October: all 140 UI tests pass
(one ignored); queued Transport now survives unrelated edits, Session actions
remain revision-bound, accepted/rejected accounting uses the corrected load
order, rows/cells have exact stable IDs, and request waiting feedback resolves
through one bounded latest Session ticket. Strict Clippy, guards and desktop
build pass. Rapid launch/space intent and active/queued snapshot consumption
remain next work. No fresh native interaction was possible.

Route telemetry source accepted: 140 engine, 29 audio and 41 Session-layer tests
pass. The 256-slot atomic snapshot is coherent with accepted audio revision;
deferred/rejected candidates do not replace it and shutdown clears it. Unchanged
snapshot publication visits no slots, and engine due-route generation uses a
local successful-commit flag without an additional comparison scan. Source
inspection excludes callback allocation/waiting; allocator auditing is open.

Performance issue — 9 October: the prepared route-control benchmark preserves
PCM/route correctness at 100 and 256 tracks. One earlier 256-track changed-control
sample took 12,372.125 µs, above the 2,666.667 µs block period. Later counted
runs show zero threshold exceedances; that does not invalidate or explain the
earlier outlier. Capture/command cost versus scheduler interruption is unresolved.
Keep the original log, investigate under reproducible load, and separate
callback-control timing from full rendering and sustained device deadlines.
See docs/performance.md and session-routes-256-profile.log plus counted logs.

Current `PlaybackCommand::Stop` resets position to zero on its first application.
The official Live 12 Session manual documents two Stop presses for returning to
1.1.1; native first/second-press behavior remains unmeasured. Preserve the bounded
priority audio Stop contract until deliberately separating user stop/rewind
semantics. Also, Playback's existing Arrangement period-copy optimization must
not replay a launched Session source at every Arrangement wrap. Mixed playback
needs explicit source and quantization clocks plus loop/seek fixtures; see
research/observations/session-routing-001.md. No change to transport is claimed.

Changing-tempo loop concern: monotonic Session frames queried against the
original finite PreparedTempoMap do not repeat tempo changes inside a wrapped
Arrangement loop. If global tempo follows the wrapped Arrangement, launched
sources need a periodic musical-time mapping, not just a second sample counter.
Current manual evidence does not resolve this coupling, and the new headless
kernel must not claim it as reference parity. The periodic headless clock now
repeats the prepared loop tempo segment, with independent finite-compiler PCM
and inverse-minimality checks. Canonical endpoint selection includes markers
quantized exactly to the boundary; markers strictly beyond it are ignored on
repeats. Native Live coupling and graph-phase handoff remain integration gates.

Periodic-clock acceptance resolves the original nonperiodic headless timing
limitation. Review caught and repaired effective-rate admission for very short
loops, unvalidated public map/metadata pairing, loss of deferred error detail and an
Arrangement-only precommit regression for unrepresentable musical endpoints.
Shared-frame endpoint tests now cover the actual end, not only the start.
All 109 engine tests and 41 Session-layer tests pass; finite PCM profiles and
desktop build pass. No native playback or Live parity is inferred. Active voice
continuation across source/tempo/revision edits remains the next reliability gate.

## Session revision handoff review — 9 October 2026

The new engine-only handoff has focused gain, tempo, source and removal checks;
native activation remains deferred. Initial review repaired historical-onset
conversion on repeated edits, moved clips retaining held voices, swallowed
matching-gate errors and quadratic active-voice de-duplication. Continuation
metadata is now one optional record; stable suppression keys support bounded
lookup. The canonical 100-track gain benchmark matches independent finite PCM.

Acceptance review subsequently found control-path gaps: route Stop/retrigger
must cut copied held continuations and avoid querying historical template starts;
backward Seek must reset the effective clock through the base map. A remaining
per-current-track release ceiling conflicts with a larger transferred bank,
especially after the last track is removed. Resolve these against one shared
prepared capacity, with refusal/retry and carry-aware transport tests before
claiming full headless handoff acceptance. Current Arrangement/native playback
publication behavior remains unchanged.

## Session mixed-source accumulation prerequisite — 8 October 2026

The finite Arrangement renderer applies prepared channel coefficients to each
clip/voice contribution before summing. The ongoing MIDI template's current
standalone `render` instead sums mono voices into an overwritten stereo window.
Multiplying that completed window once by track gain can change floating-point
rounding and cannot directly accumulate into a mixed-route destination. Keep the
standalone API contract, but add a bounded additive span path through the shared
track mix coefficients before Session activation. It must query before mutation,
preserve voice order and define query-failure silence/rollback semantics. This is
a runtime integration dependency, not a reason to change current template PCM.

Resolved source prerequisite: the new additive template path completes its query
before destination mutation and applies the shared mix to each voice in span
order; errors preserve the destination and clear visible query storage. The
standalone overwritten-render implementation remains unchanged. Finite/ongoing
loop/tempo-seam and actual rounding-discriminator tests pass. Runtime callers
still need a whole-block failure policy when a later track's query refuses after
earlier tracks have contributed; this API's guarantee is local to one source.

Headless caller follow-up: SessionPlayback now stages routes/releases and clocks
and zeroes the entire requested block on a later refusal, including after a due
pending transition. Retry PCM and state rollback pass tests. Current native
Playback has not delegated to that kernel; graph-phase handoff and native
acceptance remain its integration gates after periodic timing implementation.

## Draw integration evidence gap — 8 October 2026

Pure resolver tests are source evidence only. Current fixed-anchor clamping needs
review at the clip end, and lowest-ID cell erasure must not change the exact
existing note clicked on pointer-down. Integer-pitch interpolation depends on
sample quantization; a friendly straight-line test is not proof of universal
event-density invariance. Native local-preview/history/cancel acceptance remains
required. Off quarter-beat stroke spacing, per-cell overlap choice and modifier
transition behavior remain Hex policy or unknown until measured in Live.

Continuous draw source review resolved fixed-grid tail anchors and short off-grid
clicked-note deletion. Tests cover preview sharing and lattice-aligned path cases;
universal density invariance remains unproven. Native initial five-note locked
stroke passes visual/revision checks but the saved artifact includes subsequent
edits and must be preserved, not used for exact-project acceptance. A fresh
uniquely identified acceptance process loads the pristine fixture; read-only AX/
screenshot and running-process inventory work, but repeated native pointer calls
fail window lookup. Rebinding that same process did not restore input. No crash
was established. Keep native erasure/history/minimum-layout gates pending and
retry the existing app when the automation route recovers; don't replace user work.

Follow-up on the same confirmed-running process: Raise is exposed and succeeds;
clicks return successfully now, but clip selection and Cmd+O leave the screenshot
and AX state unchanged. Revision remains 1 with the original fixture loaded.
This is still an input-delivery evidence gap, not a verified crash or successful
gesture. Continue independent implementation while retaining this native gate.

## Group expression parity gap — 8 October 2026

The existing Hex velocity buttons use eight-unit steps. Their group extension
preserves relative differences and refuses an entire boundary-crossing batch.
These are Hex button policies, not a claim of Live velocity-editor parity.
The [Live 12 manual, editing velocities](https://www.ableton.com/en/live-manual/12/editing-midi/#editing-velocities)
describes selected markers converging at minimum/maximum velocity, numeric entry,
and Cmd-arrow ten-unit steps with Shift fine adjustment. Implement and measure
those expression-lane/keyboard workflows separately; the new buttons do not
satisfy that explicit feature-parity requirement.

## Native input and initial redraw evidence — 8 October 2026

The newly built isolated group-parameter process initially shows a black canvas.
The exposed native Raise/zoom actions trigger ready-editor painting; restoring
zoom works, but Cmd+O still does not display its dialog. A one-second sample
(`artifacts/group-parameters-startup-sample.txt`) places the main thread mostly
in AppKit's event loop with editor update tasks executing. This is not proof of
a deadlock or an application input bug; renderer/input-route causes are unresolved.
Keep this exact process and investigate without restart or modifying user-owned
sessions. Source group checks pass, but no new native group acceptance is claimed.

Follow-up on the same process: AX click opens the native macOS app menu, and its
exposed Cancel action dismisses it. Simulated Escape does not dismiss that native
menu. This narrows the keyboard failure beyond Hex shortcut dispatch: input
delivery/activation is unresolved even for OS-owned UI. Do not patch Hex's key
bindings solely from this observation. Native redraw remains independently open.

Pinned dependency trace: GPUI 0.2.2 `replace_root` installs the editor and calls
refresh; its dirty frame callback draws/presents. Hex focuses the editor root and
retains gesture-only lifecycle observers. No source cause of the initial canvas
or OS-menu keyboard failure was demonstrated. Do not add speculative redraw loops
or change framework/backend to conceal this missing runtime evidence.

The pinned GPUI source also exposes no public custom-element accessibility
semantics API in the investigated Div/window/platform code. Tooltip strings do
not establish accessible controls. Rich native roles/actions remain a dependency
capability gap requiring a reviewed GPUI boundary decision, separate from this
input investigation; do not claim that Canvas controls are accessible.


## Local MIDI loop integration boundary — 8 October 2026

At the projection prerequisite checkpoint, both MIDI compilers reused the
projection while canonical MIDI clips/schema 9 remained one-shot. That checkpoint
did not deliver local-loop functionality. Whole-project prepared occurrence
admission is distinct from authored note capacity and is performed before span
allocation. The required integration connected source fields/commands, strict
migration and every source-oriented editor bound together, as recorded below. Stored metadata alone would
leave shorter Arrangement clips exposing the wrong editing domain.

The independent arithmetic oracle and existing engine/session regressions pass.
Intro/start marker workflow comes from the public Live manual; seam truncation,
onset chase, release/seek details and native/audible comparisons remain unmeasured.
See `research/observations/midi-local-loop-001.md` for the pending experiment.
Potentially ongoing Session loops still require bounded prepared templates;
finite Arrangement expansion cannot serve as the final Session implementation.


## MIDI Arrangement thumbnail source mismatch — 8 October 2026

Canonical source/loop integration exposes an existing display assumption:
`tracks.rs` draws at most the first 512 authored notes as `clip.start + note.start`
once per render. That cannot represent source offsets, repeated occurrences,
intro/partial segments or hidden notes correctly. Do not accept compact loop
controls/native loop UI until this preview is updated. Playback compilation is
independent of thumbnail shapes; the thumbnail must remain display-only.

Architecture review recommends bounded occurrence sampling, generation/revision
invalidation and per-clip/per-row shape budgets. The current piano geometry cache
is source-local editing/hit testing and must stay separate. An O(authored notes)
UI cache build can still stall large-project interaction, so review reuse of the
existing control-side compiler/worker for a prepared display summary before
choosing the final thumbnail path. Never call global MIDI event compilation from
paint. Native resize/appearance and measured interaction cost remain required.


## Local build disk exhaustion — 8 October 2026

Source-aware UI test compilation failed with rustc/LLVM `No space left on device`;
filesystem free space was 136 MiB. The failed handle was terminal and process
inspection found no Cargo/rustc builds active. Recovered 3.8 GiB by deleting only
Hex's generated `target/debug/incremental` cache (4.9 GiB reported size), after
checking its exact nonsymlink workspace path. Preserved apps, artifacts, source,
project files and active user/acceptance states. Subsequent focused checks use
`CARGO_INCREMENTAL=0` per command, with no global configuration change. Recheck
available space before bulky builds; this failure is environmental, not a passing
source result. Build/test recovery is in progress.


Canonical integration acceptance: schema 10 source settings, atomic edits/history,
strict schema-9 defaults and schemas 1–8 audio migration are now integrated.
Workspace acceptance found and corrected one stale writer-version assertion in
a session save/reopen test; the literal schema-9 fixture remained unchanged.
358 workspace tests pass, one existing UI benchmark ignored. Finite loops render
and export through the shared graph with checked tempo endpoints, release tails,
seek/partition/global-loop tests and atomic expansion refusal. Prepared thumbnail
summaries, compact native controls, reference/listening and ongoing Session
playback remain open. The disk failure was recovered and test reruns completed.


## Prepared undo/redo publication gap — 8 October 2026

Thumbnail lifecycle review found Undo/Redo restore canonical history before graph
compilation. A fallible graph/preview preparation failure could then leave the
published project and previous playing graph mismatched. New preview allocation
makes preserving this invariant necessary within the current milestone.

Architecture decision: add narrow generic prepared Undo/Redo APIs to hex-project,
peek/clone the candidate and preserve ID high-water, reserve opposite history
storage and prepare before popping/committing. Session returns the exact prepared
graph/preview pair; service only publishes on success. Preparation refusal must
preserve project/history/IDs/revision/dirty/redo and matching preview/playback.
Keep existing boolean history APIs for headless callers. Do not rely on applying
the opposite history operation as rollback. Generic refusal tests and successful
artifact/publication coherence checks are required. Import preparation/publication
is also under lifecycle inspection; no new native completion claim is made.

Prepared publication source resolution: generic prepared history APIs and staged
resident-media import now compile graph/preview before canonical commit.
Refusal tests and matching publication checks pass; 364 workspace tests pass
with one existing benchmark ignored and strict workspace Clippy passes.
The thumbnail mismatch is resolved in source; native four-occurrence display
and eight-beat source fit are observed. Native cropping, dense/minimum layout,
marker save/history and listening gates remain open.

## MIDI source-control button identity collision — 8 October 2026

Native source panel acceptance found Arrangement length + did not change the
project. Both directions passed the same tooltip into midi_button, whose GPUI
ID is that tooltip; minus/plus thus shared a sibling identity. Loop toggle
independently committed revision 2 and Undo restored settings at revision 3.
Repair requires distinct field/direction IDs and native edit/history/save checks
in a rebuilt isolated app. The old acceptance state contained only agent-created
toggle/undo history and was closed through confirmed Discard.

Source-control identity repair is integrated: each field/direction has an explicit
unique GPUI ID and distinct tooltip. Native numeric edits now commit individually;
loop enable Undo/Redo retains both markers. 132 UI tests pass with one existing
benchmark ignored. Save As artifact `native-midi-source-controls-accepted-20261008.hex`
has SHA-256 `3fc612c2704bb8f612f26f9b916b30ca16559826b4c6ea06745428ee8f28dd78`;
full document comparison confirms only the five intended numeric fields changed.
Reopen shows the saved marker values, but interrupted native input during the
final fit check left that interaction unverified; acceptance app state is retained.
No listening or minimum-layout pass is claimed.

## Ongoing MIDI template admission proof — active dependency

Prepared canonical tempo conversion is integrated engine-only. The next ongoing
edge template must bound partial-period contributions as well as full repeats,
handle absolute rounding across tempo changes, and avoid inverse-boundary skips:
frame_to_beat selects the latest quantized boundary, so earlier beat events that
round to that same frame must not disappear from a query. The previous simple
ceil-window/shortest-period suggestion is not an accepted density proof. Resolve
this before callback integration, with an independent finite projection oracle
and varied block/seek/seam tests.

Exact range-query prerequisite resolved: canonical prefix/ceil lookup and
PreparedTempoMap delegation now preserve the earliest beat at a rounded frame;
representative inverse behavior remains intact. Seven final canonical and eight
prepared timing tests pass. Infinite-template admission and bounded query code
remain under implementation; no callback or Session acceptance claim follows.

Ongoing template admission/query prerequisite is source accepted. The proof now
uses closed beat-window density for intro, cyclic repeat and a conservative seam
upper bound, with independent tiny-domain oracles. Query rate/max-tempo checks
retain that bound; work/output caps are explicit. Beyond-domain starts return
BeatDomainExhausted and clear visible output. Short gates/periods that cannot be
proven one-frame long are refused without claiming actual zero-frame conversion.
This may reject exact-one-frame intervals and some valid seam-heavy patterns.
63 engine tests and final targeted checks pass; runtime active voices/releases,
aggregate load and physical device timing remain unresolved required work.

## Ongoing instrument release-tail admission — source review resolved

Review of the first instrument-span implementation found that counting repeat
voices only at the initial loop seam can undercount voices later in the intro
release tail. Repeat notes may begin after that seam while intro notes still
sound. Preparation must bound the entire overlap interval, with an adversarial
mid-tail regression and an independent small-domain voice oracle before acceptance.
Also under review: separate intro/repeat source limits and atomic render output
semantics when an error occurs during additive mixing. This implementation is
engine-only and has not been connected to runtime playback.

Further review found interval-tree pruning used a right subtree's median onset
as its earliest onset, potentially skipping earlier notes in that subtree.
The repair must use a proven subtree bound and a focused seek regression. The
release candidate interval and period-work proof must also use the same widening
rule; subtracting release from a query already indexed by release-extended spans
would add unnecessary work and invalidate a singly widened bound.

Source review resolved: seam admission now adds all intro spans alive at the first
seam to the maximum periodic voice count, covering later repeat onsets during
intro release. Both child subtrees use minimum onset and maximum end bounds;
the seek regression retains an onset preceding its right subtree median.
Queries use direct frame lower bounds and release-extended indexed ends, with
a singly widened period proof. Each intro/repeat index separately permits the
canonical 65,536-note source limit. Rendering overwrites one source's scratch
output and silences it on failure; additive graph mixing is a later caller policy.

Final root checks pass 7 instrument tests and 3 independent density oracles.
Finite PCM matches exact first/later loop seam tails, tempo changes, one-frame
and varied partitions and seek reconstruction. Played invalid Nyquist pitches
are refused while unplayed notes remain preserved. Conservative seam/short-period
refusal remains intentional; no runtime or device-deadline acceptance is implied.

## Canonical Session source ownership — active implementation

The initial design suggestion bound Session slots to Arrangement-owned clip IDs.
Review rejected that ownership: deleting an Arrangement placement would remove
the Session source, and editing a copy could affect its original. The approved
model uses independently owned SessionMidiClips and a shared MidiSource value
type/validation. Copies receive new clip/note IDs; no dummy Arrangement timing
or back-reference is stored. Sparse explicit-stop cells preserve their distinct
meaning without prematurely deciding empty-slot launch behavior.

Session launch/runtime admission remains unresolved. A serial 100-source average
does not establish a safe aggregate callback limit, and graph replacement must
specify old-source tails and bounded retirement before runtime integration.

Historical aggregate finding (resolved by the first-tail repair below):
32 simultaneous four-beat notes looping from source start 0 are refused because
the intro-tail count is added to the bi-infinite periodic maximum (32 + 64),
although the first seam and steady state each need only 64 voices. Preserve
this finding for a tighter first-tail proof; do not claim complete polyphonic
loop admission. The dense periodic benchmark uses source start beat 2 with no
intro onsets, still exercising 32 held / 64 seam voices. This does not repair
the refusal or change runtime capability.

Independent Session ownership is source accepted. Copies/moves/deletion, generic
note/batch editing, combined limits, ID/high-water and strict schema-11 persistence
are tested; schemas 1–10 migrate empty grids. Root repaired two mechanically
affected session fixtures (Snapshot session default and schema-version assertion).
Workspace tests/strict checks/build pass. Prepared source inventory, aggregate
launch combinations, transport, outgoing-release retirement and native UI remain
required, so no playable Session or physical deadline claim follows.

## First-tail admission resolved; aggregate deadlines remain open — 8 October

The valid source-start-0 32-note loop no longer fails admission. An exact bounded
one-sided first-tail sweep replaces the conservative intro-plus-bi-infinite
periodic sum; independent oracle and delayed-repeat overload tests pass. Generic
edge-density closed-window seam conservatism is separate and unchanged. New
100-source timing distributions are in performance.md; they do not prove physical
callback admission, and wall-clock scheduling outliers cannot be assigned to DSP.

## Session tempo-edit continuity prerequisite — 8 October

Unchanged MidiSource Arc identity does not preserve oscillator phase when a tempo
map remaps historical onset frames. Before Session runtime publication, retain
currently sounding spans' frame onset anchors, already-releasing gate/end anchors,
and held voices' musical gate endpoint. Remap only held future gates, suppress
new-template duplicates while overrides live, and prove bounded Copy storage and
release retirement. Pointer equality alone must not authorize seamless replacement.
This is an original stateless-sine contract; stateful plugins need separate design.

## Desktop linker disk capacity — 8 October

artifacts/midi-first-tail-desktop-build.log ends in linker ENOSPC (exit 101).
Core source tests passed; no refreshed desktop binary is claimed. The previous
verified development bundle remains unchanged. Resolve regenerable build-cache
capacity before the next native rebuild; preserve projects and running app state.
Core preparation work can continue, so this is not a total implementation blocker.

Build-cache recovery — 8 October: after confirming no compiler/linker process
was active, removed 160,271 regenerable target/debug/deps/*.rcgu.o files and
target/debug/incremental. Free capacity rose from 2.9 to 7.3 GiB. Rlibs, binaries,
project files, artifacts and running app state were retained. Continue building
with CARGO_INCREMENTAL=0; successful desktop rebuild is still a separate check.

## Per-track Session/Arrangement ownership — 8 October

The earlier proposed global PlaybackMode Arrangement/Session would exclude
Live's documented mixed per-track behavior. Keep default Arrangement rendering
for the inventory milestone, then implement per-track override/resume rather
than treating the visible editor view as an audio mode switch. Stop does not
automatically return that track to Arrangement. Manual and read-only installed
UI evidence are in SESSION-ROUTING-001; exact runtime transitions still require
a disposable measured fixture. No existing user/template content was changed.

Desktop capacity follow-up: artifacts/session-prepared-desktop-build.log now
records a successful desktop build after cache recovery (4.43 s). Root finite
Arrangement regression acceptance passes 6 loop-playback and 11 MIDI-compile
tests; architecture and UI style guards pass. About 7.0 GiB remains available.
Prepared Session mixed-cell/worker atomicity tests are still being strengthened;
this build success alone is not Session runtime/native acceptance.

## Session revision handoff control acceptance — 9 October

The initial engine handoff copies voice phase/gates and reconciles stable IDs,
but review found remaining control gaps: StopTrack/retrigger queried historical
anchored onsets and omitted already-carried held voices; Seek retained its clock
anchor; the smaller current-track release ceiling conflicted with the global
prepared carry bank after track deletion. Repairs and focused regressions are
active. Do not activate native publication until refusal/retry, transport and
source-edit acceptance pass.

The independent gain-edit benchmark adds a repeating-tempo mode. Its first run
passes exact finite-oracle PCM at every 64-frame block after 100 canonical edits,
starting at frame 144,000 beyond the first 120,000-frame tempo cycle. Log:
artifacts/session-handoff-loop-tempo-profile.log. This is gain-only evidence,
not tempo-edit continuity, callback deadlines or native acceptance; rerun after
the remaining control repairs.

Further review found an unnecessary conversion of already-releasing musical
gates during handoff. Under a large slowdown the translated historical gate may
be unrepresentable, even though its existing sample gate/end must remain intact.
Skip that conversion for releasing voices and test repeated edits; held future
gates must continue to return a typed atomic refusal when unrepresentable.

Acceptance follow-up: all 25 handoff-focused tests now pass independently,
including the repeated occurrence after its first seam, a truly unrepresentable
historical gate at 1 bpm, shared-frame launch intent, scene snapshot retention,
removed/reordered track mixing and fresh note IDs. A controlled reduced-bank
fixture proves an audible first chunk followed by refusal within one 512-frame
call, complete output silence/state rollback, and exact retry PCM against a twin.
Root also passes all 41 Session-layer tests, desktop build, formatting and both
source guards. Final full-engine/strict-Clippy results are recorded in plan.md.
Native publication/retirement and physical acceptance remain dependent work.

## Native publication prerequisite — 9 October

The defensive held-retirement retry returned early when the retirement ring was
still full, skipping bounded command consumption and the independent Stop flag.
Normal reserved swaps do not enter this path, but ownership backpressure must
never postpone Stop. The callback now consumes bounded control on that retained
ownership path and reports stopped state during shutdown. A forced-backlog
regression verifies full command-queue Stop, output silence, weak graph lifetime,
and final reclamation through the control side. All 15 audio tests and strict
all-target audio Clippy pass; logs are artifacts/native-publication-stop-tests.log
and native-publication-stop-clippy.log. Desktop rebuild, formatting and both
source guards also pass (native-publication-stop-desktop-build.log). The native
bundle was preserved; Session kernel publication is still pending.

## Scene launch scales with unrelated cells — 9 October

The new `profile_session_grid` benchmark isolates 256-track scene launches with
512 versus 65,536 track-major stored cells. Width-32 direct-action median rises
from 374.542 to 1183.750 µs; maximum rises from 662.042 to 1716.792 µs. No sample
exceeds the nominal block period, but the existing full-grid scan uses a large
share of that budget. This is offline evidence, not a physical callback failure.

Bounded repair acceptance: construct the scene index on the control side with
fallible allocation; retain public canonical scene/cell order and empty/sparse
scene semantics; visit only selected scene cells during launch, preserving their
canonical relative order; add interleaved/empty index regression checks and run
existing engine launch tests and strict all-target Clippy. Compare the unchanged
near-limit profile after repair, record memory overhead and all maxima, then run
the affected audio/publication boundary checks. Indexing must not add callback
allocation, locking, final graph drops or a competing canonical model. Native
queue/deadline acceptance and near-limit clip/voice workloads remain deferred.

Scene-index source acceptance: full engine tests and strict all-target Clippy pass;
root audio/Session boundary suites, formatting and architecture/style guards pass.
The first integration run caught using range positions directly as canonical cell
indices; the repaired path dereferences the index vector, and both 100-track exact
PCM regressions pass. New tests cover interleaved cells, reordered scene IDs,
canonical public order and empty ranges. The identical dense width-32 profile falls
from 1183.750 to 391.458 µs median, maximum 1716.792 to 566.375 µs; small-grid single
maximum instead increases to 126.250 µs and remains recorded. Maximum index payload
is 530,432 bytes on 64-bit. Physical queues/deadlines and large clip/voice workloads
remain open. Evidence and tradeoffs are in performance.md; source/binary manifest:
artifacts/session-scene-index-manifest.json.

Final audio-profile acceptance: corrected the first switch sample to select B
after A, then reran both shared/distinct modes after scene indexing. All PCM/reset/
handoff checks and strict all-target engine Clippy pass; root desktop build passes.
The earlier mixed switch/retrigger series remains historical evidence, with its
limitation stated. Final logs and source/lockfile/compiler/binary hashes are in
artifacts/session-audio-indexed-manifest.json. Native/physical gates remain open.

## Callback allocation evidence gap — 9 October

Realtime ownership/allocation safety has been inspected and source-tested, but
not allocator-instrumented. The opt-in allocation-probe integration target is being
implemented over actual prepared callback entry points. Use pinned test-only
allocation-counter 0.8.1 through its safe API; retain the workspace unsafe-code
prohibition. Root calibration passes debug/release and proves detection of pure
preexisting-object destruction as well as allocate/free pairs. Architecture policy
rejects runtime use of the counter; the normal desktop dependency tree excludes it.
Require exact PCM, bounded action/Stop ordering and graph/PCM weak-lifetime checks
alongside zero total/net count and bytes. Full worker/media construction or oracle
allocation must not contaminate measured callback scopes. These per-thread Rust
allocator results will not discharge native API, locking or physical deadline gates.

Allocator probe source acceptance: five debug/release workload tests pass. Root
strengthened the MIDI case with exact callback-versus-kernel PCM and no-rejection
checks, then reran it in both profiles; strict all-target audio Clippy passes.
Actual 32-action execution is distinct from the pre-Stop discard case. Canonical
PCM after gain/source/tempo handoff, real retirement saturation/priority Stop,
fresh-project silence and old/fresh graph lifetime through control reclaim are
covered. All measured callback scopes assert zero total and signed-net counts and
bytes. Calibration ensures the counter would catch allocate/free pairs and pure
preexisting-memory destruction. Full runtime wrappers, quantified lock behavior,
quantized/dense MIDI/fault/rejection/backpressured-shutdown paths and physical
callbacks remain open. See performance.md and callback-allocation-probe-manifest.json
for scope, stable source provenance and exact logs; no production DSP changed.


### discard report recheck — 9 october

Latest follow-up: the corrected review app is running, but CUA still reports
the Mac locked and cannot inspect its window. The existing compiled UI test
binary reran all six close-related tests successfully
(`artifacts/close-discard-current-binary-check.log`). A fresh Cargo rebuild was
attempted but encountered the concurrent streamed-engine implementation between
edits (`artifacts/close-discard-current-check.log`); that attempt is not a passing
build check. The close source remains unchanged. Do not confuse the separate
corrected review bundle with the older development/Session-preview executables.

The repeated report was checked against the actual running executables. The
running development and Session-preview apps still have the older hashes
`51b22f7b…` and `47a7e6e0…`; the corrected separate review bundle is
`artifacts/hex-discard-review-20261009.app` (`5a4b619f…`). No additional source
change was needed: the untyped-window-handle and retained-editor fix remains in
`hex-ui/src/editor/close.rs`. Six close-related tests pass again, including
actual asynchronous GPUI Cancel/Discard and Cmd-Q window-removal regressions
(`artifacts/close-discard-report-recheck.log`). Native app selection failed and
the computer-use inventory explicitly reported a locked Mac. Fresh native
acceptance and switching from the older running builds remain pending; their
unsaved project state was preserved.


### stream endpoint review findings — 9 october

The initial in-progress endpoint draft used direct-mapped cache replacement,
which can evict a warmed mandatory page on a hash collision despite spare pool
capacity. Its request-coalescing probe chain also became unreachable after a
middle entry was removed, and it woke the worker with Thread::unpark from the
render-side endpoint. These are implementation review findings, not accepted
behavior. The writer has been asked to use a fixed collision-safe lookup/owned
slot policy, preserve probe-chain coalescing after deletion, and keep worker
scheduling off the render endpoint. Add targeted collision/pressure checks before
callback integration. Track resolution here when source and tests prove it.

Further pressure review found that a fully cached pool needs proactive unpinned
eviction before queuing a new page; waiting for a ready page to trigger eviction
cannot progress when the worker has no free buffer. Admission must also reject
sources whose maximum conversion halo exceeds scratch/reader windows, and clamp
each manifest allocation to the remaining aggregate catalog budget. Shutdown
must report actual reclaimed buffers and join failures rather than infer counts
from pool size. These cases are part of the first worker acceptance, not deferred
to native callback integration.

Independent worker acceptance now passes known signed24 source pages against the
retained frozen v1 converter in debug/release, followed by 18,811 engine frames
with phase/fades/loop seams and zero warmed misses. All eight test buffers are
accounted after joined shutdown. The separate fault harness caught a remaining
classification issue: an aggregate manifest-budget refusal was reported as
DecodeFailed instead of CapacityExhausted. Resource-limit errors must remain
distinguishable from malformed-source errors; rerun the fault harness after the
worker mapping is corrected. These checks do not establish native streaming.

Classification repair is now independently accepted: WAV resource/allocator
refusals map to CapacityExhausted, while malformed media remains DecodeFailed.
Root debug/release fault harness passes fingerprint mismatch, insufficient halo,
aggregate manifest/kernel refusal, stale project-generation key and immediate
preparation shutdown. Each of six cases joins and accounts for exactly eight
buffers. Immediate shutdown does not prove a particular cancellation race was
exercised. Logs/provenance: stream-worker-admission-independent-manifest.json
under artifacts. Queue-pressure and long-source acceptance remain open.

Workload review caught another acceptance mismatch: the first long-source
profile opened 100 independent descriptors but warmed/rendered only asset 0 in
one track. That can establish catalog preparation and one far-page read; it
cannot establish 100 active streamed tracks. The required replacement must warm
and render one clip on each of 100 tracks, compare an independent canonical-order
mix oracle, and report preparation versus warmed timing/misses separately. Do not
substitute a 100-file catalog count for the actual active-track workload.

The corrected 100-track far-source workload now passes, and ready/failure/return
queue pressure and joined shutdown accounting pass. Final source review found a
long-turnover performance risk: cache and request tombstones were never cleared,
so eventually low-occupancy misses/inserts could scan the complete lookup table.
The warmed single-window profile does not exercise that history. Replace the
accumulating deletion scheme and verify wraparound collisions and repeated
turnover before native wiring; keep buffer/pin ownership and callback bounds.

Turnover repair is source accepted: both indexes now use backward-shift deletion
without dead-entry state. Forced wraparound collisions survive 512 churn cycles;
low-load misses stop after one probe when empty and five with four colliders.
Seven stream unit and four real-runtime tests pass, including ready/failure/return
pressure and undrained-ready shutdown. Final root worker debug/release checks
still match frozen conversion and 18,811 render frames with zero warmed misses.
The resident/MIDI helpers are shared by both render paths, with full engine/audio/
stream tests and allocation probes passing after the cleanup. Desktop build and
six close lifecycle regressions pass; native verification remains separate.
Evidence: hex-stream-root-final-tests.log, hex-stream-endpoint-turnover-clippy.log,
stream-worker-final-independent-{debug,release}.log and the common-helper logs
under artifacts. The native app still uses resident sources.

### long-file reader is available; resident limits remain — 9 october

The bounded WAV reader now opens classic RIFF sources using header seeks and
32 KiB scratch rather than a full file buffer. Tests include a sparse 17M-frame
stereo PCM16 source and exact far samples. Resident decode now shares the parser;
full Session tests pass. This does not yet change native import limits or enable
streamed playback. RF64, immutable content identity, incremental conversion,
cache and realtime page ownership remain required. Length/mtime checks cannot
prove a source unchanged against edits that preserve those metadata values.


### incremental conversion edges reviewed — 9 october

The first empty-output implementation incorrectly required a halo at (0,0)
when an EOF no-op had nonzero source offset. It now validates the output interval
and returns without source work for empty output. A regression covers EOF and
nonzero global positions. Root also replaced the weak all-zero phase-carry
fixture with a sounding impulse/neighbors and frozen raw output bits. Independent
pre-refactor comparison across nine rate pairs passes in debug/release. Per-plan
kernel duplication remains a memory issue before 100-track integration: share
immutable rate-pair tables on the control side. Metadata checks alone still
cannot prove unchanged mutable sources; production page verification is pending.


### shared tables accepted; source verification active — 9 october

Per-source table duplication is now avoidable with explicit ResampleKernel and
with_kernel plans. The source/session cache must actually reuse those handles;
convenience constructors do not hide a global cache. Reader length/mtime checks
still cannot detect all source changes. The chosen next primitive verifies
complete raw-page SHA-256 hashes and decodes those same bytes. The one-pass
manifest identifies captured bytes, not an atomic filesystem image. Sidecar
persistence, aggregate budgets, generation lifecycle and live delivery remain
required before streamed playback.


### verified bytes accepted; worker/cache delivery remains — 9 october

VerifiedWavReader catches same-size restored-mtime source edits when relevant raw
pages are accessed, including header pages. Root 24-bit cross-page/halo/hash/edit
checks pass debug/release; all 47 media tests and strict Clippy pass. Scratch cache
identity is cleared before loading another page, public reads start fresh, and
typed cancellation cannot be retried indefinitely as Interrupted. This identifies
captured bytes, not an atomic source image. Hashing does not validate every float
sample; full source admission must address late nonfinite values. Source identity
is runtime-only; persisted fingerprints, sidecars, aggregate retained budgets,
project generations and bounded live worker/page delivery remain required.

Workspace all-targets compilation exposed preexisting ungated px/Arc imports in
Session audio panel tests. They are now gated with ui-test-support, and base plus
feature-enabled all-target checks pass. No production UI behavior changed.
### repeated discard report: current source versus running apps

On 9 October, the current source reran all six close-related tests successfully
(`artifacts/close-discard-user-report-tests.log`), including asynchronous
Cancel → Discard after bootstrap root replacement and Cmd-Q confirmation.
The strong editor entity and untyped window handle repair remains in place.
Process inspection still finds the older development executable (`51b22f7b…`)
alongside the separate fixed discard-review executable (`5a4b619f…`); the
running development app must not be treated as the repaired build. Native
selection of the review app was blocked by the locked Mac. Existing windows
and unsaved work were preserved; native acceptance remains pending.
### canonical streamed compilation: accepted scope and remaining dependencies

On 9 October, the Arrangement compiler gained one mixed resident/streamed source
resolver and a shared MIDI-preview helper. Referenced bindings require exact
generation/asset/rate/count/version matches; unused history entries are allowed.
Five focused tests, full engine regressions and strict Clippy pass. Root actual
worker debug/release matches 60,452 PCM frames against resident wrappers and the
frozen v1 converter, with identical preview, zero warmed misses and complete
eight-buffer shutdown accounting. Session/audio allocation regressions and
desktop build pass. Logs and source identity are recorded in
`artifacts/stream-canonical-acceptance-manifest.json`.

Review caught an acceptance-scope error: Renderer is linear. A fixture with a
project loop set does not establish automatic wrapping. The final harness now
checks compiled loop metadata and explicitly seeks across three seams; streamed
Playback scheduling remains unimplemented. This compiler path is Arrangement-only;
PreparedRevision/Session still require resident audio. Runtime worker joining,
callback page ownership, generation leases, prewarm readiness and persisted
fingerprints remain required before native streaming or reopen-integrity claims.
### stream owner split: source accepted, native lifetime still open

The joining worker/catalog owner and bounded page endpoint are now separate.
Endpoint owns no Thread, mpsc catalog receiver, descriptor, wake or join; its
ordinary storage destruction still must occur on control after quiescence.
Control Drop can stop the worker and close its descriptors, so metadata Arc
ownership does not establish a catalog lease. Keep control alive through all
referencing graph/endpoint retirement until native lifetime integration exists.

Review corrected the proposed failure-consumer placement: Endpoint must consume
failure records to remove pending keys and allow retries. Moving that consumer
to control without another bounded handoff would strand coalesced failures.
Pair identity is independent of source keys: two identical-key real catalogs
reject cross-pair shutdown before stats/cache mutation, return both owners, and
correctly account for 16 buffers on subsequent matching shutdown.

Debug/release stream suites and allocation probes, strict Clippy and guards pass.
Root split PCM/frozen-v1 checks match 60,452 frames with zero warmed misses and
eight-buffer joined accounting. Provenance is in
`artifacts/stream-control-split-acceptance-manifest.json`. No native app changed.
Session source rendering, automatic streamed loop scheduling, endpoint return
after callback shutdown, prewarm readiness and persisted identity remain open.

### discard review: native acceptance completed — 9 October

The unlocked Mac allowed the corrected discard-review build to be exercised.
In a disposable project, native close opened the unsaved-work sheet; Cancel
preserved the window, a second native close reopened the sheet, and Discard
exited the review process (CUA inventory confirmed isRunning=false). The corrected
build was reopened clean for the user. The older development and Session preview
apps were preserved. Six close-related tests pass again in
`artifacts/close-discard-native-followup-tests.log`. The existing source repair
was sufficient; no additional production change was needed. Updating a bundle
does not update an already-running process.

### streamed Session publication boundary — 9 October

Mixed PreparedRevision and Session page rendering are now available headlessly.
The native audio Control explicitly refuses any streamed candidate before slot
admission or revision advancement, returning it intact to the caller. A regression
uses a Session-only streamed source, both publication policies and both runtime
constructors, then admits a resident revision to prove refusal consumed no slot
or revision. Full audio allocation tests and audio/stream strict Clippy pass.
The actual-worker Session oracle compares 110,758 frames in debug/release, with
no warmed misses and complete eight-buffer shutdown. Native readiness, catalog
lifetime/retirement and persistent fingerprints remain implementation dependencies.
Additional late-failure/malformed-page acceptance coverage is under review.

Streamed Session source acceptance completed: full engine suites (70 unit tests
plus integrations), strict engine/audio/stream Clippy and workspace formatting
pass. Added coverage verifies short/wrong-index pages, cold misses preserving
audible resident MIDI/audio, finite Playback loops, and later-chunk refusal after
a page request: the full block zeros while routes/clocks/releases roll back.
The existing actual-worker oracle, allocation probes and desktop build pass.
Provenance: `artifacts/stream-session-acceptance-manifest.json`. Native source
lifetime/readiness/publication, persisted fingerprints and sustained advancing
streaming remain open. The next implementation unit must attach catalog lifetime
to accepted graphs, provide bounded control-side prewarming, and return callback
endpoints after quiescence before lifting native streamed-admission refusal.

### native stream lifetime contract under implementation — 9 October

Source trace confirms StreamControl Drop currently cancels/joins, so metadata
retention cannot protect audio/export graphs. The selected replacement is an
actual shared worker/catalog owner, held by endpoint and opaque leases, with
matching/busy shutdown returning owners intact before mutation. This unit remains
headless until publication validates source coverage and callback endpoint return
is integrated. CPAL device fault may prevent normal callback acknowledgment;
existing lifecycle-worker/quarantine disposal must cover that path. Final joining
must also stay outside UI ticks to preserve responsiveness. Prewarming needs
installed-page acknowledgments, not worker enqueue acknowledgments.

### Real catalog ownership accepted; implementation paused — 9 October

StreamControl and callback pages now retain the actual shared catalog owner.
Ready catalogs provide opaque CatalogLease handles with verified sorted full
metadata membership; fingerprints/generations must match and separate runtime
identities remain distinct. Control Drop no longer cancels a worker retained by
pages/leases. Matching shutdown refuses outstanding leases before mutation and
returns both halves intact; dropping leases permits retry and exact pool
accounting. Final owner release stops/wakes/joins off rendering.

Debug/release stream suites pass nine unit, one allocation integration and nine
worker integration tests. Strict Clippy, workspace formatting and architecture/
UI style guards pass. Independent debug/release checks match 110,758 Session PCM
frames, preserve cache/stats through Busy recovery, reclaim eight buffers, reject
wrong full metadata and distinguish identical-key catalogs; an uncached real page
read succeeds after controller Drop. Private test observation confirms final lease
release joins on the named control reclamation thread after endpoint/controller
release. Provenance: `artifacts/stream-catalog-lease-acceptance-manifest.json`.

This is headless ownership acceptance. Native graph/export lease attachment,
callback endpoint retirement, device-fault teardown and non-UI final reclamation
remain required; prewarm installed-page acknowledgment and sustained native
streaming are also open. Ordinary endpoint/lease final Drop is not realtime-safe.
The user requested a pause after this unit to redirect work toward quicker
practical usefulness. No further implementation or prewarm/native admission
work was started; preserve the full roadmap for that discussion.

## Left browser acceptance — 9 October 2026

Implemented the user-selected compact browser. Review repaired project opening
to use the existing replacement guard: first activation retains the dirty
project; only repeating the same candidate confirms opening. Changing candidates
requires a new confirmation. GPUI regression coverage passes. Also repaired
window-local resize coordinates, width clamping, stale-scan coalescing, Unicode
case-expansion offsets and IME clear state. Native visual review found a dark
search placeholder; the final build uses Palette::MUTED.

Checks: hex-library 3 passed; hex-ui 188 passed and 1 existing ignored; affected
strict Clippy, fmt, architecture/style guards and desktop build passed. Evidence:
`artifacts/browser-targeted-tests.log`, `browser-clippy.log`,
`browser-placeholder-clippy.log`, `browser-architecture.log`,
`browser-ui-style.log`, `browser-fmt.log`, `browser-final-build.log`.

Native startup/layout and final search contrast were visually inspected. The user
requested to perform native interaction tests themselves; folder/search/import,
collapse/resize and Session acceptance remain pending their feedback. The running
main bundle was left intact with unsaved user work. A separate final review bundle
is available at `artifacts/hex-browser-review-20261009.app`. No claim of completed
Live browser parity: preview playback, drag-to-timeline, persistent places and
recursive indexing are deferred.

### Browser resize follow-up — 9 October

User accepted the other browser checklist items and reported that the right edge
could not be dragged; X-button collapse worked. The separator's inner container
and handle lacked height, leaving its event canvas without a usable hit area.
Both now fill the browser height. A GPUI pointer regression grabs the separator,
drags right and left, releases outside the original handle and verifies subsequent
pointer movement does not resize. Native resize retest is assigned to the user.
Final resize build: `artifacts/hex-browser-resize-20261009.app`; the running
application and its unsaved work are preserved.


## Hybrid tracks — 9 October 2026

Implemented the intentional Live deviation: one stable track/mix owns both
audio and MIDI collections. Clip kind now selects editing controls rather than
track capability. Legacy snapshot variants remain migration inputs; canonical
snapshots/writer use hybrid schema 13. Added literal schema-11 Session and
schema-12 audio migration fixtures so generated downgrade fixtures are not the
only evidence. Selected-track async import captures project generation to reject
stale destinations after replacement, even if IDs match. Existing count, voice,
source and ownership limits remain enforced. MIDI uses the original sine.

Workspace tests, strict lints, desktop build, guards/fmt and independent
debug/release mixed PCM/persistence acceptance pass. GPUI support tests pass
190/190 with one existing ignored test. Same-track Session switching/release and
mixed Arrangement headless callback allocation tests pass; the latter compare
audible source-only PCM sums and require zero allocations/deallocations. Physical
device deadlines, native mixed-source interaction and listening remain open.
The separate `artifacts/hex-hybrid-tracks-20261009.app` includes the browser
resize fix; user owns native checks. Running apps/unsaved work are preserved.
No plugin/device-chain or broader roadmap work is included.


## Arrangement drag preview and MIDI movement — 9 October 2026

Replaced the mouse-following ghost with one clipped target-track landing
rectangle; audio and MIDI now share typed preview/drop resolution and commands.
MIDI grab/start/length remain beats, with destination frame bounds mapped through
the tempo map. Review caught row-Y-only hit testing: X+Y bounds now reject
headers/browser/outside lanes, and visible-row caches are pruned after scroll.
A refactor initially routed snap-off audio frames through tempo conversion;
restored the exact frame bypass with a high-frame regression.

UI suite 192 passed/one existing ignored; final musical-time and drag suites
8/8 and 11/11, strict UI Clippy, desktop build, fmt and both guards pass. Native
release/Escape/outside-lane/cropped-source checks belong to the user and remain
open. Separate review bundle `artifacts/hex-clip-drag-20261009.app` preserves
the running app/unsaved work. No new autoscroll or overlap-policy change.


### Clip pickup regression and browser folder loss — user reports, 9 October

User reports that the landing-preview build cannot pick up clips: click+drag
produces no movement. Geometry/unit checks were insufficient to establish native
GPUI pointer pickup. Investigate actual hit testing/dispatch and cover rendered
audio/MIDI drag initiation before further UI work. Both running hybrid/clip-drag
review apps are preserved. Native acceptance is failed pending repair.

Browser places are currently view-only Vec<PathBuf>; adding a folder is lost on
restart. User requests persistence in the next build. Application settings must
remain independent of project metadata and control/audio state; preserve paths,
bound I/O and surface storage errors.

User clarification: MIDI initially drags on its own track; crossing to another
track hides the drag preview, but release successfully places it there. The
moved MIDI clip then cannot be picked up again. Acceptance must cover cross-row
preview continuity and a second drag after the applied edit/revision refresh,
in addition to audio initial pickup.

Drag diagnosis: the UniformList processor receives partial rendering batches;
per-batch lane-cache retention and preview clearing treated those batches as
the entire visible set and erased valid cross-track previews. Removing that
destructive pruning repairs actual rendered pointer preview/drop tests. The
second-pickup test initially left worker_requests_pending=1 after simulated
publication; completing that simulated response restores the correct pickup
surface. Both rendered audio/MIDI tests pass; this does not independently
explain every native pickup symptom, so user retest remains required.

Final follow-up source acceptance: current-painted-generation eligibility
replaces per-batch pruning; rendered wheel scrolling rejects stale overlapping
row bounds. Browser settings now live in hex-preferences, with native paths,
bounds, synced atomic publication, existing-file revalidation and ordered/coalesced
background UI saves. Temp settings tests avoid touching real preferences.
8 preferences/197 UI tests pass (one ignored), strict affected lints/build/guards/
fmt/diff checks pass. Combined review bundle is
`artifacts/hex-browser-places-20261009.app`; user restart/drag retest remains open.
Compilation initially failed due to disk exhaustion; only approved hex UI/desktop
incremental caches were removed, then final checks reran with incremental off.
Source, running apps and artifacts were preserved.


### User acceptance and next browser drag workflow — 9 October

User confirms both previous acceptance items work: browser folder survives
restart and audio/MIDI clip dragging works. This closes that native follow-up.
Next request adds browser WAV drag-to-track with conditional floating/locked
preview and asks about outbound drag to external apps. Record pinned GPUI
capability before any native boundary decision; do not equate internal dragging
with an OS file drag.

Pinned GPUI 0.2.2 outbound boundary finding: `.on_drag` creates an internal GPUI
view painted cursor-relative in the same window (`elements/div.rs` and
`window.rs`). macOS window code handles inbound pasteboard file drops only;
there is no public outbound native file-drag API or NSDraggingSource/session
implementation. Internal free/locked preview is supported. Cross-application
file dragging is deferred to a separate documented native integration; no
unsafe exception or new platform framework is introduced by this unit.

Browser drag source acceptance: bounded session preview preparation returns
project-rate duration and 48 waveform bins, with no PCM retained in UI. One
preparation in flight/latest pending and capped metadata cache run independently
of project edits. Conditional internal ghosts and track landing previews share
release geometry; browser import uses existing ImportToTrack. A paint-only GPUI
on_mouse_event registration attempt was removed after the rendered test caught
it; normal row handlers now cover cancellation/drop. Stale snap/viewport/rate/
project/listing states are rejected.

200 UI tests pass/one ignored, preferences/session tests, strict affected lints,
desktop build and guards/fmt/diff checks pass. Review bundle:
`artifacts/hex-browser-drag-20261009.app`. Native user checks remain open. External
file drag is unsupported by pinned GPUI; a file externally modified while held
can differ from preview, and import reopens/validates it. No held-source lease.


## Sample drop inserts track — 9 October 2026

New canonical ImportPreparedAudioTrackAt command uses stable before-track ID
or append plus frame start. Old import delegates append/start-zero. Session
prepares media/candidate graph before committing track+asset+clip as one history
operation; worker rejects missing anchors/track/asset/resident limits before
decode. Track order uses existing schema-13 persistence.

Insertion targets use current-painted adjacent row bounds and real final-row/
empty-body geometry. Full-width line suppresses floating/clip previews. Tests
found GPUI payload ghosts did not repaint on shared state changes; the browser
ghost now uses the editor overlay. A coordinate review removed subtraction of
global window bounds from window-relative mouse positions. Rendered tests cover
actual suppression, body/gap/bottom/empty/scroll and invalid/off-lane behavior.

Full project/session suites and final 205 UI tests pass (one ignored); strict
affected lints/build/guards/fmt/diff checks pass. Evidence:
`artifacts/track-insertion-core-tests.log`, `track-insertion-ui-tests.log` and
`track-insertion-review-manifest.json`. Separate review bundle:
`artifacts/hex-track-insert-20261009.app`. Native insertion/undo/reopen acceptance
remains assigned to user. Running apps/unsaved projects are preserved. Earlier
accepted browser/clip/preferences changes committed/pushed at 1d892dc; new
insertion changes are uncommitted pending acceptance.


### Native insertion feedback — 9 October

User asks to keep end-line + cursor ghost in blank append space (divider alone
still suppresses ghost), reports workspace jumping upward at bottom hover and
clip placement unexpectedly far left, and requests the same new-track gesture
for existing audio/MIDI clips. Native acceptance of insertion remains incomplete.
Concrete insertion origin bug: body.left + header width assumes a left header,
but row layout is clip lane then right header; use body.left and subtract header
only from width. Bottom jumping is being reproduced, not yet attributed.
Existing-clip extension requires canonical atomic insert+move, preserving source
track/IDs/media/notes/history and original audio-frame/MIDI-beat anchors.

Reproduction checkpoint: insertion origin was 176 px too far right (right-hand
header misaccounted); a noncentered pointer test now derives expected frame from
actual painted clip-lane bounds. Bottom append/empty retains ghost+line and uses
valid destination rather than visual-suppression flag for release. Bottom jump
was a 36 px Arrangement height shrink: `.relative()` overrode the ghost's
`.absolute()` and made it a flow item. Removing that override preserves
Arrangement origin/size in the rendered regression. Existing-clip creation is
still being implemented.

Final follow-up: typed audio/MIDI insert+move commands keep source track and
original IDs/metadata, with one compiled history edit and persistence checks.
Rendered release test found track-lane handler consumed divider drop before the
parent Arrangement handler; it now routes valid insertion destinations to the
new-track command and guards gesture identity. Floating browser and existing
clip overlays remain absolute and use shared free/locked destination geometry.

209 UI tests pass/one ignored, focused project import/MIDI 30 and Session service
18 pass; independent full project/session suites pass. Strict affected lints/
build/guards/fmt/diff checks pass. Evidence: `artifacts/track-drag-fix-core-tests.log`
and `track-drag-fix-manifest.json`. Separate build:
`artifacts/hex-track-drag-fix-20261009.app`. Native user retest remains pending;
source remains uncommitted and running app/unsaved work is preserved.


### Overwrite drop and empty-area feedback — 9 October

User asks for whole empty-section highlight; no cursor-y line on zero-track
project, and subtle below-last highlight alongside existing bottom line. Also
asks drop-on-clip to carve that clip rather than overlap. Scoped to Arrangement
drops/imports; generic legacy overlap persistence/rendering stays permissive.
MIDI fragmentation needs a deliberate contract: original source playback rejects
one-shot source-start at/past end and does not chase notes before source-start,
so merely advancing a right fragment breaks silent tails and boundary-held notes.
Independent fragment ownership also needs globally unique note IDs. Read-only
architecture analysis resolved this with finite right-fragment one-shot
projection and bounded occurrence preflight. Boundary-held notes retrigger; right
loops are linearized. Safe mixed frame/beat rounding may leave tiny gaps. See
`docs/clip-overwrite.md`; implementation and native acceptance remain pending.

Overwrite implementation is source-tested. Review caught zero-frame fragments
and nondeterministic split-ID allocation; fragments now omit collapsed frame
intervals and allocate IDs in destination order. Full-cover import can replace
at the clip cap; failures remain atomic. Source suites/build/lints pass;
`artifacts/hex-overwrite-drop-20261009.app` awaits native review. No callback or
storage-schema change.


## divider highlight correction — 9 October 2026

Native user report: between-track hover highlighted every row below the divider.
The append overlay was gated only by insertion-line presence; it now also
requires a current new-track destination with no before-track anchor. Divider
hover remains line-only for browser samples and existing audio/MIDI clips;
zero-track and bottom empty-area highlighting retain their prior behavior.
All three new rendered divider assertions failed before the fix; the fixed
full UI suite passes 209 tests (one existing ignored). Strict UI Clippy, desktop
build, guards/fmt/diff pass. Review: `artifacts/hex-divider-highlight-20261009.app`;
native retest remains pending. No project/audio model or overwrite behavior change.


## arrangement navigation review — 9 October 2026

Active implementation review identified three gates: adaptive spacing must use
actual lane width, empty grid candidates must never call step_by(0), and wheel
events accumulating below the zoom threshold must not fall through to vertical
track scrolling. Empty/below-last grid geometry must share the ruler/lanes'
right-header inset. Executor is addressing these before rendered acceptance.
No native feel verdict or measured interaction/paint timing yet.

All navigation review gates above are resolved and source-tested. Grid has one
shared behind-clip layer with matching right-header inset; empty strides are
guarded. Captured wheel consumes timeline events even below threshold/at clamps
so rows do not scroll. Mode switching cancels pan before Arrangement handlers
disappear. 216 UI tests pass/one ignored; lint/build/guards/fmt/diff pass.
Native feel/visibility acceptance remains open.


## adaptive snapping and prospective track preview — active, 9 October

Latest user correction keeps grid in blank space and zero-track Arrangement,
enables matching snapping there, and requests a locked clip preview on the
prospective appended row. Command temporarily inverts snap per official Live12
manual; dynamic preview/release modifiers must agree. Track-only rendering is
superseded. Finite scope, reference confidence and native checklist are in
`docs/grid-snapping.md`. New source remains uncommitted for native review.

Snapping acceptance gates include Adaptive+Command inversion (not just fixed
grids), nearest landmarks adjoining meter changes, and consistency between
bounded grid paint and snap resolution. Held-from-pickup, stationary modifier
changes and actual release modifiers must all be covered. These are source
review requirements; native exact Live thresholds remain unverified.

Adaptive snapping/append preview source milestone complete: shared visual/snap
policy defaults to Adaptive, Command temporarily inverts it, stationary changes
refresh previews, and actual release modifiers determine the committed edit.
Browser/audio/MIDI append previews lock to the prospective row and clip to the
body. Grid stays in blank/zero-track space per latest correction; dividers remain
line-only. 220 UI tests pass/one ignored; strict UI lint/build and guards/fmt/diff
pass. Review `artifacts/hex-grid-snapping-20261009.app`; checklist/reference limits
in `docs/grid-snapping.md`. Native acceptance open; no commit/push.


## grid appearance, native input and overview — active, 9 October 2026

User reports snapping functionality looks good, but Command hides the grid,
wheel zoom direction is inverted and native middle-button pan fails. Keep
grid painting independent of temporary snap modifiers; add alternating subtle
bands every four adaptive units anchored to musical time. User cancelled the
scrollbar/handle and instead requested an always-fit overview above the ruler.
Use a bounded cached 128×16 audio/MIDI occupancy thumbnail, aggregating dense
tracks, with viewport outline and click-to-center. Preserve compact layout.

Pinned GPUI0.2.2 lacks native otherMouseDragged callback and scroll inversion
metadata. Primary accepted a narrow external dependency repair, after readonly
architecture review: native callbacks/flag inside excluded upstream GPUI only,
no unsafe Hex or registry-cache edit. Provenance/license/check policy is in
`docs/native-input.md`. Physical positive-Y up zooms in, negative down zooms out;
browser/panel raw scrolling retains OS preference behavior.

Acceptance: Command grid remains visible while movement is free; band phase
remains stable under pan/zoom/tempo/meter; cached overview includes every track
and both clip kinds without whole-project per-paint scans; click/view geometry
and bounds; wheel Lines/Pixels both inversion states; native target compiles;
prior snapping/preview/navigation regressions pass. User owns native input
and visual verdict. No scrollbar, audio/schema change, commit or push.


## overview and native-input review milestone — 9 October 2026

Implemented the compact always-fit Arrangement overview above the ruler,
revision-cached audio/MIDI raster, viewport outline and click-to-center. Details
and native checklist: `docs/arrangement-overview.md`. Command no longer hides
the grid; four adaptive-unit alternating bands stay anchored to musical time.
Physical wheel deltas are normalized independently of macOS Natural Scrolling.
The pinned external GPUI patch registers native middle-drag delivery; source
and simulated event tests cannot establish actual physical delivery.

Review build: `artifacts/hex-overview-input-20261009.app`. Source/binary identity:
`artifacts/overview-input-manifest.json`. Native visual, physical wheel (both
Natural Scrolling settings) and middle-button pan acceptance remain open.
No scrollbar, audio/schema change, commit or push. Broader roadmap stays paused.

Verification: 228 UI tests pass, one existing ignored. Strict UI Clippy, desktop
build, formatting, architecture/UI-style/GPUI-patch guards and diff checks pass.
The final band candidate budget is bounded even when conversions yield no
painted bands. Existing upstream dependency compiler warnings remain; no Hex
warning or check failure. Native interaction/appearance verdict remains open.


## live clip resize — active, 9 October 2026

Finite scope: thin edge handles at all meaningful zoom levels, a preserved
clip-body move target with detection priority, actual in-track geometry/waveform
preview during trimming, existing adaptive/fixed snapping and Command inversion
including stationary modifier/release changes. Commit one validated canonical
edit at release; cancellation/stale/no-op gestures preserve project/history.
Audio trim source/fade limits remain canonical. MIDI Arrangement trim policy
requires source-contract review before extending this work.

Acceptance: both edges at narrow and wide rendered clip widths; move versus
resize hit targets; live preview with unchanged project before release; grid/
free candidate and final edit agreement; cancellation and stale state; source
tests, strict UI lint and native build. User owns native UI checks. No schema,
audio callback, commit or push; broader roadmap stays paused.

Resize boundary review: canonical audio `TrimClipEdge` currently changes only
the target clip; it does not apply the overwrite-on-drop neighboring-clip
policy. This UI improvement preserves that command contract. A unified
resize/overlap policy remains separate model work. MIDI Arrangement edge trim
is also unsupported; do not imply piano-roll note edge resizing implements it.

The user explicitly included MIDI on 9 October 2026. Added the canonical
beat-based `TrimMidiClipEdge` and O(1) `MidiClipTrim` preview: retain authored
notes/IDs/source bounds, adjust source marker for left edits, preserve loop
phase by the documented original policy. Right extension can expose silence
for a one-shot. No schema fields changed. Source policy: docs/midi.md; visual
contract/checklist: docs/clip-resizing.md. UI integration and native verdict
remain pending. This supersedes the MIDI deferral in this work-unit heading.


## audio/midi resize source milestone complete — 9 October 2026

Thin bottom-corner handles (up to 4×6px; narrow widths derived from the rendered
3px audio/4px MIDI minimum) preserve the upper move target. In-lane previews
redraw waveform/note geometry immediately; original hit geometry remains fixed
and transparent. Existing grid/Command inversion updates during motion,
stationary modifier changes and authoritative mouse-up. Release commits one
validated edit; cancellation/no-op/stale state preserves canonical data.
MIDI metadata-only previews retain authored notes and source/loop markers.

233 UI tests pass, one ignored after the final narrow-width repair. Workspace
integration: 695 tests pass, one ignored before that UI-only repair. Project/
storage/session/UI strict lint, desktop build, fmt and architecture/UI-style/
GPUI-patch guards pass. Logs and source/binary identity are in
`artifacts/clip-resize-manifest.json`. Existing upstream compiler warnings remain.

Review `artifacts/hex-clip-resize-20261009.app`; native audio/MIDI left/right
corner usability, real-time appearance, Command release agreement and Undo
acceptance remain open. Checklist: docs/clip-resizing.md. No commit/push or
automated native interaction; broader roadmap stays paused.


## bracket cursors and flush clips — active, 9 October 2026

User accepts full-height resize behavior; requests distinct bracket cursors,
flush full-track clip height and a one-pixel black border replacing the left
accent stripe. Finite scope: original left/right bracket shapes using the
existing external native cursor boundary; cache/retain native objects, preserve
edge hotspots/body movement, no per-hover allocation. Apply flush clip geometry
and border consistently to audio/MIDI, live resize and landing previews.
No project/audio/schema changes. Source checks/rendered geometry and native
build first; user owns physical cursor/appearance acceptance.

Waveform scope added by the user: replace 48 blue amplitude bars with black
filled signed per-channel silhouettes and zoom-dependent detail; paint the
existing musical grid through clips. Control-worker peak caches now use up to
65,536 leaves, a bounded extrema hierarchy and shared immutable PCM for exact
small-range queries. Accounting includes hierarchy bins. Media/session tests
and strict lint pass; UI integration/native appearance pending. Contract:
`docs/waveform-display.md`. No callback/schema changes or broader roadmap work.


## bracket/clip/waveform source milestone complete — 9 October 2026

Original bracket cursor variants, two retained macOS cursor objects, directional
fallbacks elsewhere, and a narrow test-only rendered-cursor request readout.
Verified original GPUI baseline and exact 12-file input/cursor delta; no unsafe
Hex code, proprietary assets or registry changes. Cursor delivery/scaling/hotspot
appearance remains native acceptance.

Audio/MIDI clips and prepared previews fill their 44px rows with a one-pixel
black border and warm fills. Shared black signed stereo waveform paths draw
zoom-adaptive detail from one query per column, with a bounded 4,096-column cap
and real sample traces at deep zoom. Grid lines remain aligned through fills
and beneath the waveform; empty-area feedback remains line/clip only.

235 UI tests pass, one ignored; workspace tests 700 pass, 1 ignored.
Media/session/UI strict lint, desktop build, formatting, architecture/style and
pinned GPUI provenance guards pass. Native paint timing remains unmeasured.
Review: `artifacts/hex-clip-visuals-20261009.app`; source/binary/log identity in
`artifacts/clip-visuals-manifest.json`. Check both bracket hovers, waveform zoom
and source detail, flush borders/grid continuity, and resize/drop previews.
User owns native checks; prior running apps/unsaved projects preserved.
No commit/push; broader roadmap remains paused.

## waveform paint correction and track context menu — 9 October 2026

The user reported a blank waveform in the clip-visuals review build. The shared
canvas had no intrinsic height, so its paint callback received zero-height
bounds and returned. Pinning the canvas to its parent fixes that layout path.
Removed the old 3.5-percent viewport-width waveform cutoff for normal and
resize-preview clips, and removed a crossing closure segment in the silhouette.
A rendered-layout regression checks full canvas bounds on 8px and 260px clips.
Native waveform appearance must be rechecked by the user; previous source checks
did not establish visible rendering.

Added a reusable compact rectangular popup with a fine border/drop shadow,
lowercase text, keyboard navigation/Enter, Escape/outside-click dismissal and
window-edge clamping. Arrangement row right-click offers only `insert track`,
inserting a hybrid track below the clicked row; empty-area right-click appends.
The new canonical `InsertTrack` command uses a stable optional insertion anchor
and one undo step. Generation/anchor checks refuse stale menu edits. Popup focus
restores on dismissal and timeline wheel/pan/seek capture respects the popup.
No submenus, extra track types, or broader roadmap work were added.

Tests cover rendered selection/dismissal/keyboard routing, popup clamping,
track order, append, missing-anchor rejection and undo/redo. Native rendering,
menu appearance and user workflow acceptance remain pending. No commit/push.

Verification: 747 workspace tests passed, 1 ignored; 239 UI tests
passed, 1 ignored. Strict UI/project lint, desktop build, formatting and
architecture/UI-style/GPUI guards passed. Separate review bundle:
`artifacts/hex-waveform-menu-20261009.app`; source/binary/check identity:
`artifacts/waveform-menu-manifest.json`. User checks: visible waveform while
zooming/resizing; right-click track/empty space; insert position; Undo/Redo;
Escape and outside-click dismissal. Running apps and unsaved work preserved.

## arrangement edit cursor and Command-E — 9 October 2026

Finite work unit: track clicks place a grid-aware pulsing one-device-pixel edit
cursor with steady top/bottom markers; Command-E splits the containing audio or
MIDI clip without reading/waiting for the audio playhead. Keep clip drag/trim
priority, transient cursor lifecycle, canonical validation/history and separate
transport behavior. Contract: `docs/edit-cursor.md`. Native acceptance pending.
MIDI split retains the left authored source and materializes the finite right
region into a one-shot; carry notes retrigger, and right loop/source settings
reset. Expansion is bounded and errors leave project/history unchanged. No
claim of exact Live split or seamless DSP parity.

Verification: 757 workspace tests passed, 1 ignored; strict UI and
project/session/storage lint, desktop build and source guards pass. Fresh review
app: `artifacts/hex-edit-cursor-20261009.app`; identity and logs:
`artifacts/edit-cursor-manifest.json`. Native click/pulse/markers, Command-E,
Undo/Redo and continued clip drag/resize checks remain with the user.

## next guided workflow: unified clip view — 9 October 2026

User requested a shared toggleable audio/MIDI clip detail view based on the
provided screenshots, with a compact left parameter section and a waveform or
piano roll on the right. Track it as separate targeted PRs after the edit-cursor
PR. First: visibility/selection retention, cached audio detail and existing
MIDI/source edits, plus canonical audio loop control. Then real per-clip gain,
pitch/playback-rate policy, warp/time-stretch modes and detail gestures/numeric
entry. The complete request remains open; do not claim full Live clip-view
parity or display unsupported controls as if functional. Plan and acceptance:
`docs/clip-view.md`.

Clip-view source verification passes. Native acceptance remains with the user;
audio detail currently fits the full source and uses step controls (1 ms, Option
one frame), not numeric entry or draggable markers. Source trims reset phase,
and one-shot play-start changes source in. The initial panel deliberately exposes
no inactive gain/pitch/warp controls; those remain requested model/DSP follow-ups.
Review fixed source-end marker/final ruler-label clipping at the right edge.

## clip edit responsiveness — 9 October 2026

User reports panel position/size flicker and slow resizing. Investigation identifies
repeated root paint/tessellation, a transparent original waveform plus resize
preview, repeated identical snapped candidates and pending control color pulses.
Finite scope and native acceptance: `docs/clip-edit-performance.md`. Overview
revision scans and control-side compile/history costs remain future measured
work; no callback/scheduling architecture changes are implied. MIDI handle
project captures are Arc clones, not deep project copies.

The smooth-edit fix is source-tested and a release app is prepared. User reports
text/status flicker specifically; pending control color pulses and shifting status
slots are addressed. Waveform preparation reuse is measured separately from
paint/GPU/input latency. Native verdict remains open; no faster-DAW or complete
100-track responsiveness claim is established by the component benchmark.

## arrangement time selection — 9 October 2026

Range paste replaces silent cells as well as clips across the full selected
rectangle and requires existing destination tracks. MIDI source loops are
materialized as finite one-shot gates; boundary carry notes retrigger. This follows
the existing split/overwrite policy, not a verified Live equivalence. Expansion
is bounded conservatively over intersecting source clips. Native selection/focus/
shortcut checks and sustained-note listening remain open; see time-selection.md.

Selection move originally intercepted all selected empty background, preventing
reselection inside the area. The requested upper-third hover handle now scopes
moving, while its lower two thirds allow a new selection. Within a selected area,
these zones take priority over individual clips. Native hover/pulse acceptance is
pending; no input/paint timing has been measured.

## cross-worktree Cargo cache identity — 9 October 2026

During local CI on the merged baseline, a shared `CARGO_TARGET_DIR` rebuilt
`hex-project` from the clean worktree. Returning to older dirty source in the
primary checkout reused that newer library timestamp and reported the new public
preview export missing. Rebuilding the primary library after refreshing its source
timestamp resolved the error; no API or source-content change was required.

Do not share a Cargo target between active checkouts for acceptance runs. Prefer
each checkout's default target directory. Keep commit/content identity in the CI
report and validate from the intended checkout; a warm cache alone is not proof
of matching source. The new runner does not configure a shared target itself.

## GPUI debug bounds retain removed selectors — 9 October 2026

The time-move audio regression initially treated `debug_bounds(old_selector)`
as proof that the original clip still rendered. The vendored GPUI
`WindowFrame::clear` retains its debug-bounds map; the test API reads that map
directly. Removed selectors therefore remain queryable across frame reuse.
Do not use selector absence after a redraw as visibility evidence. Check current
fragment/interval decisions and newly rendered geometry, and keep native source
disappearance as user acceptance. No GPUI dependency change is included here.

## time gestures stall outside visible lanes — 9 October 2026

User report in the combined editing/menu review build: held time selection stops
updating below tracks, over the browser and beyond window edges; fast crossing
moves fail to cover the full range. Window-level capture listeners already exist,
but updates require exact x/y containment in a fresh visible lane. Painting
freezes on a miss, while time-chunk movement clears its preview. There is no
horizontal edge autoscroll for these gestures.

The focused repair clamps projection to fresh visible lanes/existing row bounds,
uses raw x with the clamping time conversion, and retains capture until explicit
release/cancel. Use the existing bounded UI tick for held-pointer horizontal
scrolling and reproject on release. Preserve viewport scale and current limits;
no extra rows, callback work or project edits during dragging. Native macOS
outside-window event delivery/release remains a user check; headless simulation
cannot establish AppKit behavior. The existing pinned GPUI boundary stays intact.

Native acceptance, 9 October 2026: the user accepted the captured-selection
review build `hex-time-gesture-20261009.app` and authorized PR #17 to merge.
It is merged as `a250f83`; exact source `25b797e` passed all 14 local CI steps.
Pulse ticket 38 records this completion. This establishes the reviewed interaction
acceptance, not physical input/paint timing measurements.
