# Canonical Lattice instrument slots

Pulse2114 is a focused prerequisite within native runtime16 before Operator19.
The current built-in instruments use immutable analytic sampling. Mutable FM
voices, expression, plugin instruments, engine event scheduling and general
chase/tail integration remain deferred.

## Model and persistence

Each hybrid track has one optional instrument node in its ordered chain. The
canonical model retains the effect vector plus a gap index: the number of effects
before the instrument. Its globally unique DeviceId, bypass flag and bounded
opaque NativeDeviceState envelope use the same contracts as effects. A borrowed
ordered-chain iterator exposes explicit instrument/effect node references without
duplicating editable state. Stable-ID moves can target either node class; effect
insert/remove updates the gap atomically, and history/snapshots retain it.

New tracks, MIDI tracks and import-created tracks receive an explicit
hex.instrument.sine version1 empty envelope. A model-owned neutral helper defines
this default without depending on DSP. Storage schema16 requires both the
nullable instrument field and nullable gap. Strict schemas1–14 install that same
envelope once at the front, using fresh IDs above the existing high-water mark;
schema15 instruments migrate to the front because that format had no position.
Migration refuses exhausted ID space; original saved data is not overwritten by
decode. Explicit schema15/16 null means silence and must not be migrated back to
sine.

Storage serializes the generic envelope, never a per-instrument DTO. Unknown
namespaced types and future state versions round-trip unchanged; control-side
preparation refuses an unsupported present slot even if bypassed. Removing or
bypassing an instrument silences MIDI while audio on that hybrid track remains
independent. Canonical MIDI clip extent and previews must remain editable/visible.

## Processing and ownership

hex-lattice owns a separate NativeInstrumentRegistry and definition contract;
effects prepare matrices/processors, instruments prepare notes. hex-devices owns
the original sine codec and sampler. Both Arrangement compilation and Session
inventory resolve the canonical slot through this registry. A second independent
stereo definition is exercised through both paths without engine/storage type
branches. No implicit engine sine fallback is allowed.

Control-side native metadata/state access takes an explicit effect or instrument
class and queries only that class's registry. The existing unqualified engine
helpers remain effect-only compatibility entry points. Catalog listing, default
state, descriptor/value views and parameter patching share the neutral metadata
contract; a type ID is never used to guess processing class or fall back to the
other registry. This matters because effect and instrument registries validate
their identities independently and may contain the same type ID.

Control prepares an immutable sample-rate-specific configuration. Arrangement
and Session processing now honor the stored gap: audio sources enter before the
effect chain, the single instrument adds generated MIDI at its gap, and the
remaining effects process the summed signal. An absent or bypassed instrument
adds no MIDI, while audio still traverses the effects. Track gain, balance and
mute apply after the ordered chain. Stateful effects before and after the gap
share one paired runtime chain and one whole-block history transaction; segment
processing does not commit between sides. Memoryless effects use separate fused
prefix/suffix matrices for a non-front gap. The original front-gap path retains
its existing fused matrix behavior.

When a revision cuts an active MIDI voice, a same-revision release enters at
the instrument gap. A release carried from an older graph uses that graph's
captured suffix and mix, never its prefix or the candidate's effect settings.
Stateful suffix ownership moves through the bounded tail bank and is reclaimed
with the retired graph on control. The finite active/tail processor limits and
declared runtime budgets are unchanged.

The prepared
note is a Copy value containing 41 u64 payload words, release duration and a safe
stereo sample function pointer: 344 bytes on 64-bit targets. The first three
words retain the sine phase step, amplitude and attack representation; the
remaining words are zero. The 41-word capacity can also carry the validated
standalone FM4 kernel without a heap owner or unsafe conversion. `Fm4Definition`
is registered alongside sine in the built-in instrument catalog. Its bounded
version-1 codec and generic parameters use the same catalog, project preparation,
Session inventory and panel paths. Definitions
validate fields before packing. Sample functions must be deterministic, bounded
and free of allocation, deallocation, waiting, I/O and mutable shared state.
This trusted native definition contract is not a sandbox for arbitrary external
code.

Sampling receives authored age and original gate duration, preserving exact
sine seek-mid-note/release behavior. Existing bounded occurrence/span/query and
copied-release banks remain in use. Copied samplers retain their original law and
payload across revision handoff without Arc churn or final heap drops. Existing
revision/source owners still retire on control after callback quiescence.

Session route continuity matches stable track identity with MIDI source identity,
instrument DeviceId, the complete versioned instrument state, and whether that
slot is enabled. Effect position is not part of sampler identity, so moving a
slot across an effect keeps compatible processing history and the active note.
On a changed enabled configuration, held old notes that began before the cut
continue from copied old samplers and matching candidate occurrences are
suppressed until those copies end; later onsets use the candidate configuration.
Notes already in release keep their old gate and sampler. Bypass or removal
releases eligible old held notes at the cut, emits no candidate notes or
suppressions while disabled, and re-enabling does not chase onsets from the
disabled interval. A note beginning exactly at the handoff frame belongs to the
candidate. A fixed route-local onset floor prevents a later re-enable from
chasing notes that began while that route was disabled; it follows the stable
TrackId across graph edits and clears on seek or stop. Source or timing edits keep
their existing global historical-floor behavior and carry other active Session
routes across that floor. Instrument-only edits do not advance it or capture
unrelated routes. The route floor remains through a monotonic Session loop and
allows later looped source occurrences whose onsets are after the floor.

Arrangement handoffs suppress a matching candidate occurrence until the later
of the copied old voice's end and the candidate occurrence's own end. Repeated
edits and track removal carry an already-copied voice once instead of recapturing
the still-visible source span. Candidate occurrence spans are collected once
into fixed-capacity scratch and matched by stable occurrence key.

Prepared `Playback::from_prepared` revisions always use this shared
`SessionPlayback` owner, even when all routes initially remain Arrangement-owned.
This gives Arrangement MIDI the same copied-voice, suppression, suffix-tail and
handoff lifecycle as Session MIDI. The legacy `Playback::new` and
`Playback::from_shared` constructors retain their finite Arrangement renderer.
Promotion from that renderer refuses while an enabled Arrangement note is held
or a stateful effect has untransferable history; a safe retry is allowed after
the voice ends or a discontinuity clears it. It also refuses after a nonzero
legacy performance frame when stateful-effect history cannot be transferred. A
Session-to-legacy demotion refuses active Arrangement notes, stateful-effect
history after rendering has begun, and any carried releases, pending route,
selected Session owner or suppression. Refusal leaves both candidate and
previous transport/graph usable. Disabled instrument slots have no held voice
and do not create a false promotion/demotion fence.

If Session runtime preparation fails for a prepared revision with an instrument
slot or stateful effect, the explicit `Unavailable` capability is retained and
rendering refuses before touching output. Such revisions do not silently fall
back to the finite renderer, which cannot preserve their handoff ownership.
Revisions without instrument slots or stateful effects retain the finite
fallback when Session timing is unavailable.

Prepared Arrangement tracks and Session instrument configurations intern each
distinct pitch/velocity pair into a track- and configuration-scoped immutable
note bank. Arrangement spans and Session intro/repeat source spans retain a
checked `u16` index; span queries resolve the sampler into the existing fixed
render/release value, so copied releases have no bank lifetime dependency.
Only notes with an emitted Arrangement occurrence or Session source-span
occurrence enter a bank; authored notes hidden outside their current source
window do not trigger instrument-specific admission failures.
An unchanged Session source/configuration reuses its prior bank directly;
changed note sets rebuild only that track's bank, and exact key sets can reuse a
compatible prior bank only when the compiler has already matched the stable
track/device identity, device type and state, bypass state and sample rate.
`PreparedNativeInstrument::prepare_note` is deterministic for fixed prepared
configuration/rate/pitch/velocity, as required by interning. No bank `Arc` is
cloned or dropped by callback query or release processing.

Admission derives from current model ceilings: 16,256 valid pitch/velocity pairs
per track, 65,536 authored notes per project, 65,536 Arrangement occurrences,
131,072 maximum Session intro+repeat references and 65,536 Session cells. A
full prepared revision therefore accounts at most 131,072 bank entries,
196,608 `u16` references, 512 track-bank owner headers/handles and 65,536
Session template handles. With the 344-byte sampler, `BankEntry` is 352 bytes
on 64-bit targets. Checked byte limits include entry keys, owner/handle overhead
and all indices; `try_reserve_exact` refuses allocation failure before
publication. The derived component ceiling is 47,091,712 bytes: 46,137,344
bytes for entries, 36,864 for track-bank owner headers/handles, 524,288 for
Session template handles and 393,216 for indices. This keeps the existing
count limits rather than adding a narrower byte cap. Active and candidate
prepared revisions can overlap, so their corresponding bank/index component
bound is 94,183,424 bytes.
The checked test exercises one revision at the derived limit, refusal above it
and the doubled active/candidate calculation. Queued, retired or otherwise
retained owners are outside that pair accounting; this is not a whole-process
memory maximum.

The bank lookup is one indexed read when a source interval enters the fixed
render-span buffer. Rendering samples the copied sampler by reference, so the
344-byte payload is not copied per frame; it does not binary-search or allocate
per sample. The retained-memory benefit depends on repeated pitch/velocity
pairs: unique-only Arrangement notes pay for the key/index and owner overhead,
while repeated Session intro/repeat occurrences share one bank sampler.

Each `SessionPlayback` retains two preallocated release buffers of 16,384
entries and a 192-slot MIDI span buffer. On 64-bit targets, the current layout
accounts 15,990,784 bytes for both release buffers (`16,384 × 488 × 2`) and
82,952 bytes for spans, 16,073,736 bytes total. Relative to the prior 40-byte
sampler, growth is 10,019,840 bytes (9.56 MiB): 9,961,472 bytes across the
paired release buffers plus 58,368 bytes in span slots. The test checks these
layout sizes and the combined ceiling below 16 MiB. This is the changed fixed
MIDI scratch only; it excludes the rest of `SessionPlayback` and does not bound
all simultaneously queued/retired playback owners. Two overlapping playback
instances account twice this scratch component, independently of the
prepared-revision bank/index component above.

The sine table initializes once on control during instrument preparation.
Callback sampling uses the noninitializing OnceLock::get path. No lookup table
is rebuilt per track or sample. Existing SineTable APIs remain compatibility and
oracle inputs; they do not override a canonical slot's registered instrument.
General mutable instruments need a separately accepted voice/event/chase/runtime
contract; this analytic stage does not provide mutable Operator voices.

## Evidence and remaining gates

Indexed-bank engine, Session, export and audio allocation-probe checks pass;
strict engine Clippy, formatting and architecture/style guards pass. The
retained release sampler test drops the bank owner before sampling the copied
voice. Existing exact legacy sine PCM covers note gate/release, partitions,
seek, pause/stop and replacement. These headless fixtures do not prove all
density/fault paths, native APIs, physical device deadlines or listening
acceptance. Root runs clean full local CI after integration.

Release timing methodology and provisional regression are recorded in
[performance](performance.md). Indexed-bank and wide-payload before/after
profiles are separate evidence there. The preexisting mixed Session profile comparison
failure is separate Pulse2113. Native instrument UI/catalog editing and built-in
FM4 admission are implemented; mutable FM contracts and full Operator remain open
work, independent of this source foundation.

## Instrument panel and canonical class routing

The focused track's device panel renders its one canonical instrument slot at
its stored position among serial effects. Add requests only fill an empty instrument slot; attempting to
add while occupied is refused without a project edit or playback publication.
Replacement is a separate request that names the stable device ID, class and
registered type. It validates that the target still exists in the requested
class, then transactionally replaces its authored state while preserving device
ID, chain position and bypass. Removal remains separate. Instruments and effects
can be reordered through stable-ID device-header drags; the first instrument
added to an empty slot defaults to the front. The admitted sine instrument has no parameter
descriptors, so the panel invents no controls.
An absent instrument leaves no placeholder block in the chain. Settled user
removals retire controls without a stale-target warning; loss of a target while
it still owns a draft or pending gesture remains diagnosed. Unavailable or
malformed state remains visible rather than silently resetting.

Session parameter reads, patches and transient gestures resolve processing class
from canonical track-slot ownership. A gesture retains and revalidates class and
type identity; the shared UI control cache also includes both identities. New-device
requests explicitly select a class catalog, while existing IDs never authorize a
registry choice supplied by the UI. Existing effect gesture barriers and history
remain in use. Adding a registered instrument requires no device-specific storage
or UI codec.

Session tests cover class refusal, occupied-add atomic refusal, instrument
insert/remove Undo/Redo, explicit replacement identity/order/bypass and Undo,
parameter/gesture refusal for sine, and save/reopen. Rendered
GPUI coverage also exercises settled Utility and sine removal, Undo restoration,
warning cleanup, and fixed panel geometry; active/draft target-loss diagnostics
remain covered separately. Full local CI and native acceptance are separate gates.
The browser exposes registered FM4 through the same canonical slot and generic
parameter panel. FM4 authored-state and replacement continuity use the shared
instrument contracts; this does not implement mutable Operator voices or plugins.

## Mutable runtime contract foundation — Pulse2157

`PreparedNativeInstrument` now has an optional mutable-runtime preparation
capability whose default is `None`, leaving current sine and FM4 sampler
behavior unchanged. A prepared mutable runtime declares a nonzero version,
per-twin fixed storage, maximum event count, maximum active occurrence count,
and a finite maximum duration from final note-off to output quiescence. The
tail bound includes envelope release and internal feedback/output. The caller
must supply finite pair-byte, event-count, occurrence-count and tail-duration
ceilings; the lattice checks every declaration and checked twin-byte arithmetic
before constructing either runtime. It then verifies that the newly-created
twins can copy history in both directions and that their reported fixed storage
does not exceed the declaration. Declarations include the concrete fixed voice
array and owned buffers; reported sizes exclude allocator bookkeeping and remain
the implementation's accounting contract. The API adds no engine-wide voice,
onset or block-event constants.

The caller passes an ordered event slice and mutable stereo slice. Event offsets
are half-open (`0..frames.len()`), offsets must be nondecreasing, IDs must be
nonzero, and repeated note-on IDs in a batch are refused. Equal-offset order is
preserved, so note-off followed by note-on can free capacity when the runtime's
release policy makes a slot available. Each runtime's read-only preflight must
also reject duplicate IDs already active in prior blocks and its own
occurrence-capacity failures. No event or
PCM mutation occurs until both generic and implementation-specific preflight
succeed. After admission, processing, reset, history copy and twin commit are
infallible; callers can abandon scratch after a later-stage failure and retry
from committed primary history. Runtime compatibility must remain invariant for
the lifetime of both twins. A committed-state quiescence query lets a future
owner check whether the bounded tail has finished before control-side retirement.

History-copy compatibility is implementation-defined but must include every
immutable configuration value that affects future samples, not only runtime
version. A later engine compiler must additionally check device ID, device
type/state and sample rate before asking this runtime to inherit. Runtime
objects own any immutable configuration needed after their prepared factory is
released; graph retirement and final destruction stay on the control side.

The discontinuity contract at this stage is hard reset with no chase or replay.
Mutable-only definitions must explicitly refuse analytic `prepare_note`; a
future compiler must select the mutable capability and may not substitute a
silent or sine analytic sampler when analytic preparation fails. The test-only
feedback instrument proves overlapping equal-pitch occurrence IDs, same-frame
off/on ordering, partition equivalence, duplicate/capacity refusal, rollback,
compatibility, actual twin storage, additive output over nonzero caller PCM, and
quiescence within its declared release/output-tail bound. The proof feedback law
uses a finite tail counter to clear residual output. These tests inspect the
bounded proof runtime but do not instrument production engine callbacks. No
built-in instrument uses this runtime yet. Old-runtime suffix-tail ownership
(Pulse 2160) and the prepared shared-playback export adapter are described
below; built-in mutable admission remains deferred.

## Test-only shared engine integration (Pulse 2158)

The engine can compose an injected instrument registry into both Arrangement
compilation and Session inventory, then drive a mutable-only feedback proof
instrument through the shared SessionPlayback scheduler. It does not register
that instrument in hex-devices or expose mutable instruments to the UI. Events
use checked `(track, clip, note, iteration, loop-pass)` keys mapped to bounded
runtime IDs; the shared onset/span scratch buffer holds at most 192 entries,
while source-density validation limits actual note onsets to 128 per 256-frame
query. The ordered event buffer holds at most 256 events, with at most 64
simultaneous occurrences per track. Onsets are sorted
by source frame and the complete occurrence key before checked IDs are assigned;
events use frame offset, NoteOff-before-NoteOn, then occurrence ID as a total
order. This keeps voice-slot assignment stable across query partitions and
source iterator order. Note-offs at an onset's in-query gate end are included
in the same ordered batch. The complete
project remains subject to the existing compiled occurrence cap.

Before publishing an Arrangement revision or Session inventory, mutable MIDI
admission also proves every sliding 256-frame event window against the smaller
of the device's declared event capacity and the host's 256-event buffer. This
counts both NoteOn and NoteOff edges, including short gates and release-tail
overlap, with exact-boundary NoteOffs ordered before new NoteOns. The existing
release-inclusive voice proof is checked against the smaller of the declared
device voice capacity and the host's 64-occurrence limit. Invalid zero or
oversized declarations and dense candidates are refused before history or
playback publication; callback event handling remains bounded by the same
validated limits.

Mutable processing participates in the same whole-block transaction as audio,
effect history, routes and transport. Any later segment failure discards the
mutable scratch ledger and runtime history with the rest of the candidate block.
The feedback test covers Arrangement and Session scheduling, block partitioning,
short in-query notes, event limits and hybrid additive prefix/instrument/suffix
rendering. Legacy finite Arrangement renderers refuse an enabled mutable track
before advancing time or emitting audio. The shared SessionPlayback path admits
finite Arrangement source loops and Session source loops with distinct occurrence
iterations. Enabled Arrangement transport loops are admitted only after bounded
onset, periodic-voice and first-wrap release-seam proofs; wrap chases preserve
authored remaining gates and use a monotonically checked loop pass. See
`docs/mutable-transport-loops.md` for the contract and evidence. Live-state seeks
and quantized mutable route changes remain explicitly refused. Stop is the hard
reset. Live mutable state can cross a revision handoff
when the bounded tail preflight succeeds. A
candidate mutable Arrangement owner also refuses a pre-cut sounding note,
including a newly added track. Route commands check the actual candidate
Arrangement owner, so returning from a quiet Session route refuses while an
Arrangement note is already sounding. A future pending route inherited from an
analytic graph is refused before handoff. These are finite proof gates, not
general mutable instrument behavior.

This engine path remains a test-only admission seam. Pulse 2160 now carries a
nonquiescent mutable runtime pair and its occurrence ledger through revision
handoff together with the old effect chain and captured suffix. Enabled source
replacement keeps old gate deadlines; removing or bypassing the instrument
clamps held gates to the publication frame and emits NoteOff at offset zero,
preserving feedback and release state. Already-releasing notes keep their
existing deadlines. Tail rendering adds mutable and copied analytic releases
before running the old suffix and old track mix once. Completed tail owners stay
attached until their graph retires on the control side.

When a mutable owner retains an old chain, candidate history inheritance skips
the old suffix entries by stable device identity. The retained chain keeps its
full old suffix history for tail rendering; compatible prefix and unrelated
track histories still transfer, while matching candidate suffix twins start
fresh for new input. This avoids using one filter history for both the outgoing
old signal and the candidate signal when an effect moves across the instrument
gap.

Mutable tail groups share the 256-group/stateful-instance ceiling and 2 MiB
declared twin-storage ceiling. Each live mutable owner reserves its declared
maximum of 64 voices against the existing 16,384-entry release bank. Preflight
refuses before ownership changes when groups, voices or bytes do not fit.
Seek refuses while a mutable tail is nonquiescent; Stop resets active and tail
runtime state. The component host-storage formula includes active owner slots,
their two ledgers, all 256 retained tail-owner slots and their two ledgers, the
shared 192-span onset buffer and 256-event buffer. On this 64-bit arm64 build it
is 3,761,744 bytes per maximal SessionPlayback; an active/candidate pair is
7,523,488 bytes of this component. The declared payload limits are separate:
each graph admits up to 2 MiB active pairs and up to 2 MiB retained tails. Old,
candidate and retired publication owners can coexist, so these figures are not
a process-wide peak bound and omit allocator overhead and other graph assets.

This is still a restricted, proof-gated path. Exact compatible mutable-owner
continuity is implemented separately below; changed-route suppression,
seek/preroll chase, effect residual-tail quiescence and built-in mutable
admission remain deferred to Pulses 2166, 2164, 2165, 2162 and later Operator
work. Pulse 2159 adds
prepared linear Arrangement WAV export through shared SessionPlayback, but
nonzero range starts are refused whenever the compiled Arrangement contains an
enabled mutable instrument; those ranges could require mutable history/chase.
Mutable Arrangement source loops are refused, while unused Session loops and
the global transport loop do not wrap this linear export. This does not admit a
built-in mutable instrument or establish general mutable export support.

Pulse 2163 adds exact compatible-owner continuity for unchanged mutable routes.
The handoff can copy the committed mutable pair, both occurrence ledgers and
next occurrence ID when stable track identity, instrument ID/state/bypass,
instrument gap, ordered effect IDs/state/bypass, source ownership and placement,
sample rate, and the complete timing map still match. Arrangement owners also
require the exact retained MIDI source identities and clip start/length; Session
owners require the same launched clip, track, scene, source identity, instrument
configuration and launch position. Name and other unrelated track edits can
therefore keep active feedback history without creating a duplicate old-law
tail. The candidate's compatible effect histories still inherit through the
existing stable-device matching path. Exact carry preflights the full candidate
effect chain, including a suffix that the general old-tail preflight may have
excluded. An adversarial lattice runtime-chain fixture verifies rejection when
that retained suffix cannot copy history. PreparedRevision's test seam injects
instrument definitions but not effect registries, so this specific mismatch is
not constructible as an end-to-end compiled handoff fixture without a broader
registry seam.

Preparation retains bounded descriptors only for tracks with a compiled
mutable instrument: at most 256 track records, 64 effect records per track,
and 16,384 Arrangement MIDI clip records across the project. The maximum inline
descriptor-buffer formula per revision is
`256 * size_of<track-record> + 16,384 * size_of<device-record> + 16,384 * size_of<clip-record>`;
the track record includes its two `Vec` headers. There are at most 512 nested
device/clip buffer allocations plus the outer track buffer; allocator metadata
and Arc-owned state/source payloads are separate. Device-state payloads and MIDI
sources are retained through their existing Arc-backed immutable values, not
copied into per-note snapshots. Preparation checks the project caps and uses
fallible reservation. Preparation zips canonical and compiled track order with
at most 256 stable-ID checks. Handoff compares the shared sample rate and
complete timing map once, then binary-searches descriptors by stable TrackId.
The continuity route map is bounded by 256 old routes × 256 candidate route
entries; the existing route handoff also uses fixed project-track bounds.
Across all mutable tracks there are at most 16,640 device records (instrument
plus 64 effects per track) and 16,384 Arrangement clip records to compare.
Device payload equality is exact and bounded by the 4,096-byte state limit per
device. It does not scan MIDI notes or recompile sources on the callback.
Descriptor Vec allocations and retained source/state Arcs are per prepared
revision; multiple active, candidate and retired revisions can coexist, so
these bounds are not a process-wide memory maximum.

Changed instrument state/type/identity, enablement, position, effect topology or
configuration, Arrangement source identity/placement, Session launch/source,
timing, or pending route continues through the existing conservative refusal or
old-law tail path. This continuity slice does not add source or transport loop
continuity, seek/chase, effect residual-tail quiescence, or mutable built-in
admission; see Pulses 2165, 2164, 2162 and the later Operator gates. The
continuity fixtures use the injected proof instrument and do not claim
production mutable-device availability.

## Mutable occurrence suppression — Pulse2166

Pulse2166 keeps an owned dispatch proof after NoteOff, including onsets whose
NoteOn and NoteOff occur in one render block. Each fixed ledger entry records
stable track/clip/note IDs, source iteration and Arrangement loop pass, exact
authored onset/gate beats and pitch/channel/velocity, plus the runtime
occurrence ID, actual performance onset/gate and old-law quiet deadline. The
device configuration is deliberately excluded: the same stable authored
occurrence can be proven to have played under the outgoing configuration. No
source Arc or revision pointer survives in the proof.

The proof ledger has 64 preallocated slots per runtime twin. That bound follows
the compiler's maximum of 64 simultaneously intersecting `[start, end)` spans,
including each declared release tail. It is independent of the runtime's
`max_active_occurrences`, whose counting semantics are runtime-defined and do
not specify whether or how released voices count; that declaration was not
tightened.
After a gateoff is delivered, proofs expire at `actual gate end + outgoing
declared tail`, or at the cut plus that old tail when bypass/removal sends an
early NoteOff. A still-unsent NoteOff remains proven through its exact gate
boundary even when the declared tail is zero; it is pruned only after the
NoteOff is delivered and the deadline has passed. A whole-runtime quiet commit
may clear proofs sooner. The unchanged host-storage accounting formula
uses `size_of::<MutableOccurrence>()` for both twins on every active and retained
owner; on this 64-bit arm64 build, that component is 6,910,544 bytes at 256
tracks, up from 3,761,744 bytes (an increase of 3,148,800 bytes). Device-declared
DSP payload limits remain separate.

At a changed mutable handoff, candidate spans intersecting the cut are matched
against the previous active owner and every retained mutable tail for the same
TrackId. Exact signatures receive a suppression through the later of the
original old-law quiet deadline and candidate gate end plus candidate declared
tail. Carried suppressions extend to the new candidate end without copying the
already-carried owner into another tail. A same-key but changed historical
signature still refuses with `MutableInstrumentChase`; no proof is fabricated
for an occurrence that was never dispatched. Future onsets remain eligible.
Suppression tokens are sorted by stable occurrence key and full authored
signature; callback lookup uses bounded binary searches instead of scanning all
same-key revisions. The compiler bounds source queries to 192 spans and mutable
dispatches to 256 events; handoff proof scans are bounded by 64 proofs per
runtime and the fixed 256-slot retained-tail bank.

The authored signature cannot distinguish an intentionally deleted and then
recreated event when the project reuses the same stable IDs and reproduces the
same onset, gate and payload. Such a byte-for-byte semantic recreation is
treated as the same occurrence while its old tail is alive.

Preflight accounts for inherited and newly created suppressions against the
existing 16,384-entry release capacity, and validates release voices, tail
groups, processor count, declared bytes and mutable history capacity before the
no-fail ownership phase. A failed engine handoff leaves the previous runtime,
PCM and transport clock unchanged and permits a fresh candidate retry. This
establishes engine-level runtime admission atomicity only. Whether a prepared
candidate already committed to canonical editor history while audio admission
is pending is separately tracked by Pulse2180; this change does not claim
history rollback or redesign the controller/audio acknowledgment protocol.

## Shared analytic contexts

Pulse 2169 adds an optional immutable shared-context path beside the existing
41-word `PreparedAnalyticNote`; that existing public representation and its
sine/FM4 PCM law remain unchanged. A prepared context validates and prepares
pitch/velocity note payloads on the control side, then samples a compact
eight-word `PreparedSharedAnalyticNote` by borrowed reference using authored age
and gate frames. Implementations must be deterministic for fixed prepared
configuration, sample rate, pitch and velocity, and `sample` must not allocate,
mutate state or wait. Context preparation is not a callback operation.

One `PreparedRevision` creates a single sorted context table shared by its
Arrangement and Session banks. The table is keyed by stable instrument
DeviceId; exact track ID, instrument type/state and sample-rate identity permits the
immutable context owner to be reused across compatible prepared revisions.
Each process-local context ID is checked for exhaustion and is never saved or
derived from a provider pointer. Source notes hold only a checked table index
and compact Copy payload. If the candidate configuration changes, copied old
releases retain their old context law through the shared playback lease table;
references are remapped before handoff commit. The callback resolves the
context by a bounded index and does not clone or drop an Arc per note or frame.

Preflight bounds one context's declared owned payload at 4,096 bytes and unique
prepared context payload at 2 MiB per revision, with at most 256 context owners
(one per project track). The current track ceiling and per-context ceiling make
the largest presently reachable prepared payload 1 MiB; the 2 MiB aggregate
check remains a defensive bound if either constituent limit changes. The
control-side table also stores one owner `Arc` handle and one sorted ID/index
entry per context. At the 256-entry limit, their 64-bit layouts are 2,048 bytes
of owner handles, 4,096 bytes of ID/index entries, and a 56-byte table header
(6,200 bytes total, before allocator rounding and separately allocated owner
objects). These allocations are separate from declared payload and from the
per-playback callback scratch accounting below. A playback may retain
at most 16,384 distinct old
context leases and 2 MiB of declared retained payload. Its preallocated
lease/remap plan uses 393,216 bytes and optional owner slots use 131,072 bytes
on this 64-bit target. Including the existing paired release and span storage,
the checked scoped shared-analytic playback component is 16,598,024 bytes per
playback; an active/candidate pair is 33,196,048 bytes, below the 18/36 MiB
scoped limits. These figures exclude effect and mutable runtime storage,
prepared source/context metadata, allocator overhead, and additional queued,
retired or externally retained graphs. They are not a whole-process memory
bound. Context payload declarations are implementer-provided, checked on the
control side, and do not count the separate owner/table handles.

Handoff preflights release references, lease deduplication, slot count and
declared bytes before changing either graph. The last fallible phase stages one
owner Arc clone per unique needed context in the candidate, including already
retained contexts. The previous playback keeps its strong roots, so a refusal
cannot invalidate its releases or final-drop context storage. Only then does
the no-fail phase move bounded release values and runtime owners. There is no
Arc clone per voice/frame; overlapping handle slots are included in the paired
scratch accounting, and shared payload is not duplicated. Expired lease slots stay
owned until their graph is retired and reclaimed on the control side. The audio
allocation probe verifies zero total and signed-net allocation/deallocation
for callback processing, a changed-context handoff and retirement; a destructor
probe verifies that a retained context's final drop occurs on control. This is
instrumented evidence for that fixture, not a claim about every provider or
physical callback deadline.

The original Sine and FM4 built-ins still use their compact analytic note
samplers. Operator is registered through the shared analytic context API and
supports the four native periodic waveform choices; see the
[Operator waveform contract](operator-waveforms.md). Mutable voice continuity,
seek/chase, loop handling and effect residual-tail ownership remain separate
work.

## Control-owned runtime checkpoints — Pulse2181

`SessionPlayback` now exposes a `MutableRuntimeCheckpointStore` for bounded,
in-memory snapshots of one mutable instrument track at a time. Each snapshot
retains the committed paired runtime, both fixed occurrence-proof buffers and
the next occurrence ID, plus the exact immutable prepared-revision identity,
sample rate, track ID and captured performance frame. The proof records remain
owned values; no source pointer is used as an occurrence proof. A store accepts
at most 16 snapshots and charges the prepared pair declaration, two fixed
64-entry proof buffers and all 16 inline entry slots against a 256 KiB
aggregate ceiling. The fixed slots are part of the store's `size_of` charge,
including when empty; `clear` drops all runtime owners but retains this fixed
charge. Shared immutable `PreparedRevision` graph data and allocator bookkeeping
are excluded. Restore preflights the retained store plus a declared temporary
candidate below a separate 512 KiB operation peak before calling the factory;
this operation bound likewise excludes shared graph allocations and allocator
bookkeeping. The current feedback fixture charges 17,976 bytes for one
checkpoint on macOS arm64; exact `size_of` contributions are architecture
dependent.

Capture refuses an open speculative block. Restore requires the same exact
prepared `Arc`, sample rate, track and performance frame, prepares a temporary
runtime through the revision's ordinary factory, and replaces the target only
after complete-history compatibility and occurrence-ledger transfer succeeds.
The checkpoint remains reusable; target retirement happens on the exclusive
caller/control thread. `copy_history_from` now explicitly guarantees complete
future-sample state, including held and released voices, envelopes, phase,
feedback and random-generator state. The test-only feedback device proves
instrument-runtime-only sample-identical held-note and ended-gate feedback
continuation across checkpoints with different render partitions, along with ID/proof continuity,
uncommitted-state refusal, exact revision/frame invalidation, atomic refusal and
factory-call-free count/byte admission refusal.

This is not a full project or seek checkpoint. The continuation oracle covers
only one instrument runtime plus its engine occurrence ledger; it does not
prove transport/source-loop, route, return or effect-history continuation.
Supported replay distance is zero: restore is valid only at the captured
performance frame. Serialized persistence, offline bounded replay, native seek
and publication-worker integration remain
separate work under Pulses 2164/2165 and later integration. Focused engine and
lattice tests pass; full local CI and parent integration review are still open.
