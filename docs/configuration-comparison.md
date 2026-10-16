# Native configuration comparison profile

The `configuration_profile` example times the public
`NativeRuntimeChain::configuration_matches_range` method with a bounded,
synthetic inventory. It describes this comparator only. It does not establish a
native callback deadline, plugin delay compensation (PDC), deployable effect
behavior, or parity with another application.

## Fixture and bounds

The example creates 256 regular track chains and 16 return chains, each with 64
devices. That is 17,408 compared device slots, using the current project limits
for these groups. Each prepared instance uses identity matrix metadata, no
processor definition, 48 kHz, and an opaque 4,096-byte state with the original
synthetic type ID `hex.synthetic.configuration-profile`. The state bytes have no
device meaning and do not describe deployable native effects.

The old configuration bank contains one 4,096-byte state per slot, or 68 MiB.
The separately allocated but byte-identical candidate is another 68 MiB bank.
The shared-Arc candidate reuses each old state allocation. The changed candidate
is a third independently allocated 68 MiB bank: its first 63 states per chain
are byte-identical copies, and the final state differs from its old state only
in the final payload byte. Thus the process holds about 204 MiB of fixture
payload, plus runtime entries, vectors, and allocator overhead. Setup and final
destruction happen outside timed loops.

Before timing, the example calls the actual comparator across all chains and
requires shared-Arc and separate-equal candidates to match, and the final-byte
change candidate to fail. That candidate keeps all device states independently
allocated, compares all 64 states, and differs only in the final byte of the
final device. This places the mismatch at the latest chain entry and payload
offset. It is an adversarial late-mismatch fixture; it does not guarantee the
highest possible CPU cost or that the equality implementation reads every byte.
Changing every device would exit at the first entry and would not represent a
late mismatch.

## Reproduce

Run from the repository root on an otherwise idle machine. The source hardware
and compiler metadata must be kept with the raw output so results can be
reproduced and compared. The `artifacts/` directory is ignored by Git.

```sh
mkdir -p artifacts/configuration-comparison
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=artifacts/build-cache/operator-fixed-tuning \
  cargo build --release -p hex-lattice --example configuration_profile
{
  uname -a
  uname -m
  sysctl -n hw.model
  sysctl -n hw.ncpu
  rustc -Vv
} > artifacts/configuration-comparison/environment.txt
artifacts/build-cache/operator-fixed-tuning/release/examples/configuration_profile \
  --repeats 101 --warmup 20 \
  > artifacts/configuration-comparison/run.txt 2>&1
```

For fixture and CLI validation without timing, use:

```sh
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=artifacts/build-cache/operator-fixed-tuning \
  cargo run --release -p hex-lattice --example configuration_profile -- --check
```

`--repeats` accepts 1 through 10,000 samples per case and `--warmup` accepts 0
through 10,000 iterations. Each iteration runs all three cases in a rotating
order. Allocation, state filling, chain construction, fixture validation,
timing-vector allocation, and destruction are outside the timed interval. The
only timed work is calling the real comparator for every chain in the reported
group; `black_box` surrounds comparator inputs and results.

Output reports median and p95 nanoseconds for the aggregate calls in each group,
the number of chains/comparisons per sample, candidate state payload length in
the compared range per chain and across that sample, and median nanoseconds
divided by chain count. The per-chain number is a derived average, not an
independently timed chain. The raw output's `payload_bytes_per_*` fields are
candidate payload-size bounds for each case, not measured bytes read. Shared Arc
matches take the pointer identity path, so their bound is zero. Separate-equal
and final-byte mismatch cases each include 256 KiB of candidate state per chain.
Even for the late mismatch, the equality implementation may vectorize or stop
without reading every byte. These fixture-size values do not measure general CPU
memory traffic or allocator residency.

Track and return groups are timed and reported separately. A third `all` sweep
also measures both groups in one timed interval for each case, with 272 chain
comparisons per sample. Its distribution comes from those paired measurements;
it is not calculated by adding track and return percentiles. Retain the raw
`environment.txt` and `run.txt` alongside any published measurement. Do not
infer callback safety, PDC, configuration correctness beyond the exercised
outcomes, or product compatibility from this microbenchmark.

## Exploratory result: busy host

One run was recorded on 2026-10-10 at 08:56 UTC with 101 measured repetitions
and 20 warmups per case. This is explicitly a **busy-host exploratory result**,
not an idle baseline or a callback deadline. The machine was a Mac17,9 with an
arm64 Darwin 25.6.0 kernel and 15 reported CPUs. The compiler was rustc
1.98.1 (`48a229cea`, LLVM 22.1.8). The release example used source base
`3176222fbf8014291e3aad98d49002da97d74f2e`, example SHA-256
`62bcd219bda3d4cd02574f74b5c7561d47d187db618edc5719576be637bca3a1`, and
binary SHA-256
`b13866cd64c8c21293042b604d98e4b1f9eba1d9e680a7d444e580daec74d1e1`.
Source and binary hashes were unchanged in the before/after snapshots.

| Group | Case | Comparisons/sample | Candidate state bytes/sample | Median (ns) | p95 (ns) |
| --- | --- | ---: | ---: | ---: | ---: |
| track | shared Arc | 256 | 0 | 121,667 | 200,208 |
| track | separate equal | 256 | 67,108,864 | 2,591,708 | 4,312,667 |
| track | final-byte mismatch | 256 | 67,108,864 | 2,572,292 | 4,138,292 |
| return | shared Arc | 16 | 0 | 8,000 | 16,959 |
| return | separate equal | 16 | 4,194,304 | 164,375 | 304,000 |
| return | final-byte mismatch | 16 | 4,194,304 | 165,584 | 282,292 |
| all | shared Arc | 272 | 0 | 143,334 | 214,250 |
| all | separate equal | 272 | 71,303,168 | 2,871,083 | 4,174,792 |
| all | final-byte mismatch | 272 | 71,303,168 | 2,900,000 | 4,909,459 |

Process snapshots were captured immediately before and after the run in the
ignored `artifacts/configuration-comparison/before.txt` and `after.txt` files.
They show ongoing Ableton Live, Rust compiler, Codex renderer, and system
activity; for example, `ps` reported Live at 215.4% before and 229.8% after, and
the unrelated Rust compiler at 77.3% before and 86.3% after. These process
values are context for this one run, not a controlled whole-machine load
measurement. The raw benchmark output and wrapper duration are retained in
`artifacts/configuration-comparison/run.txt` and `wrapper_elapsed.txt`.
Re-run on an idle host before using these measurements as a stable machine
baseline; no implementation optimization is justified by this exploratory run
alone.

## Linux guest baseline — Pulse2219 / PR176, 13 October 2026

Three sequential direct-binary release runs retain the same fixture SHA-256
`62bcd219bda3d4cd02574f74b5c7561d47d187db618edc5719576be637bca3a1`
as the historical busy-host experiment. Each uses 101 measured samples and
20 warmups, with the unchanged rotating case order, fixture validation,
chain inventories and nine separately timed group/case combinations.
No comparator, runtime, fixture or admission behavior changed.

Measurement source is clean plan commit
`a41c63f3e0935b3483ecdaaaa095e9d0ac0df738`, tree
`ab112fb3c05242f8cb996402903a44ac65269510`, atop PR166 merge `06ccfef`.
The evidence commit adds only its task plan, documentation and captured logs;
measurement runtime source remains unchanged. The optimized binary SHA-256 is
`a23ec9da3bb5b1c5847e195f1404428e7a5d81b4ca483f795df56fc0d0ac9f61`.
Build and measured commands, after the published toolchain activation:

```sh
CARGO_TARGET_DIR=/workspace/hex/target cargo build --release -p hex-lattice --example configuration_profile --locked
/workspace/hex/target/release/examples/configuration_profile --check
/workspace/hex/target/release/examples/configuration_profile --repeats 101 --warmup 20
```

Environment: Linux 6.18.44 x86_64 guest, reported AMD EPYC 9V74, five visible
CPUs with `cpu.max` of `400000 100000`, Rust 1.94.0 / LLVM 21.1.8.
The first pre-run load averages were `0.0 / 0.0 / 0.0537`; the final snapshot
was `0.1538 / 0.0332 / 0.0645`. No Cargo/rustc or other profiling process was
present during measurement; the capture wrapper and normal agent/runtime
services remained. Cgroup `nr_throttled` stayed 642 and `throttled_usec`
stayed 5,813,894 across all three runs. No process was stopped or reconfigured.
These snapshots describe the naturally quiet selected guest, not activity on
the underlying physical host or hypervisor, which is not observable here.

Ranges below are the smallest and largest statistic from the three runs, in
nanoseconds. They are not distributions formed by pooling samples.

| Group | Case | Median range (ns) | p95 range (ns) |
| --- | --- | ---: | ---: |
| track | shared Arc | 430,449–483,990 | 554,797–812,296 |
| track | separate equal | 5,525,187–6,432,827 | 6,254,728–8,677,513 |
| track | final-byte mismatch | 5,454,320–6,549,453 | 6,344,383–8,308,568 |
| return | shared Arc | 6,910–7,361 | 8,193–10,046 |
| return | separate equal | 109,846–112,710 | 174,003–228,305 |
| return | final-byte mismatch | 108,303–111,187 | 168,364–219,130 |
| all | shared Arc | 478,732–506,405 | 598,433–1,123,486 |
| all | separate equal | 5,921,885–6,729,245 | 7,041,755–9,458,763 |
| all | final-byte mismatch | 5,895,727–6,646,029 | 6,766,691–9,531,162 |

The comparison method matches the original experiment: identical fixture bytes,
101/20 sampling, rotating order, actual comparator and independently measured
Track/Return/all sweeps. Hardware, architecture, compiler and virtualization
differ. In particular, these numbers do not measure the effect of making the
historical Mac idle and must not be presented as a speedup or regression caused
by host load. An idle baseline on that original Mac remains unavailable to this
cloud executor. Neither dataset establishes physical callback deadlines,
memory traffic, product scalability or Live parity.

Raw runs, fixture check, before/after process/load/CPU snapshots and their SHA-256
digests are committed in
[the evidence directory](../research/observations/configuration-comparison-2219/manifest.json).
The original busy-host evidence above is retained unchanged. Full local CI for
the final evidence commit is recorded in Pulse2219 and PR176 separately from
these source-pinned release measurements.
