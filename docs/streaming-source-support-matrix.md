# Streaming source and provider support matrix — Pulse #2536

Planning snapshot: 15 October 2026. This document inventories source and evidence;
it does not enable streaming, expand a provider API, or declare #24 complete.
Pulse remains the planning authority. The original intake was planning only;
the user subsequently authorized merging this documentation deliverable. That
authorization does not enable runtime implementation, release #24's review hold,
or establish native acceptance, deployment or goal completion.

## Provenance and status vocabulary

Live reads verified Hex project 2/version 2 (0.1), PORTALSURFER/Pulse user 1,
no prerequisites for #2536 revision 9, and its existing open draft
[PR231](https://github.com/PORTALSURFER/hex/pull/231).
Its branch is `pulse/task-2536-g1-287a855275f5`, intake head
`29d896fc6116377cf2662470a3a3519230141710`, base
`1c22d2f44f8faf10c032533528d04d071fcbb3f6`. That head contains only its intake
plan above the base; the newer streaming implementation is not integrated there.

Canonical streaming source was inspected separately at [PR177][pr177] head
`8d444c06c295e52a8aa0dde275a89ecec6f454e2`, tree
`f15b03eb52b8b9f17eefddd68d7267687e58b7da`. GitHub reports PR177's base as
`5785bf7c47c5c8a168cfec33bfe1ecbe3aa72c1c`; Pulse's historical workflow base is
`e0780afe122fedb751a31941ad8cc56a49180f5a`, while its live review reports current
base `1c22d2f44f8faf10c032533528d04d071fcbb3f6`. These are different observations,
not interchangeable tested integrations. Original unpublished Arrangement-root
WIP is unavailable here and remains unverified in its original worktree.
Agree its frozen source/integration checkpoint before #2537 implementation.

The [primary Mac report][preview-pass] records FULL16 PASS on **8d444c0 only**,
run `20261015T090126Z-c48a725c`, clean/stale=false, plus bounded preview review.
It accepts the preview stage, not end-to-end root rendering, complete accounting,
native streaming or this document. Its underlying local logs were not available
for independent rerun in this Linux audit. Earlier failed/unverified comments
remain history; the later exact-head report resolves that preview verification.

Each matrix separates existing behavior from remaining work:

- **I — implemented/evidenced:** named source or historical scoped test evidence.
  This does not imply complete claim admission or native acceptance.
- **M — missing source:** a concrete constructor, accounting, lifetime or
  integration obligation remains. Existing `Unknown`/`Incomplete` refuses.
- **N — native-only:** actual optimized Mac/device/operator evidence is required
  after the corresponding source works.
- **D — unresolved product decision:** a named decision with no inferred approval.

No row removes a currently supported source from #24. The initial class is a
staging subset; every broader source below remains an obligation or an explicit
decision. None of these labels is implemented by filtering diagnostic strings.

## Exact initial class

The [owned preparation preflight][preparation] requires a nonzero generation,
no regular-track devices, no instrument slot (even bypassed), no Arrangement MIDI,
and no Return devices. [Session source preparation][session-preparation] rejects
MIDI cells and admits audio/Stop; structural empty cells/scenes remain valid.
Thus an audio track with its default instrument still present is not eligible
merely because it has no MIDI notes. Gain automation, track/master mix, fades,
source phase/Arrangement loops, empty-device Returns, sends and the supported
Return-feed DAG remain represented by the existing preparation stages.

At 8d444c0 `with_audio_preview_inventory` composes actual private clip/Track,
Return/feed, Session inventory and empty-preview stages. It does **not** construct
the final Arrangement Project, shared preview root or complete PreparedRevision.
The [root handoff][root-handoff] is a proposal, not published implementation.
Mandatory sine/context/meter owners and empty allocations still cost storage
even with no authored instrument or device.

## Public source and processing matrix

| Source class / public ingress | Existing behavior and evidence | Remaining scope / disposition |
| --- | --- | --- |
| Resident Arrangement audio: `Audio::new` → `Clip::{new,region,region_with_fade}` → `Track`/`Project`; canonical `compile::project*` | **I:** validated nonempty finite PCM, shared immutable buffers, overlaps, trim/phase/fade/loop/mix. `compile_project`, `clip_fades`, `track_mix`, `loop_playback` tests. [Engine][engine], [compiler][compile]. | **M:** legacy/raw graph roots cannot be upgraded into complete claims. Owned constructor and source-slot stages exist; final roots/accounting/ending lifetimes #2537–2541 remain. **N:** resident continuity during streamed edits/replacements #2546. |
| Streamed Arrangement audio: `PreparedAudioSource::Streamed(Arc<StreamedSourceMetadata>)`; `Clip::streamed_region*`; `compile::project_with_prepared_audio_sources*` | **I:** metadata/generation validation and borrowed `MediaPages` rendering; `compile_streamed_sources`, `streamed_render`, `observed_pages` cover mixed sources, loops, local cold misses and malformed pages. [Compiler][compile], [pages][page-contract]. | **M:** arbitrary metadata Arcs have no owned declaration; owned value constructor is separate. Verified catalog pairing is not page installation or complete native admission. #2537–2544; **N** #2545–2546. |
| Resident Session audio: `PreparedSessionInventory::prepare_with_media`, resident `PreparedAudioSource` | **I:** independent frame-anchored one-shots, quantized launch, exhaustion retaining route, Stop/BackToArrangement/handoff. `session_audio_large` and shared Session tests. [Inventory][inventory]. | **M:** inventory/root aliases, source bank, dormant source handles and final disposal must be accounted, including Session-only assets. **N:** mixed launch/edit/reopen continuity #2546. |
| Streamed Session audio: `prepare_with_prepared_audio_sources`; prepared revision mixed bank | **I:** shared resolver and page renderer; `session_streamed_audio` includes one-shot, exact launch, short/wrong pages, seek/Stop/handoff, loops and FM4/device mixing. [Inventory][inventory], [Session tests][session-tests]. | **M:** same root/catalog/handle gaps as Arrangement; headless launch does not establish readiness. #2542–2544 must use actual engine demand. Session audio looping/warp is **D**, separately specified, not silently invented by frame looping. |
| Hybrid audio + MIDI Arrangement and Session routes; explicit Silence/Stop, empty tracks/scenes/project | **I:** common track IDs/mix and source-specific schedules; empty and silent paths are valid. MIDI with no instrument remains silent without deleting authored extent. [Revision][revision], [inventory][inventory]. | **M:** supported mixed graphs remain outside the initial owned audio-only predicate. Empty/silent/default containers retain real allocations and aliases; no zero-cost or retirement inference. **N:** route/Stop correctness under streamed pressure. |
| Sine instrument, analytic notes in Arrangement/Session | **I:** registered `hex.instrument.sine`, registry preparation returns a Box; `PreparedSineInstrument::control_storage_declaration` declares its exclusive sample-rate pointee. Static OnceLock sine samples are inline program storage. `registered_instrument` tests. [Built-ins][builtins], [provider contract][provider]. | **M:** that narrow declaration does not cover trait-object Arc allocation, canonical state, banks/templates, graph sine Arc, Weak or lifetime union. Existing #16 owns runtime scope; audited ownership work must be separately scoped before broader claimed admission. **N:** listening. |
| FM4 instrument, analytic compact note payloads / live notes | **I:** registered `hex.instrument.fm4`; `Fm4Definition::prepare_instrument` owns rate/oscillator/routing/envelope values; `fm4_native_admission` and streamed hybrid tests. [FM4][fm4]. | **M:** inherits default `PreparedStorageDeclaration::Unknown`, despite apparently fixed fields. No size-based auto-approval. Audit exact provider/state/Arc/bank/context paths under #16; native evidence separate. |
| Operator (displayed as vector), shared analytic mode | **I:** registered `hex.instrument.operator`; no mutable feedback/LFO factory selects the shared context path. Prepared provider also owns authored state/configuration. `operator_native_admission`, context tests and existing #16 history. [Operator][operator], [contexts][contexts]. | **M:** provider defaults Unknown; inner context trait-object Arc, `SharedContextOwner`, configuration/state and separate context-table handles require exhaustive reporting and lifetime custody. Context payload declarations alone do not close graph ownership. **N:** listening/current-build acceptance. |
| Operator mutable feedback and mutable LFO modes, held/released voices | **I:** `prepare_instrument` prepares optional feedback/LFO factories; actual modes must follow factory selection, not the displayed name or raw feedback field. Paired runtime/event/chase/loop and retarget infrastructure exists. [Operator][operator], [mutable][mutable]. | **M:** factory/proof/state/provider owners, both runtime twins, occurrences, old suffixes and retained contexts remain opaque to complete storage reporting. Reuse #16/#2164/#2165/#2229 rather than reimplementing DSP. **N:** held/released seek/loop/live-edit listening/deadlines. |
| Empty device chains, including no-effect Returns and zero-input routes | **I:** `NativeRuntimeChain::empty_control_storage` reports actual empty-entry capacity; private stages construct separately declared empty processor slices and meters. [Lattice][lattice], [Track assembly][track-assembly], [Return assembly][return-assembly]. | **M:** `Arc::from([])`, default/empty-Vec and arbitrary incoming empty slices are ambiguous origins. A bypassed/nonempty matrix-only chain is not an empty chain. Complete roots and handles remain #2537–2538. |
| Registered Utility, EQ8, Saturator, Compressor, Gate, Auto Pan; ordered prefix/suffix, track/Return/feed paths | **I:** all six are in the actual built-in effect registry; canonical compiler owns matrix/processor/configuration/meter assembly, shared render/export and residual-tail logic. [Built-ins][builtins], [compiler][compile], [lattice][lattice]. | **M:** processor trait-object owners, state/meter Arcs, entry storage, runtime twins and retained tails have no exhaustive complete report. Even memoryless/bypassed owners count. #16/#2162 own existing lifecycle semantics; broader accounting is additional scoped work, not initial-class support. **N:** audible tails and native deadlines. |
| Musical typing/live input, active and tail `TypingPerformance` | **I:** prepared arm, bounded notes/commands and actual `Weak<PreparedRevision>` admission link; `typing_arm`, `typing_foundation`, musical-typing and allocation tests. [Typing][typing]. | **M:** arm/provider/note/context/state ownership and Weak-only allocation lifetime are explicitly unresolved. Account current and every tail Box independently. Preserve existing PR190 typing/meter behavior in #2541. **N:** typing across streamed seek/launch/edit/Stop. |
| Public custom registries/providers; feature-gated mutable test instruments; manually built Project/Playback; generated demo | **I:** public registry/renderer seams and conditional fixtures are intentional existing callers. [Compiler][compile], [provider][provider], [engine][engine]. | **M:** untracked/raw/custom origins remain refused by complete admission; fixture origin remains explicitly INCOMPLETE. Existing resident/borrowed APIs remain supported. **D:** define opt-in declaration contract/eligible extension before admitting other providers; no API expansion is authorized here. Demo output is not long-project evidence. |
| Export/offline jobs and retained history/open generations | **I:** `hex-export::render_prepared_new` shares prepared Arrangement behavior; source/catalog APIs support independent control ownership. [Export][export], [catalog][catalog]. | **M:** prove actual original-generation leases, history/caller aliases, worker lifetime and retained/construction union across edit/Open/export. A borrowed page renderer is not native export wiring. **D:** persisted fingerprint/relink migration guarantee remains explicit open policy in [page delivery][page-contract]. **N:** concurrent export/save/reopen while native playback advances. |

These rows cover the public source-kind alternatives (resident/streamed audio,
analytic/shared/mutable MIDI, live typing and silence), their Arrangement/Session/
offline consumers, both public compilation and manual origins, and all registered
built-ins. Processing rows apply to every source routed through that chain;
mixing a supported source with an unresolved owner does not erase the owner.

## Constructor and ownership/accounting manifest

| Allocation / handle family | Exact provenance at the inspected checkpoint | Accounting boundary and required closure |
| --- | --- | --- |
| PCM slice / caller source aliases | `Audio::new` records a receipt at validated nonempty Vec→Arc; `OwnedAudioSource::try_resident` refuses unsupported layout before consuming the original Vec. Bank aliases are created only into reserved bank slots with distinct handle IDs. [Inputs][inputs]. | **I:** source-bank data dedup and separate unresolved handle obligations in `prepare_input_storage_on_control`. **M:** caller/clip/Session/history/root union. Equal samples in distinct allocations remain separate. |
| Stream metadata | Legacy caller `Arc::new(metadata)` is untracked. `OwnedAudioSource::try_streamed(metadata)` makes a new `OwnedShared` allocation from the value. | **I:** owned metadata declaration and bank obligations. **M:** association with actual catalog/source and every clip/cache/caller handle; no raw Arc adoption by matching values. |
| Sized roots / nonempty slices / explicit empty slices | Private `OwnedShared::new_owned`, `from_owned_vec(nonempty)`, and `new_owned_empty_slice` pair actual constructors with receipts. The last allocates a sized zero array then unsizes that same Arc. `untracked`, Default and empty Vec conversions remain Unknown. [Owned shared][owned-shared]. | **I:** assumption-bound requested layouts only on audited Rust 1.94.0 commit `4a4ef493e3a1488c6e321570238084b38948f6db`, supported official native Linux x86_64/Mac arm64 gate. **M:** retirement receipts; no allocator rounding/RSS or measured alignment claim. |
| Initial sine/context handles, meters and empty slices | `SessionSharedConstruction` reserves sine + two distinct context handles: two allocations, three handles; both context handles alias the same strictly empty allocation. Track/Return assembly pairs actual meter/empty-chain owners with declarations before moves. [Shared preparation][shared-preparation]. | **I:** exact staged identities and checked reservations, including empty/spare refusal. **M:** transfer/restoration into final roots and independent obligations. Static provider sine lookup storage is different from the allocated graph sine table. |
| Arrangement, Session, preview and PreparedRevision roots | Legacy revision preparation wraps values with `OwnedShared`; public child Arc getters/Clone allow aliases. `OwnedPreparedRevision` exposes restricted views/control clones but its existence alone supplies no complete ledger. [Owned revision][owned-revision], [revision][revision]. | **M:** original #24 Arrangement-root WIP → #2537 remaining shared-root assembly → #2538 exhaustive report. Nine Project fields, actual Track/Return/feed/context destinations, preview root and mutable descriptors must all be covered. No `try_unwrap`/raw adoption shortcut. |
| Containers, names, tempo, gain, previews, notes/templates | Ordinary traversal reports actual Vec/String capacities, tempo points/prefixes, DAG inner vectors, preview samples, MIDI source and instrument template buffers/note banks; private initial stages prepare gain/tempo/structure/empty preview. [Storage report][storage-doc], [graph visitor][graph-report]. | **I:** bounded family-level coverage. **M:** enclosing root/nested declarations/aliases, growth/copy/conversion peak overlap and incomplete children. Capacity replaces logical length; containing headers/inline arrays count once. |
| Providers, canonical device state, configurations, contexts | Compiler allocates migrated state via Arc and prepared processor trait objects from Box; shared context table allocates `Arc<SharedContextOwner>` containing `Arc::from(context)`. Operator owns its authored NativeDeviceState. [Compiler][compile], [contexts][contexts], [Operator][operator]. | **M:** opaque/shared/nested field contracts and actual allocation provenance. Sine's ExclusiveOnly is the sole narrow provider exception; FM4/Operator/custom providers default Unknown. Numeric payload/runtime declarations cannot be treated as nested heap coverage. |
| Weak, mutable, typing and residual owners | `TypingPerformance.admitted_source` is an actual Weak; active/tail runtime twins, typing Boxes, track suffixes, Return tails and prewarmed/retained/retiring context handles survive graph transitions. [Runtime visitor][runtime-report], [owner groups][groups]. | **I:** member transfer journals and partial real-owner disposal infrastructure. **M:** Weak-only backing/header retention and exhaustive independent subset fanout/role closure. A copied DSP history transfers no owning member; silence does not retire dormant owners. |
| Stream domain / cache / catalog leases | Actual domain owns pool/transport/worker/manifest/kernel/path/scratch/readiness storage. Stable indexed catalog coverage retains a real lease; teardown reports actual join/pool conservation. [Footprint][footprint], [catalog][catalog], [page delivery][page-contract]. | **I:** component/custody reports. **M:** cross-ledger union, cleanup-successor dependencies and failure payload Unknown. Charge whole domain once; indexed custody backing is an already-included subset. Never subtract cached lease totals to infer overlap. |
| Runtime/root archives, pending reclaim and failure custody | `PlaybackClaims`, sealed owners/anchors, actual returned-runtime teardown and catalog custody retain charges; `owned_arc` still emits `RetirementReceipt` Unknown. [Claims][claims], [graph visitor][graph-report], [storage report][storage-doc]. | **M:** #2539 never-installed and #2540 final-active endings, actual install/final-transfer proof, all inherited roles and current coverage/cleanup successor, then #2541 slot release. ACK/Stop/Pause/Seek/Box return/pool report alone is insufficient. Panic keeps irreversible incomplete custody/first payload; no destructive retry. |

Deduplicate **data** only using actual anchored allocation identity, charge
location and equal declaration. Conserve every owning **handle** independently,
including aliases to the same data. Reserve metadata before moves; retain source
banks, caller aliases, active/candidate/predecessor/tail/export/archive storage,
failed partial reservations and old/new growth/conversion overlap. Report checked
overflow and original-owner recovery. No naked saved addresses, AssetIds, hashes,
Arc counts or equal contents prove allocation identity or lifetime closure.

`PlaybackStorageSummary::require_complete` counts typed Unknowns independently of
the sink. Keep it and `RetirementReceipt`/fixture INCOMPLETE truthful. Separate
known storage from **unresolved obligations**; storage-only success cannot admit
publication. No hard cap changes: keep resident import guards, existing component
ceilings, immutable-graph exclusion from runtime/preroll 32 MiB, and domain N2 as
a simultaneous-domain-count policy. Neither a 192 MiB aggregate nor an eight-graph
policy is approved by this matrix.

## Every original #24 acceptance criterion

The following IDs and scopes were read from live `get_task_review(24)` (ticket
revision 14, verification scope SHA256
`c0b8a4cc055a569978a78fe2347a4f1127b765dfb280f5b7939049db07cda069`).
Checks and acceptance are null; `merge_mode=review_only`, no merge release.

| Exact criterion ID | Existing evidence | Remaining disposition |
| --- | --- | --- |
| `full_local_ci` | **I:** historical exact Mac FULL16 for the bounded 8d444c0 preview stage, [report][preview-pass]. | **M:** current-base integration and full clean unchanged final canonical candidate with commands/source/base/head/digest/log evidence. No inherited green for WIP, #2536 or descendants; no registration in this planning task. |
| `graph_catalog_leases` | **I:** real verified Arrangement/Session membership, rates/counts/conversion/generation/fingerprint, distinct catalog identity, Busy recovery and control joins in [catalog][catalog] and historical [page evidence][page-contract]. | **M:** complete graph/storage/handle union and production lifetime attachment through every active/candidate/tail/export ending; #2537–2541 plus broader provider rows. |
| `callback_endpoint_retirement` | **I:** headless endpoint split, bounded return/escrow, owner-group and actual disposal fixtures. | **M:** real supported production publication/retirement across pressure/Stop/fault/shutdown, no callback allocations/frees/clones/final drops/joins and exact pair checks; #2539–2541. **N:** actual backend terminal behavior #2546. |
| `prewarm_readiness` | **I:** exact installed/protected extent protocol and engine demand infrastructure in PR177. Decoded is distinct from installed. | **M:** complete admitted production initial/replacement demand #2542; seek/launch #2543; facade #2544. Preserve sole producer, active protection, stale/cancel fences and pressured retry. **N:** first-audible/native buffering #2545–2546. |
| `native_100_track_advancing` | **I:** scoped headless 100-source warmed-window/PCM/resource profiles only. | **N:** #2545 optimized exact-source sustained advancing mixed-rate 100-track playback, real hardware/workload/duration/timing and audible output. Source path is a prerequisite, so this is not presently executable acceptance. |
| `native_edit_seek_launch_fault` | **I:** source-level route, cancellation, loop and lifecycle fixtures; related work below. | **M:** integration #2541–2544; **N:** #2546 same optimized candidate's edits/seeks/launches/faults/responsiveness/continuity/Stop/shutdown and recovery. Broader providers remain part of acceptance. |
| `resident_import_limits` | **I:** native resident limits and unsupported-stream refusal remain guarded; compressed ADPCM stays resident fallback. [Media][media], [service][service]. | **M:** retain guards until complete graph/catalog, endpoint/readiness and required native acceptance establish supported admission. Raising limits alone never satisfies streaming. |
| `human_review` | No exact full-scope approval is recorded. | **N:** explicit human review of exact full-scope source before merge for the original review batch; stage green/automatic assertions do not substitute. Preserve maintenance/review_only holds even though generic standing merge policy exists. |

## Reuse existing work and order the remaining units

| Existing Pulse scope | Reusable implementation / evidence | Work still belonging there |
| --- | --- | --- |
| #16 native device model/runtime (blocked at read) | Shared canonical state/history, analytic/mutable preparation, transactional twins, suffix ownership and native/offline processing; [lattice][lattice]. | Reconcile broad parent acceptance and native evidence; separately scope audited provider/state/context accounting. Do not reimplement instruments in #2536. |
| #2162 residual tails/quiescence (blocked) | Contract PR90; track suffix PR93; live Session effect PR94; Return ownership PR97, as recorded by that issue and present in source. | Broader processor coverage/native residual listening; complete owner declarations and retirement union remain additional streaming obligations. Do not assume final note-off means effect quiescence. |
| #2164 mutable seek/chase (blocked) | Checkpoint/preroll/worker/interactive/native integration PR83/86/88/89/91 and children #2192/#2208; bounded 8,388,608-frame preparation, exact revision/request/route fences. | Reconcile held and post-gate feedback/history destinations and native readiness/listening. Streamed page prewarm is #2543; no duplicate resident seek engine. |
| #2165 source/transport loops (blocked) | Source PR98 (`e5f0fca`), transport PR100 (`b7aa1a2`), children #2214/#2215 Done. Distinct passes/seam release/chase and allocation fixtures exist. | Current-build repeat-gate/native listening/deadlines. Do not classify all mutable loops as missing merely because older PR62 refused them. |
| #2229 active Operator edits (blocked) | Merged PR119, exact feature `ec2321d0dce91752f63fa3493554c8187bb97cf1`, merge `ed7daa115c0264e38ab2d7ccdeb1fd5e69af6871`; historical FULL16 `20261010T212450Z-20afab01`. Target-bound retarget handles skipped candidates and active ramps. | Supported parameter/native listening and parent linkage reconciliation; unsupported active laws/class changes still refuse. Preserve exact history-copy semantics, 16-voice/256-event/8192-byte paired ceilings; no smoothing parity claim. |

These broad parents' blocked state includes legacy canonical-linkage/review
reconciliation; it does not erase their merged source. Their prerequisite PRs
must not become duplicate canonical implementations. Reported historical tests
are reusable evidence of their stated fixtures, not validation of new combinations.

The discovered streaming goal members already provide the initial-class sequence:

| Unit | Prerequisite / bounded result |
| --- | --- |
| #2537 | Exact verified original #24 Arrangement-root checkpoint and agreed integration, then remaining private PreparedRevision/Session/preview shared owners. No formal dependency on broad #24 Done. |
| #2538 | Formal #2536 + #2537; exhaustive typed initial audio-only storage **and unresolved obligations**, caller/source/domain/root/retained/construction union. |
| #2539 / #2540 | Each depends on #2538; distinct never-installed disposal and final-active terminal-quiescence ending proofs. |
| #2541 | Both endings; actual install/final-transfer, independent A→B→C subset associations and native publication/reclamation with complete eligible constructors. |
| #2542 → #2543 → #2544 | Initial/replacement engine-demand readiness → streamed seek/launch prewarm → neutral Session/service and truthful UI buffering, all revisions of opted-in generations including resident-only/empty catalogs. |
| #2545 → #2546 | Optimized sustained native 100-track playback → same-candidate edits/seeks/launches/fault/shutdown. Source/artifact/workload identity must match. |

Broader provider declarations, canonical state/context/Weak accounting and their
native combined workloads are **M**, not covered by the initial-class chain.
Before implementation, agree finite owning-module units with #16/#2162 and
coordinate the existing seek/loop/retarget scopes. No new tickets, dependencies,
source integration or API redesign are silently created by this document.
Persisted fingerprint/relink migration, additional provider opt-in contracts,
and unsupported warped Session audio remain **D**; record explicit decisions
without using them to waive already-supported sources. Goal membership uses
returned tag 11; these ordinary members are not a new parent/child hierarchy,
and finishing them does not automatically complete goal 2 or #24.

## Required verification and native cells

Reuse the existing tests before adding regressions: engine `compile_streamed_sources`,
`session_streamed_audio`, `observed_pages`, `registered_instrument`,
`fm4_native_admission`, `operator_native_admission`, gain/loop/device/Return and
typing fixtures; stream `worker_runtime`, readiness/domain custody and
`callback_allocations`; audio `stream_publication`, `owner_group_allocations`,
typing/Operator/effect allocation fixtures; Session `operator_seek`. The private
preparation tests exercise actual resident/streamed constructors, exact handles,
spare capacities, reserve/install faults, original-bank recovery and destructor
panic. Their completion branches must actually execute on a supported compiler;
an unsupported-layout early refusal is not positive constructor evidence.

For each later source unit, require exact source/base/head and local logs, genuine
same-content/distinct-owner and same-owner/multiple-handle controls, wrong/raw/
default-empty/provider/Weak refusal, all bounded reserve/transfer failures,
normal disposal versus panic/no-retry, stale/duplicate/wrong-runtime facts, real
worker pool conservation and zero **total and signed-net count/bytes** callback
allocation probes with sensitivity controls. Do not test completeness with a
forged admitted fixture, a sink discarding Unknown or a diagnostic string list.
Run full required local CI on the eventual clean final candidate; Linux checks
cannot establish CoreAudio behavior or native default-stack/deadline acceptance.

| Native cell after source integration | Workload and observable result | Scope |
| --- | --- | --- |
| Sustained advancing playback | 100 independent long mixed-rate sources beyond resident working-set limits; fixed hardware/device/rate/buffer, duration, advancing output oracle/capture, first-audible and sustained callback timing, misses/protected pages/budgets. | #2545; audio-only first, then the supported instrument/effect/typing combinations above before broad #24 claims. |
| Edit/history/reopen/export | Repeated trims/moves/fades/gain/Undo/Redo, same-catalog edits, streamed→resident-only/empty catalog, project replacement with reused IDs, save/reopen/concurrent export. Observe continuity, source generation identity and bounded retained/construction charges. | #2544/#2546 plus persisted-identity decision; no UI join or silent fallback publisher. |
| Seek/loop/launch/typing | Cold/warm mixed-rate seeks, loop seams and quantized audio/MIDI launches; held and post-note-off mutable feedback/LFO, residual chains and typing. Check transient preservation, exact routes/clocks/ACK, superseded requests, Pause/priority Stop and rapid edits. | #2543/#2546; reuse #2164/#2165/#2229. |
| Fault/pressure/recovery | Full request/ready/return/ACK/retirement slots, domain N2 pressure/refusal/retry, missing/changed file, worker failure, device-rate change and stop/pause failure. Keep prior admitted audio and truthful buffering/refusal/fault state. | #2541–2546; no forced resource disposal, cap widening or stale commit. |
| Terminal ownership | Delayed release of both backend closure owners, pre/post-ACK panic and destructor failures, final active and never-installed graph disposal, actual callback/error/payload destructor thread, no repeat, exact pool/join/report/reopen barriers. | #2539/#2540/#2546; successful pause/Drop alone proves neither destructor thread nor full quiescence. |

Use the supported optimized stable-path packaging and guarded delivery described
in [test builds](test-builds.md), with exact source/PR revisions, signature/hash,
CI receipt and finite checklist. Preserve settings, process-running refusal,
rollback, serialized activation and disabled-gateway safeguards. User owns native
interaction/listening while present. This planning task authorizes no install or
launch. This Linux workspace supplies no Mac/CoreAudio measurements or listening.

## Planning validation

This is a documentation-only deliverable. Source declarations, constructor
paths, test names and immutable citation paths were inspected against the exact
Git objects above; all eight live #24 requirement IDs are mapped. Documentation
links and whitespace are checked separately from runtime tests. No Rust behavior,
provider trait, schema or cap changes. The documentation merge requires its own
exact-source local CI; historical parent evidence supplies no test, FULL16 or
native pass for #2536. PR231 remains the sole linked deliverable with PORTALSURFER
assigned; prerequisite source integration and broader provider units remain
explicit outstanding work. Its final validation and merge receipt belong to the
PR/Pulse workflow, separately from this source/evidence snapshot.

[pr177]: https://github.com/PORTALSURFER/hex/pull/177
[preview-pass]: https://github.com/PORTALSURFER/hex/pull/177#issuecomment-6188617266
[root-handoff]: https://github.com/PORTALSURFER/hex/pull/177#issuecomment-6188716461
[engine]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/lib.rs
[compile]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/compile.rs
[revision]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/prepared_revision.rs
[inventory]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/session_inventory.rs
[session-tests]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/tests/session_streamed_audio.rs
[builtins]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-devices/src/lib.rs
[provider]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-lattice/src/instrument.rs
[fm4]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-devices/src/fm4/device.rs
[operator]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-devices/src/operator_state.rs
[contexts]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/shared_analytic.rs
[mutable]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/mutable_instrument.rs
[lattice]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-lattice/src/lib.rs
[typing]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/live_input/performance.rs
[export]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-export/src/lib.rs
[inputs]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/audio_only_inputs.rs
[owned-shared]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/owned_shared.rs
[owned-revision]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/owned_prepared.rs
[preparation]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/audio_preparation.rs
[session-preparation]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/session_audio_preparation.rs
[shared-preparation]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/session_shared_preparation.rs
[track-assembly]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/audio_preparation/arrangement_clips/tracks.rs
[return-assembly]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/audio_preparation/arrangement_clips/returns.rs
[graph-report]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/storage_report/graph.rs
[runtime-report]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/session_playback/storage.rs
[groups]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/storage_report/owner_groups.rs
[claims]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-engine/src/storage_report/claims.rs
[catalog]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-stream/src/revision.rs
[footprint]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-stream/src/footprint.rs
[service]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/crates/hex-session/src/streamed_candidate.rs
[media]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/docs/media-streaming.md
[page-contract]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/docs/page-delivery.md
[storage-doc]: https://github.com/PORTALSURFER/hex/blob/8d444c06c295e52a8aa0dde275a89ecec6f454e2/docs/stream-storage-report.md
