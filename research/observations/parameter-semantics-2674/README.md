# parameter snapshot measurement

17 October 2026; original Hex control-side experiment, not a Live reference study.
Exact source, tool, workload and measurement boundaries are in
[measurement.json](measurement.json); raw aggregates are
[before](operator-release-before.csv) and [after](operator-release-after.csv).
The benchmark source is `crates/hex-devices/examples/profile_parameter_snapshots.rs`.
The external measurement crate copied that file byte-for-byte and depended on
the exact clean verification checkout; its small separate target avoided native
GPUI cache interference. Both runs used the same release settings and machine.

The production Operator registry applies a 64-entry harmonic snapshot to the
default authored state. Four warm-ups precede 16 measured patches. Independent
readback checks every requested value after each patch. Compilation, source
construction, readback and caller destruction are outside the timer. Internal
registry preparation and allocation are included. With 16 samples, nearest-rank
p95 is the maximum; these are descriptive aggregates, not a stable latency bound.

Before: median 11.089208 ms, p95/max 12.526333 ms. After: median 4.004041 ms,
p95/max 4.777375 ms. Uncontrolled system activity and sequential measurements
limit causal/timing claims. This supports a narrow preparation-cost improvement,
not whole UI acceleration, callback safety, native deadlines or a proven cause
of the full-suite timeout.
