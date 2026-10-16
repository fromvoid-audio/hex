# Vector global panel

Pulse #2653 / [PR269](https://github.com/PORTALSURFER/hex/pull/269) implements
the algorithm strip, controller assignments, and global voice/quality/pan state.
Reference provenance and remaining comparison limits are recorded in
[the observation](../research/observations/vector-global-2653.md).

## Routing and persistence

The eleven diagrams share one catalog with the device definition. Connections
are directed phase modulation at Hex's existing 0.25-turn depth; carriers use
unit weights. Oscillator levels control both modulation and audible amplitude.
New patches start with D→C→B→A, A at unity and B/C/D silent. This is a behavioral
implementation, not measured Live FM-index or loudness equivalence.

Algorithm parameter 600 derives its value from the twenty stored routing
coefficients. Choosing an algorithm replaces those coefficients in one canonical
edit. It preserves oscillators, envelopes and unrelated settings. A saved custom
matrix reads as `custom`; its sound is retained until an algorithm is selected.
The raw coefficient matrix is no longer the global panel's interaction model.
Historical preset parameter 61 retains its original four choices and normalized
carrier weights; it is not reinterpreted as the eleven-algorithm selector.

State v15 appends 112 bytes after the complete 1,616-byte v14 prefix: 32 bytes of
global settings, including an explicit legacy-mode flag, and 80 bytes of
controller assignments. Old versions migrate with legacy processing enabled;
decoding or saving an old patch does not silently change its oscillator levels,
topology, voice policy or quality law. Editing a new global control or controller
assignment deliberately opts that patch into the new runtime. New patches use
the new runtime. Existing IDs remain stable; new IDs are 600–608 and 620–639.
The explicit behavior selector 608 displays `legacy` or `operator`. Legacy mode
disables global and assignment affordances until operator mode or an algorithm
is selected. Returning to legacy processing requires quiescence; an active
history must not silently reset to a different processing law.

## Voice and pan laws

Voice choices are 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24 and 32.
The 64-entry admitted-occurrence ledger is independent of sounding polyphony.
At the selected limit, the oldest sounding voice is replaced; its later note-off
retires only its original occurrence. Retrigger replaces a sounding same-pitch
voice. One-voice overlapping different pitches play legato, keeping envelope,
phase and filter progress; releasing the newest key returns to the latest held
key. Lowering polyphony applies oldest-first retirement to the actual history.

Pan is a linear per-note balance: center preserves dual-mono bits, and either
endpoint attenuates the opposite channel to zero. Key pan uses pitch relative
to MIDI 60 over a 60-semitone span. Random pan is a deterministic per-onset hash;
rollback and history transfer retain its onset identity. These laws have not
been calibrated against Live's panning law.

## Controller assignments

Each source has two destination/amount slots. Key's second destination is fixed
to envelope time, and bend's second destination is fixed to pitch. Available
destinations are off, oscillator A/B/C/D level, filter frequency/resonance/morph,
LFO rate/amount, volume, pan, pitch and time. Unsupported destinations are not
advertised. Filter morph changes sound only with the morph filter selected.

Velocity and pressure/wheel use 0–1; key is centered on MIDI 60 and normalized
over 60 semitones; bend is bipolar with exact -1/0/+1 endpoints. Freely assigned
amounts are signed percentages. Full depth corresponds to four octaves for
oscillator amplitude and LFO rate, eight octaves for cutoff, 100 percentage
points for resonance/morph/LFO amount, 24 dB for volume, the full pan range,
12 semitones for pitch, or four octaves of envelope rate. Accumulated values
are bounded, destination domains are clamped, and envelope rate is limited to
0.25–4 to retain a finite release horizon. Bend's fixed range is 0–48 semitones,
default 5. These are documented original transfer laws, not measured Live curves.

Assignments derive effective values from authored baselines; they never write
modulated values into project state. Event/retarget preparation computes effective
filter coefficients. A separate envelope clock preserves accumulated progress
when its rate changes. Current typed expression events carry pressure, bend and
wheel at sample offsets on one logical instrument channel. Equal-offset event
order belongs to the caller; note occurrences retain independent identities.

Connected MIDI controllers provide pressure, bend and wheel through a separate
identity-bound lease for Arrangement, Session and computer-keyboard notes.
Choose a source/channel in the native controller-input row; input starts off.
See [controller input](controller-input.md) for coalescing, cancellation, channel
scope and lifecycle limits. Physical MIDI notes, sustain, MPE and editable or
recorded clip controller lanes are outside this change. Native port behavior and
listening remain user acceptance checks; headless tests do not establish them.

## Quality processing

The new runtime evaluates the oscillator/FM graph and carrier filter at 2× the
host rate. Envelopes and LFO control values are held over each pair of subframes.
Antialias selects an eighth-order Butterworth lowpass at 0.4× host sample rate
before decimation; disabling it selects the second unfiltered subframe. Both
paths keep the decimator warm so held-note quality changes retain history.
There is no buffering/lookahead latency; IIR phase delay is frequency dependent.
The finite zero-input drain is 4,096 output frames.

Interpolation switches between interpolated and nearest table evaluation for
oscillators and the sampled 1,024-step periodic LFO table. Sample-and-hold LFO
retains its piecewise-constant definition. This is an original quality law;
2× processing cannot remove every high-order FM alias and does not establish
Operator-equivalent antialiasing.

Slow time modulation can extend the companion envelope bound fourfold. The
engine's finite mutable-tail admission horizon is therefore 600 seconds, covering
560 seconds plus decimator drain even at 8 kHz. Independent voice, event, memory,
checkpoint and replay limits remain enforced. A longer admitted horizon does
not guarantee that every dense or long preroll fits the existing budgets.
Each mutable twin occupies 116,520 B; its primary/scratch pair occupies
233,040 B and draws from the 2 MiB aggregate DSP-pair pool. This is substantially more storage than the legacy feedback
runtime; [the measured CPU/storage limits](performance.md) and Pulse2226 retain
the density/performance follow-up. The voice menu is not an unlimited track-
density claim.

## Verification boundary

Device regressions cover catalog routing and silent-modulator behavior, old-state
migration, controller domains, voice stealing/retrigger/mono, held-note changes,
partition independence, copy/reset and spectral attenuation in finite fixtures.
Final-source local CI, actual callback allocation probes, optimized native build,
input-producer integration and user visual/listening results must be recorded
with their exact source identities. Passing mathematical DSP checks does not
prove physical-device deadlines or numerical Live parity.
