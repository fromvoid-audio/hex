# sampler

Requested 17 October 2026: a native equivalent of Ableton Live **Simpler**, named
**sampler** in Hex. This does not target the separate Ableton Sampler instrument.
Pulse2679 owns the full request; [PR296](https://github.com/PORTALSURFER/hex/pull/296)
contains the first original playback-kernel prerequisite. Sampler is **not yet
registered, selectable or usable as a device in Hex**. No reference parity or
native interaction/listening acceptance is claimed.

## current source boundary

`hex-devices::sampler` accepts borrowed, validated finite stereo PCM at its
original rate. Construction happens on control; no decoding, paths, asset IDs,
project edits, queues, shared-owner clones or resource destruction occur in the
kernel. `Sample` validation scans PCM once. Preparing another voice does not
scan or copy the sample. The caller must keep PCM and slice-marker storage alive
for all borrowed configurations/voices; Rust enforces these lifetimes.

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
