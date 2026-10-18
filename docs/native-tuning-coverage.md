# Native project tuning coverage

This audit covers the registered source at main cb8befc4 plus the standalone
coverage additions in PR435. It adds tests, not a second tuning model or device
publisher. A source test passing is not installed-app, MTS-client or listening
acceptance. Final integration/full required CI belongs to the integration owner.

## Registry and pitch contracts

The actual registry in `crates/hex-devices/src/lib.rs` registers Sine, FM4 and
`hex.instrument.operator` (display name **vector**). Seven registered effects
(utility, eq8, eq, saturator, compressor, gate, auto pan) have no authored note-pitch
role; assigning them oscillator tuning would be incorrect. Do not turn this
inventory into a fixed catalog-size gate for future devices.

| Device | Current preparation and pitch law | Distinct controls |
| --- | --- | --- |
| Sine | `lib.rs::PreparedSineInstrument::prepare_note` reads the effective table, then prepares its finite phase step on the control side | No separate ratio/fixed-Hz, detune, glide or pitch-bend parameter in its registered definition |
| FM4 | `fm4/device.rs::PreparedFm4Instrument::prepare_note` passes the effective frequency to `PreparedFm4::new_with_frequency` | Four oscillator ratios multiply that frequency; FM/routing/envelope laws remain their authored laws |
| Vector | `operator_state.rs::OperatorMetadata::prepare_instrument` receives `DevicePreparationContext.tuning`; `harmonic_runtime::Factory::new_with_tuning` retains it. `fm4/operator/context.rs::modified_steps` and `fm4/operator/tuning.rs::prepared_steps_with_table` choose the effective table for ratio oscillators | Fixed-Hz oscillators keep authored Hz times multiplier. Oscillator-velocity modulation multiplies the selected ratio/fixed base; its quantization affects the multiplier. Key scaling and key pan deliberately use MIDI key as amplitude/pan controls. LFO clock remains its authored free/sync rate; a pitch destination modifies the already tuned step. No separate registered glide/pitch-bend control is invented |
| Full Sampler | Not registered in this audited source; unmerged PR395 remains source4-owned | The borrowed-PCM precursor `sampler.rs::PreparedSampler::note` has root-note/transpose/detune and Slice selection, but no effective-table context. This is a future registration dependency, not proof of a currently registered sampler route |

Current-mode Vector coverage keeps global mode1 and default quality. Existing
legacy-mode tests remain useful but cannot stand in for current-mode evidence.
No per-device MTS FFI/master/client is added: the existing desktop broker publishes
the canonical table; native sound uses that table even with no MTS library.

The accepted future pitched-Sampler contract for current root-note-only metadata
is `source_rate/output_rate * effective_hz(note)/nominal_12tet_hz(root_note)` times
its authored transpose/detune factor. A root69 A440 sample under reference455 must
play at455/440, not cancel the change by dividing by retuned root Hz. Check any
actually authored root-Hz field before adapting that contract. Slice key selection
stays unpitched. Source4 owns its implementation/resource lifetime/registration;
PR435 does not import its unpublished source or claim Sampler completion.

## Actual route matrix

`crates/hex-engine/tests/native_tuning_coverage.rs` uses real prepared revisions,
public Playback/Session commands and live keyboard input objects, with no backend.
Its independent oracle computes authored degree quotient/remainder, root-degree
and reference Hz without calling the effective-table frequency getter. Two unequal
maps use five degrees per tritave and seven degrees per900-cent period.

| Route or invariant | Sine | FM4 | Vector | Evidence and limits |
| --- | --- | --- | --- | --- |
| Arrangement and Console/Session | Covered | Covered | Covered | 36 rendered pitch scenarios across two scales, three keys, two hosts; real zero crossings after attack; Sine also independent PCM phase/envelope |
| Enabled neutral table vs disabled neutral | Covered | Covered | Covered | Bit-exact host PCM, not merely equal labels |
| Authored oscillator ratio | N/A | Covered | Covered | Ratio2.5 multiplies unequal effective pitch in both hosts |
| Fixed and mixed fixed/ratio oscillators | N/A | N/A | Covered | Fixed487.3Hz times0.75 is key/table-independent; mixed-carrier frequency projections distinguish both intended components from wrong440Hz/octave targets |
| Live selected-track keyboard | Covered | Covered | Covered | Paused Arrangement, actual input lease; no concurrent clip used to mask a missing keyboard sound |
| Held/released unequal-map retune | Covered | Covered | Covered | Canonical SetTuning retains MIDI identity, real preparation/family admission/handoff; retired donor cannot cancel transferred input; Sine independent continuous phase/release envelope |
| Seek and Arrangement loop | Covered | Covered | Covered | Real control-prepared interactive preroll, then actual loop wraps; settled windows after three onsets retain mapped pitch. Raw mutable held-note callback Seek is deliberately refused, not silently substituted |
| Prepared reuse/isolation | Covered | Covered | Covered | A→B→A, old immutable A output unchanged, restored A bit-exact; fresh/Open and ordinary-edit family refusal leaves original audible lease intact |
| Real create-new WAV export and bounce | Covered | Covered | Covered | `hex-export/tests/native_tuning_coverage.rs`: independent non-octave pitch on decoded PCM16 plus exact agreement with partitioned real Engine output. Current Vector mode1 asserted. Nonzero-range float bounce via actual preroll agrees exactly with the independently pitch-checked full render. Existing destination bytes survive refused second create-new export |
| Reference note audition | Shared dedicated synth | Shared dedicated synth | Shared dedicated synth | This is not a selected device's patch. Existing `hex-audio/tests/callback_allocations/project_tuning.rs::reference_audition_retargets_with_exact_phase_envelope_and_zero_callback_allocations` checks the actual separate table-driven audition path |
| History/refusal/project generation | Shared canonical path | Shared canonical path | Shared canonical path | Existing `hex-session/src/service/tuning_tests.rs` checks edit/Undo/Redo, preview cancel/skipped candidates, preparation refusal, stale revision/generation library writes and rebased actual PCM. No duplicated model or rewritten old dialog context |
| Current-format persistence | Shared canonical table | Shared canonical table | Shared canonical table + opaque state | Existing storage tuning round trips and fixed-state round trip; no legacy-alpha conversion/backcompat added or claimed |
| Callback allocation/retirement | Existing fixture | Existing fixture | Existing fixture | Existing real publication/held/released/reference fixtures measure the callback. PR435 changes no runtime callback path; standalone offline pitch measurements do not themselves prove RT allocation behavior |
| Physical MIDI input / installed MTS client / listening | Open | Open | Open | Root's finite native/user-present phase only; no library/API/device/app operations in these tests |
| Full Sampler routes, tuning, export and lifecycle | Dependency | Dependency | Dependency | Must be extended and verified when source4 registers the full device; no coverage waiver or whole-catalog completion claim |

Seek tests use the admitted preroll API because mutable note history cannot be
replayed in a callback command. Vector's loop release overlaps its new attack;
a sum of decaying, differently phased sinusoids can temporarily shift crossings
without changing either oscillator's frequency. The independent loop frequency
window starts after the fixture's4800-frame Vector release; the transition still
must be finite and audible. This is not a promise of instantaneous-frequency
continuity across an intentional retrigger.

## Evidence discipline and targeted qualification

Use the original LocalCiRunner summaries and every log, with exact source/tree,
mode, status and hashes in the immutable handoff. Early fixture failures are
preserved: wrong authored degree-array capacity, a decay below descriptor minimum,
fresh snapshot identity wrongly used as a continuation, frame-vs-beat loop input,
and a raw mutable seek whose refusal was hidden by the convenience `apply` API.
An initial allocation-target invocation omitted its feature and matched zero tests;
that receipt is not allocation evidence. The feature-enabled command below is required.
The loop crossing diagnostic also remains historical; no product guard, admission
policy or assertion was removed to conceal an unsupported route.

Focused commands (positive matches required):

```sh
cargo test -p hex-engine --test native_tuning_coverage --locked -- --nocapture
cargo test -p hex-export --test native_tuning_coverage --locked -- --nocapture
cargo test -p hex-devices --lib --locked project_tuning_tests -- --nocapture
cargo test -p hex-engine --lib --locked computer_keyboard::tuning_tests -- --nocapture --skip profile_keyboard_tuning_handoff_copy_cost
cargo test -p hex-session --lib --locked service::tuning_tests -- --nocapture
cargo test -p hex-audio --features allocation-probe --test callback_allocations --locked project_tuning -- --nocapture
cargo test -p hex-storage --test tuning --locked fractional_reference_frequencies_roundtrip -- --nocapture
cargo clippy -p hex-engine -p hex-export --all-targets --locked -- -D warnings
```

The keyboard profile is historically ignored and must remain disclosed as ignored;
exclude the profile explicitly for the ordinary non-profile command, do not count
it as a pass. Cloud toolchain configuration is not native default-stack evidence.

Root may schedule the two new integration selectors on an immutable actual combined
candidate, default native stack, no app/device/library changes. Do not repeat old
unchanged406/407 native suites for this coverage-only PR. Any later Engine/Sampler
production repair needs its own source-bound targeted/native allocation evidence.
Physical acceptance remains a separate finite user-present checklist: actual
selected device patch in Arrangement/Console/live input; unequal and neutral tuning,
held/released edits and Undo/Redo, intentional fixed-Hz exemption; listening, saved
project reopen and exported/reopened WAV; desired/native-applied vs MTS-published
revision and a real compatible external client only under root's authorized lane.
No new MTS installation, master takeover, security change or automatic OS action.
