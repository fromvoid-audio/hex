# private signed-envelope mathematical precursor

Pulse #2327 / canonical draft PR189. This approved private, unregistered math
precursor is compiled only in tests. No native device, descriptor, state version,
provider, voice runtime, tuning or UI consumes it. Existing amplitude-envelope
source and behavior remain unchanged. The audio allocation probe compiles the
same private source through a test-only path; it adds no production dependency,
allocator, unsafe code or policy exception.

## original finite law

Four normalized f64 modulation levels (initial, peak, sustain, end) must be
finite within [-1, 1]. They are not cents, semitones or amplitudes. Attack is
0–20 seconds; decay and release are 0–60 seconds. Preparation accepts
8,000–192,000 Hz and rounds durations to their nearest sample (positive halfway
values round up). Integer conversion and stage addition are checked. Conversion
rejects 2^64, including the f64 representation of u64::MAX that rounds up to it.

Held queries interpolate linearly initial→peak over attack and peak→sustain
over decay, then hold sustain. At the exact optional NoteOff age, release starts
from that instant's held-law value and interpolates linearly to end over release.
After release, end remains the modulation value; it does not imply audible voice
retirement or silence. This original linear law makes no Live numerical claim.

Instantaneous precedence: zero attack reaches peak immediately; zero decay then
reaches sustain immediately. NoteOff observes that held value after those
zero-stage transitions. Zero release reaches end immediately, overriding the
NoteOff origin even at age zero. Positive-duration endpoints are returned exactly,
including authored signed-zero endpoints. Interior roundoff is bounded by the
segment's two endpoints.
Positive durations that round to zero use the same instantaneous precedence.

Gate is Option<u64>, distinct from query age: None and Some(u64::MAX) are not
aliased. Queries before a future gate use the held law. Released queries subtract
gate from age only after checking their order; the sampler never forms an
overflowing gate+release sum. Separate checked release_end rejects an
unrepresentable future lifetime for later admission. Extreme-age sampling
remains defined even when that future lifetime cannot be represented.

Authored and prepared values are fixed Copy structures, each 56 bytes on the
checked cloud target, without drop/heap ownership. Preparation and queries do
not allocate. Arbitrary-age evaluation performs no replay or mutable cursor,
so partitioning queries does not alter output.

## acceptance and boundaries

Independent hand-computed held/release sequences cover NoteOff inside and
immediately before/at/after attack and decay boundaries. Tests cover signed
levels/end, zero stages, half-sample rounding, all field bounds/nonfinite inputs,
rate boundaries, checked conversion/lifetime overflow, extreme ages,
copy/storage/endpoint bits and partitions 1/17/64/128/256. The approved existing
allocation harness measures total and signed-current counts/bytes across
preparation, queries, copies and refusals. This is a math probe, not evidence
of native callback publication or processing.

Strict lint/full exact-source local CI remain required before draft push. HOLD
prohibits passing Pulse gate/check receipts or readiness registration.

## full pitch/filter work remains pending

The official [Operator manual](https://www.ableton.com/en/live-manual/12/live-instrument-reference/#operator)
describes Fine as fractional-ratio tuning; cents and the preview's signed Fine
domain are not established. This precursor chooses neither native tuning units
nor a pitch-envelope range. Native slopes, routing, fixed-mode interactions,
transpose/Spread, Time scaling, looping/chase and held/released retarget need
their own approved contracts.

Do not substitute the amplitude envelope's zero End/exponential law or wire
this test-only precursor to native controls. Source-codec/provider and
phase/history/Nyquist admission, complete ownership/budgets, normal history/
presets, Arrangement/Session/export, allocation/performance and native/reference
acceptance remain required for full #2327. Filter processing/placement/circuit/
bypass/tail contracts remain #2326. The mutable host's 10-second declared-tail
limit is independent and is not relaxed by the pure 60-second law domain.
No production integration, native artifact, listening or completion is claimed.
