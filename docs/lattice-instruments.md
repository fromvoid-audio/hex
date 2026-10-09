# Canonical Lattice instrument slots

Pulse2114 is a focused prerequisite within native runtime16 before Operator19.
This change admits immutable analytic instruments. Mutable FM voices, expression,
plugin instruments, event queues and general chase/tail contracts remain deferred.

## Model and persistence

Each hybrid track has one optional instrument slot, separate from its ordered
serial effects. The slot has a globally unique DeviceId, bypass flag and the same
bounded opaque NativeDeviceState envelope as effects. Generic state replacement
and bypass commands address it by ID; InsertInstrument and RemoveInstrument
change slot ownership transactionally with normal history/Undo.

New tracks, MIDI tracks and import-created tracks receive an explicit
hex.instrument.sine version1 empty envelope. A model-owned neutral helper defines
this default without depending on DSP. Storage schema15 requires the nullable
instrument field. Strict schemas1–14 install that same envelope once, using fresh
IDs above the existing high-water mark. Migration refuses exhausted ID space;
original saved data is not overwritten by decode. Explicit schema15 null means
silence and must not be migrated back to sine.

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

Control prepares an immutable sample-rate-specific configuration. Its prepared
note is a Copy value containing 41 u64 payload words, release duration and a safe
stereo sample function pointer: 344 bytes on 64-bit targets. The first three
words retain the sine phase step, amplitude and attack representation; the
remaining words are zero. The 41-word capacity can also carry the validated
standalone FM4 kernel without a heap owner or unsafe conversion. FM4 remains
unregistered and cannot be selected by a project in this change. Definitions
validate fields before packing. Sample functions must be deterministic, bounded
and free of allocation, deallocation, waiting, I/O and mutable shared state.
This trusted native definition contract is not a sandbox for arbitrary external
code.

Sampling receives authored age and original gate duration, preserving exact
sine seek-mid-note/release behavior. Existing bounded occurrence/span/query and
copied-release banks remain in use. Copied samplers retain their original law and
payload across revision handoff without Arc churn or final heap drops. Existing
revision/source owners still retire on control after callback quiescence.

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
publication. The derived component ceiling is 47,079,424 bytes: 46,137,344
bytes for entries, 24,576 for track-bank owners, 524,288 for Session template
handles and 393,216 for indices. This keeps the existing count limits rather
than adding a narrower byte cap. Active and candidate prepared revisions can
overlap, so their corresponding bank/index component bound is 94,158,848 bytes.
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
81,416 bytes for spans, 16,072,200 bytes total. Relative to the prior 40-byte
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
contract; this analytic stage does not provide it or implement Operator.

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
failure is separate Pulse2113. Native instrument UI/catalog editing is implemented in the follow-up below; mutable
FM contracts and Operator remain open work, independent of this source foundation.

## Instrument panel and canonical class routing

The focused track's device panel renders its canonical instrument slot before
serial effects. Registered instruments can be inserted into an empty slot,
replaced transactionally, removed or bypassed. Replacement preserves the device
ID and bypass flag; instruments have no effect-chain reorder actions. The admitted
sine instrument has no parameter descriptors, so the panel invents no controls.
Unavailable or malformed state remains visible rather than silently resetting.

Session parameter reads, patches and transient gestures resolve processing class
from canonical track-slot ownership. A gesture retains and revalidates class and
type identity; the shared UI control cache also includes both identities. New-device
requests explicitly select a class catalog, while existing IDs never authorize a
registry choice supplied by the UI. Existing effect gesture barriers and history
remain in use. Adding a registered instrument requires no device-specific storage
or UI codec.

Session tests cover class refusal, instrument insert/remove Undo/Redo, replacement
identity and bypass, parameter/gesture refusal for sine, and save/reopen. Rendered
GPUI tests cover instrument-first order and routed actions alongside Utility;
nine existing device-panel gesture tests pass. Full local CI and native acceptance
are separate gates. This panel does not register FM4 or implement Operator;
general instrument replacement-tail comparison remains part of the FM4 admission
work.
