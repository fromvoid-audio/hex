# graph-dominant eq layout

Pulse2800 / canonical PR406 follows the user's actual overlap screenshot and
request to make the curve editor and spectrum the main device surface. The
screenshot is retained in Pulse; its exact executable source was not verified.
Earlier2731/344 and2784/390 green source checks are historical, not evidence
that this screenshot was acceptable.

The existing600px device card now gives the graph the full body height and over
70% of its width. A160px side section groups global mode/output, bank selection,
analyzer/input/listen, and selected-band shape/frequency/gain/Q/slope controls.
Real shared numeric fields replace tall dial rails; canonical codecs, invalid
entry refusal, focus, gestures and history stay with their existing bindings.
Long channel names have a72px selector and selected numeric values use104px
fields. Normal controls fit; extra automation information scrolls independently
while response/remove actions stay pinned. Choosing automation replaces the
selected-band fields with existing lane controls; returning restores them.
There is no permanent top/bottom control rail or new musical/DSP law.

The shared graph's optional normalized axes reserve one plot rectangle for
curve/analyzer painting, nodes, hit radii and pointer capture. Owners without
axes retain the original seven-pixel inset. EQ's logarithmic frequency helper
is shared by tick positions, authored response samples, nodes and spectra.
Major/minor ticks follow the current rate's upper display limit, with measured
label spacing; blank minor labels do not consume text space. The left scale is
-24..+24db response gain; the explicitly separate right scale is -96..0dbfs for
actual spectra. No analyzer data is fabricated. The64-node and256-response-point
limits and current authored response cache/DSP/automation state remain intact.

The disposable layout catalog adds a separate sparse EQ entry, preserving
legacy eq8. It uses the same production graph/sidebar geometry, axis source and
real NumericControl/Selector/ResponseGraph. Its registered control-side response
changes when its local controls change. It is explicitly a fixture with no audio,
project worker or preferences access; its analyzer layers remain absent. Actual
production-panel tests, not this catalog, govern canonical editing/history.

## analyzer boundary

The existing audio client supports one spectrum target. Spectrum intent defaults
on for each sparse EQ. Deliberately selecting a card or focusing its controls
chooses that EQ through the existing admitted-revision/rate/target API; merely
mounting or hovering another panel cannot steal it. Explicit Off is local to the
EQ and survives ordinary track/return owner switches; project replacement resets
view intent, and removed or no-longer-EQ identities are pruned. This is view state,
not a new persisted preference, schema or history command. Existing legacy eq8
controls are unchanged.

Only an available current project audio publication can activate capture. Target
changes clear the cached UI result; the original identity/token/generation/rate/
activation-floor checks still filter actual worker frames. No stale frame is
rebranded as the selected EQ, and no analyzer data is fabricated. When audio is
unavailable, the enabled intent remains visible and the toggle explains that it
waits for current project audio. No new audio owner, callback or worker is added.

A bounded test injects the actual public spectrum target control channel, drives
real two-card focus and Spectrum clicks, and checks token/epoch retirement and
canonical removal. Its fixed admitted floor/rate are explicitly fixture context;
it does not construct a guarded Client, open an audio backend, capture samples,
or prove native worker/admission/sound behavior. The offline panel tests also
verify that intent changes do not edit the project or invent spectrum results.

## original evidence and limits

The bounded fixture uses PR363's merged pre-discovery constructor with private
disposable browser/preferences/cache paths, never ambient libraries/settings.
The original production graph regression atcbcb5e21 failed with a592x74px graph
inside the600x168px body (20261018T154203Z-33469887). Earlier font-registration
and missing-selector setup failures are retained separately and are not layout
proof. A later broader run retained7passes/3failures, exposing clipped automation
actions and control-test assumptions; the repair keeps actions reachable.
Compiler, guard and lint failures are also retained with original source IDs.

Linux GPUI's test text system cannot register bundled Ioskeley: real-font metrics
are explicitly reported UNSUPPORTED there while all geometry/input assertions
still execute. The same native production fixture requires successful bundled
font registration. Linux bounds checks are not pixel/GPU/font acceptance. Final
exact-source focused summaries and all original logs are linked in PR/Pulse;
the dedicated merger owns final prospective-tree full local CI and integration.
No hosted CI or Mac/app/backend/permission operation is performed by this source
work. User physical input/listening/real capture and optimized delivery remain
root/coordinator-owned and open.

## finite optimized native checklist

Use only the coordinator-selected immutable source and serial Mac lane. Preserve
the installed app, receipts, user files, settings and TCC state. These headless
fixtures require no app launch, backend/input capture, actual MTS load, permission
prompt or user data. Use the default native stack and record positive test counts:

- `cargo test -p hex-ui --features ui-test-support --locked eq_ui_tests -- --nocapture`
- `cargo test -p hex-ui --features ui-test-support --locked 'eq_panel::' -- --nocapture`
- `cargo test -p hex-ui --example layout_review --features ui-test-support --locked eq_catalog_ -- --nocapture`
- Relevant shared response-graph/numeric tests and strict feature Clippy on that
  actual candidate, as selected by the coordinator; no old-source relabeling.

Later with the user present in the qualified optimized app, inspect the real EQ
at supported compact/expanded window sizes and the screenshot's aspect. Record
actual before/after pixels, bundled font, popup/hit bounds and selection behavior.
Check all channel/filter/slope choices,20khz/+24db/Q18 and output units, pointer
and keyboard popup selection/cancel, typed valid/invalid/cancel, Undo/Redo and
current-format save/reopen. Confirm grid labels align with curve/node positions,
response and analyzer scales are distinguishable, and automation leaves graph
geometry and essential actions stable. Start audio only through the coordinator's
user-present plan: confirm real pre/post spectrum data/default/toggle behavior,
track/device switching, two-EQ deliberate selection/focus, explicit Off and
return ownership, then listening. No sound/parity or
physical deadline claim follows from headless UI tests.

## returned status and font-fixture repairs

The original9f56 source is preserved with its source reports. Independent review
found two real status omissions, reproduced by `eq_analyzer_status_` tests:
closing an actual guarded public Client refused with `OutputClosed` but left no
UI status, and focusing an intentionally-Off second EQ retained the first EQ's
waiting message. Selection now handles the typed refusal, clears previous tap
pixels and reports a lowercase error. Off/success retire only analyzer-owned
status, preserving unrelated save/edit messages. Intent and target lifecycle,
frame filtering, canonical project/history and backend ownership are unchanged.
The closed-client regression uses the existing dev-only headless constructor;
it opens no OS device and renders no samples. It does not test real shutdown.

The first actual native9f56 fixture run matched15 tests: one shared-width test
passed and14 EditorView cases failed at bundled-font registration, before any
editor geometry was constructed. GPUI's default TestAppContext uses
NoopTextSystem even on macOS; accepting bytes there cannot establish registration.
The corrected fixture explicitly installs the existing macOS text system into
both App and test-context shaping references before opening any window, retaining
the strict embedded-family assertion. Other test contexts remain unchanged.
The opt-in rejects replacement beneath live windows. Its native compile, actual
embedded registration/shaping and layout bounds require new-source Mac evidence;
Linux green cannot establish those facts. No system fonts are installed.

The additional production rail test checks actual control-row/field separation,
plot/sidebar geometry, slope hit testing, all popup choices and keyboard cancel
at960x600,1314x600 and1800x900. A deliberately short1314x426 whole-window stress
case exposes the rail extending beyond that viewport, which is below the actual
production960x600 minimum. Its original failure and exact bounds are retained;
it is explicitly unsupported for input acceptance. The screenshot is a cropped
1314x426 image with unverified display scaling, not proof of that logical app
window size. No production minimum or parent layout was changed to satisfy it.
All supported-size input assertions remain active. The user's visual rejection
remains open until actual redesigned-source native pixels and interaction are
reviewed; these tests do not establish visual acceptance.

For the repaired immutable candidate, the coordinator first runs
`cargo test -p hex-ui --lib --features ui-test-support --locked eq_native_bundled_font_test_context -- --nocapture`
(one positive native-only test), then the affected `eq_ui_tests` and strict
feature Clippy. `eq_analyzer_status_` selects the two new semantic regressions;
`eq_production_rail_screenshot_sizes_` selects the additional real control/menu
geometry case. Existing model/catalog/widget evidence remains historical on its
original source until included in the merger's actual candidate qualification.
No old native report is relabeled, and no user app, backend, TCC or MTS operation
is requested by these disposable tests. Shared device outline2829/433 and future
shared spectrum fills2832/436 are separate source work.

## selection, document and edge-hit review return

The original24c62 native19 pass and the merger's combined2b1f51d1 full20 pass
remain evidence for those exact sources. Fresh review found three additional
omissions; the new `eq_review_` cases reproduce them on2b1 before repair:

- An actual non-EQ header selection left the previous sparse EQ's public target
  channel active, even after selection had changed.
- New replaced the document with an empty project but retained the prior analyzer
  waiting message. The replacement had no EQ card to run a panel-local cleanup.
- An actual press four pixels beyond a boundary node was rejected in the axis
  margin before the existing eight-pixel node-radius test ran.

EQ-owned reconciliation now runs at the existing controller target-retirement
hook and the shared panel's document/selection boundary. It retires sparse-EQ
capture and cached pixels when the selected device is no longer an enabled sparse
EQ, and clears its owned status on document-generation replacement. It preserves
per-EQ Off intent within a project, unrelated save messages, legacy graph owners,
the existing public target channel and its admission/token/generation rules. It
creates no new analyzer owner, worker, callback or persisted state.

Node hit-testing now measures the actual pointer's distance before rejecting a
blank axis press. Value and drag coordinates retain their existing clamps. Tests
click and drag the outward halves of20Hz,20kHz and±24db nodes, preserve their stable
identity/kind/Q, verify canonical Undo, and still refuse blank-axis creation.
The generation test exercises both actual New and disposable-file Open, with
waiting and real closed-public-client status, preserving a newer unrelated save
message. The ordinary-device test uses the real pointer selection path and public
channel retirement, not a synthetic selected-device assignment.

The private graph accessor used by these tests is the same test-only accessor as
PR407; integration must keep one definition. PR407's pending Option-Q raw-delta
and momentum repairs remain separate. This repair preserves the merger's combined
GPUI font/quad support unchanged and does not modify Open/changelog/controller
handlers outside the declared retirement hook.

The finite new native selector, on the coordinator's immutable actual candidate,
is:

```sh
cargo test -p hex-ui --lib --features ui-test-support --locked eq_ui_tests::graph_layout_tests::eq_review_ -- --nocapture
```

Require three positive cases and zero ignored; the inherited bounded fixture
still requires the actual bundled font on macOS. Original cloud reds, fixture
compile/selection-setup failures and subsequent greens remain source-bound.
This command uses private temporary projects and headless public channels; no app,
OS audio device, permission or user-file action is needed. Do not blindly repeat
the old native19 for unchanged font/geometry source. Physical edge targeting,
optimized pixels, real capture cessation after selection and listening remain
coordinator/user acceptance, not claims from these simulated cases.

## setup and retry ownership review return

The later published2547 integration passed its full20 checks but fresh review
returned three additional status/target paths; those older results are retained
with their original source and do not resolve the return. New controlled real
UI regressions reproduce hidden stored setup errors, closed-channel retries
replacing a newer unrelated save error, and Off sparse EQ selection clearing an
independently active legacy EQ8 public target. The first run also exposed a
private-fixture collision at the existing exclusive headless-output lease;
that is recorded as fixture failure, not proof of the setup-reporting defect.

Analyzer context now distinguishes no output/pending publication from terminal
stored spectrum setup failure and the existing unsupported-sample-rate reason.
The concrete setup reason is retained in view-local owned status; no backend is
opened to diagnose it. Reporting may update an absent/analyzer-owned shared
message, but repeated render/retry cannot overwrite a newer unrelated message.
Dismissing the unrelated message allows the current analyzer failure to appear
again. Explicit Off and document-generation retirement still clear only owned
status, preserving unrelated recovery errors.

Off clears only the exact sparse processor owner/device/type/generation. A
legacy EQ8 target remains active when an intentionally-Off sparse EQ is focused;
deliberately enabling the sparse EQ may choose its own target, and disabling it
then immediately retires its own accepted public callback target. No target
protocol, audio capture, worker, callback, controller or engine redesign is made.

The private tests use the existing guarded headless constructor/public target
channel; they never open CPAL, request a device, render a callback or change OS
permissions. Setup/rate/pending metadata uses the actual EditorAudio carrier,
with no fabricated analyzer frames. All four fixtures using the process-local
exclusive output lease serialize with one private test mutex and bounded waiting
for the existing retirement worker after previous fixture Drop. No environment,
settings, production lease guard or audio lifecycle code changes. Fresh app
contexts preserve strict native bundled-font selection before each first window.

The new finite native request is the three `eq_status_review_` cases plus the
existing New/Open-six-variant case because its private headless-constructor
helper changed. Require four unique positive tests/zero ignored, all three
setup/rate/pending variants and all six New/Open variants. Do not repeat old
layout/font19, already qualified407six or436four absent a relevant source change.
Actual backend setup/OS device failure, physical UI/pixels/listening/optimized
app acceptance remains coordinator/user-owned and is not simulated as accepted.


### same-project audio recovery status

The recovery regressions reproduce both a stored setup failure and a real
closed public control endpoint surviving after `recover_audio` removes the
old output without changing the project generation or selected EQ. Pending
context now retires only the analyzer-owned status and old spectrum pixels.
A newer unrelated save/edit message and a current explicit-On waiting notice
remain intact. No new target is claimed
before the replacement endpoint's current project graph is published; repeated
pending-publication renders cannot revive a retired endpoint's error.

Two private status tests exercise actual recovery, then a replacement guarded
headless endpoint pending publication, each with and without newer save feedback.
All four variants retain the project, selected EQ and enabled intent. They use
fresh app/font contexts and the existing serialized headless lease, not an OS
audio adapter or rendered callback. Native follow-up, if scheduled, needs only
`eq_status_recovery_` (two unique positives and all four completed variants),
not a repeat of unchanged layout, gesture, spectrum or earlier status suites.
Physical audio recovery, visual quality and listening remain unaccepted here.
