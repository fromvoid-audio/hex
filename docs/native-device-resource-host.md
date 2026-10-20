# native device resource host admission

Pulse2929 / [PR514](https://github.com/PORTALSURFER/hex/pull/514) extracts the
source-facts and configuration-scope boundary from preserved PR389 commits
`3e1473b8` and `f757e8fe`. Pulse2783 / PR389 retains the broader native host
admission requirement, and Pulse2789 retains the complete usable sampler flow.
This increment does not open referenced processing or register a sampler.

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

The initial source family is the existing immutable **stereo** native constructor.
The primitive reports that actual layout and does not claim a mono native owner
or streamed-device support. Explicit native mono ownership, source delivery,
actual root/union accounting and host callback wiring remain required work here.
Unsupported channels must never be silently substituted.

## current normalized pcm identity

Content is SHA256 over the byte prefix
`hex.device-resource.original-stereo.v1` followed by a NUL byte, then source rate
as little-endian u32, frame count as little-endian u64, and each stereo f32 bit
pattern in left/right order as little-endian u32. Asset IDs are not part of content.
Derivation version is1; its digest is SHA256 of that prefix including the NUL.
This describes normalized original-rate decoded stereo PCM, not encoded file bytes.
Native file verification remains the media owner's separate responsibility.
The exact existing sha2 0.10.9 dependency is reused; no new hashing algorithm.

An independent Python hashlib/struct oracle pins the content digest. Actual
native-constructor tests check stereo samples/rate, configuration slot reuse,
equal-content/foreign-bank refusal, cancelled/work/metadata/shape failures and
unchanged input accounting. They report whether the pinned constructor gate was
supported; unsupported environments do not establish native ownership evidence.

## bounded borrow verification

The existing analytic/effect/mutable allocation fixture also consumes these
scopes over an actual sealed native input bank. Actual stereo PCM sums must match
its independent fixture, with zero total allocation count/bytes and zero signed
net count/bytes. Sources, catalog, scopes and runtime construction/destruction
remain outside measurement. Bank accounting and unresolved-handle validation
must remain unchanged afterward. The existing synthetic Mono/Stereo probe stays.

The actual-bank test runs under the existing `hex-audio` `allocation-probe`
feature. Unsupported native constructors fail that required probe; an early
return cannot qualify it. The ordinary engine source tests report unsupported
constructor branches explicitly; those branches are not positive bank evidence.
Qualifying local runs must execute the supported controls and preserve their
logs. These bounded headless probes do not prove native callback deadlines,
all fault/density paths or host publication/retirement.

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
implementation, hosted CI or app replacement is included.
