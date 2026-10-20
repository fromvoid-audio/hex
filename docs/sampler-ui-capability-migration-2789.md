# sampler ui capability migration

This is an **incomplete, unqualified component** of #2789 / PR395. It continues
the scalar panel staged at `ee14b363`; it does not establish a usable native
Sampler, a connected panel, playback admission or merge qualification.

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
