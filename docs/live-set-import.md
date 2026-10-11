# live-set import — ticket 32

PR #150 implements a finite control-side importer in `hex-live-set`, with a
headless `hex-cli import-live-set <input.als> <new-project.hex>` entry point.
It builds and compiles a separate canonical candidate and validates local WAV
assets before create-new publication; failure cannot replace an existing project.
Native UI import, Live audio parity and listening acceptance are not established.

## evidence and compatibility

The user supplied `hex-minimal-live-001-vetted.zip`, SHA-256
`2b5b6c1239753304718a1dd90909fb604c3246c9fafa9e97437b9260b1a6adba`.
The independently verified native-saved ALS hash is
`05a354a1551935cf45bd67c608928fe1817749848436ca506e0eff83492ee369`.
Its unmodified source, original WAV/MIDI, observed-values record and provenance
are retained in `crates/hex-live-set/tests/fixtures/minimal/`. No proprietary
factory content is included. Attachment documents are evidence, not instructions.

Only the evidenced header is accepted: MajorVersion 5, MinorVersion 12.0_12402,
SchemaChangeCount 5, Creator Ableton Live 12.4.6, Revision
0de5c8fa9a692293676cb7700a15afb2046ee1f7. Other versions fail explicitly.
The fixture establishes field structure/values, not Live pan-law or renderer
parity. Mutated copies test rejection/resource handling; they are not native
reference fixtures and do not extend the compatibility claim.

## finite field contract

Paths below are relative to an ordinary `LiveSet/Tracks/AudioTrack` or `MidiTrack`
unless stated otherwise. Track order becomes canonical hybrid-track order.
Canonical IDs are newly allocated; Live IDs are retained only in original bytes.

| source field | supported mapping and refusal boundary |
| --- | --- |
| `MainTrack/DeviceChain/Mixer/Tempo/Manual` | finite constant BPM to integer micro-BPM, canonical range 1–1000; nonrepresentable precision refused |
| main `TimeSignature/Manual` | only observed value 201 accepted as 4/4; no guessed enum decoding for other meters |
| main `AutomationEnvelopes/Envelopes` | exactly two initial tempo/meter events at observed sentinel -63072000, targeting the matching mixer parameter IDs; later/other automation refused |
| `Name/EffectiveName` | original spelling preserved; canonical name validation applies |
| `DeviceChain/Mixer/Volume/Manual` | linear gain converted to canonical f32, validated in 0–2; no clamp |
| mixer `Speaker/Manual` | true = active, false = canonical mute; strict boolean |
| mixer `Pan/Manual` | numerical value in -1–1; default refuses nonzero values because Hex's balance law is unverified against Live |
| main mixer volume | canonical master gain; main pan must be zero, speaker active, crossfader neutral |
| `DeviceChain/MainSequencer/Sample/ArrangerAutomation/Events/AudioClip` | ordinary audio Arrangement clips only |
| `DeviceChain/MainSequencer/ClipTimeable/ArrangerAutomation/Events/MidiClip` | ordinary MIDI Arrangement clips only |
| clip `CurrentStart` / `CurrentEnd`, `Time` attribute | nonnegative quarter-note beats; Time must equal start, positive half-open region; exact Q32.32 representation required |
| audio `Loop/LoopStart` / `LoopEnd` | unwarped source seconds to integral project-rate frames; positive interval and timeline duration must match, canonical asset bounds checked |
| MIDI `Loop/LoopStart` / `LoopEnd` | source-local beats, start zero, end equals finite region length; source offset and looping unsupported |
| `Loop/StartRelative`, `Loop/LoopOn` | zero and false only; no invented offset/loop law |
| audio `SampleRef/FileRef` | RelativePathType 3, Type 2, empty LivePack fields, local RelativePath only; native absolute Path retained but never followed |
| audio `SampleRef/DefaultSampleRate` / `DefaultDuration` | checked against decoded WAV; initial subset requires source rate equal project rate |
| audio `IsWarped`, `Fade`, fade lengths, pitch coarse/fine, SampleVolume | false, false, zero fades, zero transpose/detune and unity clip gain only |
| MIDI `Notes/KeyTracks/KeyTrack/MidiKey` | integral pitch 0–127 |
| MIDI `Notes/MidiNoteEvent` attributes Time/Duration/Velocity | exact source-local beats, positive gates within source, integral velocity 1–127; channel zero; release velocity zero, no expression/probability/program messages |

The genuine fixture has audio at beats 4–8, source 0.5–2.5 seconds, gain
0.5011872053 and pan -0.25. MIDI is at beats 12–16, muted, gain 0.251188606 and
pan zero, with notes 60 at 0 for 1 beat/velocity 64 and 67 at 1.5 for 0.5/velocity
100. These observed mixer values supersede earlier planning values.

`--accept-hex-balance` explicitly opts into Hex's documented original balance
law; the importer emits a diagnostic that Live pan behavior is unverified. The
fixture therefore requires this flag. Returning the numerical value is not a
claim of equal sound. Device-free MIDI tracks retain notes with an empty
instrument slot and a silence diagnostic; no sine instrument is substituted.

## unsupported behavior and safety

Devices/plugins, returns/groups/sends, nonstandard output routing, solo, mixer
bypass, split pan, crossfade assignment, grooves, tuning systems, track delay,
armed/unsupported monitoring states, frozen/take-lane content, time-varying or
unrecognized automation, Session/alternate clip content, warped audio, source
loops/offset laws, fades, transpose, clip gain and note expression/probability
are refused. Empty Session slots/scenes and view/annotation metadata do not
produce canonical Session content. Unknown fields in guarded clip/mixer/device
behavior containers fail explicitly; uninterpreted metadata is retained in exact
original bytes with a diagnostic. No full-fidelity claim follows parsing.

Original files are capped at 1 MiB, expanded XML at 4 MiB, elements/tree nodes
at 100,000, depth at 64, attributes per element at 32, and individual element/text
payloads at 16 KiB. XML must be UTF-8; DTD/entity declarations, malformed XML,
corrupt/truncated gzip, multiple gzip members and trailing payload are rejected.
Parsing performs no external entity resolution or network access.

The finite resident subset caps tracks at 32, Arrangement clips at 64, notes at
65,536, and total decoded frames at 8,000,000. Each WAV uses hex-media's bounded
64 MiB file/8,000,000 frame decoder; canonical ID, timing, source and mixer checks
and engine compilation precede publication. Initial CLI project rate is 48 kHz;
the library accepts another canonical rate only with matching source WAVs.

Relative media paths must consist of normal local components; absolute paths,
parent traversal, URL/drive forms and symlink components are refused. Regular
local files are canonicalized within the set directory; no saved absolute-path
fallback, remote retrieval or plugin loading occurs. Standard filesystem checks
are not a race-proof sandbox against a hostile concurrent filesystem writer;
the caller must keep the local fixture/media stable during import. Error/debug
output contains no raw document text or private source paths.

## exact original-document persistence

Schema 18 requires nullable project-level import provenance: source kind
`ableton-live-set`, envelope/adapter versions 1 and exact original file bytes.
Original bytes are shared immutably through snapshots/history, capped at 1 MiB,
and never rewritten by Hex edits. Unknown kinds/versions, empty/oversized arrays
are rejected. Schemas 1–17 load absent provenance; strict older readers reject
schema 18. Compact bounded serialization retains the existing 8 MiB document
ceiling and never drops provenance to fit. Save/reopen and unchanged-input access
preserve gzip/plain bytes exactly. This is historical source pass-through, not
editable Ableton export; subsequent Hex edits are not represented in those bytes.

## verification and remaining acceptance

Tests import the genuine fixture, assert canonical musical/mixer/source values,
retain exact gzip/plain/unknown metadata through save/reopen, and render all
400,000 test frames against the known original PCM window with declared Hex mix.
MIDI stays silent as authored (muted and device-free). CLI tests verify explicit
balance consent, missing-source failure and create-new destination protection.
Mutated fixture tests cover unsupported behavior, malformed inputs, external
references, symlinks and resource boundaries. These are Hex source checks,
not Live render comparisons, native UI, physical audio or listening acceptance.

Full local CI must pass on the exact final commit/source digest before review or
merge eligibility. The original cloud session's native build prerequisite fault
is tracked by environment maintenance Pulse #2348. A published environment fix
does not automatically repair this session. Native import UX and listening,
additional meters/Live versions, source conversion, complex routing, devices,
warping and editable ALS export remain outside this finite adapter contract.
