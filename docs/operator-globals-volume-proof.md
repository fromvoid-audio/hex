# private Operator Volume proof — Pulse2328

This is an unregistered original Hex mathematical proof in canonical PR180.
Only `tests/operator_globals_volume_proof.rs` compiles the new source module
through a test-only path. No provider, registry, descriptors, codec, runtime,
engine, UI or widget adopts it. It does not establish Live numerical parity,
production admission, native listening or hardware deadlines. Full2328 remains
incomplete. The base is81bc44521c484fcec3c63c5ad465a42276fb5254 overa882f9d2;
its registered Operator state is stillv7. Existing historical approvals and
v8/v9/v10 reservations are not changed by this proof.

## approved original law

The coordinator approved a finite subset of output volume: -60 through+6dB,
neutral0dB, with control-prepared `10^(dB/20)` in f64. The proof has no silence
endpoint and rejects NaN/infinity, including negative infinity. The future
public volume/mute contract remains a separate decision. UI preview -16dB is
not canonical state and is not a migration default.

Gain composes after the complete carrier sum. It never changes input frames,
oscillator/FM/feedback history, envelope progress, phase or gate. Gain1 returns
the exact original stereo bits, including signed zero and subnormals. Other
gains multiply in f64 and round once to f32. Nonfinite input or an unrepresentable
final stereo output is refused as a whole, without clipping or a partial frame.
This proof does not install output fault handling or application acknowledgments.

The accepted rate range is8,000–192,000Hz. A5ms linear-gain ramp rounds to nearest
integer frame, half upward; durations are40–960frames (240 at48kHz). A transition
is inclusive at its original start gain and reaches its exact target at the
exclusive end. Reversal evaluates the current ramp at the supplied boundary,
not the preceding authored endpoint. Repeating the same endpoint does not
restart its ramp. Retarget preparation checks rate equality, boundary order and
u64 end-frame addition before replacing any fields. There is no wrapping clock,
heap ramp chain, callback exponentiation, allocation or lock in this scalar code.

The caller must supply an actual committed performance boundary. The scalar
checkpoint cannot establish audio application, external command ordering,
owner identity, epoch cuts or an applied watermark. Queries are pure; the
checkpoint checks order relative to its latest retarget boundary, not an
unrepresented host's last rendered frame. A copied snapshot retains that exact
scalar ramp independently of later edits. It is not a detached context lease or
a proof that the engine can carry/reclaim that snapshot.

## finite evidence and limits

Tests use hand-known gains (including -20dB=0.1), independent quarter/midpoint
ramp values, an authored-endpoint negative control, unchanged endpoint repeats,
reversals and partitions1/17/64/128/256. They cover adjacent out-of-domain f32s,
nonfinite numbers, half-frame rate rounding, wrong rates, retrograde boundaries,
u64 overflow/last representable endpoint, conversion overflow and exact neutral
bits. Registered v7 Operator carrier PCM at8/48/192kHz is composed without
rewriting canonical bytes. Real feedback, free-running LFO and combined mutable
factories retain bit-identical raw onset/gate/release histories after output
composition. These are device-level proof fixtures, not an integrated provider
edit, open-gate typing test or engine rollback/admission result.

Measured x86_64 layout: prepared controls16B, complete scalar checkpoint64B,
alignment8. One copied checkpoint is another64B. No pointer-owned storage is
hidden in these types. Their hypothetical pair128B is not a complete added
factory/context/recipe/scratch/rollback charge and does not prove a fit in any
existing native cap. Source inspection identifies fixed scalar operations only;
allocation instrumentation remains with the existing authorized audio probe
owner. No allocation-counter dependency or architecture-policy exception is
added, and no zero-callback-allocation result is invented.

The explicit ignored profile is run separately in optimized Linux mode. Five
paired trials alternate order for original registered carrier sampling, neutral
composition and continuously reversed active gain ramps. Each trial uses48kHz,
128frames,200warmups and1,000 recorded wall-time blocks. All p50/p99/max values
and checksum are printed; no outlier removal or deadline acceptance follows.
This is a private one-note device composition probe, not a maximum-polyphony,
feedback/LFO performance or CPAL callback profile. Preparation and timing-vector
storage are outside each measured block.

Checks: targeted locked device tests; strict device Clippy; optimized proof tests
and paired profile; required full exact-clean local CI. Retain actual commands,
source identity, results and logs in the draft PR evidence. No passing result is
asserted by the presence of this documentation.

## production contracts still outstanding

Keep harmonicv9 and globalsv10. Freeze the realv9 prefix and approved global
IDs before defining offsets or a decoder. The earlier proposal is three finite
f32 words: neutral Time1, Tone1, Volume0dB. Preserve the complete v8 enable prefix
and every earlier authored setting; never import local Time23%/Volume-16dB
defaults. Strict original/newer-version, length, domain and malformed-state
checks and real migration/history/save tests still require implementation.

Current enable continuation format1 validates words4–7 as zero. PR158's private
compact/root ownership proofs depend on that invariant. A global coefficient
requires explicit provider format/version compatibility and revised complete
storage/accounting evidence. Reserved frame words are not proven free space.
Retain shared context4096B/note64B, mutable pair8192B/16voices/256events,2MiB
context and32MiB playback ceilings and all existing independent slot/tail caps.
Include every source/target context, factory/proof, snapshot, scratch and retained
ownership charge before admission; do not borrow PR158's private arithmetic as
a production-fit claim.

Time still needs rate-versus-duration and active-stage continuity decisions plus
an explicit release-horizon/carried-note contract. Current note validation binds
release_frames to its prepared configuration. ToneA remains a preliminary
original fundamental/highest-safe-table blend; custom harmonic interactions,
provider transitions and FM-sideband limitations must be resolved separately.
Algorithm ID61 already derives custom/serial/parallel/two-pairs from20 canonical
routing values. No eleven-algorithm Live catalog or unmeasured graph is added.

Before production Volume registration, specify exact owner/frame/source fences,
analytic and mutable retarget/rollback/retirement, new-onset and original-gate
ownership, combined enable/global transitions, open Held/Released sampling and
whole-budget refusal. Verify Arrangement/Session/export, canonical history and
save/reopen, actual callback allocations, worst changed paths and native optimized
user acceptance. No future-onsets-only narrowing or UI-only control counts as
full2328 completion. PR180 remains draft/review_only with no release or readiness.
