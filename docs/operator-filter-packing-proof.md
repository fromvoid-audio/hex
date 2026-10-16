# private feedback packing attempt — admission rejected

Pulse #2326 / canonical PR212 retains all original production acceptance.
This bounded attempt is compiled only by the new integration-test harness.
It changes no registered device, codec, provider, clock, engine or UI API.
The original low-pass proof and its timing/storage history remain unchanged.

## lossless subset and negative controls

The native control producer prepares velocity as f64(u8)/127. A 56-byte private
note replaces only that derived f64 cache with its exact u8 source. All four
u64 steps, u32 caps/active flags and four f32 scale bit patterns are retained.
Packing requires exact velocity-bit equality and refuses arbitrary real,
negative-zero or nonfinite velocity and malformed origin metadata. It is not a
replacement for the registered analytic-payload decoder.


17 October compatibility update for Vector globals (#2653): current prepared
notes also carry a tagged original MIDI pitch/velocity in the upper half of the
former caps word. This private witness retains the pitch and tag in its existing
padding, reconstructs all bits, and rejects an invalid tag, out-of-range pitch or
mismatched velocity. Its 56-byte note layout is unchanged. Untagged historical
private witnesses remain lossless. The proof explicitly selects legacy processing
and the historical unmodulated-A routing; it does not claim the new operator-mode runtime fits this
rejected packing proposal.

The tests configure all oscillator ratios to 1, then use actual registered
preparation at 8/48/192 kHz, pitches 0/60/100/127 and every velocity 1–127.
Actual native above-Nyquist refusal at 8 kHz is retained. Every supported
note's eight words reconstruct exactly; actual native shared-context PCM is
bit-identical at onset, held and released ages, including nonneutral amplitude
velocity settings. Initial testing incorrectly assumed default ratios were
unity; the explicit fixture correction fixes that assumption, not the provider.

The feedback-only history retains all sixteen notes, previous-output bits,
ages, full occurrence IDs, independent active/gate masks, and two exact
configuration-ID/feedback-value cohorts. Some(MAX), Some(0) and None remain
distinct. Equal coefficient bits never erase different configuration IDs.
Inactive fields are retained too. A third distinct cohort refuses rather than
discarding data; a complete two-cohort invariant for the real native runtime is
not established. Malformed internal tags refuse rather than selecting a law.

Both histories have their own cohort storage: replacement of all sixteen
scratch occurrences with distinct IDs leaves the original committed notes,
gates, cohorts and ages intact. Value-copy rollback restores them. Existing
public retarget objects and their source/target identities are copied unchanged;
values at elapsed 0/1/120/239/240 match, and voice age is independent of elapsed.
This is field transport, not an implemented native mutable-history adapter.
Actual native mutable-state restoration/PCM and LFO-history packing remain
unproved; the analytic-note PCM oracle must not be presented as those tests.

## measured rejection and complete charge boundaries

Actual selected 64-bit Rust layouts:

| value | bytes |
| --- | ---: |
| derived note | 56 |
| packed feedback voice | 112 |
| sixteen-voice history including both cohorts/tags | 1880 |
| twin including original optional retarget, elapsed, root handle and four physical carrier histories | 4168 |
| two complete independent twins | 8336 |
| prepared root payload | 1168 |

The pair exceeds the unchanged 8192-byte ceiling by 144 bytes **before** the
root. An actual control-side Arc root allocation measures 1184 bytes, including
the reference-count words. The optimistic one-root total is therefore 9520
bytes before pair/host metadata. Shared roots are not free or admitted by
moving their charge outside the twin. Any required distinct source/target root
adds its full allocation and must have real ownership, not merely a numeric ID.

Existing measured host components remain: 72-byte pair header inside the
136-byte active owner, 144-byte retained-tail owner, two reserved 64-entry
104-byte occurrence ledgers (13312 bytes), 112-byte feedback retarget-proof
payload and 2584-byte inline checkpoint store. At the optimistic one-root
boundary an active owner totals at least 22968 bytes, or 22976 for its tail
owner, excluding separately retained proof and prepared graph data. Checkpoints
must retain/charge the pair, both ledgers and every unique root, with their
existing 256 KiB store / 512 KiB restore ceilings. These are rejection bounds,
not a functioning complete host-admission implementation or universal graph fit.
No count, precision, voice, event, checkpoint or retirement limit is reduced.

A scratch test harness using the existing allowed dev-only allocation counter
measures zero total/net allocation count/bytes for packing, unpacking and
third-cohort refusal. It separately measures the one 1184-byte root allocation
on control, retaining the owner until outside the measurement. This is not a
callback/publication/reclamation proof. No hidden note pool or shared mutable
scratch owner is used. Exact commands and log/source digests remain with the
draft PR evidence and local feasibility files.

## v7 and v8 evidence remain distinct

The canonical test source uses v7/364-byte state on PR212's original base.
The same private test harness was separately linked against frozen PR158
89b1efd2e8ff1d6238b44dba13301ccfe6ae2683: actual v8/380-byte state. Both
produce exact note/PCM reconstruction and the same feedback packing sizes.
That does not transfer native acceptance or prove v8 LFO/harmonic admission.
The separate harmonic packed pair remains 8024 bytes without filter histories.

This bounded attempt ends with a measured blocker. No complete fitting filter
runtime is demonstrated, no finite IIR tail is invented, and no larger-cap
fallback or production registration follows. Full #2326 filter laws, stable
live transitions, state/UI, ownership/admission, Arrangement/Session/export,
actual changed callback and native/reference requirements remain unmet.
