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

Mutable track runtimes retain the configuration lease and pass it through actual
controller preflight and processing. History inheritance/retarget checks the
actual source configuration; tail moves retain that owner. Checkpoint capture/
restore clones the same scope on control and charges its buffer before cloning.
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
