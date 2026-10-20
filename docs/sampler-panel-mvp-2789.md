# sampler scalar panel component

The staged MVP panel consumes the agreed fixed array of 43 copied parameter
facts after the caller has ended its retained snapshot read. It retains no
project/device state, sample, waveform, resource capability or read stamp.
Existing parameter controls remain responsible for guarded edits and gestures.

The first panel shows source start/end and reverse, Classic loop enable/bounds,
root/transpose/detune, voices, gain/pan/velocity and amplitude ADSR. Units and
mode-specific enabled flags come from the existing metadata. Inactive modes do
not show Classic loop controls. It exposes no loading action, fake waveform,
registry admission, extra mode, filter/modulation or warp claim.

Four shared rails use existing 136 px standard numeric controls, a two-by-two
source-flag group of 64 px compact dials, and 40 px compact buttons inside a
594 px body. Source flags use the existing Number codec under the explicit
`source · frames` heading; their canonical frame values and numeric entry stay exact. Leaf fixtures build real widgets from the actual
production section recipes and metadata descriptors, then check containment at
620/1200 px window widths, numeric display strings/units, typed input events, disabled
controls and inactive loop bounds. These fixtures own no Service or media worker;
they do not establish canonical command/ACK, host playback or native acceptance.

The exact four-file scalar-query prerequisite was published as
`1d5b89a2f658a22cb9c1f76c5bfc1631326e192d` on base
`b94b0e2f6fbec0932d422c00d4584739c192cb9a` and imported onto this component branch.
It adds Engine metadata decoding and the nominal `SnapshotRead::sampler_parameters`
method without opening the registry.

The modern Main component now mounts the production Sampler instrument
renderer and its 594 px frame. Track, Return and Main rails prepare ephemeral
scalar recipes through the actual retained capability and strict native
`controls()` decoder. Each recipe contains IDs, class/type/bypass facts and
copied parameter descriptors/values; its allocation backing is reserved outside
the read. The read ends before shared controls or GPUI run. No owning device
state is decoded, cached or supplied to the Sampler leaf. The former persistent
`DevicePanel.views` cache is removed. Recipe association uses Native's opaque
source key, including when project revision values are saturated.

This connection is incomplete and unqualified. The frozen partial library check
on 20 October reported 560 compiler errors across the remaining UI migration,
including three mechanical connection errors subsequently repaired: a missing
Return sample-rate argument, a field accessed as a method and a fallback body
missing its element ID. The new scalar recipe, shared bindings, Sampler leaf
and parameter-row reconciliation had no primary diagnostic in that run. The
repaired source has scoped formatting and diff review only; this is not a clean
UI build or a canonical binding/ACK test receipt. At that diagnostic, specialized EQ, transfer-curve and audible meter adapters
and remaining old cache consumers were still unported. Their compile failures are retained rather than hidden behind
raw-state fallbacks.

After that diagnostic, the owner-switch checked finalizer was moved before
spectrum/audition clears or switch mutations. Transport refusal displays the
existing temporary busy/unavailable shell while preserving stored owner,
drafts, inventory and spectrum/audition requests; it does not run the previous
owner's mutating renderer or retain another tree/cache. Event waveform requests now carry
the actual opaque source key and reject a different installed source before
every initial/paint query, including equal saturated revision values. Those
bounded corrections have scoped format/diff/layout-guard checks only. No current
UI-source tests have run; existing focused Core query tests passed separately.

A later bounded stage replaced the Saturator/Compressor owning state previews
with actual borrowed native transfer queries and scalar/source-key caches,
sharing a read-only effective-value helper with the existing bindings. The
frozen partial library check at `372a933a` reported 542 compiler errors elsewhere
and no primary diagnostic in those panels, the Sampler recipe or new helper.
The earlier three connection errors were absent. EQ, audible meters, remaining
readers and controller/backend ownership still prevent a clean UI build. This
failed check adds no UI test, native or merge qualification.

A further frozen component ports Operator metadata, finite browser/Console drag
reads and device comparison to the same nominal boundary. Comparison keeps its
menu/focus on read or finalizer refusal; pointer-down/drop preflights retain the
captured input. Its partial library check reports 493 actual compiler errors,
with none primary in Operator, comparison or those newly ported hunks. Owning
group previews, remaining editor consumers, actual registered UI fixtures and
Native startup/backend closure remain explicit dependencies. No pressure/UI
behavioral test, clean build or usable delivery is established by that diagnostic.

The exact Merger UI delta through `96ff579e` is also composed without replacing
core or local work. The following navigation/Return-menu/loop-scratch stage has a
frozen partial check with 453 actual errors, none primary in its new hunks.
Loop membership scratch no longer frees tree nodes inside a snapshot read;
allocation instrumentation and real nominal UI fixtures remain unexecuted.
This is continued source integration, with startup/playback/full qualification
and optimized delivery still open.

The next loop-rail read port retains the original mouse-up through snapshot
Busy, restores prior pending-loop display after refused submission, and checks
its gesture incarnation before retry. The partial library diagnostic reports
441 actual errors with no primary loop-rail diagnostic. Automatic retry of UI
queue capacity remains explicitly incomplete until enqueue returns typed
admission results; permanent refusal must not be retried as if it were Busy.
Its pure nonce test and actual registered UI pressure tests have not run.

A subsequent controller continuation connects typed actual admission outcomes,
including device defer/barrier capacity. The loop now retries only actual queue
capacity or snapshot Busy, retaining its original release, while semantic/closed
refusals stop visibly. Bool callers and FIFO laws remain unchanged. Its frozen
partial diagnostic still reports 441 errors elsewhere and no primary error in
new admission code; actual queue-pressure/release/ACK tests remain unexecuted.

The production device-composition source guard passes with the real instrument
dispatch. Earlier staged leaf tests (UI2, Engine2, Session30) remain evidence for
that earlier immutable component only. The earlier feature-only mount and its
dead-code/guard failures are historical, not the current integration shape.
Canonical edit/ACK, pressure, history, loading, waveform, native playback and an
optimized usable build still require the completed UI/Service integration and
new exact-source verification. No native app operation has been performed.

## source readout measurement

The initial 46 px standard numeric value field left about 40 px after padding
and borders. A headless CoreText query of the bundled regular Ioskeley Mono at
9 pt measured `16777216` at 43.2 px, so removing units alone could not make the
maximum frame coordinate fit. The source flags now use existing compact dials:
actual rendered readout bounds are 64 px, leaving about 58 px for that exact
unit-free number. No binder/widget extension, smaller font, rounded numeric
codec or changed source-frame domain is used.

GPUI's test text platform did not register the bundled family through the normal
font loader; that failed probe is retained. Leaf fixtures request the product
family but establish component geometry and numeric display strings only. The
independent CoreText measurement above uses the actual bundled font, and native
paint/text interaction remains a separate acceptance gap.
