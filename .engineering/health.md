# engineering health

Snapshot: 5 November 2026. The runtime goal remains active; no scheduled task was
created. This is historical evidence at exact sources, not certification of the
latest upstream tree. Reconcile [state](state.json), Git and live Pulse on recovery.

Independent Linux correctness verification followed the Mac-owned callback fixes:

| owner | canonical PR | tested merge | selected passes | existing ignored |
| --- | --- | --- | ---: | ---: |
| [meter lookup](https://pulse.iocore.dev/#/issues/4965) | [2066](https://github.com/PORTALSURFER/hex/pull/2066) | `1cdddde01` | 105 | 1 |
| [dry-track batching](https://pulse.iocore.dev/#/issues/5036) | [2091](https://github.com/PORTALSURFER/hex/pull/2091) | `4dbb4fae3` | 130 | 1 |
| [native-stage batching](https://pulse.iocore.dev/#/issues/5039) | [2092](https://github.com/PORTALSURFER/hex/pull/2092) | `b9fa6b55a` | 134 | 1 |
| [EQ processing](https://pulse.iocore.dev/#/issues/5041) | [2093](https://github.com/PORTALSURFER/hex/pull/2093) | `2beb5c2a1` | 51 | 2 |
| [Reverb tank](https://pulse.iocore.dev/#/issues/5043) | [2094](https://github.com/PORTALSURFER/hex/pull/2094) | `416c31b91` | 56 | 0 |
| [independent stage windows](https://pulse.iocore.dev/#/issues/5049) | [2098](https://github.com/PORTALSURFER/hex/pull/2098) | `a84c15735` | 111 | 1 |

All selected runs passed, including callback allocation probes and calibration.
Counts overlap across revisions and must not be added as unique coverage. Host:
Linux x86_64, Xeon Platinum 8370C, five visible CPUs, Rust/Cargo 1.94.0. Exact
commands, timings, sources, manifest hashes and omissions are appended to
[measurements](measurements.jsonl); canonical Pulse contributor notes retain the
essential evidence if ignored executor-local logs are lost. The final window merge
also contains concurrent control-side latency observations, tested separately.

Matched optimized performance comparisons belong to the owning Mac PRs; these
Linux runs supply platform/correctness evidence. They do not qualify native UI,
listening, physical deadlines, VST3 SDK execution, or current whole-workspace lint.
The opt-in dense native/mixed VST3 baseline is documented in
[local checks](../docs/local-ci.md). Keep private project/media custody unchanged.

[Subframe test partitions](https://pulse.iocore.dev/#/issues/5034) are merged in
`af0d9bd154fb4d8d8fd89b2ede067a4bd2e5d1d8` via
[PR #2090](https://github.com/PORTALSURFER/hex/pull/2090). Nine rate/mode cases retain
all 27 histories and PCM oracles. Isolated old-case median was 2.942 s; largest new
isolated case was 0.666 s and parallel matrix median 1.740 s. Serial total and full
suite did not improve; contended cases can exceed 1 s. Integrated Session source
`bafdd2ac8a0f9f90c513e7ad01ff052c00aec6b8` passed 481 cases, three manual profiles
ignored, in 12.141 s, with strict default/additive owner lint. These older results
do not establish current [broader budget acceptance](https://pulse.iocore.dev/#/issues/3850).

The [Sampler experiment](https://pulse.iocore.dev/#/issues/3851) was retired after
inconclusive timings and byte-for-byte fixture restoration. Its two agent-owned
temporary benchmark binary copies were removed after hash/process/custody checks,
reclaiming 1,908,183,040 bytes; original targets and experiment evidence remain.
The [ranked index](backlog.md) points to live work and preserves explicit holds.
