# shared tuning and resident-resource preparation contract

This is the bounded cloud4/cloud5 handoff for Pulse2739 and W10/Pulse2681. It
records the current API and pending host admission; it does not claim that
resource-backed native instruments are already admitted. No historical alpha API
or format adapters are required. Current-format validation, history integrity and user files remain
protected.

## source and APIs available now

Main at the PR373 merge below contains W05/PR290, W06/PR291, W10/PR297 and
canonical tuning PR350/351. Schema31 preserves tuning and complete device-asset
manifests together. Registry processing still refuses nonempty authored bindings;
the borrowed resource ABI alone does not confer resident-root admission.

PR370 now retains original-rate decoded PCM independently of clip derivatives.
The exact control APIs available on that main are
`hex_session::Session::source_audio(asset) -> Option<&hex_engine::Audio>`,
`Audio::sample_rate()`, `Audio::frames()`, `Audio::shares_pcm_with()` and
`Audio::pcm_capacity_frames()`. `Session::resident_frames()` charges unique PCM
buffer capacity across source/derivative aliases. `Audio` is the current decoded
stereo-frame abstraction; these control borrows and bank totals are not typed
callback resource leases or graph coexistence accounting. Cloud4 owns the remaining retained-root host admission under the contract below.

Native preparation-context PR373 merged at
`37fee996c5fe54d36459cd22c7f41eb4320b77c8`, preserving repaired tuning PR351
(`977bed8982ea8698fb3c83ad6f248f2d67c89a95`, merged `93d22ab0`). The current
entrypoint is `NativeInstrumentRegistry::prepare_instrument(state,
&DevicePreparationContext { sample_rate, tuning, resources })`. The validated
`TuningTable` contains 128 effective Hz values; disabled tuning uses the exact
existing equal-temperament law. `DeviceResources` supplies bounded borrowed
`DeviceResourceView`s with slot, content/derivation identity, original sample rate
and explicit mono/stereo PCM. Preparation and analytic/shared/continuation,
mutable-instrument and effect processing use the one resource boundary.

Current engine hosts explicitly pass `EMPTY_DEVICE_RESOURCES`. The registry
continues to refuse authored asset references before provider migration or
preparation; retained-root admission, coexistence accounting and retirement
remain cloud4's PR389 work. There is no historical `with_tuning` or
`with_resources` adapter. Providers prepare on control; applied tuning transitions
preserve phase, envelopes and histories. Exact mutable-source matching includes
the effective tuning identity; unchanged authored bytes alone do not authorize
history copy. Fixed-Hz oscillators keep fixed-Hz semantics, and Operator's existing
4KiB context/2MiB shared-context ceilings remain unchanged. Asset-free tuning
preparation remains independent of the optional MTS broadcaster.

PR350 owns the combined current schema31 after asset30's landing. It requires
both tuning and the complete device manifest, including empty/default data, and
explicitly refuses unsupported alpha30. It adds no historical30 readers or
cross-conversion. Existing earlier readers are not removed as unrelated cleanup.

## one current preparation and processing boundary

Cloud4 owns the shared Lattice/resource API and PR389 host admission. PR373
already incorporates the repaired, merged PR351 source supplied by cloud5. Cloud4
can continue original-rate decode, lease/accounting and sampler-lifetime work
independently. This is sequential file ownership, not a requirement to wait for
all tuning editor/MTS/native acceptance. Reconcile actual source changes before
pushing either shared seam. Cloud5 owns the combined storage integration.

Use one preparation context containing sample rate, a borrowed validated tuning
table and typed borrowed resource views. Update providers and hosts coherently
under the alpha policy, rather than retaining combinations of historical
`with_tuning`/`with_resources` adapters. Existing asset-free providers receive an
empty resource view and the same neutral/effective table. Validate all required
slots, types, bounds and readiness before candidate publication.

A resource view identifies its device slot, verified project asset/content and
derivation identity, original sample rate, supported channel layout and borrowed
immutable PCM. Preserve original-rate sample data separately from project-rate
clip derivatives. Represent supported mono/stereo layouts explicitly; refuse
unsupported channels or conversions instead of silently substituting sources.
The exact Rust names/layout are cloud4's implementation responsibility within
this contract; the types must remain independent of project/media/file I/O.

Preparation may calculate coefficients and retain bounded resource indices,
lengths and immutable identities. It must not hide PCM, an owning Arc, a dynamic
cache or a self-referential borrowed slice in a small analytic context or device
payload. Pass typed borrowed resources to the actual analytic/mutable processing
calls as well as preparation. A prepared resource identity alone is not a live
borrow. The host proves the corresponding root lease outlives every call and
held/releasing voice that can access it.

## ownership, accounting and real-time restrictions

Use the existing candidate/current/outgoing/history/source-retirement ownership
families. A project asset reference is not itself an audio lease. Resolve, decode,
validate and reserve on workers; retain the exact immutable resource owner used
by the prepared graph. Cache keys include content/derivation identity, original
rate/layout and every preparation input that affects coefficients, including the
effective tuning identity. Do not confuse equal authored bytes with equal inputs.

Charge each unique allocation once across pending work, candidates, current graph,
outgoing voices, Undo/Redo history and unreclaimed retirement custody. Reserve
the coexistence union before publication under existing caller ceilings. Preserve
small-state/context limits; neither a larger cap nor hidden shared ownership is
an admission proof. Define bounded slots, table size, query work and readiness;
unsupported, stale, canceled or failed candidates preserve prior canonical state,
Redo and applied audio. Keep resources alive across tuning-only retargets without
redecoding PCM.

The callback only borrows already validated resident views and performs bounded
DSP/events. It never opens files, decodes/resamples, waits for readiness, allocates,
locks a control mutex, calls MTS or releases final resource owners. Retire all
owning graphs/resources through existing control custody, including cancellation,
refusal, Stop/seek, replacement, shutdown and saturated retirement queues.

## note lifetime and required proof

Make gate-relative release and onset-relative natural end explicit host laws.
Sampler Trigger must reach its source-defined natural end after a short MIDI gate;
Stop/seek still cancel it. Tuning-only edits preserve fractional source position,
resampler/filter/envelope history and change future increments; they do not
restart the sample or reset its end. Bound duration/work and handle loops,
pitch-dependent remaining duration and unsupported cases explicitly. Apply the
same law in Arrangement, Session, keyboard/controller monitoring and export,
including carried/outgoing voices and density/tail admission.

Before sampler registration, prove original-rate/content identity, unique-union
accounting and lease retirement through the actual host path; clip-only retirement
proof is insufficient. Cover independent PCM/partition oracles, held/released
retunes, short-gate Trigger natural end, Stop/seek, stale/canceled/refused
publication, queue saturation, total/net-zero callback allocation and unchanged
memory ceilings. Cover combined save/open/Undo and explicit asset-preset refusal
until portable manifests/remapping exist. Record release workload/hardware and
measurements separately from Mac physical/listening acceptance.

No product decision is missing for this boundary. Concrete Rust representation
and bounded host implementation belong to cloud4; an actual ownership/lifetime
contradiction should be escalated through the coordinator before changing it.

## analytic processing checkpoint

PR373 now requires a borrowed DeviceResources inventory on inline analytic,
shared analytic and analytic continuation sampling. Preparation context remains
one effective rate/table/resource view. A synthetic, unregistered conformance
provider consumes original mono/stereo PCM through the actual callback; tests
cover age/gate, direct seek and partition equality, and actual callback total/net
zero allocations and frees. This is an ABI proof, not sampler/root admission.
Current engine hosts remain asset-free and explicitly pass the empty inventory.
The inventory is scoped to one prepared device configuration, so carried voices
must later receive their original source scope when a new graph reuses slots.
Lower mutable-instrument preflight/clock/processing and native/owned processor
processing/bypass callbacks now also borrow and forward the actual inventory.
The conformance effect consumes real mono/stereo PCM through the actual effect
trait; bypass preserves dry output while advancing its source cursor. The audio
allocation probe measures both analytic and effect callbacks with exact PCM sums
and zero total/net allocations and frees. Outer asset-free host chains still pass
the empty inventory: per-configuration graph lease scopes, complete coexistence
union accounting and retirement proof remain pending under unchanged budgets. No neutral shared-library
2773 implementation or historical-alpha compatibility work is introduced.

The unregistered mutable PCM conformance source now exercises actual committed/
scratch twins: exact original mono/stereo samples, stale content-identity refusal
before PCM/history mutation, rollback retry and committed continuation/exhaustion.
This free-running source intentionally does not implement sampler MIDI semantics.
The callback allocation probe includes pair preflight/process/commit with exact
PCM sums and zero total/net allocations and frees. ResourceUnavailable is an
explicit allocation-free runtime diagnostic; no registry admission is opened.
