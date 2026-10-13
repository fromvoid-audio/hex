# Private 2x halfband study

Pulse2252/PR191 is a private, unregistered kernel/layout checkpoint. Production
EQ, coefficient preparation, parameter clocks, providers, registry, state codec,
UI and latency compensation remain untouched. This does not complete2252.

Original33-tap Hann-windowed halfband coefficients have center0.5, exact zero
noncentral even-offset taps, and odd sinc/window coefficients normalized to0.5.
Only the16 odd f64 coefficients are stored in a fixed immutable128-byte table.
An independent mathematical generator checks the encoded bits within1e-15.
There is no vendor code or numerical Live-equivalence assertion. The offline
hex-media resampler's1024-phase tables and8..192k rate contract are not reused.

The dimensionless streaming kernel interpolates with2h, accepts two explicitly
ordered high-rate phases, then decimates withh on the EVEN phase. Two physical
channels retain f64 histories. Internal even rates16..384k are validated (base
8..192k); rate validation alone does not prepare the real EQ at those rates.
No intermediate f32 conversion is added to production. The mapped test transform
and injected test tones are independent oracle fixtures, not registered effects.

Chosen response contract: single-stage ripple<=0.06dB through0.4*baseFs,
stop attenuation>=40dB from0.6*baseFs through internal Nyquist, identity pair
pass ripple<=0.12dB. Between0.4Fs and0.6Fs is an explicit transition band.
The full pair is approximately-6.02dB at base Nyquist; no unchanged-response claim.
The dense numerical response test bounds between-grid uncertainty with
|dA/df|<=2pi*sum|k-16||h[k]|. With65536 cells per band the bound is
0.00002440473222292144, plus explicit1e-10 floating evaluation allowance.
Observed pass amplitudes0.9959316532794854..1.0061673594278588 and stop amplitudes
-0.006167359427858539..0.00406834672051469 leave the asserted limits intact
including that uncertainty. An independent full complex DFT checks the symmetric
zero-phase shortcut. This is numerical validation with an analytic interpolation
bound, not a formal proof of platform libm error bounds or native performance.

Independent zero-insertion/full-convolution tests compare against the polyphase
implementation for identity and a nonidentity test transform. Image and injected
high-rate alias tones are actually filtered. The identity pair impulse is
2*sum h[k]h[2n-k], supported on base frames0..32, symmetric with sum1/centroid16.
It starts before its center: declared16-base-frame alignment delay is not a
16-frame initial silence promise. Fixed FIR delay is separate from EQ IIR phase,
block scheduling and interface latency. Existing latency wording and all paths
must support this declaration before any toggle can be admitted.

Persistent rings are15 previous stereo input frames(240B),16 odd-phase stereo
frames(256B),8 even-phase delay frames(128B), and3 cursor bytes/alignment(8B):
actual Rust History632B/alignment8. Reset clears every ring/cursor; arbitrary
partitions preserve exact output bits and histories. Nonfinite input, mapped
phases or output refuse before any history/cursor mutation. Failed retry matches
an untouched twin. History equality compares f64 bits, including signedzero.
This does not implement rollback for a real EQ callback or graph handoff.

Actual Rust measurement uses the REAL public Eq8Runtime and REAL prepared
processor, without new exports: existing declaration/sizeof3792B; proposed
Eq8Runtime+History wrapper4424B,328 over unchanged4096. Actual chain admission rejects the proposed declaration before either trap
factory runs. Actual boxed twin payloads
8848B; typed Box handles16B together. Real prepared EQ object40B, boxed trait
handle16B; runtime trait handles would be16B each in existing runtime entries.
The fixed FIR table128B is separately shared. Existing EQ target coefficient
allocation2560B+16B Arc header remains separately charged, shared by prepared
and twin owners. No pointer-only declaration, precision reduction, cap raise or
history discard. Production generic chain headers, entry Vec capacities and
prepared state/graph allocations are outside this unregistered unit and remain
in their owning ledgers; this is not whole graph physical closure or admission.

Kernel-owned Vec capacity isZERO: histories inline, coefficients static, no
heap scratch/block buffer. Construction returns an inline632B value; fixed phase
and output locals replace a doubled block allocation. Typed locals are32-byte
phase arrays and16-byte frames; actual machine stack/register use is not measured.
Oracle Vecs and actual layout-test boxes allocate on control outside measured
scopes. Kernel construction/process/reset/refusal probes report zero total/net
allocation counts/bytes. Balanced allocate/free and external-free controls prove
instrument sensitivity. No new unsafe allocator or source suppression is added.

The simple wrapper is NOT admitted. A separate lossless representation proof is
required before real EQ integration: interrupted coefficient ramps defeat naive
endpoint-only compression; Stereo-only sharing does not cover independent LR/MS.
Calling existing process_frame twice also halves256-base-frame ramp durations
and inserts f32 conversions, so it is not an authorized implementation shortcut.
Base authored frequency limits must remain distinct from internal2Fs design rate.

Further gates: storage fit, actual EQ reuse and clocks, state migration/history,
safe latency epoch and path compensation in28/PR184, serial/parallel/Return/bypass
alignment and refused-handoff rollback, actual registered callback probes,
release performance and native/reference/listening acceptance. Public/persisted
oversampling stays absent; no silent off/bypass fallback or zero latency claim.
Blocked/draft HOLD prohibits evidence registration/readiness/merge/Done/deploy.
