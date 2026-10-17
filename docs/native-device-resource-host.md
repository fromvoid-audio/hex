# native device resource host admission

Pulse2783 / PR389 continues W10/2681 after current resource API373. It preserves
sampler2679 as the canonical full feature; mergedPR296 (8a5f559b) remains
the original borrowed-PCM playback kernel prerequisite and goal3's sample-first
order, samples-or-instruments Grid and reusable modulators for every device.
This draft is incomplete; no referenced processing is newly admitted.

## source facts and configuration scopes

The first implementation uses the existing native OwnedAudioSource/OwnedAudioInputs
constructor family, followed by AudioInputStorageAndObligations. The latter's
custodian is established before catalog tokens are captured. The actual immutable
input storage must remain in the retained native host root. A catalog owns only
plain facts and nonowning checked tokens; scopes own only authored-slot-to-fact
indices. No new PCM cache/owner, Arc escape or device-owned PCM is introduced.

A resource inventory is specific to one prepared device configuration. Two
configurations may use the same slot for different assets; the old configuration
must keep its old scope. Catalog identity and bank/custodian/handle tokens are
checked separately from content identity. Equal content does not rebind a token
or authorize a scope from another catalog. Typed borrows carry the actual bank's
lifetime, rather than a temporary view object's lifetime.

Control preparation verifies expected original rate/frame count and resident
source kind, hashes under an explicit finite frame-work reservation, checks
cancellation before each source and every256frames, and checks actual metadata
capacity against the caller's reserved bytes. Failure does not mutate or clone
bank owners. The declaration includes value/header and vector backing capacity;
it is not physical allocator/RSS, lifetime or publication proof. Existing root
storage accounting must charge actual allocations and handles before admission.

The first checkpoint used the immutable stereo native constructor. The current
source family also has an explicit mono native constructor, with original mono
samples stored once in a retained Vec<f32>. Both use the existing source/bank
custody and constructor-paired shared allocation family. OriginalResourceLayout
must match the actual mono/stereo owner; mismatches refuse rather than substitute
channels. Source delivery, complete graph accounting/wiring and streamed-device
admission remain separate required work.

## current normalized pcm identity

Content is SHA256 over the byte prefix
`hex.device-resource.original-stereo.v1` followed by a NUL byte, then source rate
as little-endian u32, frame count as little-endian u64, and each stereo f32 bit
pattern in left/right order as little-endian u32. Asset IDs are not part of content.
Derivation version is1; its digest is SHA256 of that prefix including the NUL.
This describes normalized original-rate decoded stereo PCM, not encoded file bytes.
Native file verification remains the media owner's separate responsibility.
Mono uses the byte prefix `hex.device-resource.original-mono.v1` plus NUL,
the same rate/frame fields, then one little-endian f32 bit pattern per source
frame. Its derivation digest uses that mono prefix. The distinct prefixes keep
actual layouts distinct; this is the current decoded-data protocol, with no
historical format conversion. The exact existing sha2 0.10.9 dependency is reused.

A Python hashlib/struct fixture independently pins the content digest. Actual
native-constructor tests check stereo samples/rate, configuration slot reuse,
equal-content/foreign-bank refusal, cancelled/work/metadata/shape failures and
unchanged input accounting. They report whether the pinned constructor gate was
supported; unsupported environments do not establish native ownership evidence.

The existing analytic/effect/mutable allocation probe also runs against actual
sealed input storage through these scoped typed borrows. It observes exact
stereo PCM sums and zero total/net callback allocations and frees, then verifies
unchanged bank accounting and unresolved-handle integrity on control. This proves
the borrow/query boundary only; it does not prove graph publication, carried-root
retention or final-owner retirement. The actual constructor probe now covers one mono and one stereo source.

## remaining admission work

Retain the actual bank/catalog/scope values in existing graph/context ownership
families and include original sources, derivatives, all current/candidate/outgoing/
carried/history/pending/unreclaimed roles in one unique-allocation budget. Account
all real handles and metadata; identities and raw Arc counts are not leases.
Wire actual Arrangement/Session/keyboard/export instruments and active/bypassed/
outgoing effects to their actual source configuration, including slot reuse.
Reuse control/reaper retirement for refusal, cancellation, Stop/seek, replacement,
shutdown and saturated queues. Prove no final owner drop or allocation on callback.
Only then open referenced processing. PR296 is already merged and does not
register a usable sampler. Read current2679 and matching followups before supported
focused intake for remaining registration/loading/persistence/panel/filter-LFO-warp
work; preserve296 as historical kernel evidence.

Current format31/strict current validation and alpha feature-first policy apply.
No compatibility readers/migrations, shared-chain2756 implementation, neutral2773
implementation, hosted CI, Mac self-dispatch or app replacement is included.

## retained host implementation in progress

`DeviceResourceBank` now moves the actual input storage/catalog into the existing
constructor-paired `OwnedShared` family. `PreparedDeviceResourceLease` keeps that
same immutable root and one configuration's scope; providers still own only
coefficients/indices. Control cloning reserves scope metadata before the owning
handle. Retained and construction ceilings are explicit and unchanged. The
existing `AudioRootStorageLedger` visits the real root, every physical handle,
input-storage vectors, catalog and scope buffers, deduplicating shared PCM once.
A missing selected owner prevents closed-handle validation. Complete retirement
is still explicitly unknown; these rows alone do not permit publication.

Shared/continuation sampling now queries its actual selected
`SharedContextOwner` lease. A source-root identity joins reuse/continuation
identity, so equal authored state/content cannot substitute another actual bank.
The actual carried-note path is tested after caller custody is released and a
candidate reuses the slot; final root destruction is observed on control.

Mutable track runtimes retain the immutable configuration and pass its borrowed
resources through actual controller preflight and processing. History inheritance/
retarget checks the actual source configuration; tail moves retain that owner.
Checkpoint capture/restore reserves bounded control aliases of the same immutable
lease/binding body; no new binding buffer or PCM copy is created.
Uncertified resource-backed analytic-seed imports refuse. Asset-free hosts still
construct with None. Shared control preparation still supplies None until its
resident source plan/all-role admission is integrated; no registry gate is open.

Exclusive decoded Audio can now move its retained Vec through a fresh existing
native constructor without copying PCM. The old raw Arc is destroyed on control;
its receipt is never upgraded. Shared/weak/derived inputs refuse intact. Frame
work and cancellation are checked before consuming the source; positive tests
verify the actual buffer pointer/capacity, rate, samples and new declaration.
Media import integration remains required; true native mono custody now uses
the extended existing source family described above.

Independent negative host oracles substitute the prior empty-resource sampling
path and fail; restoring actual owner borrows passes. Mutable host cases cover
rollback/retry, checkpoint scope clone, moved-tail continuation, wrong-source
refusal before PCM changes and equal-content foreign-root history refusal.
Full engine tests and strict joint engine/audio allocation-feature Clippy are
run at each coherent source boundary. The readonly harmonic accounting proposal
also charges the actual new host lease header; no memory limit was raised.


## native mono and clip restriction

Native mono constructors retain the actual mono buffer, return it unchanged on
numeric refusal and use the existing bounded bank alias/handle machinery. The
real ledger charges its constructor-paired shared payload and actual mono buffer
once across aliases. Tests pin original layout/rate/buffer, an independent Python
PCM hash, alias saturation, exact handle closure and typed sample output. Actual
mono/stereo bank and retained-lease analytic/effect/mutable probes preserve their
PCM oracles with zero total/net callback allocation or free.

Clip constructor/commit accepts a private two-variant restriction of the existing
source ownership family: resident stereo or streamed metadata. The generic
storage visitors borrow either source/clip custody without minting ownership.
Native mono resource bindings refuse in Arrangement and Session preflight before
alias creation or transfer; commit remains exhaustive and infallible over its
supported types. Existing source/claim/cancellation/retirement tests are retained.
The Session refusal fixture's pre-existing caller alias was removed when building
its new mono bank; its original failed assertion is retained as fixture evidence.
This does not expand the audio-only native publication support matrix.

## native media decode before sharing

`hex_media::decode_device_source` decodes immutable bounded WAV bytes directly
into the existing native source constructors, before any shared raw `Audio`
escapes. Original mono uses one mono buffer; stereo uses one stereo buffer.
PCM/float/companded conversion and IMA/Microsoft ADPCM kernels are reused.
Compressed decoding needs only one explicitly charged stereo block scratch,
including when a fact chunk trims the retained mono output. There is no second
full decoded PCM allocation, source cache or ownership manager.

Caller limits independently cover encoded bytes, retained frames, RIFF headers,
decoded work including ADPCM padding, and actual output/scratch buffer capacity.
Cancellation is checked per header, every256 fixed frames and around each
compressed block; final constructor validation is bounded by the retained frame
limit. Failure discards only unpublished worker custody. Host ledger/header,
coexistence, source-generation publication and retirement admission remain
required; this component is not yet wired into Session's shared media bank.

The existing independent FFmpeg codec oracles also check native source custody
and true mono/stereo layout. Exact buffer/work refusal, malformed float input,
fact-trimmed scratch charges and deterministic cancellation boundaries are tested.
The architecture check caught that the earlier engine-side source hash dependency
was absent from its declared boundary. The policy and owning README now explicitly
allow pinned sha2 for bounded control-side hashes of actual retained PCM only;
file verification stays in media and callback resource queries never hash.

## closed media view over the same root

`OriginalSourceLease` is a closed handle into the same retained resource root,
not a second cache or a fake device configuration. It exposes only the selected
original source's borrowed mono/stereo view and checked source facts. It has no
raw Audio/Arc/Weak escape or infallible Clone. Control creates/clones it only
after quoting its header and obtaining a distinct physical handle. Aggregate
coexistence/retirement reservations still belong to the host; this header quote
does not authorize graph publication.

The existing root ledger visits every media/snapshot handle alongside actual
provider leases, deduplicating the real PCM and root storage once. Omitting a
live media snapshot prevents closed-handle validation. Tests retain media views
after caller bank/provider retirement and observe final root destruction only
after the last view returns to control. The independent media codec oracles
also traverse decode → native input storage → catalog → retained root → source
snapshot, preserving actual PCM buffer pointers and original channels.

This prepares the narrow Session/waveform backing handoff without altering
waveform geometry or silently converting already-shared raw Audio into custody.
Session publication, all-role memory reservations and complete native retirement
remain open acceptance work.

Source plans can obtain a bounded control alias from an `OriginalSourceLease`
into a new `OwnedAudioInputs` bank. The result remains a closed `OwnedAudioSource`
with a fresh physical handle and the same constructor-paired mono/stereo PCM;
no raw Audio/Arc or extra sample buffer escapes. Failed destination construction
returns that actual source intact. Both old and candidate banks, snapshots and
provider leases must be included before handle closure can succeed.

A mixed mono/stereo current/candidate test pins both actual buffer pointers,
refused-destination recovery, four unique shared allocations (two PCM owners and
two resource roots), all nine physical handles and exact-minus-one union refusal.
Candidate PCM remains correct after old-root retirement. The existing analytic/
effect/mutable callback allocation probe also traverses this real source-plan
alias path, retires old control custody, and retains its exact PCM sums with zero
total/net allocations or frees. This is source-plan custody evidence, not a
complete all-host/native publication permit.

The v1 resampler also accepts an actual retained original view through
`ResamplePlan::render_original_window`. Stereo and mono specialize the same
indexed kernel; mono is read in place with no full stereo source copy. Window
limits, exact source rate/extent, malformed values and error-output clearing
remain checked. Existing golden kernels and independent codec PCM oracles run
alongside bitwise whole/window comparisons, including identity signed zero.
This is worker preparation, never pointer-motion or callback DSP preparation.
Identity-rate stereo source plans must keep using the closed owning alias rather
than allocating a redundant project-rate PCM buffer.

Waveform preparation now has a bounded cancellable `summarize_original` path
whose backing is an actual closed `OriginalSourceLease`. Mono/stereo peak
construction and deep-zoom queries read that retained PCM; wide cached-query
math and public query signatures stay the same. The legacy Audio summary uses
the same binning implementation. Immutable summaries are shared with Arc;
Waveform itself no longer has an infallible owning Clone. Control can borrow
`Waveform::original_source` for the real union inventory without cloning a handle.

Codec-oracle tests drop the source snapshot and caller bank, then check native
waveform deep-zoom extrema directly against independent PCM. Work/metadata/cancel
refusals preserve caller custody. Whole-workspace all-target type checking covers
the API change. This is PCM backing/summary work only; cloud6 renderer geometry
and sampling-grid files are untouched. Source ownership was checked against its
completed UI handoff before editing. Session source publication remains pending.

Native stereo file import now establishes constructor lineage before Session
shares original, project-rate and waveform PCM. Identity-rate imports reuse the
same allocation; nonidentity resampling constructs a fresh native stereo owner.
The existing Audio representation carries a private physical handle per native
owner. Its clones preserve actual constructor provenance and issue distinct
handles without copying PCM. Unknown origins and identity exhaustion remain
unqualified. Moving between Audio and OwnedAudioSource conserves the same handle;
neither Arc counts nor caller-provided identities can establish provenance.

An actual stereo Session import test checks independent PCM, original buffer
identity, both project-rate cases, waveform custody, resident limits and Undo/
Redo. A selected resource root cannot close its handle inventory while Session
and cache owners are omitted; after their control-side retirement, its retained
source remains readable and the actual selected inventory closes. This is a
stereo ingress and retention proof, not complete graph admission. The remaining
work at that checkpoint included native Mono Session storage and graph admission.

## actual Session originals and source plans

`DecodedOriginal` now retains native mono/stereo source custody on the worker
before Session sharing. Native mono keeps its actual mono buffer and prepares
only the required project-rate stereo clip derivative, using the same v1 kernel
without an intermediate full stereo original. Single-file Arrangement/Session
imports, ordered batches, reopen/recovery, browser preview and bounced-file
preparation use this path. Portable normalized imports remain explicitly
unqualified. Stereo identity conversion still aliases the original PCM.

Session resident accounting observes actual capacities for both layouts. Each
mono original capacity counts in source-frame units alongside its stereo project
derivative; no ceiling changes. Tests cover original pointer/samples, three
project rates, exact identity bits, actual rendered PCM, retained history,
file/reopen, cancellation and exact-minus-one atomic refusal. Existing batch
budget fixtures now account both actual mono/stereo buffers while preserving
their ten-frame free headroom and second-member refusal condition.

The Session resource-plan boundary creates bounded closed aliases from actual
retained originals into the same input bank/catalog/root. It checks sorted
requests, work, temporary metadata/capacities, cancellation and construction
peak; failures release only unpublished control custody. Raw/derived Audio and
mismatched native handles still refuse. The borrowed resident PCM inventory
holds the existing waveform-cache borrow while visiting original, project and
waveform PCM fields. It neither clones PCM nor declares enclosing maps, graph
metadata or transform budgets known.

Actual mixed Session tests check typed PCM and original buffer identity. Omitting
live Session/cache owners or a candidate bank prevents selected handle closure.
Selecting both current/candidate banks charges four actual original/derivative
shared PCM owners once, plus each distinct retained root. Exact-minus-one union,
work/metadata/alias/cancel refusal and control-retired snapshot survival are
covered. These are actual media-plan/PCM union proofs, not a complete publication
permit. Processed-media budget ownership, all-role metadata, inline/effect host
wiring and atomic publication/reaper qualification remain open. Resource-backed
registry processing stays refused until those actual host proofs are complete.

## freshly prepared clip PCM and transform custody

Worker-produced clip PCM now has a distinct declaration captured at its fresh
`Arc::new(AudioData)` constructor. It is separate from original PCM: old/raw
derived Audio remains Unknown, and even a known freshly prepared clip cannot
enter an original device-resource bank. Native clones preserve the prepared
origin with distinct physical handles. Actual Vec capacity must fit its retained
reservation before the owner is created; invalid values/rates or under-reserved
capacity refuse the unpublished candidate.

The existing production 768MiB derived-buffer counter now has process lifetime.
Reservations borrow that same static counter, retaining the existing atomic
reservation/release law without hiding an owning budget Arc in each PCM root.
Isolated test budgets remain explicitly unqualified. The last actual PCM owner
still releases its reservation; no callback final-drop permission is introduced.

Scoped Session/transform inventory visits actual source/prepared Audio fields
in committed and pending caches as well as resident originals, derivatives and
waveforms. Its higher-ranked callback prevents inventory guards or a borrowed
ledger from escaping across transaction commit/eviction. A compile-fail lifetime
test covers that restriction. Enclosing cache/domain/tempo metadata and native
host publication admission remain separate work.

Actual pending-candidate tests refuse closure when cache owners are omitted,
then close the PCM union when every real field is selected. An injected
publication refusal retires pending source aliases and the new prepared PCM on
control; retry/commit selects the actual committed owners. A last-owner test
observes the actual reservation's final release on control. The native clip
renderer probe uses an independent constant-gain PCM oracle through partitioned
render/seek and records zero total/net callback allocations and frees. This is
prepared-PCM/custody evidence, not complete graph/reaper or sampler admission.

## inline analytic host scopes

Inline note coefficients remain in the existing 352-byte analytic sampler and
360-byte bank entry. When a note leaves the bank, its host voice copy adds only
a nonowning configuration key. Actual source ownership stays in the existing
current/retained context owner, with a resource lease and no invented DSP
context. Lookup checks the actual owner ID and class before borrowing its scope;
a target reusing the same slot cannot satisfy an old carried key.

The real bank/host and MidiReleaseVoice paths preserve old stereo or mono PCM
after caller-bank retirement while the new configuration uses its own source.
Missing carried owners refuse. Replacing the actual scope with EMPTY produces
zero PCM and fails the independent oracle; restored code passes. Actual callback
probes observe zero total/net allocations/frees, and final root destruction is
observed on control. The registry's nonempty-reference processing gate remains
closed, and ordinary asset-free preparation supplies no resource plan.

Host metadata is charged at its real size. On the measured 64-bit source, a
scoped host note is368bytes, ReleaseEntry504bytes, and the current release/span
storage16,601,096bytes; including existing context slots/plans is17,125,384bytes.
The existing16MiB current,18MiB scoped and36MiB paired bounds are unchanged.
Only the owning layout fixture's exact byte assertions changed; detached-effect
and harmonic stack fixtures are untouched. This is not complete effect-host,
all-role metadata, atomic publication or saturated reaper qualification.

## effect entry resource custody

The existing `NativeRuntimeChain` stores a host scope in each actual runtime
entry. Its default asset-free scope is zero-sized. A scoped prepared instance
moves its scope into that entry; history, rollback/commit, active processing,
clocked bypass and whole-chain tail moves use the same retained configuration.
Both legacy twins and owned transactions receive only `DeviceResources` borrows.
No provider factory or coefficient payload owns PCM.

History identity still finds the old stable device even when its resource scope
changed. The host then refuses inheritance unless the actual scopes match, or
the old entry is explicitly excluded because its history stays with a tail.
Configuration matching and parameter retargeting apply the same scope guard.
Equal authored state and equal provider keys cannot bypass a foreign root.
A negative test removing the scope guards fails the independent host assertion.

`DeviceResourceConfiguration` retains one immutable lease/binding vector through
the existing constructor-paired `OwnedShared` family. Bounded control aliases
retain this same allocation and original PCM. The configuration header, actual
shared allocation/header, binding capacity and every physical alias join the
existing ledger. Omitted owners refuse closure. An inventory of actual current,
candidate, tail, history and pending-reclaim fields accepts exact retained/peak
bytes, refuses one byte less and refuses exhausted handle rows. This inventory
is control-only; it never borrows a concurrently active mutable runtime.

Actual host-entry tests cover bank retirement, source slot reuse, partitioned
PCM, rollback/retry, committed bypass advancement, foreign-scope history refusal
and whole-chain moves. Both legacy and owned effect transactions are exercised.
The actual mono/stereo host-entry callback probe checks independent PCM values
and zero total/net allocations and frees after caller banks retire.

These changes do not extend the selected native constructor's existing Utility
physical-runtime visitor or its sealed group-transfer origin proof. Production
Arrangement/Session/keyboard/export preparation, all-role whole-graph metadata,
atomic publication and saturated reaper/final-drop coverage remain unfinished.
An immutable alias anchor and selected closed handles alone cannot qualify an
active runtime or prove retirement. Resource-backed registry processing stays
closed until that integration is complete; no memory ceiling is raised.

Closed native scopes now reuse the catalog's immutable finite-PCM verification
when a runtime entry is constructed. They recheck every actual source token,
slot and asset reference in bounded metadata work, without another whole-buffer
scan per configured host. Unverified scopes retain full PCM validation. Missing
or mismatched slots and zero output rate refuse before a runtime factory runs;
original source rate remains independent of project/output rate. This validation
hook is control-only and conveys no publication authority.

## retained context metadata custody

Current/retained context handles now preserve their actual `OwnedShared`
constructor receipt when cloned or moved. No old Arc is adopted. Inline resource
owners make a bounded fresh copy of current state payload/bindings before
capturing their allocation declarations. This is current metadata preparation,
not historical project/preset conversion. Empty payloads use an explicit fresh
zero-array Arc constructor and unsize that same allocation. The same original
PCM lease remains in the host owner; provider coefficients acquire no PCM owner.

Payload and asset-binding allocations have distinct ledger domains, even when
stored in the same state value. Read-only observations expose neither Arc. A
payload alias cannot supply a binding handle or satisfy its closure. Equal
replacement bindings retain their own separate allocation. Removing the domain
guard or substituting caller state metadata fails independent negative oracles;
restored source passes. Raw outer Arcs and opaque provider contexts remain
Unknown. A refused public inventory is invalidated under the existing ledger
policy and has no reusable partial total.

Selected revision/Session inline-owner inventories visit actual immutable
current and retained fields, nested state metadata, scope buffers, original
roots and every physical alias. They can close the selected metadata/PCM union
and refuse missing caller/archived metadata owners or exact-budget overflow.
They do not declare table/vector spare capacity, mutable/effect runtimes or the
whole graph known, permit concurrent active-runtime borrowing, prove saturated
reaper retirement or open registry processing. Enclosing graph visitors can
reuse the contents visitor without charging embedded owner headers twice.

Actual owner handle receipt storage doubles a retained slot from 8 to 16 bytes on
the measured 64-bit source. Current release/span storage remains 16,601,096 bytes;
scoped storage is 17,256,456 bytes and the 256-owner table handle/index/header
charge is 9,304 bytes. Fixed 16/18/36 MiB and context payload caps remain unchanged.
Only the matching source layout model/header assertions changed; no harmonic
stack fixture or behavioral assertion was weakened. Whole-host resource
preparation, atomic publication and actual reaper/final-drop qualification
remain in progress under389, before usable sampler2679 and full goal3.

Mutable runtime configuration checkpoint: actual track owners, control-captured
checkpoints and moved tails retain bounded aliases of the existing immutable
`DeviceResourceConfiguration` body. Checkpoint capture/restore no longer allocates
another binding vector. Selected Session and checkpoint-store inventories visit
these actual fields; every alias must be present for closure. This view excludes
runtime payloads, enclosing containers, revision metadata and active callback
custody. Shared-body closure alone cannot authorize publication or retirement.
The existing checkpoint upper bound conservatively reserves nested metadata per
snapshot; the physical union ledger separately counts its shared allocation once.
A refused lease-to-configuration conversion returns the unchanged original lease
for bounded retry. No provider owns PCM, and no process memory cap changes.


The instrument-test-support mutable-host fixture exercises the real track wrapper
with original Mono/Stereo configuration owners after bank retirement. Instrumented
callbacks cover reset, speculative rollback and partitioned processing with zero
total/net allocation/free. An independent nonconstant stereo oracle reproduces
all 64 frames across 17-frame partitions and ends in silence. Deliberately
committing speculative state fails that oracle; restored rollback passes. This
fixture uses the unregistered resource ABI conformance provider, not Sampler note
semantics, a native publication permit or a final reaper-drop receipt.

Prepared-context configuration unification uses the same existing immutable
configuration body as effects and mutable tracks. Actual Arrangement/Session
instrument preparation borrows the source owner's view; mutable runtime creation
retains a bounded alias from the immutable prepared-context table. Mutable
continuations with resources retain that source anchor instead of discarding it
when selecting the continuation path. An actual inline owner and mutable host
close one shared body only when both physical handles are inventoried; either
can retire on control without invalidating the other's PCM.

This is source wiring, not opened registry admission. Referenced registry
processing still refuses, opaque provider contexts remain Unknown, and a complete
resource-aware closed graph constructor plus publication/reaper proof is still
required. No raw Arc, source identity or provider byte declaration upgrades that
missing physical proof.

Fresh context table preparation now carries its original `OwnedShared` constructor
receipt through Arrangement, Session, compiled instruments and MIDI templates.
The selected revision table inventory includes both physical root fields and all
actual compiled/Session/routed MIDI template table fields, complete table backing
capacities and nested supported resource owners. A template reused by two revisions
has one physical table field; existing ledger rows deduplicate that actual field.
Distinct aliases still require separate handles and headers. Cached templates
retain their original receipt after the older revision retires, with no raw Arc
upgrade or replacement allocation. Independent raw table origins and opaque
provider contexts still refuse. MIDI bank/template containers, other prepared
metadata, active runtime custody and full construction peak remain outside this
selected view; the whole graph still needs its closed constructor admission.

Note-bank preparation and reuse now retain their original constructor receipts
through actual Arrangement and Session/routed instrument templates. A selected
inventory visits real bank headers, the shared owner allocation and complete
entry-buffer capacity. Entries use the existing by-value Copy coefficient ABI;
context IDs remain nonowning and cannot replace actual source/context leases.
Cached templates retain their original bank after older-revision control
retirement. Actual current/predecessor union tests pin the three distinct banks
and their entry backings, so omitting an entire Session bank cannot pass merely
because the remaining selected handles close. Empty/raw bank Arcs still refuse.
Actual receipt headers consume the existing policy allowance; the complete-bank
byte ceiling remains 47,747,072 on the qualified 64-bit layout. These selected
rows do not declare enclosing template storage, full graph construction peak,
active runtime custody or native publication/retirement complete.

Actual Session/routed source and instrument templates now retain their original
container receipts alongside bank and context receipts. Their selected prepared
MIDI inventory includes all source edge/span and instrument voice/index backing
capacities, nested supported owners, compiled Arrangement span/index buffers and
each physical alias/header. Empty repeat arrays can have allocated spare capacity
and are included. Real cached-revision tests retain the original source/instrument
bodies through predecessor retirement; unique physical fields are not counted
again merely because two revisions share them. Raw outer template Arcs refuse
even when their nested bank/context declarations are legitimate.

This selected template view excludes canonical `MidiSourceIdentity` model-source
storage, enclosing Session clip/revision containers and full construction peak.
The approved canonical borrowed model-owner boundary and its separate new/COW
native-layout qualification are documented in [native MIDI storage](native-midi-storage.md).
Its selected actual model/history/prepared identity inventory remains separate
from enclosing container storage, root custody, full construction peak and
retirement. No pointer/count/hash promotion, alternate PCM owner/cache or
registry admission is added.

Prepared effect entries now carry `Option<DeviceResourceConfiguration>` in
their actual source array; Arrangement Renderer, Session current/tail chains,
Returns and export use the same scoped runtime type. Construction clones the
bounded configuration handle on control. Scope storage is never held by provider
coefficients or a parallel cache. Preflight and actual runtime accounting use the
real scoped entry size under the existing ceilings. `Some(empty)` differs from
`None`; the closed Utility constructor and transfer/storage proof require actual
absence of a scope. The ordinary compiler still supplies `None` pending the
resource-aware preparation/closed-admission path. An unregistered fixture through
the actual prepared Track/Renderer path proves retained original PCM, seek and
partition consistency and control final-root drop, with allocation instrumentation
over that Renderer path. This is not public referenced registry admission or a
claim that Session/Return/keyboard/export resource acceptance is complete.

The canonical model-source oracle also exercises an actual mutable Operator
Arrangement descriptor: omitting its retained identity fails closure; including
the real field closes, and it retains the original body/notes after canonical
model retirement. It remains a selected-view oracle, not whole-root proof.
