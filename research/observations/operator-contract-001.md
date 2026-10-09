# operator-contract-001 — instrument boundary and reference experiments

Status: planned experiments; public-documentation inventory observed.
Pulse 19 owns implementation; Pulse 31 owns comparison evidence. This document
does not establish a working Operator or waveform parity.

## provenance

On 9 October 2026, `sh scripts/inventory-live.sh` rechecked the installed
`/Applications/Ableton Live 12 Suite.app`: 12.4.6
(2026-09-10_0de5c8fa9a), universal arm64/x86_64 executable SHA-256
`5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`.
The initial comparison target is arm64. Public reference: Ableton's
[Live 12 Operator manual](https://www.ableton.com/en/live-manual/12/live-instrument-reference/#operator),
sections 31.9.1–31.9.10, retrieved 9 October 2026. No reference application
interaction, audio capture, decompilation or proprietary asset copying occurred
in this study. No measured fixture or output hash exists yet.

## documented capability inventory

The manual describes four oscillators and eleven routing algorithms, with FM,
additive waveform editing and subtractive filtering. Oscillators support
ratio/fixed tuning, phase and feedback controls. Seven envelopes cover the
oscillators, filter, pitch and LFO. The device exposes modulation routing,
polyphony/retrigger, glide/spread and interpolation/antialias controls. User
partials can be edited at 16/32/64 harmonics. The interface keeps oscillator
controls around a contextual central display. These are documented capabilities,
not measurements of their numerical implementation. Consult the linked complete
parameter list before treating a starter synth as feature-complete.

## independent Hex contract proposal

Operator requires a distinct registered instrument contract before an audio
effect chain. The current stateful effect consumes summed stereo frames and
cannot substitute for note input, voice ownership or source scheduling. This is
a proposal requiring architecture acceptance, not an already implemented API.

Preparation owns validation, state migration, coefficients/tables, maximum
voices/events and allocation. The audio owner consumes timestamped bounded
events, retains preallocated voice histories and emits stereo frames through the
same Arrangement/Session/offline paths. Compatible graph edits, outgoing tails,
failed-block rollback, note-off delivery, seeks, Stop and retirement must have
explicit semantics. Old files retain the existing sine behavior through an
explicit migration/default; inserting a new instrument must not stack it on the
legacy sine implicitly. Generic storage keeps its device envelope rather than
adding an Operator-specific DTO. See [lattice](../../docs/lattice.md).

Implementation gates, each independently reviewable: instrument registration
and event lifecycle; original oscillator/envelope kernel; verified routing
graphs and voice behavior; modulation/filter/quality features; compact controls
and waveform/envelope editing; reference comparisons and performance. Every
visible control needs functioning validated state and processing. Missing
features remain explicit; presets or a four-sine demo cannot prove completion.

## planned original experiments

Create fresh user-authored test sets without factory presets. Capture the exact
rate, tempo, tuning, note pitch/channel/velocity, sample-offset Note On/Off,
polyphony and every relevant parameter. Store the input event manifest, set and
audio SHA-256 hashes privately with the reference identity. Use 44.1 and 48 kHz,
matching export rates, no dither/normalization or unrelated devices. These are
proposed fixtures, not completed captures.

1. Isolated sine: one audible oscillator, unity ratio, other levels off. Vary
   initial phase, note duration and release independently. Measure fundamental,
   level and onset/release boundaries without guessing the phase-reset policy.
2. Ratio/fixed tuning: sweep one parameter at a time with two note pitches.
   Record actual bounds, defaults and conversion; do not infer units from a
   label alone.
3. Routing: establish all eleven directed graphs from original control
   observations. Sweep one modulator against an isolated carrier; inspect
   spectra and feedback separately. Do not guess connectivity from an unseen
   icon or reuse another FM synth's algorithm numbering.
4. Envelopes: isolate each stage at multiple velocities, note lengths and
   pitches; exercise looping/retrigger. Compare sample boundaries and curves.
5. Voice lifecycle: overlapping equal-pitch notes, pressure/bend messages, voice
   exhaustion, glide and spread transitions. Test block partitions of 1, 64,
   128 and 256 frames in Hex and distinguish scheduling from reference output.
6. Quality/filter/modulation: compare each implemented mode individually,
   checking aliasing, gain, resonance, noise behavior and modulation rates.
   Random outputs require a documented statistical comparison unless reset or
   seed behavior is established.

Maintain raw captures; never align away a timing discrepancy silently. Report
frequency in Hz/cents, levels in dB, timing in frames and spectral/noise error
with explicit windows. Tolerances are to be established per experiment; none
is an accepted parity threshold yet. Separate source oracles from native
listening, one-to-one compatibility and sustained callback deadlines.

## confidence and unknowns

High confidence in the public capability inventory; no measured confidence in
parameter laws, graph numbering, feedback discretization, voice stealing,
oscillator/noise tables, filter response, antialiasing or exact default sound.
The proposed boundary follows Hex ownership rules; it still needs source-grounded
architecture review before implementation. No Rust Operator implementation or
matched comparison is claimed.
