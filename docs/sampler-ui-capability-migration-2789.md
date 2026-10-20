# sampler ui capability migration

This is an **incomplete, unqualified component** of #2789 / PR395. It continues
the scalar panel staged at `ee14b363`. The current modern Main source connects
that panel to ephemeral scalar recipes, but does not establish a usable native
Sampler, playback admission or merge qualification.

## modern Main checkpoint, 20 October

The current source composes the reviewed UI ports surgically with modern Main,
preserving its selection-intent hooks, adaptive-grid quotas, Main/Return rails
and current EQ gesture code. The Sampler leaf is restored from `ee14b363` and
now has a real production instrument dispatch and width arm. It uses the actual
native borrowed descriptor decoder, not registry migration or a state getter.

Track/Return/Main panel recipes contain at most 65 devices, each with at most
512 copied parameter facts, and reserve title/inventory/parameter backing
outside reads. All mounted and hidden descriptors participate in reconciliation.
Native's opaque source key associates the two-pass recipe and its render target.
The persistent owning `DevicePanel.views` cache is removed; its remaining old
consumers are explicit compile failures. Track-gain retirement inspects every
actual read instead of skipping on equal generation/revision, so a new source
with a saturated revision cannot bypass retirement.

The shared finalizer now returns `Result<bool, SnapshotTransportError>`.
Transport pressure is detected before control retirement, widget draft
finalization or command submission. Save/Open/confirmation futures preserve an
already picked native response and retry it on their existing timer. Ordinary
initial commands show a retryable error rather than silently queuing new work.
The audio-event editor uses actual root/event facades, bounded Copy payloads
materialized outside reads, and the opaque source key for gesture currentness.
Its clipboard compares exact ordered domain points, not just IDs or counts.

The new event-segment waveform canvas captures weak EditorView plus IDs and
Copy display facts. It fills at most 4096 columns of preallocated peak scratch
through the current capability, ends the guard, then builds/caches geometry.
Prepared-waveform absence for an edited clip refuses display rather than falling
back to the raw asset. Cache versions use the actual native cache identity;
there is no pointer/hash substitute. Legacy owning stretch and ordinary canvas
callers remain unported. Existing raw-waveform receipt helpers do not yet
exercise these versioned entries and need nominal fixture coverage.

Three frozen partial checks are retained under
`artifacts/sampler-ui-migration/`: initial modern composition reported 647 actual
compiler errors, nominal waveform stage 581, and connected scalar recipe stage
560 (561 headings including Cargo's summary). The last run used HEAD
`247f16c0`, unchanged source SHA-256
`06a8cab6d5c02c1c2b6861ec2827ea39d87faa84480a82ffb84dbed4d098127d`. Its new recipe, shared bindings, track retirement,
parameter-row reconciliation, Sampler leaf, nominal waveform and the three
save/dialog and three audio-event files had no primary diagnostic. Three
mechanical connection errors from that run were repaired afterward and have
format/diff review only. Remaining errors include controller/Event/backend
owners, clip/trim/fade descendants, EQ, meters, comparison and old cache callers.
No current UI-source tests, pressure regressions, strict configuration, merge plan,
native input, playback or delivery qualification has passed. Existing focused
Core query tests passed separately; they do not qualify this UI source. The source layout
guard passes with the actual Sampler dispatch; it is not a typecheck.

Two further post-diagnostic acceptance corrections are source-reviewed only.
The owner-switch path calls the checked finalizer before spectrum/audition
clears and other switch mutations. A transport refusal returns a temporary
busy/unavailable panel using the existing shell, preserving the stored owner,
drafts, inventory and spectrum/audition requests. It does not invoke the
previous owner's mutating renderer or retain another tree/cache. Waveform requests now capture Native's actual
opaque source key from the audio-event data; both initial lookup and each paint
read verify it against the installed capability before any nominal query. A new
source with the same saturated revision cannot paint the old request. Geometry
cache reuse still depends on the actual waveform identity and mapping, not an
invented source token. These changes have scoped format, diff and layout-guard
checks, without another Cargo run or new UI test claim.

The next bounded stage ports Saturator/Compressor transfer previews through
actual borrowed native controls and fixed effective overrides (Compressor IDs
1/2/5/6/7, retaining Detector 8 in its display). Their caches now hold only the
fixed scalar curve, effective values and Native source key. They retain no
native state or patched draft. A shared read-only effective-value helper uses
the existing gesture/pending precedence while ignoring idle pending values
without removing them before a read. Ordinary render cleanup delegates that
same law after source preflight. Device-drag admission now returns transport
refusal separately from a stale target and validates actual slot class/type,
including Main/Return; its display-only bool wrapper does not mutate the drag.
The inherited drag payload still uses generation/revision, not a new source-key
contract, and no additional saturation/ABA guarantee is claimed for it.

The frozen check for that six-file stage at HEAD `372a933a` completed with
exit 101 and 542 actual compiler errors (543 headings), with unchanged source
SHA-256 `987602b03f0dc74489e911507ae71345a8cd78e06a86c3a0dcd4c0c591d1de22`.
Its raw `modern-transfer-drag` JSON/log/patch is retained alongside the earlier
checks. There were no primary diagnostics in the two transfer panels, shared
binding/value helper, scalar recipe, Sampler, nominal waveform, or newly ported
drag-currentness method. The three earlier connection repairs were also absent
from diagnostics. This remains a failed partial library check, not a passed
UI test, strict check, complete migration or native/merge qualification.

The sections below preserve the older b94-based component's source history;
their unconnected-dispatch and unavailable-query descriptions are historical.

The current WIP changes the types of the same four snapshot cells (`EditorView.state`,
`PendingPlayback.state`, `PendingResidentPublication.state` and
`PlaybackCandidate.state`) with the existing move-only
`SnapshotReadCapability`. A candidate is no longer `Clone`. The ordinary
pending-publication/candidate/audible transitions already move these cells;
additional retained reads are needed only at actual fanout points. This type
change does not complete intake or remove the old backend owners still present
in the partial controller/publication paths.

The initial consumer port uses the actual nominal queries published in
`evidence/hex395-event-reader-331` (`aad86d80171b9f5fd09ed52f8c59a11134cd5bac`),
plus the existing capability operations and fixed Sampler parameter query. The
actual minimal nominal mount is composed as `c30eafce`; the actual same-family
`follows_source` dependency is composed as `bc8372fc`. This query compares both
real reads under one family guard and must be called outside existing reads.
The finite MIDI/Session/audio/tempo consumer queries are published as `423fed8a`
and `8118b0bc`, with their actual public mount at `4cbdd1c9`.
It copies basic transport, track-name, audio-selection, main-gain and save/close
facts before GPUI or command submission. It adds no owning model, native-state
cache or child capability.

Read refusal is separate from absence. Track-name reconciliation and commit,
track resizing, saving, closing and track removal preflight reads before taking
drafts, changing focus, finalizing controls or submitting commands. An
unavailable read preserves the existing draft/gesture and refuses that action.
Audio selection returns `Result<Option<scalar facts>, SnapshotTransportError>`
so callers cannot interpret pressure as a removed selection. The dirty-state
readout reports an unknown read instead of claiming a clean document.

The musical-grid, Session-navigation and track-selection algorithms use small
internal query traits. Their production implementation accepts the actual nominal read;
`Project` implementations exist only under `cfg(test)` for the existing pure
fixtures. No map is reconstructed, boxed or cached. The musical conversions and
all grid/tick limits remain in the same algorithms.

MIDI piano rendering now prepares its existing scalar note-geometry cache from
borrowed note queries. It retains the prepared geometry only after the read
succeeds, then creates GPUI controls. The cache contains draw facts, not a
`MidiSource`, project or additional capability. Piano pan/zoom and note/group
edit paths preflight reads before consuming captured gestures. `visible_viewport`
accepts the caller's scoped read and computes pure geometry without reacquiring
the same family. MIDI draw strokes capture only bounded baseline note facts;
read pressure leaves an active stroke intact. Cursor maintenance, split, pan,
time-range capture/move/release and clip painting preflight their currentness
and projection facts before consuming a gesture. Time clipboard and export
admission distinguish refused reads from an absent range or selection, preventing
pressure from changing the action's target or output scope.

Ordinary waveform canvas captures now carry only Copy clip placement/source
facts, retaining the existing two-range source sampler and transformed-source
refusal. The stretch path remains owning and unported. Gain canvases retain
projected vertices and point markers rather than a project or prepared curve.
Routing and return controls copy their bounded row facts before GPUI. Captured
clip/sample/device drops use Result currentness and projection preflights before
clearing or committing an intent; display-only refusal helpers do not authorize
cleanup.

Additional scoped queries belong to the coordinated Session query lane. The
Native source-event dependency at `493d348e` is now surgically composed in
`4a8bd150`, preserving the UI consumer query child. It provides the actual
`SnapshotSourceEventReceiver` inspection/claim API and nominal Event metadata.
Ordinary Service receiver wiring, Initial production and backend publication,
completion, audible and seek associations remain open. There is no guessed
endpoint or legacy owning Event fallback in the new reader leaves.

The UI still contains unported readers and owning descendants, including
clip-content children, stretch projections, audio-event clipboard domains,
trim/fade origins and device preview/state caches. Generic device metadata
needs a bounded decoder: the old registry view helper migrates into a temporary
owning native state, so calling that helper inside a read would not repair the
ownership boundary. The fixed Sampler query already uses Copy parameter facts.
These are remaining work, not qualified exceptions.

Before receive side effects, reserve every required fanout read and copy the
required facts. Read-capacity pressure must retain the real head Event and allow
backend ACK/retirement work before a later retry. Releasing a read marks its ACK;
control collection later frees its slot. An immediate drop followed by derive
does not prove progress. Producer reservation must also leave UI fanout headroom.

The first partial-stage `cargo check -p hex-ui --locked --lib` failed, as expected,
at the missing nominal mount and unported field/clone/Event call sites. The raw
log is retained under `artifacts/sampler-ui-migration/`. Formatting, diff and
UI-style checks passed at that stage. No source tests, pressure regressions,
strict checks or merge plan have passed for this migration. A later retained
partial check at HEAD `8118b0bc` plus dirty diff SHA-256
`e4e426d83c75409f238d154a4aaefbdf0164dbfcbd0d3af38c2878893514ea5d`
reported 684 errors. A subsequent attempt completed with exit 101 at a Session
iterator lifetime error introduced and repaired during concurrent query edits;
it did not evaluate the UI and was not a frozen-source receipt. The retained
check at `44c6f187` plus dirty diff `fc24c9148cadb63c651534577bd89c60b50c8798380056ad1cd636c516078898`
reported 492 errors, including a new gain-pointer conversion error repaired with
`.ok()?`. The next check at `4a8bd150` plus dirty diff
`96820b5dab62289202c51ba2de7fee7df70253bc391bcfcda1d520355a0b46ce`
reported 457 errors; it no longer diagnosed that conversion or the new drag and
Copy waveform leaves. These are partial diagnostics, not qualification. They remain required
after the actual dependencies and complete reader migration compose. The earlier
leaf's UI2 + Engine2 + Session30 receipt applies only to that frozen leaf source.

No app was launched, stopped, installed or rebuilt in place. Native interaction,
load/playback/history/save/reopen acceptance and exact optimized delivery remain
outstanding.

## source-only checkpoint

This checkpoint publishes the UI diff for integration review. It is intentionally
incomplete. The 457-error diagnostic predates the later device Track/Return drop
preflights, musical-audio display/page queries and open/new/import/detail-pane
reader ports. Those later changes have formatting, diff and source review only.
The ordered-device and audio Copy queries are committed as `8e685caf`, before
the Native dependency composition at `4a8bd150`. Only the primary-owned
`has_assets` child addition remains uncommitted at this UI checkpoint and must
compose for compilation. No placeholder getter was added to the UI.

Remaining owning paths include current Event intake and backend publication,
`DevicePanel.views` and specialized previews, clip-content descendants,
audio-event clipboard/root state, trim/fade origins and stretch waveform keys.
Prepared preview/default-warp facade source and generic parameter decoders are
still dependencies. This checkpoint does not wire the Sampler parent dispatch
or claim that its existing leaf controls are playable. Existing source tests
and pressure regressions have not run on this WIP because the UI library does
not yet compile. No broad checks were run solely to publish this snapshot.
