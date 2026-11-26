# Live 12 Operator to Hex Vector (`hex.instrument.operator`) Parameter Coverage Audit

- **Pulse issue**: `#3880`
- **Date**: 2026-11-26
- **Source baseline inspected**: `origin/main` `700a6aa80`
- **Owned source files inspected**:
  - `crates/hex-devices/src/operator_state.rs` (`OPERATOR_STATE_VERSION = 18`, TLV `HXOP` format v1)
  - `crates/hex-devices/src/operator_state/{algorithms,automation,companion,enable,fine,global,harmonics,keytrack,lfo,modulation,tlv_codec,tuning}.rs`
  - `crates/hex-devices/src/operator_state/companion/envelope.rs`
  - `crates/hex-devices/src/operator_state/harmonic_runtime.rs`
  - `crates/hex-devices/src/operator_state/harmonic_runtime/{filter,modulation,quality}.rs`
  - `crates/hex-devices/src/operator_filter.rs`
  - `crates/hex-devices/src/operator_envelope.rs`
  - `crates/hex-devices/src/operator_waveform.rs`
  - `crates/hex-devices/src/fm4/device.rs`
  - `docs/design/vector-brief.md`
- **Reference provenance & limits**:
  - Vector's canonical parameter descriptors, IDs, ranges, defaults, and DSP transfer laws below are verified directly from the inspected Rust source above.
  - Live 12 Operator control inventory is derived from the public Live 12 Reference Manual (Section 31.9, Operator) and previously recorded Hex observations (`docs/design/vector-brief.md`).
  - No live binary capture against an installed Ableton Live 12.4.6 build was executed in this audit; numerical DSP parity with Ableton Live Operator remains unmeasured and is not claimed.

## Summary of Findings vs. 31 October 2026 Preliminary Grep

The preliminary descriptor-label grep noted in `#3880` predated several landed Operator engine suffixes (`harmonics.rs`, `companion.rs`, `fine.rs`, `keytrack.rs`, `tlv_codec.rs`). Direct source inspection confirms:

- **Already present (contrary to the 31 Oct preliminary grep)**:
  - **Oscillator harmonic repeat**: IDs `353..=356` (`harmonic repeat`: `off`, `2`, `4`, `8`) in `operator_state/harmonics.rs`.
  - **Envelope loop modes**: IDs `362, 370, 378, 386` (`loop`: `none`, `loop`, `beat`, `sync`, `trigger`), `loop time` (`363, 371, 379, 387`, `0.001..60.0 s`), and `beat` (`364, 372, 380, 388`, `1/64..16`) in `operator_state/companion.rs`.
  - **Per-oscillator companion controls**: `r` phase retrigger (`357, 365, 373, 381`), `osc<vel` (`358, 366, 374, 382`), `q` quantize (`359, 367, 375, 383`), `time<vel` (`360, 368, 376, 384`), `key` amplitude key-scaling (`361, 369, 377, 385`), `op1..4 amplitude velocity` (`74..=77`), `op1..4 fine hz` (`609..=612`), and `op1..4 pitch keytrack` (`613..=616`).
- **Confirmed missing (7 focused engine tickets created and linked)**:
  1. Global `time`, `tone`, and output `volume` -> **`#13607`** (linked to `#3890`).
  2. Global `transpose`, `spread`, and `glide` (`glide enabled` + `glide time`) -> **`#13608`** (linked to `#3889`, `#3891`).
  3. Bipolar per-voice **pitch envelope** -> **`#13609`** (linked to `#3891`, `#3892`).
  4. Per-voice **filter envelope** and direct `freq<vel` / `freq<key` modifiers -> **`#13610`** (linked to `#3892`; also enables follow-up sweep shading on the filter page from `#3886` and `docs/design/vector-brief.md`).
  5. **Filter shaper** (`type`, `drive`, `dry/wet`) and nonlinear **filter circuit drive / models** -> **`#13611`**.
  6. Oscillator **envelope slopes** (`attack`/`decay`/`release` curve), `end` level, and `vector-brief.md` **note-pitch sync** loop mode -> **`#13612`**.
  7. **Extended LFO** (`low`/`high`/`sync` range including audio-rate `high`, `saw down` & `noise` waveforms, `filter` destination, `r<key`, `amt<vel`, and LFO envelope) -> **`#13613`** (linked to `#3887`, `#3892`).

---

## Complete Parameter Coverage Table

Status legend:
- **Present**: canonical parameter exists and drives runtime DSP.
- **Partial**: canonical parameter exists with narrower domain, missing sub-modes, or intentional design divergence documented in `docs/design/vector-brief.md`.
- **Missing**: no canonical parameter or runtime DSP path exists today.

### 1. Oscillators A–D (Per-Operator Controls)

| Live 12 Operator Control | Status | Vector Parameter ID(s) & Label(s) | Current Range / Choices & Default | Known Law / Notes | Follow-up Ticket |
|---|---|---|---|---|---|
| Osc A–D Enable | **Present** | `513..=516` (`op1..4 enabled`) | Bool, default `true` | Independent of stored level; muting preserves authored level | — |
| Osc A–D Coarse (Ratio) | **Present** | `1, 4, 7, 10` (`op1..4 ratio`) | Float `[1/128, 128.0]` `"x"`, defaults `1, 2, 3, 4` | Linear multiplier on note frequency; `q` (`359,367,375,383`) quantizes; sample-rate modulation target | — |
| Osc A–D Fine | **Partial** (design divergence) | `609..=612` (`op1..4 fine hz`) | Float `[-1000.0, +1000.0]` `"hz"`, default `0.0` | Signed Hz added after ratio/fixed multiplication per `docs/design/vector-brief.md` (Live Operator uses dimensionless `0..1000` fractional coarse steps); sample-rate modulation target | — (intentional per `vector-brief.md`) |
| Osc A–D Fixed toggle | **Present** | `62, 65, 68, 71` (`op1..4 fixed`) | Bool, default `false` | Mirrored with `pitch keytrack == 0.0%` (`613..=616`) | — |
| Osc A–D Fixed Freq | **Present** | `63, 66, 69, 72` (`op1..4 freq`) | Float `[0.1, 20000.0]` `"hz"`, default `440.0` | Multiplied by `opX multi` | — |
| Osc A–D Fixed Multi | **Present** | `64, 67, 70, 73` (`op1..4 multi`) | Float `[0.01, 1000.0]`, default `1.0` | Continuous multiplier (Live offers discrete `0.1, 1, 10, 100`) | — |
| Pitch Keytrack (Vector extension) | **Present** | `613..=616` (`op1..4 pitch keytrack`) | Float `[0.0, 100.0]` `"%"`, default `100.0` | Continuous pitch tracking (`0%` = fixed, `100%` = 12-TET); supersedes binary Fixed-only tracking | — |
| Osc A–D Level | **Present** | `2, 5, 8, 11` (`op1..4 level`) | Float `[0.0, 8.0]` `"linear"`, defaults `1.0, 0.0, 0.0, 0.0` | Linear amplitude/modulation scale; UI formats dB; sample-rate modulation target | — |
| Osc A–D Phase | **Present** | `3, 6, 9, 12` (`op1..4 phase`) | Float `[0.0, 1.0 - ulp]` `"turns"`, default `0.0` | Normalized turns (`[0, 1)`) | — |
| Osc A–D Retrigger (`R`) | **Present** | `357, 365, 373, 381` (`r`) | Bool, default `true` | `false` free-runs phase from note origin | — |
| Osc A–D Quantize (`Q`) | **Present** | `359, 367, 375, 383` (`q`) | Bool, default `false` | Quantizes pitch modifier law | — |
| Osc A–D Feedback | **Present** | `78..=81` (`op1..4 feedback`) | Float `[0.0, 1.0]`, default `0.0` | Active only on operators with zero incoming modulation edges; sample-rate modulation target | — |
| Osc A–D Waveform | **Present** | `57..=60` (`op1..4 waveform`) | 23 choices: `sine`, `saw d`, `square d`, `triangle`, `user`, `sine 4bit`, `sine 8bit`, `saw 3/4/6/8/16/32/64`, `square 3/4/6/8/16/32/64`, `noise looped`, `noise white`; default `sine` | Band-limited 10-cap wavetable bank + deterministic white noise (`operator_waveform.rs`) | — |
| Osc A–D Harmonics (64 partials) | **Present** | `93..=348` (`harmonic amplitude`) | 4 × 64 Float `[0.0, 1.0]`, default `h1 = 1.0`, others `0.0` | Editing seeds from current builtin waveform and switches to `user` (`4`) | — |
| Osc A–D Harmonic Range (`16`/`32`/`64`) | **Present** | `349..=352` (`harmonic range`) | Choice: `16`, `32`, `64`, default `64` | Masks active harmonic output without discarding stored partials | — |
| Osc A–D Harmonic Repeat | **Present** | `353..=356` (`harmonic repeat`) | Choice: `off`, `2`, `4`, `8`, default `off` | Spectral harmonic repeat law in `harmonics.rs` | — |
| Osc A–D Level<Vel | **Present** | `74..=77` (`op1..4 amplitude velocity`) | Float `[0.0, 1.0]`, default `0.0` | Per-oscillator amplitude velocity sensitivity | — |
| Osc A–D Osc<Vel | **Present** | `358, 366, 374, 382` (`osc<vel`) | Float `[-1.0, 1.0]`, default `0.0` | Companion note-velocity pitch/osc modifier | — |
| Osc A–D Level<Key | **Present** | `361, 369, 377, 385` (`key`) | Float `[-1.0, 1.0]`, default `0.0` | Per-oscillator amplitude key-scaling | — |
| Osc A–D Time<Vel | **Present** | `360, 368, 376, 384` (`time<vel`) | Float `[-1.0, 1.0]`, default `0.0` | Scales A/D/R by `2^(-4 * amount * vel / 127)` clamped to stage maxima | — |
| Osc A–D Env Initial / Peak / Sustain | **Present** | `33..=56` (`op1..4 initial/peak/sustain`) | Float `[0.0, 1.0]`, defaults `0.0 / 1.0 / 0.7` | Linear amplitude levels | — |
| Osc A–D Env Attack / Decay / Release | **Present** | `33..=56` (`op1..4 attack/decay/release`) | Attack `[0.0, 20.0] s` (def `0.005`), Decay/Release `[0.001, 60.0] s` (def `0.1`) | Linear attack; exponential decay/release snapping at `RELATIVE_CUTOFF = 1e-5` (`operator_envelope.rs`) | — |
| Osc A–D Env Loop Mode / Time / Beat | **Present** | `362/370/378/386` (`loop`), `363/..` (`loop time`), `364/..` (`beat`) | `loop`: `none`, `loop`, `beat`, `sync`, `trigger`; `loop time`: `[0.001, 60.0] s`; `beat`: `1/64..16` (11 choices) | Implemented in `operator_state/companion/envelope.rs` | — |
| Osc A–D Env End Level | **Missing** | — | — | Release currently always targets `0.0` | `#13612` |
| Osc A–D Env Attack / Decay / Release Slopes | **Missing** | — | — | Required by Live 12 Operator and `docs/design/vector-brief.md` | `#13612` |
| Osc A–D Env Note-Pitch Sync Loop Mode | **Missing** | — | — | Required by `docs/design/vector-brief.md` (loop synced to played-note period with ratio) | `#13612` |

### 2. Pitch Section (Pitch Envelope & Pitch Performance)

| Live 12 Operator Control | Status | Vector Parameter ID(s) & Label(s) | Current Range / Choices & Default | Known Law / Notes | Follow-up Ticket |
|---|---|---|---|---|---|
| Pitch Env On/Off | **Missing** | — | — | No pitch envelope in engine | `#13609` |
| Pitch Env Amount & Amount<Vel | **Missing** | — | — | No pitch envelope in engine | `#13609` |
| Pitch Env Initial / Peak / Sustain / End | **Missing** | — | — | Bipolar semitone levels (`-48..+48 st`) absent | `#13609` |
| Pitch Env Attack / Decay / Release & Slopes | **Missing** | — | — | Absent | `#13609` |
| Pitch Env Loop Mode / Time / Beat | **Missing** | — | — | Absent | `#13609` |
| Pitch Env Destinations (A–D, LFO) | **Missing** | — | — | Engine targets in `#13609`; generalized via shared modulation in `#3892` | `#13609` |
| Global Transpose (`-48..+48 st`) | **Missing** | — | — | No canonical transpose parameter | `#13608` |
| Global Spread (`0..100%`) | **Missing** | — | — | No stereo voice detune/spread parameter | `#13608` |
| Glide On/Off & Glide Time | **Missing** | — | — | Required by both Live 12 Operator and `docs/design/vector-brief.md` / `#3889` | `#13608` |

### 3. Filter & Shaper Section

| Live 12 Operator Control | Status | Vector Parameter ID(s) & Label(s) | Current Range / Choices & Default | Known Law / Notes | Follow-up Ticket |
|---|---|---|---|---|---|
| Filter On/Off | **Present** | `20_001` (`filter enabled`) | Bool, default `false` | Pre-envelope carrier SVF (`operator_filter.rs`), 2x oversampled in operator mode | — |
| Filter Type | **Present** | `20_002` (`filter type`) | Choice: `low-pass`, `high-pass`, `band-pass`, `notch`, `morph`, default `low-pass` | Matches Operator's 5 filter types | — |
| Filter Slope | **Present** | `20_003` (`filter slope`) | Choice: `12`, `24`, default `24` | 12 dB/oct and 24 dB/oct SVF | — |
| Filter Frequency | **Present** | `20_005` (`filter frequency`) | Float `[20.0, 20000.0]` `"hz"` (log), default `1000.0` | Clamped to `0.45 * rate`; sample-rate modulation target | — |
| Filter Resonance | **Present** | `20_006` (`filter resonance`) | Float `[0.0, 100.0]` `"%"`, default `0.0` | Sample-rate modulation target | — |
| Filter Morph | **Present** | `20_007` (`filter morph`) | Float `[0.0, 100.0]` `"%"`, default `0.0` | Active in `morph` mode; sample-rate modulation target | — |
| Filter Circuit & Drive | **Partial** | `20_004` (`filter circuit`) | Choice: `clean` only (1 choice), no `filter drive` | Missing `osr`, `ms2`, `smp`, `prd` models and `drive` (`0..24 dB`) | `#13611` |
| Filter Shaper Type / Drive / Dry-Wet | **Missing** | — | — | Missing `off`, `soft`, `hard`, `sine`, `4bit`, `drive`, `dry/wet` | `#13611` |
| Filter Envelope (Amount, A/D/S/R, Initial/Peak/End, Slopes, Loop) | **Missing** | — | — | `docs/design/vector-brief.md` specifies `-96..+96 st` bipolar depth with `base * 2^(depth * level / 12)` | `#13610` |
| Filter Freq<Vel & Freq<Key | **Missing** (direct) | — | — | Reachable only indirectly by consuming one of the 2 generic velocity/key controller slots (`620..627`), no dedicated filter modifiers | `#13610` |

### 4. LFO Section

| Live 12 Operator Control | Status | Vector Parameter ID(s) & Label(s) | Current Range / Choices & Default | Known Law / Notes | Follow-up Ticket |
|---|---|---|---|---|---|
| LFO On/Off | **Present** | `82` (`lfo enabled`) | Bool, default `false` | Migrates into `"lfo 1"` slot modulator in `#3887`/`#3892` | — |
| LFO Waveform | **Partial** | `83` (`lfo waveform`) | Choice: `sine`, `triangle`, `saw`, `square`, `sample and hold`, default `sine` | Missing `saw down` and `noise` | `#13613` |
| LFO Range (`Low`/`High`/`Sync`) | **Partial** | `84` (`lfo sync`) | Bool (`false` = free `0.01..100 Hz`, `true` = sync) | Missing `high` audio-rate range | `#13613` |
| LFO Rate & Sync Division | **Present** | `85` (`lfo rate`), `86` (`lfo division`) | `rate`: `[0.01, 100.0]` `"hz"` (log, def `1.0`); `division`: 9 choices `1/16 beat`..`16 beats` (def `1 beat`) | `lfo rate` is a sample-rate modulation target | — |
| LFO Amount | **Present** | `87` (`lfo amount`) | Float `[0.0, 100.0]` `"%"`, default `0.0` | Sample-rate modulation target | — |
| LFO Retrigger (`R`) | **Present** | `88` (`lfo retrigger`) | Bool, default `true` | Restarts LFO phase per note when enabled | — |
| LFO Destinations A–D | **Present** | `89..=92` (`lfo a..d`) | Bool × 4, default `true` | Pitch modulation targets; migrate to routes in `#3892` | — |
| LFO Destination Filter & Dest B / Amount | **Missing** | — | — | No `lfo filter` toggle or Dest B slot (`#3892` generalizes destinations via shared routes) | `#13613` |
| LFO R<Key & Amt<Vel | **Missing** | — | — | Rate key-tracking and amount velocity scaling absent | `#13613` |
| LFO Envelope (A/D/S/R, Initial/Peak/End, Slopes, Loop) | **Missing** | — | — | No LFO amplitude envelope | `#13613` |

### 5. Global, Routing, and Controller Mapping Section

| Live 12 Operator Control | Status | Vector Parameter ID(s) & Label(s) | Current Range / Choices & Default | Known Law / Notes | Follow-up Ticket |
|---|---|---|---|---|---|
| Routing Algorithm (11 topologies) | **Present** | `600` (`algorithm`), `61` (`routing preset`), `14..27` (`opX to opY`), `29..=32` (`op1..4 carrier`) | `algorithm`: `custom` + `algorithm 1..11` (def `1`); directed DAG edges `[-8, 8]` turns (catalog uses `0.25` turns); carrier gains `[-1, 1]` | `algorithms::CATALOG` matches Operator's 11 four-operator topologies 1:1 | — |
| Voices | **Present** | `601` (`voices`) | 15 choices: `1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24, 32`, default `8` | `608` (`mode`: `legacy` / `operator`) preserves historical pre-v15 playback | — |
| Global Retrigger | **Present** | `602` (`retrigger`) | Bool, default `true` | Voice stealing/retrigger policy | — |
| Interpolation & Antialias | **Present** | `603` (`interpolation`), `604` (`antialias`) | Bool, defaults `true` | 2x oversampling + 8th-order Butterworth decimation (`quality.rs`); scheduled for retirement (always on) in `#3892` | — |
| Pan, Pan<Key, Pan<Rnd | **Present** | `605` (`pan`), `606` (`pan key`), `607` (`pan random`) | `pan`: `[-50, 50]` (def `0`); `pan key`: `[-100, 100]` (def `0`); `pan random`: `[0, 100]` (def `0`) | Center is bit-identical dual mono (`global::Settings::gains`) | — |
| Time<Key | **Present** | `627` (`time key`) | Float `[-100, 100]` `"%"`, default `0.0` | Fixed `key destination b` (`626 = time`); scales envelope speed by `2^(4 * depth)` clamped `[-2, 2]` octaves | — |
| Pitch Bend Range | **Present** | `635` (`pitch bend range`) | Float `[0, 48]` `"st"`, default `5.0` | Fixed `pitch bend destination b` (`634 = pitch`) | — |
| Velocity / Key / Aftertouch / Pitch Bend / Mod Wheel Dest A/B & Amt A/B | **Present** | `620..=639` (`modulation.rs`) | 14 destinations (`off`, `osc a..d level`, `filter freq/res/morph`, `lfo rate/amount`, `volume`, `pan`, `pitch`, `time`), amounts `[-100, 100]` `%` | Migrate to shared modulation routes in `#3892` | — |
| Global Time (`-100..+100%`) | **Missing** | — | — | `modulation.rs` exposes `time` as a modulation destination (`Effective.time_rate`), but no base `time` parameter exists | `#13607` |
| Global Tone (`0..100%`) | **Missing** | — | — | No output spectral tilt / low-pass tone control | `#13607` |
| Global Volume (`-inf..+6 dB`) | **Missing** | — | — | `modulation.rs` exposes `volume` as a modulation destination (`Effective.gain`), but `hex.instrument.operator` has no canonical base output `volume` parameter | `#13607` |

---

## Created Engine Tickets & Dependency Links

Every missing feature group above now has a focused, boundary-scoped engine ticket in Milestone M25 (`version_id = 23`, `milestone_id = 31`), each owning its fresh TLV state version, lossless migration from prior supported versions, historical-fixture/round-trip/Undo tests, and zero-allocation `allocation-probe` verification:

| Engine Ticket | Missing Feature Group | Linked UI / Migration Dependents |
|---|---|---|
| `#13607` | Global `time`, `tone`, and output `volume` | `#3890` |
| `#13608` | Global `transpose`, `spread`, and `glide` (`enabled` + `time`) | `#3889`, `#3891` |
| `#13609` | Bipolar per-voice pitch envelope | `#3891`, `#3892` |
| `#13610` | Per-voice filter envelope & direct `freq<vel` / `freq<key` | `#3892` (and filter-page sweep follow-up after `#3886`) |
| `#13611` | Filter shaper stage & nonlinear filter circuit drive / models | Filter-page shaper follow-up after `#3886` |
| `#13612` | Oscillator envelope slopes, `end` level, and note-pitch loop sync | `docs/design/vector-brief.md` envelope shaping follow-up |
| `#13613` | Extended LFO (`high` range, extra waveforms, `r<key`/`amt<vel`, LFO envelope) | `#3887`, `#3892` |
