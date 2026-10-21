# sampler

Requested 17 October 2026: a native equivalent of Ableton Live **Simpler**, named
**sampler** in Hex. This does not target the separate Ableton Sampler instrument.
Pulse2679 owns the full request; [PR296](https://github.com/PORTALSURFER/hex/pull/296)
contains the first original playback-kernel prerequisite. A bounded first-load
browser action now fills an empty instrument slot with a canonical sampler and
supports computer-keyboard audition, frame-zero Arrangement playback/Stop and
offline Main export. Sampler
is not registered in the general device catalog. Stopped gain, pitch, source-region and Classic-loop editing are supported; other controls, replacement and user history remain unavailable. Current-format SaveNew/SaveCurrent and
original-content-checked reopen are supported through the normal project flow. No reference parity or
physical native interaction/listening acceptance is claimed.

## current source boundary

`hex-devices::sampler` separates scalar source-coordinate preparation from
borrowed original-rate Mono/Stereo PCM queries. The original `Sample`/`Voice`
stereo API delegates to that same law. `SampleShape` describes bounded DSP facts;
it never establishes physical storage, provenance or a source lease.

`PreparedSamplerPlan::bind_source_on_control` checks the actual borrowed view's
shape/rate and finite PCM once. Its prepared voices carry only copyable scalar
coefficients, slot and full content/derivation identity; slice marker storage may
be released after voice preparation. Rendering borrows the retained host
configuration and refuses missing/replaced identities, rates or layouts with
silence. There is no per-note PCM scan/copy, owning source handle, forged static
borrow or second cache. The host must still prove complete admission/custody and
retain the exact source configuration for current, carried and outgoing voices.
This adapter boundary does not register the device or open resource processing.

The original stereo adapter retains its control-validated immutable `Sample`
contract. Its shared interpolation specialization avoids checking those same
finite samples again per frame; raw borrowed-PCM queries still check the queried
endpoints and emit silence for nonfinite values. Both paths use the same source
coordinate, envelope and interpolation implementation.

Pulse2923 / PR509 compares the existing optimized `profile_sampler` fixture on
macOS arm64, Apple M5 Pro:48kHz,128-frame blocks,100 warmup blocks and2,000 timed
blocks per row, using `Instant` with allocation and voice preparation outside
timing. Source before extraction is91a4883a (same kernel as main0e425030).
The first generalized query was slower; that result is retained separately.

| Voices | Before p50 µs | Final p50 µs |
| --- | ---: | ---: |
| 1 | 1.417 | 0.792 |
| 16 | 16.875 | 12.375 |
| 32 | 41.667 | 26.708 |

All three output checksums match. Raw p95/p99/max values and the exact final
source are retained with the PR. These single-run timings include uncontrolled
machine load; they establish no native callback deadline or matched Live result.

The kernel implements these original laws, pending reference measurement:

| Area | Implemented contract |
| --- | --- |
| Region | Nonempty half-open source-frame interval; playback cannot read outside it. |
| Pitch | Root note60 by default; key tracking, ±48 semitones and ±100 cents; source/output rate ratio; linear interpolation. Slice keys select regions instead of transposing them. |
| Position | Q32.32 source position with u128 multiplication/modulo; large note ages retain loop phase without converting age to f64. Quantization error is at most half a Q32.32 source frame per output frame. |
| Classic | Optional nonempty loop contained in the region; initial preloop material plays once, then repeats the loop until gate/release ends. Interpolation crosses the loop seam. |
| Reverse | Reads backwards within the selected region/slice; the source loop interval is transformed into reversed travel coordinates. |
| One-Shot | Gate honors note-off; Trigger ignores it and runs to natural source end. No looping. |
| Slice | Explicit ordered source-frame markers, starting at region.start; up to128 MIDI-mapped slices, no unmapped-key fallback. Each stops at the next marker/region.end. Gate/Trigger supported. |
| Envelope | Original linear attack/decay/sustain/release, rounded output-frame durations; release starts from the level at note-off, including during attack/decay. Default is immediate unity with immediate release. Trigger ignores note-off but retains the authored attack/decay/sustain. |
| Level | Linear gain0–4; velocity depth0–1; original linear stereo balance−1–1 preserving both channels at center. Finite f64 interpolation/gain avoids intermediate f32 overflow. Values beyond finite f32 saturate only at its representation boundary. No normal-range clipping or normalization. |
| Rendering | Arbitrary age queries or caller-owned block fills; no mutable cursor or block-size-dependent result. Overflowed block ages emit silence rather than wrapping. |
| Lifetime | A nonlooping voice reports its natural output-frame length; a looping voice reports no natural end. Gate release and Trigger natural-end retention are handled separately by the runtime below; actual host custody remains required. |

Kernel limits: 8–192kHz input/output; 16,777,216 source frames maximum
(128MiB stereo f32); seven-bit MIDI; at most128 nonempty slices fitting above
the chosen base key; envelope times0–60seconds; sustain0–1. These are original
finite validation domains, not measured Live limits or host-admission promises.
Construction refuses malformed PCM, invalid intervals, nonfinite/out-of-range
controls and unordered/out-of-range markers. MIDI velocity zero is silent.
There is no anti-alias filter, warp, crossfade or zero-crossing snap in this
kernel. Linear resampling alone does not establish high-quality downsampling.

## checked voice lifetime preparation

Pulse2761 / PR369 adds `Voice::end_age_frames(gate_frames)`, an exclusive
onset-relative bound derived from the existing kernel law. Trigger returns the
natural source end, independent of note-off. Gate returns the earlier of source
exhaustion and gate-plus-release; a loop requires a representable gate-plus-release.
An exhausted nonlooping source still supplies a finite bound when that addition
would overflow. The query is not a silence/energy detector: silent PCM or an
inaudible envelope may end sounding sooner. The caller must separately check
absolute onset addition and retain resources through actual host retirement.
No Stop, seek, retuning, density, resource-admission or registry policy is added.

Eleven focused sampler tests pass, including three new lifetime fixtures that
compare the reported bound with actual positive-PCM exhaustion at five source
and output rates, five pitches, both directions and multiple gate lengths.
Loop release, Trigger slices after short notes, irregular partitions and u64
boundary refusal are covered. Strict device all-target Clippy passes. Final
exact-source full CI remains required; these are kernel tests, not host callback
or native/listening evidence.

## bounded event-driven runtime prerequisite

`BoundSamplerPlan::prepare_runtime_on_control` prepares all 128 key coefficients
against project tuning and the exact borrowed source identity. Classic and
One-Shot interpret the authored root note in A440 12-TET and divide each absolute
project note frequency by that fixed root frequency. Uniform project retuning
therefore changes root playback too; it is not cancelled by a tuned denominator.
Authored transpose/detune and source/output-rate conversion still apply. This is
an original contract, without a measured Live parity claim. Slice keys retain
their region-selection law and ignore project pitch retuning.
`PreparedSamplerRuntime` implements the existing Lattice mutable-instrument factory: at most 32 fixed voice slots and 1,024 events
per block, further restricted by the caller's voice/event/byte/tail ceilings.
The prepared factory and its two runtime twins retain scalar data only, without
owning PCM or marker storage. Fixed buffering latency is zero.

Actual NoteOn/NoteOff processing uses occurrence identity, output-frame event
ordering, the existing velocity law and Gate release. Trigger voices survive
note-off until natural source exhaustion. Event preflight simulates capacity and
lifetime changes in fixed scratch data before output or history changes; the
ordinary runtime pair supplies speculative commit/rollback. Reset clears both
histories, and exact compatible scalar configurations may copy active history.
Changed source identity refuses before rendering, preserving the prior state.
Valid expression events are currently ignored; modulation is not implemented.

Owning tests process original mono/stereo PCM through the actual
`NativeInstrumentRuntimePair`, including short-note Trigger retirement,
partitions, capacity refusal/rollback, source/slice refusal, absolute and
nonuniform tuning, velocity, gated release, reset/quiescence and caller-budget
refusal. Allocation-free
processing is checked by source inspection of fixed arrays and borrowed views;
this is not an instrumented callback or physical deadline result. No new native
audio allocation fixture, host publication, registration or UI is added here.
The existing standalone kernel profile above does not measure this runtime pair.

## host integration contract still required

The current 4KiB device-state and analytic-context bounds and separate 2MiB
active/retained context budgets do not admit ordinary samples. Do not increase
them or encode PCM in opaque state as a shortcut. Existing verified streaming
admission is for audio sources; it does not automatically support arbitrary
per-voice pitched, reversed, looped sample access.

The neutral device contract must represent stable asset references, independent
of device-specific payload decoding in storage/history. Media workers decode or
prepare random-access resources before candidate admission. The engine borrows
prepared resources while control retains their owner/lease. Count, byte, page
and outstanding-release bounds must include current plus captured outgoing
voices. Replacement, seek, loops, Stop, cancel, project Open and failed candidate
compilation must preserve the last valid project/audio and retire resources only
after audio quiescence. A queued or carried release must never outlive its sample.

Asset import plus instrument assignment must be one undoable prepared edit.
Project reopen and presets must resolve the same original sample, region and
markers; missing media must produce an explicit recoverable refusal. Original
user filenames and paths keep their case. No device-specific storage DTO or
callback filesystem work. Arrangement, Session, keyboard monitoring and export
must consume one generic preparation contract; none may synthesize substitute
PCM when an asset cannot resolve.

The registered adapter must translate occurrence-aware note lifecycle into
bounded voice ownership, including Trigger after short notes. Source lengths,
pitch excursions, tails, transactional preflight and callback retirement must
be proved through actual prepared host paths. No host registration is added by
this prerequisite because those requirements have not been met.

## full device acceptance

The user's one-to-one objective includes all three modes, sample replacement and
waveform/region/loop interaction, reverse/crop/normalization, mode-dependent
fade/snap behavior, slice creation/editing/playback styles, warp algorithms and
tempo behavior, filter types/models/routing and envelopes, LFO/modulation,
pitch/spread/glide, voices/retrigger/stealing, controller/automation behavior,
presets and exact defaults/ranges/units. Inventory each against the pinned
reference rather than assuming these kernel laws are Live's laws.

The panel must compose the shared native frame/controls and reusable waveform
interaction primitives. All visible controls need actual canonical/DSP or host
functions. Display is lowercase through `theme::text`, with bundled Ioskeley Mono.
No proprietary code, factory samples, presets or artwork are included.

Acceptance requires malformed/failed import and save/open/history fixtures,
independent PCM oracles for all modes, shared Arrangement/Session/monitor/export
checks, actual callback total/net allocation and retirement probes, bounded dense
voice performance, and the user's separate native UI/listening checklist.
The current kernel tests do not satisfy those host or reference gates.

## evidence

`cargo test -p hex-devices --locked` passes296 unit tests plus affected
integration suites (two pre-existing ignored proofs remain ignored).
Eight new fixtures cover source-coordinate/rate/pitch oracles, seam interpolation
and huge-age loop phase, reverse, ADSR/note-off, Trigger/Gate, slice selection,
stereo/velocity/finite extremes, partition/query invariance and malformed input.
`cargo clippy -p hex-devices --all-targets --locked -- -D warnings` passes.
Allocation-free kernel rendering is established by source inspection, **not an
instrumented host callback probe**. No engine/audio/storage/UI behavior changes.

Standalone release profile on Linux6.18.55 x86_64, virtual AMD EPYC9V74 (five
logical CPUs exposed), Rust1.94.0, 48kHz, 128frame blocks, one-second original
stereo sine PCM, fractional pitch and nontrivial loop, 100warmup plus2,000measured
blocks per row:

| Voices | p50 µs | p95 µs | p99 µs | maximum µs |
| --- | ---: | ---: | ---: | ---: |
| 1 | 1.592 | 1.612 | 4.537 | 94.924 |
| 16 | 25.128 | 61.493 | 125.991 | 366.313 |
| 32 | 50.166 | 97.978 | 166.301 | 621.209 |

Reproduce with `cargo run --release -p hex-devices --example profile_sampler
--locked`. Timed scope includes output clearing, voice sampling/summation and
black-boxing the output; PCM/voice construction, timing storage, sorting and
checksum accumulation are outside it. No outliers removed. Shared-cloud load
is uncontrolled. There is no prior sampler baseline, host graph, physical
callback or comparison with Live. These measurements establish only standalone
throughput for this fixture, not audio deadlines or scalability acceptance.

The macOS reference app is unavailable in this Linux cloud environment. No new
installed-build/hash check or reference experiment was possible. The existing
documented reference is Live12.4.6; recheck its build/hash before taking captures.
Record source settings, original fixture hashes, exact actions, numerical
tolerances and per-feature results using the repository research template.

## loaded authored state and control preparation — pulse2961 / PR545

`sampler::state::SamplerState` encodes current finite musical settings in
`NativeDeviceState` under `hex.instrument.sampler`, current state version2.
It contains no PCM or file path. The existing Classic/One-Shot/Slice, region,
loop/reverse, pitch/gain/balance/velocity and amplitude-envelope laws are retained;
no new DSP is added. Strict decode refuses unsupported versions, malformed or
truncated payloads, reserved fields, nonfinite/out-of-domain values and invalid
source-slot declarations. This is the current alpha format, without historical
readers or conversion. Replacing settings preserves actual host gains and the
same asset-reference storage; source replacement remains a separate host edit.

`prepare_runtime_on_control` is the production state-to-runtime boundary for the
next prepared-project/export compiler. It requires one actual source in slot1,
resolves its exact asset identity, validates actual source shape/rate and authored
bounds, then prepares scalar coefficients against the effective project tuning.
Unbound settings may be assembled on control but cannot prepare a loaded runtime.
The generic registry's asset-free admission rule remains unchanged; no builtin,
browser or native publication registration is added.

Owning tests cover payload roundtrips/refusals, actual decoded-state PCM, source
refusals and authored gain/pitch/region plus absolute project tuning in a real
runtime pair. The real mono/stereo WAV producer fixture now passes through this
canonical state and control preparer before its literal PCM oracle. Marker and
state storage can retire after control preparation; actual source ownership must
still survive processing. These checks do not complete host/export custody,
project-to-WAV export, controls, app playback or optimized delivery.


## unassigned native source worker admission — pulse2962 / PR546

`Service::with_native_reservation` opts its initial resident Session into an
explicit `ResidentNativeReservation`: ledger data/handle/root rows, retained
bytes, construction bytes and maximum frames per finite PCM scan/hash pass.
Existing constructors remain opt-out; `Request::ImportDeviceAsset` then refuses
visibly. The request runs the existing bounded WAV decoder and consumes its
original-rate normalized stereo PCM through the PR515 constructor bridge. It
returns the actual `Created::Asset` without inserting a track or clip.

One canonical asset namespace has two mutually exclusive custody arms: the
ordinary clip media map and `ResidentDeviceAssets`, a bounded Vec of asset IDs
and boxed sealed input-bank/catalog roots. No original PCM is copied, resampled,
aliased into the clip map or exposed through an owning clone. Mono files use the
existing explicit stereo normalization. Source rate, frames and path remain
original. `ImportDeviceAsset` stores checked expected conversion metadata, not
a materialized clip derivative; `AddAsset` still requires project-rate PCM.
Arrangement and Session clip resolution visibly refuses device-only IDs until
an accounted derivative path exists. Undo can leave source custody retained,
just like ordinary clip media, within the same finite limits.

The reservation domain starts with owned decoded source admission. Actual PCM
Vec capacity, constructor-paired wrapper, native bank/source/obligation metadata,
catalog backing, boxed root payloads, destination Vec and ledger capacities are
inventoried together; old destination backing overlaps new backing only in the
construction charge. The source Vec moves through bank preparation; its
allocation/obligation/catalog buffers remain in the final root, without temporary
owning clones. Legacy resident PCM uses its existing unique-capacity accounting
as an explicit allowance in this same check; it gains no native provenance.
Ordinary imports, including batches, check the combined frame and byte limits.
Repeated borrowing does not create a handle; separately decoded equal-content
files remain separately charged. Allocator-private overhead, stack workspaces,
ordinary project/history/waveform/view preparation (including the transient
nonowning ID set), encoded file buffers and decoder scratch/pre-admission decode
peak are outside this inventory. Existing independent file/decode limits remain.
This is an admission inventory, not a total process/RSS or lifetime proof.

Staging preserves the old vector/canonical state on refusal. The Editor's
candidate closure prepares native custody and ordinary playback, then checks
cancellation before fixed source moves and the infallible canonical history
commit. No callback owner is published. File decode, staging failure drops and
Session teardown remain on the `hex-session` control worker by ownership and
control-flow inspection; these checks do not instrument arbitrary destructors.
At PR546, New/Open kept their existing constructor policy. PR547 below extends
the explicit service reservation across replacement, without native source reopen.

Focused tests cover the real worker result, literal decoded PCM/Vec identity,
source metadata at a different project rate, exact/one-byte-under retained and
construction limits, rows/work/frames, independent equal-content sources,
pre-cancel and staged callback refusal, ID exhaustion, ordinary import limits
and explicit clip refusal. Next: canonical sampler assignment through normal
PreparedRevision and the actual export worker with an independent WAV PCM oracle,
extending this same reservation through captured export and Session custody.
Native publication/checkpoints, app playback, native controls/history/reopen,
full native qualification and optimized delivery remain outstanding.


## private canonical assignment and offline export — pulse2963 / PR547

`Service::with_native_reservation` now accepts `AssignSampler { track, asset }`
for an admitted original-rate source. It creates or replaces canonical loaded
sampler state and compiles a normal `PreparedRevision`. Its sampler factories
contain scalar configuration only. A private service sidecar owns the exact
constructor-paired source bank, catalog and per-device scope. Configuration,
rate, effective tuning, source references and host gains must match that scope;
the generic lattice registry remains asset-free.

The existing export worker and `SessionPlayback` render frame-zero Main through
that borrowed scope into the ordinary controlled WAV publication path. No second
sampler scheduler or DSP renderer is introduced. Public Session preparation
callbacks, canonical audio sources and `Event.playback` do not receive this
source-bearing snapshot. Generic runtime constructors, native storage admission,
mutable checkpoints and nonzero seeks refuse the private-source class. The
service reports offline export support and unavailable native playback. History,
save/reopen reconstruction, preview, recording and runtime gestures still refuse.
CapturedMix/stems and nonzero-range export remain unsupported for this slice.

The explicit resident reservation is a service capability carried into New/Open
before admitted construction. Streaming documents refuse sampler operations.
Opening loaded sampler metadata refuses before ordinary media decoding; it does
not silently turn the original source into a clip derivative. A captured old
export remains independent of the replacement document.

Admission visits the actual union of current/staged native stores, sidecars,
scalar factories, current and captured-export legacy PCM and the existing checked
runtime construction allowance. Shared native DATA is charged once while each
physical source wrapper/handle is charged separately. A sealed non-cloneable
outbound obligation accompanies exactly one fresh job capture. Before dispatch,
inspection visits the actual job owner; after dispatch, it visits the retained
anchor and charges the absent job field using its constructor-paired identity.
Actual owner plus token, duplicate token and wrong snapshot/job bindings refuse.
The token does not traverse or clone source DATA. It and its fixed lane container
stay charged until matching post-drop terminal or confirmed worker join; a
changed Arc count, cancellation, UI poll, disconnected notification or timeout
cannot discharge it. Generic ledger handle checks are unchanged.

Private completion follows destruction of the actual renderer/runtime/job source.
The Session worker drains it while idle, before admissions and during public-event
backpressure. Failed queue transfer destroys the returned job before the anchor.
Service Drop sets shutdown and remains nonjoining; the control worker closes the
export input and joins the exporter while retaining its anchor. UI-result
backpressure observes shutdown so it cannot deadlock that join.

This domain starts at decoded-source admission. Original encoded buffers, decoder
scratch/pre-admission decode peak, ordinary project/history/view/waveform and nonowning selection
bookkeeping, allocator-private overhead, stack and total process RSS remain
outside it. The existing mutable runtime/preroll 32MiB limit is independent and
unchanged; immutable source PCM does not inflate that private-state limit.

Focused evidence includes real mono and stereo 8kHz source import into a 16kHz
project, canonical assignment and literal half-step PCM16 WAV comparison;
actual scope survival and no public graph source ownership; token conservation
and duplicate/foreign refusal; exact/one-byte-under retained and construction
limits; failed queue transfer; a held export across New, ordinary media Open and
new source assignment; cancellation and create-new file failure followed by a
successful export. A feature-only final-field observer retains no source owner
or weak alias and confirms source bank/catalog teardown on the control thread,
without UI export polling, including prompt Service Drop during blocked export.
These are headless control/export checks, not native UI, device deadlines,
listening, full release qualification or optimized delivery. Exact required
merge-run evidence belongs to the canonical PR receipt.


Ordinary track/clip/Session import, whole prepared batches and MIDI bounce also
check the actual staged original/derivative PCM against current plus captured
export custody before their existing atomic commit. No reservation ceilings are
mutated and no summary totals are subtracted. Legacy dedup borrows an iterator;
it creates no owning PCM handle or temporary dedup heap buffer. The regression
reproduces a 4-frame captured export plus a 4-frame original and 8-frame
converted import incorrectly fitting a 15-frame ceiling. Exact/one-under track,
clip and whole-batch cases now preserve canonical/media/history state on refusal.


Private sampler Edit uses the same media-creation guard as public Session Edit.
AddAsset, ImportDeviceAsset and ordinary prepared-media/conversion commands
require their real prepared import/bounce transaction; metadata-only Edit cannot
invent unverified source custody. The owning regression records both AddAsset
and ImportDeviceAsset accepting nonexistent files before the fix and refusing
after it, with identical project/history/revision and independently checked
subsequent sampler export PCM.

## fresh native arrangement current — pulse2964 / PR548

The desktop's explicit resident reservation now supports canonical sampler
Arrangement playback from frame zero, Stop/reset and replay. A synchronous
Session-worker borrow supplies the exact original PCM and canonical source
context to `NativeAudioSink`; the sink copies original-rate frames into a
persistent, never-published constructor cell. No path reread, resampling or
source-bearing Event is involved. The public canonical snapshot carries only
its existing legacy bindings and exact context; its public quote/constructor
continues to refuse sampler preparation.

This is one closed constructor class: finite Arrangement MIDI, resident source
assets, loaded sampler instruments and the existing Utility-only effect chain.
Project and MIDI-source loops, Session MIDI/launches, routing/return devices,
mutable handoff, tails, checkpoints, live expression, preview and nonzero seeks
remain closed. Source/configuration/topology edits refuse before canonical
commit while this limited native owner is active, except stopped gain/pitch/region/Classic-loop
transaction described below. New/Open explicitly retires it. Current-format
reconstruction is supported; user history remains separate work.

Construction regenerates the catalog, device scope and authored tuning against
the newly copied bank. The actual `SessionPlayback`, sampler runtime twins,
finite event backings and bank handles participate in the existing constructor
inventory and storage seal. The resource facade borrows the same bank into the
existing `process_segment_with_resources` path. It introduces no second sampler
scheduler, callback source lookup, preparation or owner clone.

Preflight and sealing happen before disturbing the old output. A targeted UI
message relinquishes only the matching old Client. Successful acquisition of
an exclusive fresh output lease proves old output retirement. The intact cell
then moves into a virgin publication reservation; exact generation, editor
revision, rate, source capacity and actual storage checks precede the move.
The engine root, runtime addresses, bank and scope survive it. Destination
staging reseals actual storage before the new Client becomes visible. A cleared
Option, UI acknowledgement or Stop is never a source-release proof. Stale and
failed unpublished cells are cleaned on control; an incomplete cleanup remains
in bounded quarantine and blocks another preflight rather than being dropped
or retried by a destructor.

The desktop derives its finite resident policy from supported constructor
cardinalities and a pure quote of the actual fixed native publication/control
backings. That quote is neither the configured DATA ceiling nor an RSS claim;
fresh construction checks its actual backing against the quote. Admission adds
native current/preflight/retiring/control holds to the same Session/export
union. Original and copied PCM share the unchanged 8,000,000-frame ceiling;
immutable metadata does not raise the independent 32MiB mutable-runtime limit.
Retained and construction-peak bounds remain distinct. Holds survive until
actual cleanup or a fresh exclusive lease proves teardown. Capacity changes
request a fresh synchronous borrow and full admission, without granting space.

Headless owning checks exercise real Service WAV import and assignment through
the desktop adapter and eligible CURRENT: mono/stereo 8kHz originals in a 16kHz
project, A440/A880 targets, literal PCM, note gate, Stop and identical replay.
The fixture removes its source file before assignment. A concurrent independent
export checks literal PCM without changing the native clock. Callback probes
cover install, rendering, gate, Stop and replay with zero total and signed-net
allocations/frees. Additional cases cover insufficient shared frame capacity
without old-output disturbance, New during drain, failed fresh output and
new-context recovery, wrong context/rate/capacity/DATA adoption refusal, and
control cleanup. A protocol regression checks lost Submitted metadata and
stale targeted Client-release messages; it is not native input evidence.

These are source/headless checks. Native UI interaction, listening, physical
device deadlines, optimized application delivery, sampler controls, general
editing/history and save/reopen remain unverified or unsupported. The exact
required merge-plan receipt belongs to the canonical PR, independently of this
description.

Final-review boundary checks also cover the ordinary graph already playing
before sampler preparation. Resident forwarding records actual graph PCM
capacities under its existing finite request-slot mutex. Suspending that same
slot and reading its obligations is atomic with respect to another forwarding
request; transport and priority Stop remain independent. Admission refreshes the
native hold after suspension rather than trusting an earlier worker sample.
Accepted current, queued and returned graphs stay charged until actual control
owner destruction. Refused owning responses conservatively keep their charge
through Root teardown, including after response collection. Verified constructor
resident copies carry their preparation/submission capacity hold through actual
return recovery; cancelled or failed preparation residuals conservatively stay charged until teardown.
Shared PCM across separate publications may be conservatively charged more than
once. These are selected PCM/construction roles, not total process RSS or an
inventory of arbitrary external legacy graphs.

New/Open cancels an old sampler context before requesting or polling another
output. A new ordinary output therefore receives its ordinary control/runtime
hold even if opening completes before the next private sampler input. Failed
cleanup retains custody and prevents another open. An already admitted canonical
offline assignment survives native budget/preparation/device-open refusal, with
truthful unavailable/refused audio status. This does not roll back the user's
edit. Canonical import/source admission remains atomic, and active limited native
replacement still refuses before canonical commit.

Native MIDI compilation now starts after the other fixed constructor backings
exist. Before compiler entry, admission checks their actual simultaneous bytes,
all tracks' finite final MIDI bound and conservative temporary clip-count,
note-key growth (including old/new allocation overlap), stable-sort, onset and
density-edge workspace. Retained and construction limits
are checked separately; the historical workspace overlap remains in the storage
seal after temporaries are freed. The pinned Rust1.94 driftsort quote includes its 48-element scratch minimum
above the 20-element insertion path, using max(N,48) element slots; this scratch
does not overlap density sorting, and the quote safely sums phase bounds. Empty MIDI clips still reserve clip-count workspace. Owning exact
and one-under tests cover empty-clip and 3/21/256 nonoverlapping-note fixtures and observe
that a refused compiler was never entered. Removing each of the three review
repairs reproduces its focused failure; raw RED/GREEN evidence is retained with
PR548's local artifacts. The interrupted earlier audit is not merge evidence.


## first browser load — pulse2965 / PR549

With a resident document, select a track containing authored Arrangement MIDI
notes and an empty instrument slot. Right-click a WAV row and choose `load into
sampler`. The intent captures the original path, track, project generation and
revision. Later selection/focus changes do not retarget the load or get reversed
by its completion. A pending row menu offers `cancel sampler load`; cancellation
is cooperative before canonical commit. A late cancellation cannot pretend to
roll back an already committed assignment. Missing/corrupt/oversize media,
stale targets, occupied slots and the unchanged resident/source limits refuse
without an asset orphan or another history commit.

One Session worker request stages original-rate PCM and prepares one neutral
Project command that creates the asset, sampler state and binding in one Editor
transition. The candidate sidecar borrows the actual staged source before commit;
no UI import-then-assign sequence or public PCM owner is introduced. Canonical
commit and native preparation are separate: output failure retains the valid
assignment for offline export and reports unavailable audio. Active sampler
replacement remains refused by the existing native owner guard.

The panel displays canonical sample identity and read-only readiness/limitations.
Frame-zero UI Play requires callback-confirmed source context, canonical revision,
project generation and output identity. There is no public seek candidate for
private sampler CURRENT. Nonzero seek, Pause, Session launch, active edits and
history remain explicit unsupported operations. The later keyboard increment
below extends this same closed CURRENT route without enabling live edits. Save/reopen is
added by the next increment below.
Stop resets the existing native runtime; Play then starts from frame zero again.
Existing browser Enter and drag import behavior is unchanged.

Owning checks cover the one-commit/Undo project boundary, staged preparation
refusal/late cancellation with actual source custody, real worker mono/stereo
mismatched-rate file export, duplicate/stale/invalid requests, correlated UI
cancellation and actual rendered right-click/menu -> real Service -> actual
native CURRENT -> keyboard Play/Stop/replay literal stereo PCM with zero
callback total/net allocations in the measured fixture. The headless output-open
factory is the only substituted native boundary. Feature-only UI startup uses
private temporary settings/home/discovery roots before any startup I/O. GPUI's
NoopTextSystem cannot verify bundled font registration; production font startup
is unchanged. These checks do not establish physical device deadlines, visuals,
listening or installation. Exact required merge evidence belongs to PR549/Pulse.

Finite future user checklist, after a separately qualified optimized build:

1. Select a MIDI track's empty instrument slot, then right-click an original WAV
   and choose `load into sampler`. The later keyboard increment below removes
   the original PR549 requirement to author an Arrangement note first.
2. Change selected track while loading; confirm the original target owns the
   sample, its canonical filename is visible, and focus stays where moved.
3. Play from start, Stop, then Play again; compare both starts and stereo image.
4. Repeat with a mono WAV and a WAV whose rate differs from the output; export
   Main and compare playback. Record the exact source/build and device settings.
5. Check missing/corrupt media, pending cancellation and an occupied slot refuse
   clearly. Save, choose New again to confirm, then reopen and compare sound.
   Replacement and nonzero seeking remain unsupported; the keyboard has its own
   checklist below.


## save and reopen original sources — pulse2906 / PR499

The normal SaveNew/SaveCurrent file-dialog, document, dirty-state and FileToken
pipeline now supports private sampler projects. A save snapshot copies expected
content facts from the actual immutable native catalog for every canonically
present device-original asset, including unassigned sources. This projection
adds no user edit/history step and transfers no PCM, bank, token or runtime.
Relative paths use the existing original `media_base`; Save As normalizes saved
references without retargeting the live model to the new directory. Existing
create-new refusal, external-write conflict and previous-save recovery remain.

Strict current schema34 adds a required complete `asset_sources` inventory.
Every asset has an explicit ordinary/null classification or device-original
facts containing lawversion1 and exactly32 digest bytes. The existing catalog
law hashes the domain `hex.device-resource.original-stereo.v1` plus NUL, original
u32 sample rate and u64 frame count in little endian, then every normalized
original stereo f32 bit pattern in little endian. Mono expansion follows the
existing decoder. This identifies decoded original sound, not WAV encoding or
header bytes; it grants no native custody. Unknown laws, ambiguous/malformed
inventories and unsupported preceding alpha33 refuse. No migration, guessed
identity, reset or destructive conversion is performed; unrelated explicit
historical importers remain. Older writers refuse these facts instead of dropping
the classification.

Open rebuilds all fact-marked originals, including unassigned assets, at their
saved AssetIds through the existing exclusive decoded-buffer/native bank path
before any `PreparedImport::at_rate` alias. It compares the actual reconstructed
catalog with saved facts. Ordinary assets keep their existing derivative path;
device-original/clip reuse without an accounted derivative refuses. The existing
reservation first bounds upcoming PCM construction; actual staged original/legacy
owners join the old Session, current/native output, captured export and outbound
obligations in the shared retained/peak/frame union. The complete private sampler
snapshot and final replacement admission precede canonical adoption. Staged
failure disposes on control and preserves old project/history/media/document and
sound. Hardware output-open failure after a valid canonical commit remains a
separate truthful native failure; it is not a content-open success guarantee.
These are bounded audio-storage checks, not total decoder-buffer or process-RSS
measurements.

Owning evidence covers actual first load -> SaveNew/Current -> New/Open ->
mono/stereo original-rate reconstruction, literal custom bounds/loop/envelope,
host trims and A880 tuning -> matching independent exported PCM after both
referenced/unassigned WAVs are removed. Missing, corrupt, oversized, same-rate /
same-frame changed PCM, missing facts and cap refusal retain the old state and
sound. Relative/Save As paths and a held-export exact/one-under frame union are
covered. Actual native rejection preserves the installed output identity,
constructor ACK and literal old PCM, including predecode admission against the
real held native frames. Valid longer WAVs are refused by the decoder's saved-frame
limit before normalized PCM construction, for both original and ordinary assets
in mixed projects. Replacement preparation quotes the actual old Session before
compiler entry; exact/one-under byte tests cover unassigned native originals and
legacy source/derivative PCM, with zero compiler entries on refusal.

Headless rendered browser/keyboard/picker tests drive the real Service and
NativeAudioSink for stereo and mono+A880: SaveNew, repository recovery-verified
SaveCurrent, ordinary New confirmation, Open, fresh matching native CURRENT,
then zero-total/net-allocation literal PCM Play/Stop/replay after the WAV is
removed. Open-picker cancellation retains the actual current output/context;
existing stale picker/confirmation guards remain shared. Only the existing
headless backend replaces physical output; the driver closes actual callback
carriers and waits real lease/install proof. Physical UI/fonts/audio/listening,
optimized build delivery and installation are unexecuted. Controls, replacement,
user history, Session launch and nonzero seeking remain open. The following
increment adds live computer-keyboard audition.
Exact required merge/review qualification belongs to canonical PR499/Pulse2906.

## native computer keyboard — pulse2966 / PR550

An empty MIDI track can load a sampler and play it with the existing computer
keyboard toggle/keys. Authored notes are optional. Native activation requires the
actual callback-installed CURRENT, exact output/context/audio revision and
canonical track/device. The existing UI input remains solely on control; public
Events acknowledge metadata and never expose sampler sources or a new graph.
The generic registry/ordinary source-free keyboard admission stays closed.

The constructor prepares one independent live pair per sampler configuration from
that configuration's existing factory. It uses the same sampler DSP and exact
borrowed CURRENT bank. Actual Box/Vec/pair/input/expression storage is charged in
the nontransferable Session inventory. Pool preflight precedes pair construction,
the existing32MiB runtime limit remains, and the final constructor ledger still
checks actual retained/peak DATA and closure. Source, PCM, history and retirement
ownership do not move into the keyboard input. The native helper returns its Box
directly, avoiding the large by-value temporary that overflowed the Session worker
in the first integrated debug test; worker stack/runtime caps were not raised.

A fixed scalar mailbox is inline in the already-owned audio Status and its
existing boundary quote. Control is the sole activation-binding writer; the UI
is the sole per-key writer. Each has its own checked sequence, and every desired
word carries its activation epoch. The existing17-key word/token/velocity law is
shared; callback readers use at most three binding attempts and three reads per key
within each attempt, without waiting. Epochs and sequences refuse exhaustion instead of wrapping.
Wrong-output/context/revision, old epochs, unacknowledged or malformed input cannot
start a voice. Refused malformed activation preserves the prior live voice.

The UI reserves an epoch before asking the worker to activate. Key down/up writes
directly through the existing Client, preserving polyphony while the worker is
busy. UI/control cancellation and the sole callback writer have separate
monotonic revocation words; callback cancellation/refusal uses bounded atomic
load/store, without an atomic-max retry loop. Both frontiers defeat late
activation; an old cancel cannot revoke a newer activation. Control Stop captures
its reserved epoch prefix; callback Stop revokes the actual runtime activation,
so a later fresh press remains valid even before that Stop is consumed. Focus loss, toggle
off, track/project/output changes, Stop and shutdown invalidate the activation.
The existing physical-key cancellation law requires a fresh press; old held
words/autorepeat do not restart a stopped voice. Capacity/pitch refusal reports a
keyboard error and keeps the canonical/native source intact.

Stopped rendering advances live voice/release age while both project clocks stay
frozen. Playing rendering sums live and authored voices before the shared
instrument output trim, Utility/local effects, track pan/fader/sends and Main.
Each applies once. Keyboard begin/commit/rollback participates in the existing
whole-block transaction. No callback preparation, allocation/free, source-owner
replacement or final owner destruction is introduced.

Owning checks include actual rendered browser load -> SaveNew/Current -> New/Open
-> real keyboard toggle/key down/up -> native CURRENT PCM, stereo/default and
mono/A880 at8k source/16k output, both stopped and alongside Arrangement playback.
An empty-track variant proves load/reopen/audition without authored notes. Looped
polyphony has independent literal pitch/velocity and two-frame release oracles;
with Service deliberately blocked, one key releases while another holds, a fresh
press retriggers, and focus cancellation silences immediately. A separate gate
pauses activation after worker validation: cancelled late activation stays silent.
Stop/post-Stop fresh-activation ordering, old-autorepeat,16-voice overflow and visible refusal are covered, with zero
measured total and signed-net callback allocations/bytes. Constructor checks cover
host/Utility/track gains once, velocity100/127, exact/one-under DATA/runtime limits
and zero pair-construction entries on preflight refusal. Bounded mailbox tests
cover stale/torn input and counter exhaustion; ordinary keyboard tests remain.
These are headless source/PCM/allocation checks, not physical device deadlines,
font/pixel/input/listening or optimized-installation acceptance. Required exact
merge evidence belongs to PR550/Pulse2966.

Future finite user checklist after a separately qualified optimized build:

1. Load a mono or stereo WAV into an empty MIDI track, enable the keyboard toggle
   (`m` when editor/MIDI focus permits), and play/release `a`, `k` and chords.
2. Use `z/x` and `c/v`; compare pitch/velocity with the project tuning. Stop and
   refocus/toggle; held-key repeats must not restart notes without a fresh press.
3. Add an Arrangement MIDI clip, play from start and audition live notes alongside
   it; confirm release, gains, stereo image and Stop.
4. Save/New/Open and compare the sample and tuning; changing track/window focus
   must stop the previous live activation without editing another instrument.
5. Confirm missing/refused loads and voice-capacity refusal remain truthful.
   Other controls, replacement/history, Console launch/recording and nonzero seek remain
   separate work. Record exact tested source/build and physical results.


## stopped native gain — pulse2967 / PR551

The sampler panel's shared numeric gain field reads actual authored gain, with
linear range0–4 and unity1. A completed numeric confirmation or drag release
submits one captured generation/revision/track/device edit. Intermediate widget
changes remain local previews. The narrow codec patch preserves every other
sampler setting, source reference and host input/output gain. Sampler remains
outside the generic registry. The same persistent widget survives accepted
revision changes; each gesture captures its initial revision and stale commits
refuse. Retiring its focused control restores editor focus only if that exact
retired field still owns focus, preserving Save/New/Open shortcuts.

Gain edits require the actual native callback to acknowledge the stopped
CURRENT, exact applied source and captured command prefix. Fixed scalar fence
storage lives inline in audio Status and its existing boundary quote. The sole
control writer uses checked odd-request/even-idle identities; callback owns
ACK/abort-retirement observations. Commands admitted before the cut retain their
semantics: an earlier Play runs and refuses the edit. New/raced non-Stop commands
are refused explicitly; their producer serial prevents replay after release or
later edits. Stop retains priority. Existing keyboard key-up/cancel continues,
while new epochs/bindings are excluded. Terminal revocation is checked against
the actual sounding epoch before reading or deferring a newer mailbox binding;
a fenced new binding cannot mask cancellation of the previous voice. Callback
readers never wait or allocate.

Session polls the fence without blocking request processing. Invalid/stale,
playing, cancelled or over-budget preparation preserves canonical state,
revision/history/document and installed configuration. A lost-callback timeout
finishes the edit with an error, retaining one scalar abort fence until callback
consumes the captured prefix. New gain attempts remain refused until that
retirement; New/Open/shutdown can instead retire the actual old endpoints.
This preserves an earlier accepted Play even if the callback returns late.

Inside the existing prepared canonical transaction, a synchronous borrowed
candidate/original-source input admits the actual old/new/private/export/outbound
and native/control overlap, then creates and seals one separately retained
pending cell. Its quote and actual ledger include sampler keyboard pools and
PCM. Canonical commit follows successful preparation and a final cancel check.
Only then does control revoke the old keyboard epoch, drain the old output and
adopt/stage that exact sealed cell, with no second PCM copy or seamless voice
carry. Actual acquisition of the new exclusive output lease proves retirement.
An incomplete unpublished cleanup remains quarantined, without implicit retry.

A hardware-open failure after drain retains the new authored gain and supports
Save/offline export. It reports audio unavailable, not applied or rolled back.
Explicit audio retry can rebuild a refused sampler output; requesting that retry
on an already working sampler refuses before disturbing it. Only a matching
actual native installation establishes readiness. Gain adds an ordinary authored
history step. Same-source configuration Undo/Redo now uses that lifecycle; see
[stopped configuration history](#stopped-configuration-history).

Owning checks exercise the real numeric field through fresh native CURRENT,
literal keyboard and Arrangement PCM at gain1/0.5/0, SaveCurrent/New/Open,
unchanged source/other settings, stale/invalid/cancelled/tight-budget refusal,
earlier/later Play and Stop, actual A-cancel/B-bind-before-fence cancellation,
lost-callback abort followed by actual callback and New/output teardown, postcommit hardware-open failure plus Save/export/retry,
and exact prepared-to-draining cell identity. Covered callbacks, including
publication, measure zero total allocations and zero signed-net counts/bytes.
These are headless native-path tests; physical display/input/device/listening
and optimized build delivery remain unexecuted. Required exact source review
and merge-plan evidence belong to PR551/Pulse2967.

Finite user checks after a separately qualified optimized build: stop transport,
confirm gain0.5/unity/0 changes keyboard and Arrangement level once, Save/New/Open
and verify it persists, then try editing while playing and confirm refusal.
Record the tested build/source and physical results. Waveform
and remaining controls, replacement, history and broader playback stay open.


## stopped native pitch — pulse2968 / PR552

The panel adds a persistent shared numeric **pitch** field beside gain, in
semitones from−48 to48, default0. Fractional authored values remain supported;
coarse/fine keyboard steps are1/0.1 semitones and the display uses two decimals.
One final confirmation or drag release submits a closed `Gain`/`Transpose`
intent. Both controls share the same one-pending stopped transaction described
above, including exact callback fence, precommit admission/sealing, cancellation,
old-output retirement, fresh output and refused-only retry. No additional native
publication path or DSP law is introduced. Each retained widget captures its
own gesture target; a revision change cannot silently retarget an old gesture.

Only the selected authored field changes. Root note, detune, gain when changing
pitch, source references, host trims and all other settings retain exact state.
The immutable authored A440 root denominator and absolute effective project Hz
numerator still compose once with source/output rate and authored transpose.
Slice keys still select regions; the authored transpose remains applicable.

The owning rendered regression exercises gain0.5, pitch+12/−12/0/0.5 and gain0,
with project A880 and original8kHz stereo at16kHz output. Independent literal
keyboard and Arrangement PCM covers each value; SaveCurrent persists exact
state and actual New/Open at gain0.5 and pitch+12/0.5 reaches fresh native CURRENT.
Typed patch checks cover both±48 limits, fractional values, strict other-field
preservation and malformed/nonfinite/out-of-range refusal. Existing stale,
cancel, command-order, timeout, late-key, custody, open-failure/retry and zero
allocation regressions remain in the required native suite. These headless tests
are not physical input/display/audio or listening acceptance. No optimized app
installation is part of this change.

Finite user checks after a separately qualified optimized build: with transport
stopped and a loaded sampler, confirm pitch0/+12/−12 changes keyboard and
Arrangement pitch by one octave while retaining gain; enter0.5, Save/New/Open
and verify persistence; attempt an edit while playing and confirm refusal.
Replacement and history controls remain separate work.


## stopped original-frame region — pulse2969 / PR553

The compact panel adds **start** and **end** beside gain and pitch in the three-column
control grid. They read the actual nonempty half-open source interval `[start,end)`:
start is included, end is excluded, in original-file frames, independently of
project/output rate. Whole-frame numeric text and one-frame arrow steps are
supported. Pointer scrubbing selects a visible whole-frame grid; small movements
accumulate across grid crossings. Fractional text refuses without rounding. A
one-frame source has fixed, noninteractive endpoints.

UI limits use the retained asset's `source_frames`, never project-rate `frames`;
they are hints only. The narrow codec patch enforces integer/global limits and
current loop/slice invariants without moving loops, markers or clip ranges. The
actual candidate prepares against retained original PCM before canonical commit,
so an end beyond the real source refuses even if below the global codec bound.
Gain, pitch, tuning, root/detune, source identity, mode, envelope and host trims
remain unchanged. The same stopped fence/sealed-cell/admission/retirement path
serves all four fields. No source ownership or callback law is added.

Owning checks exercise actual endpoint fields, a small-motion pointer scrub,
nonzero offset and exclusive end through independently specified keyboard and
Arrangement PCM with A880, original8kHz/output16kHz, gain0.5 and pitch−12. Actual
SaveCurrent/New/Open preserves accepted intervals. Typed tests cover empty,
inverted, fractional and loop/slice-conflicting intervals; a native retained-WAV
extent refusal preserves canonical state/history and old output. Existing stale,
cancel, budget, Play/Stop, late-key, timeout, failure/retry/custody and allocation
checks remain required. Bounds checks keep all rendered fields inside the compact
grid. Shared-widget checks retain continuous gain/fractional-pitch behavior and
exercise integer display/commit and fractional-text refusal.

These are headless path checks. Physical interaction/display/device/listening and
optimized delivery remain unexecuted. Finite later user checks: load a sample,
stop transport, trim start/end to a nonempty region and audition keyboard and
Arrangement; verify the exclusive end, Save/New/Open, and confirm an invalid or
playing edit refuses. Waveform selection gestures, replacement and history remain separate work.


## Classic loop controls

Current state v2 represents Classic as an enabled flag plus a remembered nonempty
original-source-frame loop interval. Both off and on persist the same endpoints;
off never erases a custom range. New sample-load templates already select Classic
with a disabled whole-source loop. One-Shot and Slice keep their existing gate,
marker and rendering contracts; loop fields disclose that they require Classic
and never convert those modes. Version1 sampler states explicitly refuse; there
is no old-alpha reader, conversion or rewrite of saved user files.

The v2 payload remains96 bytes before slice markers. Classic mode0/off and1/on
both carry loop endpoints; non-Classic loop-coordinate bytes remain reserved zero.
Strict decoding validates mode/gate flags, bounds, finite values, marker rules and
state version. A dormant loop must be nonempty and fit the original source; the
actual retained PCM extent is checked during preparation even when disabled.
It may lie outside the currently trimmed playback region. Enabling requires
containment in that region and refuses conflicts without changing canonical
state/history or applied audio. Off-range fields remain editable for explicit
repair. No automatic clamp, source/clip edit or loop-point reset is performed.

The panel exposes an on/off readout and whole-frame loop start/end fields, using
the existing one-pending stopped transaction. All eleven fields retain independent
gesture captures. The sampler frame is720px wide with the existing168px body and
a fixed three-column control grid; bounded source/status detail cannot displace the
controls. Source units and UI bounds are original frames, not project-rate frames.
Runtime preparation still maps the enabled flag to the existing optional Classic
loop; preloop, wrap interpolation, gate release and Stop behavior are unchanged.

Owning checks exercise actual off/on/range edits and Save/New/Open in both states,
exact remembered points, off-trim enable refusal then explicit repair, active-bound
refusal with unchanged project/history/output, and original-source extent refusal
for dormant loops. Independent literal keyboard/Arrangement seam PCM combines
A880,8kHz source/16kHz output, gain0.5 and pitch−12; key release and Stop silence are
checked. Covered callbacks measure zero total allocation/deallocation counts and
bytes. Current-format, wrong-version, malformed/reserved and unchanged One-Shot/
Slice checks cover the codec boundary. These headless checks do not establish
physical input/display/device/listening or optimized installation acceptance.

Finite later user checks: stop transport, set a custom Classic loop and audition
its wrap; toggle off/on and Save/New/Open to verify both states retain the range;
trim while off, confirm an outside-range enable refuses, repair endpoints and
retry; verify key release and Stop end the sound. Waveform editing gestures,
replacement/history, Session playback and full musical-use acceptance remain
separate contracts.

## Amplitude envelope controls

Attack, decay and release display milliseconds (0..60000); sustain displays
percent (0..100). These closed control-side values map to the existing authored
seconds and linear sustain fields in state v2. Fractional milliseconds retain
six display decimals, including a one-frame 0.0625ms duration at16kHz. Keyboard
steps and explicitly enabled value-readout dragging use1 unit coarse and0.1 unit
with Shift, per pixel for dragging. Small pointer movements accumulate with the
current modifier; bounds discard overshoot debt. Other numeric controls preserve
their existing drag scales and gesture/focus/cancel behavior.

The existing envelope kernel, note/gate modes, callback ownership and lifecycle
are unchanged. Trigger One-Shot/Slice still ignore note-off; their stored release
value remains displayed but disabled, with the panel explaining its inactive
status. Gate and Classic release remain editable. No control changes the mode.
All edits share the existing one-pending stopped operation. A validated patch
identical to the current full device state completes as no change before the
audio fence, preserving history, revision and actual output even when displayed
unit conversion differs by one floating-point rounding step. This is exact state
equality, not an epsilon that could discard a small authored edit.

Owning headless checks cover unit/range/mode preservation, real coarse/fine
readout dragging, clamping and cancellation, conversion-roundtrip no-change,
invalid/stale/cancelled native requests, and all eleven rendered field bounds.
Actual retained ADSR controls save and reopen through fresh native CURRENT.
Literal original-source keyboard and Arrangement PCM combines A880,8kHz source,
16kHz output, pitch−12, gain0.5 and a whole-source Classic loop with two-frame
attack/decay/release and50% sustain. Direct keyup verifies a nonzero two-frame
release tail before cancellation; Stop silences immediately. Covered render
scopes require zero total and signed-net allocation/deallocation counts/bytes.
These fixtures do not establish physical device, display, input or listening
acceptance. Later user checks should audition short/long ADSR, direct note release,
Save/New/Open, stopped-only refusal and unchanged source/loop/pitch/gain settings
on the exact optimized candidate. Waveform editing gestures, replacement and history remain open.

## Original-source waveform overview

The sampler shows a read-only normalized stereo waveform beside the existing
controls. Its cached peaks use original source frames and rate, including when
project-rate playback has a different frame count. Start/end and remembered
Classic loop markers use those same original coordinates; off-loop points stay
visible in a muted color. There is no waveform drag, zoom, replacement or new
musical behavior. The frame is720px wide:180px overview plus the existing control
area, within the unchanged168px body/213px dock and horizontal chain viewport.

Session computes at most128 disjoint min/max bins from borrowed original PCM
once on control during import and qualified reopen. Nondivisible intervals cover
every source frame exactly once. The immutable overview holds only frames, rate,
verified normalized-content facts and peaks. It holds no PCM/Audio, bank, scope,
path allocation or waveform pyramid; original file channel count is not claimed.
A private sorted asset/overview table has one shared handle in Session and ordinary
or preview ViewState. Gain, selection and other edits reuse its backing instead
of cloning rows or scanning audio. Native/export snapshots do not retain it.

Each controller has one derived-data budget domain shared across current, staged,
New/Open and externally retained old views. The admitted **live peak Box backing**
limit is8MiB:4096 sources times fixed128 stereo bins times16 bytes. Short sources
still reserve128-bin capacity, exposing only their nonempty valid bins. A separate
checked metadata ceiling derives from4096 overview headers plus two maximum4096-row
tables, including quoted wrapper, charge and shared-control-block storage.
Reservation precedes allocation and checks actual table capacity and simultaneous
old/new construction. Peak Boxes and table Vec backing are destroyed before their
capacity charges return. A retained view keeps its actual backing charged after
Session clears; insufficient capacity visibly refuses import/Open, and dropping
the final old reference permits retry.

The metadata boundary is **logical live overview/table ownership**. It excludes
Arc shells awaiting deallocation (including Weak-retained shells), allocator
bookkeeping/RSS, existing parent ViewState storage and arbitrary copied handles.
It is not a physical metadata heap ceiling or total application memory bound.
Internal owning Arcs remain private. The separate native PCM/frame/32MiB limits,
callback ownership and retirement behavior are unchanged. No generalized budget
or allocation framework is introduced.

Import builds peaks and the replacement table inside the existing staged editor/
native-source transaction, then installs the successful table by infallible move.
Reopen checks actual catalog content/rate/length before building the new overview
in the old budget domain. Cancellation is polled at most4096 scanned frames apart;
failed, cancelled or stale preparation drops staged data and preserves the old
project, history, cache and applied sound.

Published handles bind the exact project generation/revision without copying the
table. UI lookup requires this context, matching asset ID/rate/original frame count
and strict saved content facts when present. Fresh imports intentionally acquire
saved content facts only in the save snapshot; their same-view overview comes
from the actual staged native catalog and therefore enables source-bound numeric
controls before Save/Open. Missing/mismatched/stale pairs show unavailable and
cannot dispatch a hidden source-bound control. An ordinary derivative waveform
never grants original-source authority. No project schema mutation is needed.

Owning tests cover literal mono-normalized/stereo/short/nondivisible peaks and
intervals, exact/one-under budgets and staged cancellation, a real retained worker
view across New/Open/refusal/final-drop/retry, table identity across edits, actual
allocator quotes and Box/Vec destruction before budget return. The rendered native
fixture loads via the browser, edits all four source/loop endpoints and checks
literal Arrangement PCM **before any Save/Open**, then verifies reconstruction
while retaining old peak-only data under the existing native frame cap. Bounds
cover all eleven controls, overview and original-coordinate markers. Separate UI
cases reject wrong source ID/rate/frames/content and stale generation/revision.
These are headless fixtures, not physical display/input/device/listening acceptance.

Finite later user checks on an exact optimized candidate: load a WAV, confirm its
identity and original-rate waveform, edit start/end and loop points immediately,
compare markers with the audible region/wrap, and Save/New/Open. Confirm layout
and horizontal scrolling in a smaller window. Waveform gestures, broader playback
and qualified native delivery remain separate work.


## stopped configuration history

Global Undo/Redo accepts the actual next canonical history edge when exactly one
existing, non-bypassed sampler instrument differs only in the eleven supported
panel fields: gain, transpose, source start/end, Classic loop enable/start/end,
and ADSR. Source bindings, provider, device identity, bypass/comparison and every
other project field must be identical. Selection does not choose a history target.
Unsupported load/removal, routing, tuning or other steps refuse in
place; history is never searched or skipped. Empty history completes without
requesting audio. Ordinary non-sampler Undo/Redo retains its existing behavior.

`Editor::inspect_history` borrows the current project and actual stack top before
any audio fence. A neutral exhaustive project comparison accounts only for the
existing monotonic ID and gain-touch epoch normalization. The engine checks the
closed sampler configuration delta; project has no DSP dependency. The same
classifier runs again inside normalized `undo/redo_prepared_transition`, before
the shared service snapshot preparer seals a native candidate or commits history.
An unrelated active gesture refuses before a sampler history request can finalize
it. The UI's ordinary gesture barrier finishes before capturing the history intent.

Parameter and history edits share one pending operation, UI serial, cancellation
flag and final `sampler_edit_complete` correlation. History captures direction,
project generation and revision, independent of selection. The initial worker ACK
is not application. New/Open and stale/cancelled context retire the intent. The
first Undo/Redo shortcut cancels an interacting uncommitted sampler numeric draft
and consumes the shortcut; the next shortcut moves global history.

History uses the existing stopped callback fence, overlap admission and exact
sealed-cell drain/adoption. Precommit refusal preserves authored project, stack
position, original PCM and overview table. Success moves the actual history edge,
advances the worker revision once and marks the document dirty; overview backing
is reused with the new context. A postcommit output-open failure retains the new
history position and authored state for Save/export, reports audio unavailable,
and supports explicit retry. There is no second undo stack or replacement command
standing in for Undo/Redo, no source reload and no callback DSP change.

Owning headless checks cover ordered retained numeric controls and global
shortcuts while another device is selected, cancelled drafts, literal current
Arrangement PCM, redo clearing, repeated load-boundary refusal, empty history,
stale/cancelled/playing/post-seal refusal, resource/hook refusal, exact-cell
identity, postcommit open failure/Save/export/retry, and Save/New/Open with source
metadata and peak backing checks. Callback allocation probes retain zero total
and signed-net counts/bytes for their fixtures. These checks do not establish
physical device deadlines, UI/input/listening acceptance or Live parity. Sample
removal history, Session/Console playback and nonzero seek remain
outside this increment; optimized native delivery and user acceptance remain open.


## stopped Classic source replacement

The browser's `load into sampler` action becomes `replace sampler sample` for an
existing Classic sampler. It captures generation/revision, track/device identity
and exact state. Selecting another track while it completes does not retarget the
replacement or change selection/focus. The same context action offers cancellation
while the stopped fence is pending. OneShot/Slice refuse before media decoding.
The existing device ID, chain position, bypass/comparison and host envelope remain;
all non-source authored settings are retained, including root, transpose/detune,
gain, ADSR, reverse, pan, velocity and voices. Only region and Classic loop change:
whole incoming original, looping off and remembered loop points covering it.

One neutral import-and-replace command allocates the new asset identity inside the
candidate and rebinds the same active instrument. It validates captured state and
provider/host metadata; project does not decode sampler payloads or own PCM. The
existing staged import transaction prepares an original-source overview, private
resource-aware snapshot and exact sealed native cell before adopting the staged
bank, cache table and canonical history. Cancellation/shutdown is checked after
native sealing and before those commits. It uses the same single pending edit,
stopped callback fence, final completion and actual output-retirement paths as
configuration edits. A refused preparation preserves project/history, store/cache
and old sound. A hardware-open failure after commit retains the new authored
source for Save/export, reports unavailable audio and supports explicit retry.

`NativeSamplerInput` privately borrows either the committed store or the actual
staged view. Source quoting, additional native admission and constructor copy all
use that same variant. Staged admission includes the committed store, incoming
bank, actual container overlap, current/candidate/export/outbound ownership,
legacy PCM and refreshed old/new native holds. No temporary committed store,
source-owner projection, additional source Arc clone or cap increase is used.
The existing constructor copies the borrowed original into its sealed bank;
there is no second copy after canonical acceptance.

Replacement appends within the same Session. Before decoding, its allowance
includes every retained original (including unused old samples), current and
captured export/outbound ownership, legacy audio and native holds. The file is
decoded once within that allowance, then the actual decoded allocation is
inspected before original ingress or any derivative alias. Encoded bytes and
decoder scratch keep their separate existing limits; later metadata/compiler and
native-copy overlap checks still apply. Old assets, banks and peak rows remain in
the bounded catalog/stores for history, export and Save. They are not reclaimed
through history eviction or a new collector. Repeated replacement may refuse the
existing caps. Both old and new original files remain relevant to later Open;
missing media is reported through normal relinking rather than silently removed.

Replacement creates one canonical history edge. Global Undo/Redo restores that
exact edge through the same stopped History transaction. The neutral comparison
permits exactly one catalog addition/removal with identical shared assets; Session
requires the forward state to equal the adopted Classic replacement template with
only its designated binding changed to that new asset. Undo validates this same
relation in reverse, restoring old trim, loop and non-source settings. Normalized
history is rechecked before commit. Sources come from the committed bank without
decode or file access; all original banks and peak rows remain charged even when
Undo removes a source from the canonical catalog. An over-budget transition
refuses without consuming the edge or clearing Redo. Configuration edits keep
their actual global order on either side. Save serializes only canonical assets;
Open retains its existing fresh-history behavior. Endpoint widgets refresh their numeric
bounds when the source length changes even though device identity stays stable.
The owning rendered regression fails with the old cached four-frame bounds and
passes when editing frame5 immediately after a six-frame replacement.

Owning headless checks cover different original rates/lengths and literal current
PCM, nonzero host trims, preserved settings/reset bounds, actual browser cancel
and captured-target completion while another Utility device is selected, source
cache readback, immediate endpoint edits, Save/New/Open of both retained sources,
unsupported modes before decode, stale/corrupt/cancel/post-seal/tight-frame refusal,
unused-original plus captured-export admission, exact sealed/drained identity,
postcommit open failure/Save/export/retry, old held-key epoch revocation with a
fresh-key new-PCM check, and zero total/signed-net callback allocations for the fixtures. These are not physical UI/input/device/listening
acceptance or Live parity. Removal history, other playback entry paths and
qualified optimized native delivery remain separate work.

Replacement-history regressions delete both WAVs after admission, verify exact
old/new state and distinct original-coordinate PCM, and exercise actual browser
global shortcuts while another Utility track remains selected. A real frame-cap
fixture retains four-, six- and two-frame originals, undoes to six, then refuses
the next four-frame candidate because12 retained +6 current +4 candidate exceeds20.
The uncatalogued two-frame original remains charged; Redo still succeeds. These
headless checks do not establish physical UI, audio deadlines or listening acceptance.

## Constructor-owned Console playback

Native CURRENT can compile finite authored local Console MIDI cells, including
Session source loops, multiple sampler tracks and scenes mixed with ordinary
audio cells. The existing Session clock, quantization, templates and renderer
remain authoritative. Routed MIDI, nonzero seek, Pause, active Arrangement chase
and composition edits after loading remain outside this increment. Private
offline export does not acquire this Console capability.

Each source/event template is born in the same constructor as its exact sampler
factory and original PCM bank. Its paired source, event, empty note-bank and
context handles, names, backing vectors and compiler workspace participate in
reservation, sealing, restore and control cleanup. No generic prepared-source
bridge or additional PCM bank is used. The preflight capacity aggregates retain
the conservative compiler reservation when actual vectors occupy fewer bytes;
the closed ledger still visits actual backing independently.

An immediate transition validates the entire command before committing route
changes and outgoing primary gate cuts. A quantized transition preflights every
due destination before cutting speculative gates and routes in the existing
render transaction. NoteOff precedes incoming NoteOn; the same sampler runtime
continues releases under MIDI, audio or silent routes. A checked route incarnation
distinguishes retriggers while the previous release is alive. The first segment
after a cut is one frame. Reservations independently bound surviving releases,
factory voices, occurrence proofs, onsets and events; refusal preserves the old
route and pending request, and cancellation does not cut voices. Generic mutable
route guards and generic inheritance refusals remain intact.

The UI requires the actual applied source/output/generation/revision and callback
Console capability. Submission, constructor hold and legacy output do not qualify.
Headless tests cover literal mixed PCM, loops and block partitions, quantized
boundary crossing, retrigger release, atomic scene capacity refusal, exact/under
compiler reservations, actual native launch/tuning/cancel/Stop allocation probes
and rendered Console interaction. Physical interaction, listening and optimized
candidate acceptance remain separate.
