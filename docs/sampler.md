# sampler

Requested 17 October 2026: a native equivalent of Ableton Live **Simpler**, named
**sampler** in Hex. This does not target the separate Ableton Sampler instrument.
Pulse2679 owns the full request; [PR296](https://github.com/PORTALSURFER/hex/pull/296)
contains the first original playback-kernel prerequisite. Sampler is **not yet
registered, selectable or usable as a device in Hex**. No reference parity or
native interaction/listening acceptance is claimed.

## next musical-use candidate

The user's next deliverable is a playable, high-quality **Simpler-style** sampler
with a clean native panel; the other goal3 devices and Instrument Grid do not
gate it. Pulse2679 revision16 preserves the full existing feature acceptance.
PR296 and PR369 remain merged kernel/lifetime prerequisites. PR389 remains the
canonical resource foundation; selected storage/ABI proofs do not complete it.

The finite critical path, in dependency order, is:

1. Close the actual Sampler host graph. Finish prepared graph/container and
   model/history/media ownership union, source scopes through Arrangement,
   Session, keyboard and export, old/new preparation peak, atomic candidate
   publication and saturated existing reaper/control final-drop proof. Qualify
   actual supported constructors rather than raw Arc/count identities. Keep
   unsupported referenced graph classes refused. Do not wait for unrelated
   device algorithms or add a generic owner/cache framework.
2. Adapt the existing kernel into a real registered device. Prepared notes and
   mutable voices retain only bounded coefficients/slot facts; callbacks borrow
   typed original Mono/Stereo PCM from the actual host configuration. No borrowed
   `Voice` may be made self-referential or stored with a forged static lifetime.
   Retain Classic, One-Shot and Slice, checked Trigger/Gate lifetime, voices,
   retrigger/glide, pitch/tuning, region/loop/reverse and amplitude envelope.
   Complete the required filter/LFO/modulation and warp with actual DSP;
   quality includes meaningful downsampling/alias, loop seam, transient/stereo
   and partition oracles, not a claim that linear interpolation is sufficient.
3. Connect current-format authored state and loading. Use the existing neutral
   asset references, bounded original media worker/source plan, accepted
   immutable waveform backing and canonical prepared edits. Load/replace plus
   assignment is one Undo; preserve asset identity+tuning on save/reopen/preset.
   Missing/unsupported media, stale/canceled preparation and failed replacement
   keep the last valid audio and expose a recoverable error. No alpha migration
   or embedded PCM payload is needed. Warp uses the implemented stretch boundary
   and prepared worker lifecycle; it must not silently become resampling.
4. Land the functional panel in `hex-ui/src/editor/device_panel/sampler_panel.rs`
   beside existing native panels, using `hex-widgets` and Hex theme typography.
   Put sample identity/load/replace and Classic/One-Shot/Slice above a central
   waveform; expose direct bounds/loop/slice edits and clear selected/playback
   state. Group legible pitch/tuning, amplitude/envelope, filter and modulation
   controls below it, with focused mode-specific controls and an expanded view.
   Reuse the accepted waveform cache/geometry; no pointer-motion decode/DSP.
   Use visible loading/empty/missing/disabled states and discoverable essential
   actions. Every control must reach canonical state, DSP and Undo; no panel or
   catalog claim before functioning host playback/loading.
5. Freeze the combined source, run meaningful device/host/session/storage/UI
   regressions, callback allocation/lifetime and dense-voice release profiling,
   then the required exact-source local CI. Send one bounded optimized native
   build/acceptance request to the coordinator, preserving launcher delivery.

The official interaction reference is Live12 manual section31.11/31.11.1,
[Simpler](https://www.ableton.com/en/live-manual/12/live-instrument-reference/#simpler),
as verified by the coordinating lane. This cloud's direct request returned403;
no new installed reference measurements or sonic parity evidence are claimed.
This target does not add the separate multisample Sampler instrument or zones.

Finite native acceptance on the eventual pinned optimized candidate: load and
replace an original sample; play from MIDI keyboard and a clip in Arrangement
and Session; exercise Classic, One-Shot/Trigger and Slice, bounds/loop/reverse,
envelope/filter/modulation and implemented warp; Undo/Redo; save/reopen a
disposable current-format project retaining assets+tuning; compare playback and
export; test missing media/cancel/stale replacement and Stop/seek/retirement.
Inspect compact/expanded layouts and hear pitch/downsampling/loop/transient
quality on that same executable. Cloud source tests do not replace this native
workflow/listening evidence. No other device-family completion is a prerequisite.

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

`BoundSamplerPlan::prepare_runtime_on_control` now prepares a fixed 128-key
step/region table plus shared scalar coefficients against the actual project
tuning. Every reconstructed key is tested against the full scalar plan across
modes, reverse and non-neutral tuning. The complete factory remains under the
existing 4 KiB context cap. A mutable lattice runtime
uses at most 32 scalar voice slots, caller-bounded events and borrowed original
PCM; no pitch exponentiation or PCM scan occurs in processing. Its declared twin
storage includes the complete key table and active state, under the existing
runtime pair cap. Gate/release and Trigger/source exhaustion are distinct;
same-offset events retain caller order, and unsupported capacity/source batches
refuse before the paired runtime or output changes. Speculative rollback and
history copying use the existing lattice runtime pair. Exact source/configuration
identity governs history compatibility; identity is never a lease.

The assigned root is calibrated in neutral 12-TET. Effective project frequencies
retune it, including changes to concert-reference Hz; dividing by a moving
project-tuned root would incorrectly cancel those changes. Neutral tuning keeps
the exact original scalar law. Slice keys select regions rather than pitches.

The existing engine `SharedContextOwner` can construct and retain a concrete
`OwnedShared<PreparedSamplerRuntime>` child beside its actual source configuration.
Its fixed scalar leaf audit is exhaustive; no arbitrary trait object or old Arc
receives a declaration. Body/header, marker decode temporary and existing owner
metadata are quoted before construction under unchanged caps. Selected storage
traversal includes that physical child and every actual alias. Tests refuse an
omitted factory alias and exact-minus-one budgets and retain actual PCM after
source/model retirement. Removing the entire factory visitor fails the negative
oracle; byte-restored source passes. This constructor is not whole-graph peak,
custody, publication or saturated-reaper proof and does not open registry access.

This is still an unregistered runtime slice. Voice stealing/retrigger/glide,
filter/LFO/modulation, high-quality resampling and warp, authored state/loading,
the functional panel and full graph publication/retirement remain unfinished.
The 32-slot bound is the current kernel/runtime support envelope, not a measured
Live limit or permission to silently drop notes. Audio-harness tests measure
Mono/Stereo processing, events and rollback with zero allocations/frees and
independent PCM sums; they do not establish whole-root native admission.

The current sampler state codec now retains mode, source-frame bounds, loop and
slice markers, reverse, pitch/root, gain/pan/velocity, amplitude envelope and voice
count in a bounded payload (96-byte prefix plus at most 128 four-byte markers).
Settings edits preserve the actual host gains and asset-reference allocation.
Only the current version is accepted; truncation, reserved encodings, nonfinite
values and unsupported versions refuse. No old-alpha conversion is provided.
Authored preparation requires exactly the matching source slot/asset in the host
inventory, validates actual source bounds and finite PCM once on control, and
uses the host's tuning and output rate. Decoded state and marker storage may be
destroyed after scalar runtime preparation. This does not yet connect file
loading, canonical device commands, project round-trip or UI interaction.

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
| Lifetime | A nonlooping voice reports its natural output-frame length; a looping voice reports no natural end. Gate release and Trigger natural-end retention must be handled separately by the future host adapter. |

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
